//! `memstead-session-serve` — the writable multi-tenant sketch session
//! server. Serves [`memstead_serve::session::build_sketch_app`]: each visitor
//! gets a two-mount engine — their own ephemeral writable `sketch` mem plus
//! a shared read-only `memstead` content mem — behind a scoped remote-MCP
//! endpoint. `build_sketch_app` merges both session flows over one registry:
//! the page-first flow (`POST /sessions` → `/s/{id}/mcp`, `/s/{id}/graph`,
//! `/s/{id}/stream`, `/s/{id}/export`) and the connection-born flow (a stable
//! `/mcp` that mints a session per connection, with view data at
//! `/v/{id}/graph`, `/v/{id}/stream`, `/v/{id}/export`). The read-only
//! `memstead-serve` binary is unaffected — this is an additive, separate
//! deployment.
//!
//! Every deployment specific is an environment input:
//! - `MEMSTEAD_SESSION_BIND` — listen address (default `0.0.0.0:8080`; honours
//!   `PORT`, which Railway and most PaaS inject).
//! - `MEMSTEAD_SESSION_SCHEMA` — schema pin for the writable sketch mem
//!   (default `default@1.0.0`). The server takes the pin rather than hard-coding it.
//! - `MEMSTEAD_SESSION_CONTENT_DIR` / `MEMSTEAD_SESSION_CONTENT_ARCHIVE` —
//!   override the curated read-only "what is Memstead" content mem source (a
//!   folder or a sealed `.mem`). When neither is set the server serves the
//!   curated content mem embedded in the binary, so the two-tier experience
//!   works out of the box.
//! - `MEMSTEAD_SESSION_CONTENT_SCHEMA` — schema pin for the content mount
//!   (default: the same pin as `MEMSTEAD_SESSION_SCHEMA`).
//! - `MEMSTEAD_SESSION_CONTENT_MEM` — mem name the content mount registers
//!   under (default `memstead`). MUST match the content source's own mem name:
//!   an archive's ids are `<name>--slug`, so a mismatch makes its entities
//!   invisible. E.g. set `engine` to read a mem exported as `engine`.
//! - `MEMSTEAD_SESSION_TTL_SECS` — idle eviction window (default `1800`).
//! - `MEMSTEAD_SESSION_ENTITY_CAP` — per-session entity budget
//!   (default [`DEFAULT_SESSION_ENTITY_CAP`]).
//! - `MEMSTEAD_SESSION_MAX` — ceiling on concurrently-live sessions; past it new
//!   sessions are refused (a 503 / typed cap error) rather than OOM-ing the box
//!   (default [`DEFAULT_SESSION_MAX`]).
//! - `MEMSTEAD_SESSION_RATE_PER_SEC` / `MEMSTEAD_SESSION_RATE_BURST` —
//!   per-client rate limit, keyed on the forwarded client IP (default `5` / `60`).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use memstead_serve::materialize_embedded_content;

use memstead_base::MountStorage;
use memstead_serve::session::{
    CONTENT_MEM_NAME, DEFAULT_SESSION_ENTITY_CAP, DEFAULT_SESSION_MAX, SessionRegistry,
    build_sketch_app,
};
use uuid::Uuid;

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind = std::env::var("MEMSTEAD_SESSION_BIND").unwrap_or_else(|_| {
        let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
        format!("0.0.0.0:{port}")
    });
    let schema_pin =
        std::env::var("MEMSTEAD_SESSION_SCHEMA").unwrap_or_else(|_| "default@1.0.0".to_string());
    // The sketch-mem pin for the connection-born `/mcp` flow (one engine per
    // MCP connection). The page-first `POST /sessions` path takes its pin from
    // the request body instead.
    let sketch_schema: memstead_schema::SchemaRef = schema_pin
        .parse()
        .map_err(|e| format!("invalid MEMSTEAD_SESSION_SCHEMA {schema_pin:?}: {e}"))?;

    // Base for the live view URL handed to the agent through the MCP
    // handshake. Absent → a relative `/v/<id>` (the deployment's own origin);
    // set to e.g. `https://memstead.ai` to emit an absolute URL.
    let view_base = std::env::var("MEMSTEAD_SESSION_VIEW_BASE").unwrap_or_default();

    // The curated read-only content mem the agent reads to orient itself.
    // A folder or a sealed `.mem`; absent → an empty in-memory placeholder so
    // the two-mount structure holds until a curated mem is authored.
    let content_storage = if let Ok(dir) = std::env::var("MEMSTEAD_SESSION_CONTENT_DIR") {
        MountStorage::Folder {
            path: PathBuf::from(dir),
        }
    } else if let Ok(archive) = std::env::var("MEMSTEAD_SESSION_CONTENT_ARCHIVE") {
        MountStorage::Archive {
            path: PathBuf::from(archive),
        }
    } else {
        // No override → serve the embedded curated content mem. Falls back to
        // an empty placeholder only if materialization fails (e.g. a read-only
        // temp dir), so the server still boots.
        match materialize_embedded_content() {
            Ok(dir) => {
                eprintln!(
                    "memstead-session-serve: serving the embedded curated content mem from {}",
                    dir.display()
                );
                MountStorage::Folder { path: dir }
            }
            Err(e) => {
                eprintln!(
                    "memstead-session-serve: could not materialize embedded content ({e}); \
mounting an empty placeholder"
                );
                MountStorage::InMemory
            }
        }
    };
    let content_pin =
        std::env::var("MEMSTEAD_SESSION_CONTENT_SCHEMA").unwrap_or_else(|_| schema_pin.clone());
    let content_schema: memstead_schema::SchemaRef = content_pin
        .parse()
        .map_err(|e| format!("invalid MEMSTEAD_SESSION_CONTENT_SCHEMA {content_pin:?}: {e}"))?;

    // Mem name the read-only content mount registers under. Must equal the
    // content source's own mem name (an archive's ids are `<name>--slug`, so a
    // mismatch makes its entities invisible). Defaults to the curated content
    // mem's name; set it to e.g. `engine` to read a mem exported as `engine`.
    let content_mem_name = std::env::var("MEMSTEAD_SESSION_CONTENT_MEM")
        .unwrap_or_else(|_| CONTENT_MEM_NAME.to_string());

    let ttl = Duration::from_secs(env_u64("MEMSTEAD_SESSION_TTL_SECS", 1800));
    let cap = env_u64(
        "MEMSTEAD_SESSION_ENTITY_CAP",
        DEFAULT_SESSION_ENTITY_CAP as u64,
    ) as usize;
    // Global live-session ceiling: bounds memory under a spike (each live
    // session is its own in-memory engine). Raise with the box's RAM.
    let max_sessions = env_u64("MEMSTEAD_SESSION_MAX", DEFAULT_SESSION_MAX as u64) as usize;
    let per_second = env_u64("MEMSTEAD_SESSION_RATE_PER_SEC", 5);
    let burst = env_u64("MEMSTEAD_SESSION_RATE_BURST", 60) as u32;

    // Unguessable session ids: a v4 UUID is the only thing scoping a visitor's
    // throwaway empty mem.
    let id_gen: memstead_serve::session::IdGen =
        Arc::new(|| format!("s_{}", Uuid::new_v4().simple()));
    let registry = SessionRegistry::new(
        content_storage,
        content_schema,
        content_mem_name,
        ttl,
        cap,
        id_gen,
    )
    .with_max_sessions(max_sessions);

    // Idle eviction must be driven: sweep on an interval so abandoned sessions
    // release their mems and their URLs start refusing (observable expiry).
    {
        let registry = registry.clone();
        let sweep_every = (ttl / 4).max(Duration::from_secs(30));
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(sweep_every);
            loop {
                ticker.tick().await;
                let evicted = registry.sweep_expired(Instant::now());
                if evicted > 0 {
                    eprintln!("memstead-session-serve: evicted {evicted} idle session(s)");
                }
            }
        });
    }

    // Soft-launch gate (default ON; one env shared with the .com/.io/.ai-read
    // surfaces). ON serves the writable MCP endpoint only at `/try/mcp`.
    let soft_launch = !matches!(
        std::env::var("MEMSTEAD_SOFT_LAUNCH")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "off" | "false"
    );
    let app = build_sketch_app(
        registry,
        sketch_schema,
        view_base.clone(),
        per_second,
        burst,
        soft_launch,
    );

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    let view_base_display = if view_base.is_empty() {
        "<relative /v/{id}>".to_string()
    } else {
        view_base.clone()
    };
    eprintln!(
        "memstead-session-serve listening on {bind} \
(schema={schema_pin}, ttl={}s, cap={cap}, view_base={view_base_display})",
        ttl.as_secs()
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
