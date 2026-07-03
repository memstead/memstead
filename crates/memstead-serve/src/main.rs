//! `memstead-serve` — a generic read-only HTTP/MCP server over a sealed mem.
//!
//! Mounts one mem read-only and serves the read-only HTTP surface. With no
//! archive configured it serves the embedded curated "what is Memstead"
//! content mem, so the read tier works zero-setup. Every deployment specific
//! is an input from the environment:
//!
//! - `MEMSTEAD_SERVE_ARCHIVE` — path to a sealed `.mem` archive. When unset,
//!   the binary serves the embedded curated content mem.
//! - `MEMSTEAD_SERVE_AUTHORITY` — published authority identity, e.g. the host
//!   this is served under (default: `memstead`)
//! - `MEMSTEAD_SERVE_MEM` — mem name (default: `flagship` for an archive,
//!   `memstead` for the embedded content)
//! - `MEMSTEAD_SERVE_SCHEMA` — schema pin `name@x.y.z` (default: `default@1.0.0`)
//! - `MEMSTEAD_SERVE_BIND` — explicit listen address. Unset: a set `PORT`
//!   (Railway and most PaaS inject it) binds `0.0.0.0:$PORT`; otherwise
//!   loopback `127.0.0.1:8080`.
//!
//! The optional embedded static site is selected at build time via
//! `MEMSTEAD_SERVE_SITE_DIST` (see `build.rs`); with none configured the binary
//! serves a built-in placeholder landing.

use std::net::SocketAddr;
use std::path::PathBuf;

use memstead_base::MountStorage;
use memstead_serve::{
    AppState, EMBEDDED_CONTENT_MEM, ReadOnlyMcpServer, build_app, materialize_embedded_content,
    mount_read_only,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // Mem source: an explicit sealed archive, else the embedded curated
    // content mem so the read tier serves real content with no setup.
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
        eprintln!(
            "memstead-serve: no MEMSTEAD_SERVE_ARCHIVE set; serving the embedded curated \
content mem from {}",
            dir.display()
        );
        let mem = std::env::var("MEMSTEAD_SERVE_MEM")
            .unwrap_or_else(|_| EMBEDDED_CONTENT_MEM.to_string());
        (mem, MountStorage::Folder { path: dir })
    };
    // `/api` and `/mcp` each get their own read-only engine over the same
    // sealed archive — the mount is immutable, so two readers need no
    // coordination, and it sidesteps the std-vs-tokio mutex split between the
    // two surfaces.
    let api_engine = mount_read_only(mem.clone(), schema.clone(), storage.clone())?;
    let mcp_engine = mount_read_only(mem, schema, storage)?;
    // This binary serves the authority's own curated content (the embedded
    // "what is Memstead" mem, or a deliberately-configured archive), so it
    // vouches for it as first-party. An operator pointing it at arbitrary
    // content can suppress that with `MEMSTEAD_SERVE_ORIGIN=third-party`.
    let content_origin = match std::env::var("MEMSTEAD_SERVE_ORIGIN").ok().as_deref() {
        Some("third-party") => memstead_base::render::OriginClass::ThirdParty,
        _ => memstead_base::render::OriginClass::FirstParty,
    };
    // Soft-launch gate (default ON; the launch flip is one env shared with the
    // .com/.io surfaces). OFF only on an explicit off-ish value.
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
    let mcp_server = ReadOnlyMcpServer::from_engine(mcp_engine);
    // 5 req/s steady with a burst of 60 per IP — generous for a read surface,
    // enough to blunt abuse.
    let app = build_app(state, mcp_server, 5, 60);

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    eprintln!("memstead-serve listening on {bind}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
