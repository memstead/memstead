//! `memstead-serve-full` — the single-origin public surface in one process.
//!
//! Serves the embedded human site + the read-only HTML read pages (`/agent`,
//! `/overview`, `/entity/<id>`, `/entities`, `/schema`, over a sealed mem)
//! AND the writable connection-born sketch `/mcp` (each MCP connection mints its
//! own ephemeral sketch mem beside a shared read-only content mem), plus the
//! per-session view-data routes (`/v/{id}/graph|stream|export`). One binary, one
//! host: the website and the writable MCP share an origin, so a deployment needs
//! no edge to splice two backends together.
//!
//! It reads BOTH env families:
//! - `MEMSTEAD_SERVE_*` — the read side: `AUTHORITY`, `SCHEMA`, `ARCHIVE`/`MEM`
//!   for the read engine; the embedded site is chosen at build time
//!   (`MEMSTEAD_SERVE_SITE_DIST`); `MEMSTEAD_SERVE_BIND` is the explicit listen
//!   address (unset: `PORT` → `0.0.0.0:$PORT`, else loopback `127.0.0.1:8080`).
//! - `MEMSTEAD_SESSION_*` — the sketch side: `SCHEMA` (the writable sketch
//!   mem), `CONTENT_DIR`/`CONTENT_ARCHIVE` + `CONTENT_SCHEMA` + `CONTENT_MEM`
//!   (the read-only content the agent orients against), `TTL_SECS`,
//!   `ENTITY_CAP`, `MAX` (live-session ceiling — shed, don't OOM, on a spike),
//!   `VIEW_BASE`, `RATE_PER_SEC`/`RATE_BURST` (rate limit keyed on the forwarded
//!   client IP).
//! - `MEMSTEAD_SOFT_LAUNCH` — the launch gate, one name shared with the
//!   .com/.io surfaces and the embedded static face's build. Default ON
//!   (`0`/`off`/`false` to go public): read pages and the sketch `/mcp` mount
//!   under `/try`, matching the links a gate-ON static face emits. The static
//!   face and this binary MUST agree — the face is built with the same
//!   variable, so set it once for both build and runtime.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use memstead_base::MountStorage;
use memstead_serve::session::{
    CONTENT_MEM_NAME, DEFAULT_SESSION_ENTITY_CAP, DEFAULT_SESSION_MAX, IdGen, SessionRegistry,
};
use memstead_serve::{
    AppState, EMBEDDED_CONTENT_MEM, build_unified_app, materialize_embedded_content,
    mount_read_only,
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
    // ---- read side: the website + the HTML read pages over a sealed mem ----
    let authority =
        std::env::var("MEMSTEAD_SERVE_AUTHORITY").unwrap_or_else(|_| "memstead".to_string());
    let schema_pin =
        std::env::var("MEMSTEAD_SERVE_SCHEMA").unwrap_or_else(|_| "default@1.0.0".to_string());
    // Explicit bind wins; else `PORT` binds all interfaces (containers);
    // else loopback — local runs must not broadcast to the LAN.
    let bind = memstead_serve::resolve_bind("MEMSTEAD_SERVE_BIND");
    let schema: memstead_schema::SchemaRef = schema_pin
        .parse()
        .map_err(|e| format!("invalid MEMSTEAD_SERVE_SCHEMA {schema_pin:?}: {e}"))?;
    let (mem, storage) = if let Ok(archive) = std::env::var("MEMSTEAD_SERVE_ARCHIVE") {
        let mem = std::env::var("MEMSTEAD_SERVE_MEM").unwrap_or_else(|_| "flagship".to_string());
        (
            mem,
            MountStorage::Archive {
                path: PathBuf::from(archive),
            },
        )
    } else {
        let dir = materialize_embedded_content()?;
        let mem = std::env::var("MEMSTEAD_SERVE_MEM")
            .unwrap_or_else(|_| EMBEDDED_CONTENT_MEM.to_string());
        (mem, MountStorage::Folder { path: dir })
    };
    let api_engine = mount_read_only(mem, schema, storage)?;
    // Serves the authority's own curated read tier → first-party by default;
    // `MEMSTEAD_SERVE_ORIGIN=third-party` opts out for generic deployments.
    let content_origin = match std::env::var("MEMSTEAD_SERVE_ORIGIN").ok().as_deref() {
        Some("third-party") => memstead_base::render::OriginClass::ThirdParty,
        _ => memstead_base::render::OriginClass::FirstParty,
    };
    // Soft-launch gate (default ON; one env shared with the .com/.io surfaces
    // and the embedded static face's build — the face's links and this
    // binary's routes relocate between `/try/…` and `/…` in lockstep only
    // when both read the same value). Same parse as memstead-session-serve.
    let soft_launch = !matches!(
        std::env::var("MEMSTEAD_SOFT_LAUNCH")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "off" | "false"
    );
    let state = AppState::new(api_engine, authority)
        .with_content_origin(content_origin)
        .with_soft_launch(soft_launch);

    // ---- sketch side: the writable connection-born `/mcp` + view data ----
    let sketch_pin =
        std::env::var("MEMSTEAD_SESSION_SCHEMA").unwrap_or_else(|_| "default@1.0.0".to_string());
    let sketch_schema: memstead_schema::SchemaRef = sketch_pin
        .parse()
        .map_err(|e| format!("invalid MEMSTEAD_SESSION_SCHEMA {sketch_pin:?}: {e}"))?;
    // Absent → relative `/v/<id>`; set to e.g. `https://memstead.ai` for an
    // absolute view URL in the MCP handshake.
    let view_base = std::env::var("MEMSTEAD_SESSION_VIEW_BASE").unwrap_or_default();
    let content_storage = if let Ok(dir) = std::env::var("MEMSTEAD_SESSION_CONTENT_DIR") {
        MountStorage::Folder {
            path: PathBuf::from(dir),
        }
    } else if let Ok(archive) = std::env::var("MEMSTEAD_SESSION_CONTENT_ARCHIVE") {
        MountStorage::Archive {
            path: PathBuf::from(archive),
        }
    } else {
        match materialize_embedded_content() {
            Ok(dir) => MountStorage::Folder { path: dir },
            Err(e) => {
                eprintln!(
                    "memstead-serve-full: could not materialize embedded content ({e}); \
mounting an empty placeholder"
                );
                MountStorage::InMemory
            }
        }
    };
    let content_pin =
        std::env::var("MEMSTEAD_SESSION_CONTENT_SCHEMA").unwrap_or_else(|_| sketch_pin.clone());
    let content_schema: memstead_schema::SchemaRef = content_pin
        .parse()
        .map_err(|e| format!("invalid MEMSTEAD_SESSION_CONTENT_SCHEMA {content_pin:?}: {e}"))?;
    let content_mem_name = std::env::var("MEMSTEAD_SESSION_CONTENT_MEM")
        .unwrap_or_else(|_| CONTENT_MEM_NAME.to_string());
    let ttl = Duration::from_secs(env_u64("MEMSTEAD_SESSION_TTL_SECS", 1800));
    let cap = env_u64(
        "MEMSTEAD_SESSION_ENTITY_CAP",
        DEFAULT_SESSION_ENTITY_CAP as u64,
    ) as usize;
    // Global live-session ceiling: bounds memory under a traffic spike (each
    // live session is its own in-memory engine). Raise with the box's RAM.
    let max_sessions = env_u64("MEMSTEAD_SESSION_MAX", DEFAULT_SESSION_MAX as u64) as usize;
    let id_gen: IdGen = Arc::new(|| format!("s_{}", Uuid::new_v4().simple()));
    let registry = SessionRegistry::new(
        content_storage,
        content_schema,
        content_mem_name,
        ttl,
        cap,
        id_gen,
    )
    .with_max_sessions(max_sessions);

    // Drive idle eviction so abandoned sessions release their mems.
    {
        let registry = registry.clone();
        let sweep_every = (ttl / 4).max(Duration::from_secs(30));
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(sweep_every);
            loop {
                ticker.tick().await;
                let evicted = registry.sweep_expired(Instant::now());
                if evicted > 0 {
                    eprintln!("memstead-serve-full: evicted {evicted} idle session(s)");
                }
            }
        });
    }

    let per_second = env_u64("MEMSTEAD_SESSION_RATE_PER_SEC", 5);
    let burst = env_u64("MEMSTEAD_SESSION_RATE_BURST", 60) as u32;
    let app = build_unified_app(
        state,
        registry,
        sketch_schema,
        view_base.clone(),
        per_second,
        burst,
    );

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    let view_base_display = if view_base.is_empty() {
        "<relative /v/{id}>".to_string()
    } else {
        view_base
    };
    eprintln!(
        "memstead-serve-full listening on {bind} (site + HTML read pages + writable /mcp; \
sketch_schema={sketch_pin}, view_base={view_base_display})"
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
