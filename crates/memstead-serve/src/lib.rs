//! `memstead-serve` — a generic, deployment-agnostic read-only server exposing
//! one sealed Memstead mem to agents over HTTP.
//!
//! A mem is reached two ways and only two ways: an MCP-capable agent connects
//! over `/mcp` (the full read tool surface, including search); everyone else —
//! a no-MCP agent's fetch tool, a browser, a crawler — reads it as plain,
//! link-navigated HTML pages (`/agent`, `/overview`, `/entity/<id>`,
//! `/entities`, `/schema`). The HTML pages carry root-relative hrefs so the
//! served document is deployment-agnostic.
//!
//! Coordinated surfaces: the native MCP endpoint (`/mcp`) scoped to the read
//! tools, the HTML read pages above, a self-describing runbook (`/`,
//! `/llms.txt`) that points an agent at `/agent`, a discovery manifest
//! (`/.well-known/memstead-authority.json`), per-IP rate limiting, and an
//! optional embedded static site served to browsers on `/`.
//!
//! Every route reaches the engine through its **read** surface only; the
//! router carries no mutating engine operation (see the read-only invariant
//! test). The production mount is a sealed `.mem` archive
//! ([`MountStorage::Archive`]) which refuses writes at the backend as
//! defense-in-depth, but read-only-by-construction is the primary guarantee.
//! Which mem, the authority identity, and the static site are all inputs —
//! nothing here is tied to a particular deployment.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use include_dir::{Dir, include_dir};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

/// The optional static site, embedded at compile time by `build.rs` from the
/// directory named by `MEMSTEAD_SERVE_SITE_DIST`. With no site configured the
/// embed holds only the `.gitkeep` placeholder, and the handlers fall back to a
/// built-in placeholder landing.
static SITE: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/site");

use memstead_base::{Engine, EntityId, Mount, MountCapability, MountLifecycle, MountStorage, render};
use memstead_base::render::OriginClass;

/// Writable multi-tenant remote-MCP session server — the additive
/// counterpart to the read-only surface in this module.
pub mod session;

/// Coordinate-free `{nodes, edges, communities}` graph projection for the
/// live per-session stream.
pub mod graph;

/// Shared service state. `engine` is the long-lived read-only engine behind an
/// async mutex (the engine is `Send` but not `Sync`); `authority` is the
/// identity this service publishes in its discovery manifest.
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Mutex<Engine>>,
    pub authority: String,
    /// Trust origin this deployment vouches for its read-only served
    /// content. Defaults to [`OriginClass::ThirdParty`] — an arbitrary
    /// served mem is untrusted data until the operator declares
    /// otherwise. The curated memstead.ai read tier sets
    /// [`OriginClass::FirstParty`] via [`Self::with_content_origin`].
    /// Surfaced per read-only mem on the discovery manifest; a publisher
    /// cannot forge it (it is operator config, not content).
    pub content_origin: OriginClass,
    /// Soft-launch gate. ON (the production default, via
    /// `MEMSTEAD_SOFT_LAUNCH`): `/` is a dead-end for every client — the
    /// markdown agent runbook is replaced by a hint-free holding note and the
    /// human landing drops its `Link: </llms.txt>` discovery header — and
    /// `robots.txt` allows `/` while naming nothing real. OFF restores the
    /// public agent front door (the runbook at `/`, the `Link` header).
    pub soft_launch: bool,
}

impl AppState {
    pub fn new(engine: Engine, authority: impl Into<String>) -> Self {
        Self {
            engine: Arc::new(Mutex::new(engine)),
            authority: authority.into(),
            content_origin: OriginClass::ThirdParty,
            // Tests drive the public surface by default; production flips this
            // ON from the environment (see `main`). Set per-test for the gate.
            soft_launch: false,
        }
    }

    /// Enable the soft-launch gate. `main` calls this from the
    /// `MEMSTEAD_SOFT_LAUNCH` env so the launch flip is one variable, shared
    /// with the .com/.io surfaces.
    pub fn with_soft_launch(mut self, on: bool) -> Self {
        self.soft_launch = on;
        self
    }

    /// The path prefix every real read route hides behind while the gate is ON:
    /// `/try` when gated, `""` when public. Threaded through every link the read
    /// pages, runbook, and manifest emit, so one prefix relocates the whole read
    /// surface between `/try/<page>` (gated) and `/<page>` (public).
    pub fn read_prefix(&self) -> &'static str {
        if self.soft_launch { "/try" } else { "" }
    }

    /// Declare the trust origin this deployment vouches for its served
    /// read-only content. The curated read tier passes
    /// [`OriginClass::FirstParty`]; a generic deployment leaves the safe
    /// third-party default.
    pub fn with_content_origin(mut self, origin: OriginClass) -> Self {
        self.content_origin = origin;
        self
    }
}

/// Build a read-only [`Engine`] mounting one mem from `storage`. Production
/// passes [`MountStorage::Archive`] (a sealed `.mem`); tests pass
/// [`MountStorage::Folder`]. The mount is always [`MountCapability::ReadOnly`].
pub fn mount_read_only(
    mem: impl Into<String>,
    schema: memstead_schema::SchemaRef,
    storage: MountStorage,
) -> Result<Engine, memstead_base::EngineError> {
    let mount = Mount {
        mem: mem.into(),
        schema: Some(schema),
        storage,
        capability: MountCapability::ReadOnly,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let backend = memstead_base::workspace_store::instantiate_lean_backend(&mount)
        .map_err(|e| memstead_base::EngineError::Mem(e.to_string()))?;
    Engine::from_mounts(vec![(mount, backend)])
}

/// The curated "what is Memstead" content mem, embedded at compile time so
/// both servers ship real content with no external files. A flat folder of
/// `.md` concept entities; the folder backend reads them when mounted.
static EMBEDDED_CONTENT: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/content/memstead");

/// Mem name the embedded curated content mem mounts under.
pub const EMBEDDED_CONTENT_MEM: &str = "memstead";

/// Write the embedded curated content mem to a fresh temp directory and
/// return its path. Mounted read-only as the content mem when no external
/// source is configured, so the read tier and the session server both serve
/// real "what is Memstead" content out of the box.
pub fn materialize_embedded_content() -> std::io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("memstead-content-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    for file in EMBEDDED_CONTENT.files() {
        let name = file
            .path()
            .file_name()
            .ok_or_else(|| std::io::Error::other("embedded content file has no name"))?;
        std::fs::write(dir.join(name), file.contents())?;
    }
    Ok(dir)
}

/// The axum router for the read-only surface. Kept separate from `main` so
/// tests can drive it with `tower::ServiceExt::oneshot`.
pub fn build_router(state: AppState) -> Router {
    // The dynamic read surface re-renders whenever the graph (or a deploy)
    // changes, so it must never be cached — a stale render is exactly what bites
    // an agent after a deploy (a `web_fetch` tool that caches will otherwise hand
    // back yesterday's bare-id overview). `.layer` wraps only the routes added
    // before it; the static Astro `.fallback` below is added after, so the site's
    // `_astro/*` chunks stay cacheable.
    // The read surface — the runbook, the discovery manifest, and the HTML read
    // pages. OFF (public): top-level, the agent front door. ON (gated): the same
    // routes move under `/try` and carry `X-Robots-Tag: noindex`, so top-level
    // probes (`/agent`, `/llms.txt`, `/.well-known`) simply 404 and only a
    // handed-out `/try` URL gets in. Mounted as individual `/try/<page>` routes
    // (not a nest) so `/try` itself stays free for the embedded Astro experience
    // page that `static_fallback` serves. The links the pages, runbook, and
    // manifest emit are prefix-aware (see `AppState::read_prefix`), so they point
    // within `/try` while gated.
    let read = if state.soft_launch {
        Router::new()
            .route("/try/llms.txt", get(runbook_handler))
            .route("/try/.well-known/memstead-authority.json", get(authority_handler))
            .route("/try/agent", get(agent_handler))
            .route("/try/overview", get(overview_handler))
            .route("/try/entities", get(entities_handler))
            .route("/try/entity/{id}", get(entity_handler))
            .route("/try/schema", get(schema_handler))
            .layer(axum::middleware::from_fn(gated_noindex))
    } else {
        Router::new()
            .route("/llms.txt", get(runbook_handler))
            .route("/.well-known/memstead-authority.json", get(authority_handler))
            .route("/agent", get(agent_handler))
            .route("/overview", get(overview_handler))
            .route("/entities", get(entities_handler))
            .route("/entity/{id}", get(entity_handler))
            .route("/schema", get(schema_handler))
    };

    Router::new()
        .route("/", get(root_handler))
        .route("/healthz", get(healthz_handler))
        .route("/robots.txt", get(robots_handler))
        .merge(read)
        .layer(axum::middleware::map_response(no_store))
        // Every unmatched GET path falls through to the embedded Astro build
        // (the landing's `_astro/*` chunks, the graph data, favicon, …) — kept
        // cacheable, so the no-store layer above deliberately does not reach it.
        .fallback(static_fallback)
        .with_state(state)
}

/// Mark a dynamic response uncacheable, so intermediaries and fetch-tool caches
/// don't serve a stale render after the graph or a deploy changes.
async fn no_store(mut resp: Response) -> Response {
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// Tag every response from the gated `/try` read subtree with
/// `X-Robots-Tag: noindex` — the HTTP-header form of the `noindex` meta, so the
/// real read pages are reachable by a handed-out URL but never indexed.
async fn gated_noindex(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert("x-robots-tag", HeaderValue::from_static("noindex"));
    resp
}

/// Build a typed `{code, message, details}` error response at `status`.
fn error_response(
    status: StatusCode,
    code: &str,
    message: impl Into<String>,
    details: serde_json::Value,
) -> Response {
    let body = memstead_base::ops::envelope(code, message.into(), details);
    (status, axum::Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// Markdown responses (the `/` + `/llms.txt` runbook)
// ---------------------------------------------------------------------------

fn markdown_response(body: String) -> Response {
    (
        [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
        body,
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Runbook (`/` markdown + `/llms.txt`)
// ---------------------------------------------------------------------------

/// Absolute origin for the published authority. Real hosts are HTTPS; localhost
/// (dev) is HTTP. The runbook and the discovery manifest print FULLY-QUALIFIED
/// URLs so a sandboxed fetch agent — many only permit absolute URLs that
/// literally appeared on a loaded page — can follow them from those entry
/// points. (The HTML pages themselves use root-relative hrefs so the served
/// document is deployment-agnostic; the manifest and runbook are the absolute
/// entry points an allowlist-bound agent copies in.)
pub fn base_url(authority: &str) -> String {
    let scheme = if authority.starts_with("localhost")
        || authority.starts_with("127.0.0.1")
        || authority.starts_with("0.0.0.0")
    {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{authority}")
}

/// The self-describing agent runbook (`/` markdown + `/llms.txt`). A short
/// breadcrumb that points at the canonical `/agent` instruction page and names
/// the HTML read pages; the instruction content itself lives on `/agent`.
/// URLs are absolute so allowlist-bound fetch agents — many only follow URLs
/// that literally appeared in a result — can copy them as-is (see [`base_url`]).
pub fn runbook(authority: &str, read_prefix: &str) -> String {
    let base = base_url(authority);
    format!(
        "# {authority} — agent runbook\n\
\n\
A Memstead knowledge graph, served as plain HTML you can read with no tools and \
no setup. It is read-only over HTTP.\n\
\n\
## Start here\n\
\n\
`GET {base}{read_prefix}/agent` — how to read this graph (what it is, how to \
navigate it, how to talk to the user about it). The canonical entry point.\n\
\n\
Then: `{base}{read_prefix}/overview` (the map), `{base}{read_prefix}/entities` \
(the complete index), `{base}{read_prefix}/entity/<id>` (one entity), \
`{base}{read_prefix}/schema` (the vocabulary). Every page links to the next — \
follow the links.\n\
\n\
## MCP (capable clients) — search, write, a private sketch mem\n\
\n\
There is no search on the HTML surface; to search this graph, or to write to \
one, attach the native streamable-HTTP MCP endpoint at `{base}{read_prefix}/mcp`. \
It reads this graph and mints a private, writable sketch mem per connection \
(watch it at the `/v/<id>` link the handshake returns).\n\
`claude mcp add --transport http {authority} {base}{read_prefix}/mcp`\n"
    )
}

async fn runbook_handler(State(state): State<AppState>) -> Response {
    markdown_response(runbook(&state.authority, state.read_prefix()))
}

// ---------------------------------------------------------------------------
// `/healthz` — liveness probe for deployment platforms
// ---------------------------------------------------------------------------

/// A dependency-free liveness probe for the deployment platform (Railway's
/// `healthcheckPath`). It asserts only that the process is accepting
/// connections — the read routes cover whether the mem is queryable — so it
/// stays a cheap constant 200 and never touches the engine lock.
async fn healthz_handler() -> Response {
    axum::Json(json!({ "status": "ok" })).into_response()
}

// ---------------------------------------------------------------------------
// `/` — content-negotiated human face vs agent runbook, plus static serving
// ---------------------------------------------------------------------------

/// `Accept: text/html` selects the human landing. Everything else
/// (`text/markdown`, `*/*`, `application/json`, or an absent header — the agent
/// default) gets the runbook. Note `*/*` is deliberately the runbook, not HTML:
/// a bare fetch agent is the visitor this surface exists to serve.
fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|a| a.contains("text/html"))
        .unwrap_or(false)
}

/// Link-preview crawlers — the unfurlers behind X/Slack/Discord/iMessage cards
/// — identify by User-Agent but send unreliable `Accept` headers (often `*/*`),
/// which `wants_html` would answer with the card-less markdown runbook. The
/// Open Graph / Twitter Card tags they need live only in the HTML landing, so
/// match them by UA and serve HTML regardless of `Accept`. Substrings, matched
/// case-insensitively; extend as new unfurlers appear.
const PREVIEW_CRAWLER_UAS: &[&str] = &[
    "twitterbot",          // X
    "facebookexternalhit", // Facebook / Instagram / iMessage
    "facebot",
    "slackbot",        // Slack (Slackbot-LinkExpanding)
    "slack-imgproxy",  // Slack image fetch
    "discordbot",      // Discord
    "whatsapp",        // WhatsApp
    "linkedinbot",     // LinkedIn
    "telegrambot",     // Telegram
    "pinterest",       // Pinterest
    "redditbot",       // Reddit
    "skypeuripreview", // Skype
    "applebot",        // Apple (Siri / Spotlight / iMessage)
    "embedly",         // Embedly (powers many editors)
    "iframely",        // Iframely
    "vkshare",         // VK
];

/// True when the request looks like one of `PREVIEW_CRAWLER_UAS`.
fn is_preview_crawler(headers: &HeaderMap) -> bool {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|ua| {
            let ua = ua.to_ascii_lowercase();
            PREVIEW_CRAWLER_UAS.iter().any(|needle| ua.contains(needle))
        })
        .unwrap_or(false)
}

async fn root_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // A browser (`Accept: text/html`) or a link-preview crawler (matched by
    // User-Agent — its `Accept` is unreliable) gets the OG-tagged HTML landing;
    // every other client — the bare fetch agent — gets the markdown runbook.
    if wants_html(&headers) || is_preview_crawler(&headers) {
        landing_response(&state.authority, state.soft_launch)
    } else if state.soft_launch {
        // Gated: the agent gets a dead end, not the runbook. No reference to
        // `/agent`, `/llms.txt`, `/mcp`, `/try`, or any real path — an agent
        // landing here finds no trail onward.
        markdown_response(holding_note(&state.authority))
    } else {
        markdown_response(runbook(&state.authority, state.read_prefix()))
    }
}

/// The gated `/` markdown body — a hint-free dead end. Deliberately names no
/// endpoint: the real surface is reachable only by a URL handed out elsewhere.
fn holding_note(authority: &str) -> String {
    format!("# {authority}\n\ncoming soon\n")
}

/// Serve the human landing: the embedded Astro `index.html` when the site has
/// been built, otherwise a built-in placeholder carrying the one-paste
/// bootstrap so the human face works even on a Rust-only checkout.
fn landing_response(authority: &str, soft_launch: bool) -> Response {
    let mut resp = if let Some(file) = SITE.get_file("index.html") {
        html_response(file.contents().to_vec(), "text/html; charset=utf-8")
    } else {
        html_response(
            placeholder_landing(authority).into_bytes(),
            "text/html; charset=utf-8",
        )
    };
    // Discovery for the agent that arrives at the *human* page (`Accept:
    // text/html` — a person said "look at this site"). The `Link` header points
    // it at the machine runbook so it can switch to the agent surface without
    // scraping the HTML; it mirrors the `<link rel="alternate">` the embedded
    // site carries in its <head>. The in-page sr-only bootstrap covers agents
    // that read the body instead of the headers — belt and suspenders.
    //
    // While the gate is ON this discovery header is exactly the hint the holding
    // page must not leak, so it is dropped — the embedded holding page's <head>
    // drops the matching `<link rel="alternate">` at build time to stay in sync.
    if !soft_launch {
        resp.headers_mut().insert(
            header::LINK,
            HeaderValue::from_static("</llms.txt>; rel=\"alternate\"; type=\"text/markdown\""),
        );
    }
    resp
}

/// `GET /robots.txt` — generated so the toggle controls it (it shadows any
/// embedded `site/robots.txt`). ON: allow `/` (the holding page must be
/// fetchable) and name nothing real. OFF: crawlable — not the pre-gate
/// `Disallow: /`, which was the broken lock this work removes.
async fn robots_handler(State(state): State<AppState>) -> Response {
    let body = if state.soft_launch {
        "User-agent: *\nAllow: /\n"
    } else {
        "User-agent: *\nDisallow:\n"
    };
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
        .into_response()
}

/// Fallback for every GET path no route claimed — serves files from the
/// embedded Astro build with a best-effort content-type, with a directory-index
/// fallback (`/foo` → `foo/index.html`) matching Astro's output.
async fn static_fallback(req: Request<Body>) -> Response {
    let path = req.uri().path().trim_start_matches('/').to_string();
    let dir_index = if path.is_empty() {
        "index.html".to_string()
    } else {
        format!("{}/index.html", path.trim_end_matches('/'))
    };
    // The `/v/{id}` live-view page is pre-rendered once at the placeholder route
    // `/v/__id__/`; serve it for any single-segment session id (the viewer's
    // client reads the real id from `location.pathname`). The data children
    // `/v/{id}/graph|stream|export` carry a further path segment and are handled
    // by explicit routes upstream, so they never reach this fallback.
    let viewer = path
        .strip_prefix("v/")
        .filter(|rest| !rest.is_empty() && !rest.contains('/') && *rest != "__id__")
        .map(|_| "v/__id__/index.html");
    for candidate in [Some(path.as_str()), Some(dir_index.as_str()), viewer]
        .into_iter()
        .flatten()
    {
        if candidate.is_empty() {
            continue;
        }
        if let Some(file) = SITE.get_file(candidate) {
            let mime = mime_guess::from_path(candidate)
                .first_or_octet_stream()
                .to_string();
            return html_response(file.contents().to_vec(), &mime);
        }
    }
    let mut resp = html_response(
        b"<!doctype html><meta charset=utf-8><title>Not found</title><h1>404</h1>".to_vec(),
        "text/html; charset=utf-8",
    );
    *resp.status_mut() = StatusCode::NOT_FOUND;
    resp
}

/// Build an HTML/asset response with `nosniff` and a CSP that admits the
/// graph stack (`3d-force-graph` + `three.js` need `unsafe-eval`).
fn html_response(body: Vec<u8>, content_type: &str) -> Response {
    let mut resp = Response::new(Body::from(body));
    if let Ok(ct) = HeaderValue::from_str(content_type) {
        resp.headers_mut().insert(header::CONTENT_TYPE, ct);
    }
    resp.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    resp.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; \
             script-src 'self' 'unsafe-inline' 'unsafe-eval'; \
             style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
             font-src 'self' https://fonts.gstatic.com; \
             img-src 'self' data:; \
             connect-src 'self'",
        ),
    );
    resp
}

/// The built-in placeholder landing served until the Astro build is embedded.
/// Carries the **one-paste bootstrap** — the `claude mcp add` line for capable
/// clients and the bare URL to hand a fetch-only agent — so the human face is
/// actionable even before the graph build ships.
pub fn placeholder_landing(authority: &str) -> String {
    format!(
        "<!doctype html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{authority} — a knowledge graph you can point an agent at</title>\n\
</head>\n\
<body>\n\
<main>\n\
<h1>{authority}</h1>\n\
<p>A read-only Memstead knowledge graph — typed, interconnected markdown entities. \
Point an agent at it, or explore the graph yourself.</p>\n\
<!-- The interactive 3D graph mounts here once the Astro build is embedded. -->\n\
<div id=\"mem-graph\"></div>\n\
<h2>Point your agent here</h2>\n\
<p>MCP-capable clients:</p>\n\
<pre><code>claude mcp add --transport http {authority} {authority}/mcp</code></pre>\n\
<p>Fetch-only agents — hand them the bare URL:</p>\n\
<pre><code>{authority}</code></pre>\n\
</main>\n\
</body>\n\
</html>\n"
    )
}

// ---------------------------------------------------------------------------
// Discovery manifest (`/.well-known/memstead-authority.json`)
// ---------------------------------------------------------------------------

/// Minimal discovery manifest: the authority identity, the published mem(s)
/// each with its schema pin, and the endpoint URLs. The first real
/// implementation of `VISION.md`'s `memstead-authority.json` standard.
async fn authority_handler(State(state): State<AppState>) -> Response {
    let engine = state.engine.lock().await;
    let mems: Vec<serde_json::Value> = engine
        .mounts()
        .iter()
        .map(|m| {
            // Machine-readable trust origin per served mem so a
            // consuming agent/host knows whether to treat the content as
            // authority-vouched or quoted, untrusted data. A writable
            // (session sketch) mem is first-party — the user's own work.
            // A read-only served mem's origin is a *deployment fact* the
            // serving authority declares (`content_origin`): the curated
            // memstead.ai read tier vouches for its content as first-party;
            // an arbitrary served mem defaults to third-party (untrusted)
            // until the deployment opts in. A publisher cannot forge this —
            // it is set by the operator running the server, not by content.
            let origin = match engine.mem_origin_class(&m.mem) {
                OriginClass::FirstParty => OriginClass::FirstParty,
                OriginClass::ThirdParty => state.content_origin,
            };
            json!({
                "name": m.mem,
                "schema": m.schema.as_ref().map(|s| s.as_display()).unwrap_or_default(),
                "origin": origin.as_wire(),
            })
        })
        .collect();
    // Absolute endpoint URLs — a discovery client (and allowlist-bound fetch
    // agents) can follow them without reconstructing the origin.
    let base = base_url(&state.authority);
    // Every real endpoint carries the soft-launch mount prefix (`/try` while
    // gated) — the read pages here, and `/mcp` on the session binary, which gates
    // to `/try/mcp` under the same env.
    let rp = state.read_prefix();
    let manifest = json!({
        "authority": state.authority,
        "base_url": base,
        "mems": mems,
        "endpoints": {
            "mcp": format!("{base}{rp}/mcp"),
            "agent": format!("{base}{rp}/agent"),
            "overview": format!("{base}{rp}/overview"),
            "entities": format!("{base}{rp}/entities"),
            "entity": format!("{base}{rp}/entity/<id>"),
            "schema": format!("{base}{rp}/schema"),
        },
    });
    axum::Json(manifest).into_response()
}

// ---------------------------------------------------------------------------
// Relative-link projection (HTML pages)
// ---------------------------------------------------------------------------
//
// The shared engine renders (also used by the MCP tools, which resolve ids
// through tool calls, not URLs) emit bare entity ids and bare `[[<id>]]`
// wiki-links by design. The HTML read pages rewrite those into root-relative
// markdown links `[Title](/entity/<id>)` here — which the markdown→HTML pass
// then turns into `<a href="/entity/<id>">Title</a>` anchors. Root-relative so
// the served HTML is deployment-agnostic; the shared render is left untouched.

/// Sanitize an entity title for use as markdown link text: link text can't carry
/// raw `[`/`]`, so fold them to parens. Entity titles are plain prose in practice.
fn link_text(title: &str) -> String {
    title.replace('[', "(").replace(']', ")")
}

/// The root-relative href for one entity's HTML page. `href_prefix` is the
/// soft-launch mount prefix (`/try` while gated, `""` public) — the single
/// chokepoint every entity link flows through.
fn entity_href(href_prefix: &str, id: &str) -> String {
    format!("{href_prefix}/entity/{id}")
}

/// A root-relative markdown link `[Title](/entity/<id>)` for one entity. The
/// markdown→HTML pass converts it to an `<a href>` anchor.
fn entity_md_link(href_prefix: &str, id: &str, title: &str) -> String {
    format!("[{}]({})", link_text(title), entity_href(href_prefix, id))
}

/// Rewrite anchored bare-id references to relative markdown links in place,
/// PRESERVING the `anchor`/`suffix` text (an overview `- `…`\n` cluster bullet,
/// a ` `…` →` community-bridge operand). The anchors stop a shorter id matching
/// inside a longer one (`engine--schema` vs `engine--schema-registry`).
fn linkify_anchored(
    mut body: String,
    id_titles: &[(String, String)],
    href_prefix: &str,
    anchor: &str,
    suffix: &str,
) -> String {
    for (id, title) in id_titles {
        body = body.replace(
            &format!("{anchor}{id}{suffix}"),
            &format!("{anchor}{}{suffix}", entity_md_link(href_prefix, id, title)),
        );
    }
    body
}

/// Rewrite body wiki-links `[[<id>]]` to relative markdown links, CONSUMING the
/// brackets — so an entity page's relationships and inline references become
/// followable `/entity/<id>` anchors (the shared render emits bare `[[<id>]]`,
/// dead for an HTML reader).
fn linkify_wikilinks(mut body: String, id_titles: &[(String, String)], href_prefix: &str) -> String {
    for (id, title) in id_titles {
        body = body.replace(&format!("[[{id}]]"), &entity_md_link(href_prefix, id, title));
    }
    body
}

/// `(id, title)` for every entity in the store — the lookup the linkifiers need
/// to render `[Title](/entity/<id>)`. Falls back to the id as link text when a
/// title can't be read.
fn entity_id_titles(engine: &Engine) -> Vec<(String, String)> {
    let ids: Vec<String> = engine.store().all_ids().map(|i| i.to_string()).collect();
    ids.into_iter()
        .map(|id| {
            let title = engine
                .get_entity(&EntityId::canonical(&id))
                .map(|e| e.title.clone())
                .unwrap_or_else(|| id.clone());
            (id, title)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// `/overview` — the graph map as HTML
// ---------------------------------------------------------------------------

/// Rewrite the overview's bare-id references to relative markdown links, in the
/// three anchored shapes the shared render uses: cluster-member bullets
/// (`- <id>`) and the two community-bridge operands (` <id> →`, `→ <id>\n`).
fn linkify_overview(markdown: String, id_titles: &[(String, String)], href_prefix: &str) -> String {
    let mut md = markdown;
    if !md.ends_with('\n') {
        md.push('\n');
    }
    md = linkify_anchored(md, id_titles, href_prefix, "- ", "\n");
    md = linkify_anchored(md, id_titles, href_prefix, " ", " →");
    linkify_anchored(md, id_titles, href_prefix, "→ ", "\n")
}

async fn overview_handler(State(state): State<AppState>) -> Response {
    use memstead_engine::overview::{ComposeOverviewError, OverviewArgs, Surface, compose_overview};
    let mut guard = state.engine.lock().await;
    let args = OverviewArgs {
        include: &[],
        mem: None,
        rebuild: false,
        token_budget: memstead_engine::overview::DEFAULT_OVERVIEW_BUDGET,
        operator_mode: false,
    };
    match compose_overview(&mut guard, args, Surface::Mcp) {
        Ok(out) => {
            // The shared composer emits bare ids; rewrite them to relative
            // `/entity/<id>` links so the rendered HTML map is navigable.
            let id_titles = entity_id_titles(&guard);
            let rp = state.read_prefix();
            let markdown = linkify_overview(out.markdown, &id_titles, rp);
            let nav = nav_links(rp, &[
                ("/agent", "agent"),
                ("/entities", "entities"),
                ("/schema", "schema"),
            ]);
            let body = format!("{nav}{}", markdown_to_html_body(&markdown));
            let title = format!("{} — overview", state.authority);
            html_page_response(html_document(&title, &body))
        }
        Err(ComposeOverviewError::UnknownMem {
            name,
            writable_mems,
        }) => error_response(
            StatusCode::NOT_FOUND,
            "UNKNOWN_MEM",
            format!("unknown mem: \"{name}\""),
            json!({ "name": name, "writable_mems": writable_mems }),
        ),
        Err(e @ ComposeOverviewError::InvalidIncludeKeySchemaTypes) => error_response(
            StatusCode::BAD_REQUEST,
            "INVALID_INPUT",
            e.to_string(),
            serde_json::Value::Null,
        ),
    }
}

// ---------------------------------------------------------------------------
// HTML rendering — pure, classless, script-free pages
// ---------------------------------------------------------------------------
//
// The HTML read pages are syntactically framed documents using semantic tags
// only (no `<style>`, no `class=`, no `<script>`). They render the markdown the
// shared composer/renderer emits (overview, entity, schema) into HTML, with the
// entity references already rewritten to relative `/entity/<id>` links. All text
// is HTML-escaped so the documents stay valid for arbitrary entity content
// (escaping is for correct rendering; the inputs are trusted, not hostile).

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Replace markdown links `[text](url)` with `<a href="url">text</a>`. Operates
/// on already-escaped text — `[`, `]`, `(`, `)` aren't HTML-special, so the link
/// grammar survives escaping. Bare `[` with no following `](…)` is left as-is.
fn replace_md_links(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    loop {
        let Some(lb) = rest.find('[') else {
            out.push_str(rest);
            break;
        };
        let after_lb = &rest[lb + 1..];
        let Some(mid) = after_lb.find("](") else {
            out.push_str(rest);
            break;
        };
        let text = &after_lb[..mid];
        let after_mid = &after_lb[mid + 2..];
        let Some(rp) = after_mid.find(')') else {
            out.push_str(rest);
            break;
        };
        let url = &after_mid[..rp];
        out.push_str(&rest[..lb]);
        out.push_str("<a href=\"");
        out.push_str(url);
        out.push_str("\">");
        out.push_str(text);
        out.push_str("</a>");
        rest = &after_mid[rp + 1..];
    }
    out
}

/// Wrap each `delim`…`delim` pair in `open`…`close` (for `**bold**`, `` `code` ``).
/// An unmatched trailing `delim` is left literal.
fn wrap_md_pairs(input: &str, delim: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    loop {
        let Some(a) = rest.find(delim) else {
            out.push_str(rest);
            break;
        };
        let after = &rest[a + delim.len()..];
        let Some(b) = after.find(delim) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..a]);
        out.push_str(open);
        out.push_str(&after[..b]);
        out.push_str(close);
        rest = &after[b + delim.len()..];
    }
    out
}

/// Inline markdown → HTML for the overview subset: escape, then links, bold, code.
fn inline_html(s: &str) -> String {
    let escaped = esc_html(s);
    let linked = replace_md_links(&escaped);
    let bolded = wrap_md_pairs(&linked, "**", "<strong>", "</strong>");
    wrap_md_pairs(&bolded, "`", "<code>", "</code>")
}

fn flush_para(out: &mut String, para: &mut Vec<String>) {
    if !para.is_empty() {
        out.push_str("<p>");
        out.push_str(&inline_html(&para.join(" ")));
        out.push_str("</p>\n");
        para.clear();
    }
}

/// Render markdown (already entity-link-rewritten) into an HTML body fragment:
/// `#`/`##`/`###` → headings, `- ` → `<ul><li>`, blank-separated prose → `<p>`,
/// a leading frontmatter `---`…`---` block → a `<pre>`, and inline spans via
/// [`inline_html`]. Returns the inner body HTML; wrap it with [`html_document`].
fn markdown_to_html_body(md: &str) -> String {
    let mut out = String::new();
    let mut fm: Option<String> = None;
    let mut fm_done = false;
    let mut ul = false;
    let mut para: Vec<String> = Vec::new();

    for (idx, raw) in md.lines().enumerate() {
        let line = raw.trim_end();
        let t = line.trim_start();

        // Leading frontmatter (`---` … `---` on the first line) → a <pre> block.
        if fm.is_none() && !fm_done && idx == 0 && t == "---" {
            fm = Some(String::new());
            continue;
        }
        if let Some(buf) = fm.as_mut() {
            if t == "---" {
                let block = std::mem::take(buf);
                fm = None;
                fm_done = true;
                out.push_str("<pre>");
                out.push_str(&esc_html(block.trim_end()));
                out.push_str("</pre>\n");
            } else {
                buf.push_str(line);
                buf.push('\n');
            }
            continue;
        }

        if t.is_empty() {
            flush_para(&mut out, &mut para);
            if ul {
                out.push_str("</ul>\n");
                ul = false;
            }
        } else if let Some(rest) = t.strip_prefix("### ") {
            flush_para(&mut out, &mut para);
            if ul {
                out.push_str("</ul>\n");
                ul = false;
            }
            out.push_str("<h3>");
            out.push_str(&inline_html(rest));
            out.push_str("</h3>\n");
        } else if let Some(rest) = t.strip_prefix("## ") {
            flush_para(&mut out, &mut para);
            if ul {
                out.push_str("</ul>\n");
                ul = false;
            }
            out.push_str("<h2>");
            out.push_str(&inline_html(rest));
            out.push_str("</h2>\n");
        } else if let Some(rest) = t.strip_prefix("# ") {
            flush_para(&mut out, &mut para);
            if ul {
                out.push_str("</ul>\n");
                ul = false;
            }
            out.push_str("<h1>");
            out.push_str(&inline_html(rest));
            out.push_str("</h1>\n");
        } else if let Some(rest) = t.strip_prefix("- ") {
            flush_para(&mut out, &mut para);
            if !ul {
                out.push_str("<ul>\n");
                ul = true;
            }
            out.push_str("<li>");
            out.push_str(&inline_html(rest));
            out.push_str("</li>\n");
        } else {
            if ul {
                out.push_str("</ul>\n");
                ul = false;
            }
            para.push(t.to_string());
        }
    }
    flush_para(&mut out, &mut para);
    if ul {
        out.push_str("</ul>\n");
    }
    out
}

/// Frame a body fragment as a syntactically complete HTML document — doctype, a
/// minimal head (charset, viewport, title), and the body. Pure HTML: no
/// `<style>`, no `<script>`, no `class=`.
fn html_document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{}</title>\n</head>\n<body>\n{}</body>\n</html>\n",
        esc_html(title),
        body
    )
}

/// A `<nav>` of root-relative links between the graph's HTML pages, separated by
/// a middle dot (a text separator, not styling). `href_prefix` is the soft-launch
/// mount prefix (`/try` while gated, `""` public), prepended to each href.
fn nav_links(href_prefix: &str, links: &[(&str, &str)]) -> String {
    let mut s = String::from("<nav>");
    for (i, (href, label)) in links.iter().enumerate() {
        if i > 0 {
            s.push_str(" · ");
        }
        s.push_str(&format!("<a href=\"{}{}\">{}</a>", href_prefix, href, esc_html(label)));
    }
    s.push_str("</nav>\n");
    s
}

fn html_page_response(body: String) -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// `/agent` — the instruction page handed to a no-MCP agent
// ---------------------------------------------------------------------------

/// The canonical agent instruction page: what this is, how to read it, and how
/// to talk to the user about it. Stable instruction content (rarely changes),
/// kept short so it survives a fetch tool's summarisation. Pure HTML; the
/// authority and mem (the graph's subject) are interpolated.
fn agent_page(authority: &str, mem: &str, rp: &str) -> String {
    let nav = nav_links(rp, &[
        ("/overview", "overview"),
        ("/entities", "entities"),
        ("/schema", "schema"),
    ]);
    let a = esc_html(authority);
    let v = esc_html(mem);
    let body = format!(
        "{nav}\
<h1>{a} — for the agent reading this</h1>\n\
<p>This is Memstead: a knowledge graph of one subject ({v}), served as plain \
HTML so any agent can read it with no tools and no setup. It is read-only here.</p>\n\
<h2>How to read it</h2>\n\
<ul>\n\
<li>Start at <a href=\"{rp}/overview\">/overview</a> — the map: the subject's \
clusters and a link to every entity.</li>\n\
<li>Follow the links. Each <code>/entity/&lt;id&gt;</code> page links to the \
entities it relates to. Read several before you answer — the value is in the \
connections, not one note.</li>\n\
<li><a href=\"{rp}/entities\">/entities</a> is the complete, paginated index, for \
when /overview's map is truncated.</li>\n\
<li><a href=\"{rp}/schema\">/schema</a> is the type and relationship vocabulary.</li>\n\
<li>There is no search here. To search a graph, or to write to one, connect over \
MCP — the endpoint is in \
<a href=\"{rp}/.well-known/memstead-authority.json\">/.well-known/memstead-authority.json</a>. \
That is the full interface; this HTML is the read-only subset.</li>\n\
</ul>\n\
<h2>How to tell the user about it</h2>\n\
<ul>\n\
<li>Say this is Memstead's live, self-describing graph of {v} — not your own \
recollection. Cite entities by their title, with the link.</li>\n\
<li>Separate what the graph states from what you infer from it.</li>\n\
<li>If the user wants to search, contribute, or build their own graph, tell them \
Memstead exposes an MCP endpoint they can connect to.</li>\n\
</ul>\n"
    );
    html_document(&format!("{authority} — for agents"), &body)
}

async fn agent_handler(State(state): State<AppState>) -> Response {
    let engine = state.engine.lock().await;
    let mem = engine
        .mounts()
        .first()
        .map(|m| m.mem.clone())
        .unwrap_or_else(|| state.authority.clone());
    html_page_response(agent_page(&state.authority, &mem, state.read_prefix()))
}

// ---------------------------------------------------------------------------
// `/entity/{id}` — one entity as HTML
// ---------------------------------------------------------------------------

/// Drop a leading `---`…`---` frontmatter block from a markdown document. The
/// entity HTML page renders title/type/body directly, so the agent-budget
/// frontmatter (`_hash`, `_tokens`) the shared render emits is not shown.
fn strip_frontmatter(md: &str) -> String {
    let mut lines = md.lines();
    if lines.next() == Some("---") {
        let mut closed = false;
        let mut body: Vec<&str> = Vec::new();
        for line in lines {
            if !closed && line == "---" {
                closed = true;
                continue;
            }
            if closed {
                body.push(line);
            }
        }
        if closed {
            return body.join("\n").trim_start_matches('\n').to_string();
        }
    }
    md.to_string()
}

async fn entity_handler(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let entity_id = EntityId::canonical(&id);
    let engine = state.engine.lock().await;
    let Some(entity) = engine.get_entity(&entity_id) else {
        // Unknown id → 404, not a 200 page.
        return error_response(
            StatusCode::NOT_FOUND,
            "ENTITY_NOT_FOUND",
            format!("entity not found: \"{id}\""),
            json!({ "id": id }),
        );
    };
    let entity = entity.clone();
    let id_titles = entity_id_titles(&engine);
    drop(engine);

    // The shared render emits frontmatter + `# Title` + sections + bare
    // `[[<id>]]` wiki-links. Strip the agent-budget frontmatter, rewrite the
    // wiki-links to relative `/entity/<id>` links, then convert to HTML. The
    // type is injected right after the title.
    let rp = state.read_prefix();
    let md = strip_frontmatter(&render::render_entity_markdown(&entity, None));
    let md = linkify_wikilinks(md, &id_titles, rp);
    let content = markdown_to_html_body(&md).replacen(
        "</h1>\n",
        &format!("</h1>\n<p>Type: {}</p>\n", esc_html(&entity.entity_type)),
        1,
    );
    let nav = nav_links(rp, &[("/overview", "overview"), ("/agent", "agent")]);
    let body = format!("{nav}<article>\n{content}</article>\n");
    let title = format!("{} — {}", entity.title, state.authority);
    html_page_response(html_document(&title, &body))
}

// ---------------------------------------------------------------------------
// `/entities` — the complete, budget-independent entity index as HTML
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQueryParams {
    limit: Option<usize>,
    offset: Option<usize>,
}

/// Every entity linked, paginated and budget-INDEPENDENT — the reliable way to
/// enumerate the whole graph. The token-budgeted `/overview` drops its member
/// lists once the graph outgrows the budget; this page never does. Ids are
/// sorted so `offset` paging is stable across requests; an out-of-range offset
/// yields an empty page, never a 500.
async fn entities_handler(
    State(state): State<AppState>,
    Query(params): Query<ListQueryParams>,
) -> Response {
    let limit = params.limit.unwrap_or(500).min(2000).max(1);
    let offset = params.offset.unwrap_or(0);
    let engine = state.engine.lock().await;

    let mut ids: Vec<String> = engine.store().all_ids().map(|i| i.to_string()).collect();
    ids.sort();
    let total = ids.len();
    let page: Vec<(String, String)> = ids
        .iter()
        .skip(offset)
        .take(limit)
        .map(|id| {
            let title = engine
                .get_entity(&EntityId::canonical(id))
                .map(|e| e.title.clone())
                .unwrap_or_default();
            (id.clone(), title)
        })
        .collect();
    let returned = page.len();
    let next_offset = offset + returned;
    let has_more = next_offset < total;

    let rp = state.read_prefix();
    let mut list = String::from("<ul>\n");
    for (id, title) in &page {
        let label = if title.is_empty() { id } else { title };
        list.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>\n",
            entity_href(rp, id),
            esc_html(label)
        ));
    }
    list.push_str("</ul>\n");

    // Prev/next over the full set — root-relative so the page is
    // deployment-agnostic. Prev is suppressed on the first page, next once the
    // last entity has been shown.
    let mut page_nav = String::from("<nav>");
    if offset > 0 {
        let prev = offset.saturating_sub(limit);
        page_nav.push_str(&format!("<a href=\"{rp}/entities?offset={prev}&limit={limit}\">prev</a>"));
    }
    if has_more {
        if offset > 0 {
            page_nav.push_str(" · ");
        }
        page_nav
            .push_str(&format!("<a href=\"{rp}/entities?offset={next_offset}&limit={limit}\">next</a>"));
    }
    page_nav.push_str("</nav>\n");

    let top_nav = nav_links(rp, &[("/overview", "overview"), ("/agent", "agent")]);
    let body = format!(
        "{top_nav}<h1>Entities ({total})</h1>\n{list}{page_nav}"
    );
    let title = format!("{} — entities", state.authority);
    html_page_response(html_document(&title, &body))
}

// ---------------------------------------------------------------------------
// `/schema` — the mem's type and relationship vocabulary as HTML
// ---------------------------------------------------------------------------

async fn schema_handler(State(state): State<AppState>) -> Response {
    let engine = state.engine.lock().await;
    // Single-mem public surface: resolve the flagship mem's pinned schema.
    let mounts = engine.mounts();
    let Some(sref) = mounts.first().and_then(|m| m.schema.clone()) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "ENTITY_NOT_FOUND",
            "no schema pinned on the mounted mem",
            serde_json::Value::Null,
        );
    };
    let Some(schema) = memstead_engine::overview::find_schema(&engine, &sref).cloned() else {
        return error_response(
            StatusCode::NOT_FOUND,
            "ENTITY_NOT_FOUND",
            format!("schema not found: \"{}\"", sref.as_display()),
            json!({ "id": sref.as_display() }),
        );
    };
    // Content as-is: the existing schema rendering, presented as HTML.
    let md = render::render_type_catalog_markdown_for(&schema);
    let nav = nav_links(state.read_prefix(), &[("/overview", "overview"), ("/agent", "agent")]);
    let body = format!("{nav}{}", markdown_to_html_body(&md));
    let title = format!("{} — schema", state.authority);
    html_page_response(html_document(&title, &body))
}


// ---------------------------------------------------------------------------
// `/mcp` — native streamable-HTTP MCP endpoint, scoped to the read tools
// ---------------------------------------------------------------------------

use memstead_mcp::filesystem_server::FilesystemMcpServer;
use rmcp::ServerHandler;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, InitializeRequestParams, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer};

/// The exact tool allowlist for `/mcp` — the five read operations and nothing
/// else. Every mutation, lifecycle, and workspace tool the lean handler
/// carries is absent from this set and refused on call.
pub const MCP_READ_TOOLS: &[&str] = &[
    "memstead_overview",
    "memstead_search",
    "memstead_entity",
    "memstead_schema",
    "memstead_health",
];

/// A read-only MCP `ServerHandler` that wraps the lean [`FilesystemMcpServer`]
/// — so each tool's schema and output bytes are identical to the lean server
/// — but scopes the advertised tool list to [`MCP_READ_TOOLS`] and refuses any
/// call outside that set. Tool-list scoping (not just a Sealed backend) is what
/// makes the surface write-free.
#[derive(Clone)]
pub struct ReadOnlyMcpServer {
    inner: FilesystemMcpServer,
}

impl ReadOnlyMcpServer {
    pub fn new(inner: FilesystemMcpServer) -> Self {
        Self { inner }
    }

    /// Build from a pre-mounted read-only [`Engine`].
    pub fn from_engine(engine: Engine) -> Self {
        Self::new(FilesystemMcpServer::from_engine(engine, std::path::PathBuf::new()))
    }

    /// The scoped tool list: the lean router's tools filtered to the five
    /// read tools, so each `Tool`'s schema/description is byte-identical.
    pub fn read_tools() -> Vec<Tool> {
        FilesystemMcpServer::tool_router()
            .list_all()
            .into_iter()
            .filter(|t| MCP_READ_TOOLS.contains(&t.name.as_ref()))
            .collect()
    }

    pub fn is_read_tool(name: &str) -> bool {
        MCP_READ_TOOLS.contains(&name)
    }
}

/// The read-only handshake instructions. Names the surface as read-only, lists
/// the five available read tools, and states that every mutation, lifecycle,
/// and workspace tool the lean server carries is unavailable here — so a cold
/// MCP client learns the surface cannot mutate *from the handshake*, before it
/// ever calls a tool and gets a `TOOL_NOT_FOUND` refusal. The lean server's
/// own (read-write) instructions are deliberately not delegated to.
pub fn read_only_instructions() -> String {
    format!(
        "Memstead read-only surface: a sealed, read-only knowledge graph exposed \
over MCP. This endpoint cannot mutate — it lists exactly five read tools \
({tools}) and refuses every other tool on call with TOOL_NOT_FOUND. All \
mutation tools (memstead_create, memstead_update, memstead_delete, \
memstead_relate, memstead_rename), lifecycle tools (memstead_mem_create, \
memstead_mem_delete), and workspace-policy tools are unavailable here. \
Cold-start: call memstead_overview for the schema catalogue and mem \
inventory; read a mem's schema via memstead_schema; find entities with \
memstead_search; read one with memstead_entity; inspect graph shape with \
memstead_health.",
        tools = MCP_READ_TOOLS.join(", "),
    )
}

impl ServerHandler for ReadOnlyMcpServer {
    /// Self-describe as read-only. Keeps the inner server's protocol version and
    /// capabilities (the tool surface is real, just scoped), but overrides the
    /// server identity and instructions so the handshake itself tells a cold
    /// client this endpoint cannot mutate and which tool classes are absent —
    /// rather than delegating the lean server's read-write self-description.
    fn get_info(&self) -> ServerInfo {
        let mut info = self.inner.get_info();
        info.server_info.name = "memstead-serve".to_string();
        info.server_info.title = Some("Memstead (read-only)".to_string());
        info.server_info.description = Some(
            "Read-only MCP surface over a sealed Memstead mem — five read tools, no write path."
                .to_string(),
        );
        info.instructions = Some(read_only_instructions());
        info
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        // Let the inner server do its client/peer bookkeeping, then return *this*
        // server's read-only self-description — the inner `initialize` echoes the
        // lean (read-write) `get_info()`, which would defeat the override.
        self.inner.initialize(request, context).await?;
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: Self::read_tools(),
            meta: None,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if !Self::is_read_tool(request.name.as_ref()) {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "ERROR [TOOL_NOT_FOUND]: tool '{}' is not available on this read-only surface",
                request.name
            ))]));
        }
        let tcc = ToolCallContext::new(&self.inner, request, context);
        FilesystemMcpServer::tool_router().call(tcc).await
    }
}

/// Build the streamable-HTTP MCP tower service for `/mcp`. The factory clones
/// the read-only handler per session (a cheap `Arc` clone of the shared
/// engine).
pub fn mcp_service(
    server: ReadOnlyMcpServer,
) -> StreamableHttpService<ReadOnlyMcpServer, LocalSessionManager> {
    // rmcp's default `allowed_hosts` is loopback-only — a DNS-rebinding guard
    // aimed at locally-running servers. This is a public service reached through
    // a proxy under its own hostname, so that list would 400 every real client.
    // Host validation is disabled here; the surface's protection is that no
    // write path is reachable (tool-list scoping + a sealed backend), not Host
    // pinning.
    let config = StreamableHttpServerConfig::default().disable_allowed_hosts();
    StreamableHttpService::new(move || Ok(server.clone()), Default::default(), config)
}

// ---------------------------------------------------------------------------
// Production composition — HTML read pages + runbook + `/mcp` + rate limiting
// ---------------------------------------------------------------------------

/// The full public router: the read-only HTML read pages + runbook surface, the
/// native `/mcp` endpoint, and a per-client rate limiter that refuses over-budget
/// requests with a typed 429 (never a 500). The limiter keys on the real client
/// IP via forwarded headers (falling back to the peer), so serve with
/// `into_make_service_with_connect_info::<SocketAddr>()` to supply that
/// fallback — behind a proxy the forwarded header is what distinguishes clients.
pub fn build_app(
    state: AppState,
    mcp_server: ReadOnlyMcpServer,
    per_second: u64,
    burst: u32,
) -> Router {
    use tower_governor::GovernorLayer;
    use tower_governor::governor::GovernorConfigBuilder;
    use tower_governor::key_extractor::SmartIpKeyExtractor;

    let governor_conf = std::sync::Arc::new(
        GovernorConfigBuilder::default()
            .per_second(per_second)
            .burst_size(burst)
            // Per-client keying via `X-Forwarded-For` / `X-Real-Ip` /
            // `Forwarded`, falling back to the peer IP. Behind a reverse proxy
            // the peer is the proxy, so without this every client shares one
            // bucket (see `session::build_session_app`).
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("rate-limit config must build"),
    );
    build_router(state)
        .nest_service("/mcp", mcp_service(mcp_server))
        .layer(GovernorLayer::new(governor_conf))
}

/// The unified single-origin router: the same human site + HTML read pages as
/// [`build_router`], but the WRITABLE connection-born session `/mcp` (each
/// MCP connection mints its own ephemeral sketch mem beside a shared
/// read-only content mem) replaces the read-only MCP, and the per-session
/// view-data routes (`/v/{id}/graph|stream|export`) are mounted alongside.
///
/// One binary, one origin: the website AND the writable MCP live under the same
/// host, so the deployment needs no edge to splice two backends together. The
/// read engine in `state` still backs the HTML read pages; the session
/// `registry` backs `/mcp` and the view data. One per-IP rate limiter covers the
/// whole surface.
pub fn build_unified_app(
    state: AppState,
    registry: crate::session::SessionRegistry,
    sketch_schema: memstead_schema::SchemaRef,
    view_base: String,
    per_second: u64,
    burst: u32,
) -> Router {
    use tower_governor::GovernorLayer;
    use tower_governor::governor::GovernorConfigBuilder;
    use tower_governor::key_extractor::SmartIpKeyExtractor;

    let governor_conf = std::sync::Arc::new(
        GovernorConfigBuilder::default()
            .per_second(per_second)
            .burst_size(burst)
            // Per-client keying via forwarded headers (see `build_app`) — behind
            // the proxy the peer is the edge, so this is what keeps one noisy
            // visitor from spending everyone's budget.
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("rate-limit config must build"),
    );
    // `build_router` carries the static-site fallback; `sketch_router` carries
    // `/mcp` + the `/v/{id}/...` data routes and only the default 404 fallback,
    // so the merge keeps the site fallback and adds no conflict. The same gate
    // drives both halves: read pages under `/try`, the MCP endpoint at `/try/mcp`.
    let soft_launch = state.soft_launch;
    build_router(state)
        .merge(crate::session::sketch_router(registry, sketch_schema, view_base, soft_launch))
        .layer(GovernorLayer::new(governor_conf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// A read-only AppState over an empty folder-backed mem. The HTTP surface
    /// is exercised regardless of entity content; an empty mem keeps the
    /// fixture trivial.
    fn test_state() -> (AppState, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let schema = memstead_schema::SchemaRef::new("default", semver::Version::new(1, 0, 0));
        let engine = mount_read_only(
            "flagship",
            schema,
            MountStorage::Folder {
                path: tmp.path().to_path_buf(),
            },
        )
        .expect("read-only folder mount");
        (AppState::new(engine, "mem.example"), tmp)
    }

    /// A read-only AppState over a folder mem seeded with one `concept`
    /// entity. Returns the loaded entity's id (discovered from the engine, not
    /// guessed) so the entity route can be exercised on its 200 path.
    fn seeded_state() -> (AppState, tempfile::TempDir, String) {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("widget.md"),
            "---\ntype: concept\n---\n# Widget\n\n## Definition\n\n\
A widget is a self-contained, composable unit.\n\n## Explanation\n\n\
Widgets compose into larger systems; modularity is why they matter.\n",
        )
        .unwrap();
        let schema = memstead_schema::SchemaRef::new("default", semver::Version::new(1, 0, 0));
        let engine = mount_read_only(
            "flagship",
            schema,
            MountStorage::Folder {
                path: tmp.path().to_path_buf(),
            },
        )
        .expect("read-only folder mount");
        let id = engine
            .store()
            .all_ids()
            .next()
            .map(|i| i.0.clone())
            .expect("one entity loaded from the seeded mem");
        (AppState::new(engine, "mem.example"), tmp, id)
    }

    /// A `ReadOnlyMcpServer` over its own empty folder mem (the `/mcp`
    /// surface gets a separate engine from `/api`).
    fn mcp_server() -> (ReadOnlyMcpServer, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let schema = memstead_schema::SchemaRef::new("default", semver::Version::new(1, 0, 0));
        let engine = mount_read_only(
            "flagship",
            schema,
            MountStorage::Folder {
                path: tmp.path().to_path_buf(),
            },
        )
        .expect("read-only folder mount");
        (ReadOnlyMcpServer::from_engine(engine), tmp)
    }

    async fn get(app: &Router, uri: &str, accept: Option<&str>) -> (StatusCode, HeaderMap, String) {
        send(app, "GET", uri, accept).await
    }

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        accept: Option<&str>,
    ) -> (StatusCode, HeaderMap, String) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(a) = accept {
            req = req.header(header::ACCEPT, a);
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
    }

    /// The curated content mem is embedded in the binary and materializes to
    /// disk, so both servers serve real "what is Memstead" content by default
    /// with no external files.
    #[test]
    fn embedded_content_materializes_the_curated_concepts() {
        let names: Vec<String> = super::EMBEDDED_CONTENT
            .files()
            .filter_map(|f| f.path().file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert!(names.len() >= 11, "embedded content carries the concept files: {names:?}");
        for expected in ["memstead.md", "mem.md", "schema.md", "mcp-layer.md"] {
            assert!(names.iter().any(|n| n == expected), "embedded {expected}: {names:?}");
        }

        let dir = super::materialize_embedded_content().expect("materialize embedded content");
        assert!(dir.join("memstead.md").is_file(), "materialized memstead.md");
        assert!(dir.join("schema.md").is_file(), "materialized schema.md");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn runbook_served_on_root_and_llms_txt() {
        let (state, _tmp) = test_state();
        let app = build_router(state);

        for (uri, accept) in [("/llms.txt", None), ("/", Some("text/markdown")), ("/", Some("*/*"))]
        {
            let (status, headers, body) = get(&app, uri, accept).await;
            assert_eq!(status, StatusCode::OK, "{uri} {accept:?}");
            assert!(
                headers
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|c| c.contains("text/markdown"))
                    .unwrap_or(false),
                "{uri} must be text/markdown"
            );
            // Points at the canonical /agent entry, names the HTML map, and
            // carries the MCP-upgrade line.
            assert!(body.contains("/agent"), "{uri} points at the agent page");
            assert!(body.contains("/overview"), "{uri} names the overview map");
            assert!(body.contains("/mcp"), "{uri} carries the MCP-upgrade line");
            // The /api channels are gone — the runbook must not advertise them.
            assert!(!body.contains("/api/"), "{uri} must not name any /api route");
        }
    }

    #[tokio::test]
    async fn healthz_is_a_cheap_constant_ok() {
        let (state, _tmp) = test_state();
        let app = build_router(state);
        let (status, headers, body) = get(&app, "/healthz", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type(&headers).contains("application/json"));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["status"].as_str(), Some("ok"));
    }

    #[tokio::test]
    async fn landing_advertises_the_runbook_via_link_header() {
        let (state, _tmp) = test_state();
        let app = build_router(state);
        let (status, headers, _html) = get(&app, "/", Some("text/html")).await;
        assert_eq!(status, StatusCode::OK);
        let link = headers
            .get(header::LINK)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            link.contains("/llms.txt"),
            "the landing's Link header points an arriving agent at the runbook: {link:?}"
        );
        assert!(
            link.contains("rel=\"alternate\""),
            "Link header carries rel=alternate: {link:?}"
        );
    }

    #[tokio::test]
    async fn link_preview_crawlers_get_html_despite_a_bare_accept() {
        let (state, _tmp) = test_state();
        let app = build_router(state);

        // Unfurlers send `*/*` (no text/html), but the OG/Twitter-Card tags
        // they need live only in the HTML landing — so a known crawler UA must
        // override the negotiation and get HTML, or the link card is blank.
        for ua in [
            "Twitterbot/1.0",
            "facebookexternalhit/1.1 (+http://www.facebook.com/externalhit_uatext.php)",
            "Slackbot-LinkExpanding 1.0 (+https://api.slack.com/robots)",
            "Mozilla/5.0 (compatible; Discordbot/2.0; +https://discordapp.com)",
        ] {
            let req = Request::builder()
                .method("GET")
                .uri("/")
                .header(header::ACCEPT, "*/*")
                .header(header::USER_AGENT, ua)
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{ua}");
            let ct = resp
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            assert!(ct.contains("text/html"), "{ua} must get HTML, got {ct:?}");
        }

        // A bare fetch agent — same `*/*`, no crawler UA — still gets the
        // markdown runbook. The negotiation default is unchanged.
        let (status, headers, body) = get(&app, "/", Some("*/*")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type(&headers).contains("text/markdown"));
        assert!(body.contains("/agent"));
    }

    #[tokio::test]
    async fn authority_manifest_names_mem_schema_and_endpoints() {
        let (state, _tmp) = test_state();
        let app = build_router(state);
        let (status, _h, body) = get(&app, "/.well-known/memstead-authority.json", None).await;
        assert_eq!(status, StatusCode::OK);
        let manifest: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(manifest["authority"].as_str(), Some("mem.example"));
        let mems = manifest["mems"].as_array().expect("mems array");
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0]["name"].as_str(), Some("flagship"));
        assert_eq!(mems[0]["schema"].as_str(), Some("default@1.0.0"));
        // Default deployment vouches for nothing: a read-only served mem
        // is third-party (untrusted) until the operator declares otherwise.
        assert_eq!(
            mems[0]["origin"].as_str(),
            Some("third-party"),
            "default content_origin is the safe third-party"
        );
        // Endpoints are absolute so allowlist-bound fetch agents can follow them.
        // They name the HTML pages and /mcp — and no /api/* read route.
        assert_eq!(manifest["base_url"].as_str(), Some("https://mem.example"));
        assert_eq!(
            manifest["endpoints"]["mcp"].as_str(),
            Some("https://mem.example/mcp")
        );
        assert_eq!(
            manifest["endpoints"]["agent"].as_str(),
            Some("https://mem.example/agent")
        );
        assert_eq!(
            manifest["endpoints"]["overview"].as_str(),
            Some("https://mem.example/overview")
        );
        assert_eq!(
            manifest["endpoints"]["entities"].as_str(),
            Some("https://mem.example/entities")
        );
        assert_eq!(
            manifest["endpoints"]["entity"].as_str(),
            Some("https://mem.example/entity/<id>")
        );
        assert_eq!(
            manifest["endpoints"]["schema"].as_str(),
            Some("https://mem.example/schema")
        );
        // No /api/* read route is named anywhere in the manifest.
        assert!(
            !body.contains("/api/"),
            "discovery manifest must name no /api route: {body}"
        );
    }

    /// A curated deployment that vouches for its read tier
    /// (`with_content_origin(FirstParty)`) exposes `origin: first-party`
    /// on the discovery manifest — the machine-readable signal a consuming
    /// agent reads to trust the served content.
    #[tokio::test]
    async fn authority_manifest_reflects_declared_first_party_content_origin() {
        let (state, _tmp) = test_state();
        let state = state.with_content_origin(OriginClass::FirstParty);
        let app = build_router(state);
        let (status, _h, body) = get(&app, "/.well-known/memstead-authority.json", None).await;
        assert_eq!(status, StatusCode::OK);
        let manifest: serde_json::Value = serde_json::from_str(&body).unwrap();
        let mems = manifest["mems"].as_array().expect("mems array");
        assert_eq!(
            mems[0]["origin"].as_str(),
            Some("first-party"),
            "a deployment that vouches for its content exposes first-party"
        );
    }

    #[tokio::test]
    async fn overview_is_html_with_relative_anchor_links() {
        // The map: schema summary, community clusters, and every in-budget entity
        // as a relative `/entity/<id>` anchor; nav to /agent, /entities, /schema.
        // No markdown link syntax survives the conversion, and no bare entity id
        // sits outside an anchor.
        let (state, _tmp, id) = seeded_state();
        let app = build_router(state);
        let (status, headers, body) = get(&app, "/overview", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type(&headers).contains("text/html"));
        // The schema summary leads the composer's content.
        assert!(body.contains("default@1.0.0"), "overview names the schema: {body}");
        // Every in-budget entity is a relative <a href> anchor.
        assert!(
            body.contains(&format!("<a href=\"/entity/{id}\">")),
            "entity must be a relative <a href> anchor: {body}"
        );
        // Nav to the sibling pages.
        for href in ["/agent", "/entities", "/schema"] {
            assert!(
                body.contains(&format!("<a href=\"{href}\">")),
                "overview links to {href}: {body}"
            );
        }
        // No raw markdown link syntax leaked through the conversion.
        assert!(!body.contains("](/entity/"), "no raw markdown link syntax: {body}");
        // Every mention of the id is part of an /entity/<id> href — no bare id.
        assert_eq!(
            body.matches(&id).count(),
            body.matches(&format!("/entity/{id}")).count(),
            "every entity-id mention must sit inside an anchor href: {body}"
        );
    }

    #[test]
    fn linkifiers_emit_collision_safe_relative_links() {
        let id_titles = vec![
            ("engine--schema".to_string(), "Schema".to_string()),
            (
                "engine--schema-registry".to_string(),
                "Schema registry".to_string(),
            ),
            ("engine--mem".to_string(), "Mem".to_string()),
        ];

        // Cluster bullets `- <id>` → `- [Title](/entity/<id>)`; a shorter id must
        // not match inside a longer one.
        let out = linkify_anchored(
            "- engine--schema\n- engine--schema-registry\n".to_string(),
            &id_titles,
            "",
            "- ",
            "\n",
        );
        assert!(out.contains("- [Schema](/entity/engine--schema)\n"));
        assert!(out.contains("- [Schema registry](/entity/engine--schema-registry)\n"));
        assert!(
            !out.contains("engine--schema)-registry"),
            "shorter id corrupted the longer one: {out}"
        );

        // Community-bridge operands: ` <id> →` (left) and `→ <id>\n` (right).
        let mut bridge = "  - `DEPENDS_ON` engine--schema → engine--mem\n".to_string();
        bridge = linkify_anchored(bridge, &id_titles, "", " ", " →");
        bridge = linkify_anchored(bridge, &id_titles, "", "→ ", "\n");
        assert!(bridge.contains("[Schema](/entity/engine--schema) →"));
        assert!(bridge.contains("→ [Mem](/entity/engine--mem)\n"));

        // Body wiki-links `[[<id>]]` → `[Title](/entity/<id>)`, brackets consumed.
        let out = linkify_wikilinks(
            "See [[engine--mem]] and [[engine--schema-registry]].".to_string(),
            &id_titles,
            "",
        );
        assert_eq!(
            out,
            "See [Mem](/entity/engine--mem) and \
[Schema registry](/entity/engine--schema-registry)."
        );
    }

    #[tokio::test]
    async fn entities_index_lists_every_entity_as_relative_link() {
        // The budget-independent enumeration: every entity linked relatively,
        // with prev/next navigation.
        let (state, _tmp, id) = seeded_state();
        let app = build_router(state);

        let (status, headers, body) = get(&app, "/entities", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type(&headers).contains("text/html"));
        assert!(
            body.contains(&format!("<a href=\"/entity/{id}\">")),
            "entities index must link the entity relatively: {body}"
        );
        // Nav to the sibling pages.
        for href in ["/overview", "/agent"] {
            assert!(body.contains(&format!("<a href=\"{href}\">")), "links to {href}: {body}");
        }

        // Paging: an out-of-range offset yields an empty (or last) page, not a 500.
        let (status, _h, body) = get(&app, "/entities?offset=9999&limit=1", None).await;
        assert_eq!(status, StatusCode::OK, "out-of-range offset must not 500");
        assert!(
            !body.contains(&format!("<a href=\"/entity/{id}\">")),
            "an out-of-range page lists no entities: {body}"
        );

        // Across pages every entity is reachable: a tiny limit still surfaces the
        // entity on its page.
        let (status, _h, body) = get(&app, "/entities?offset=0&limit=1", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(&format!("<a href=\"/entity/{id}\">")), "first page lists it: {body}");
    }

    #[tokio::test]
    async fn dynamic_read_pages_are_uncacheable() {
        // A fetch-tool cache serving a pre-deploy render is exactly the stale-ref
        // failure mode; the dynamic surface must say no-store.
        let (state, _tmp, id) = seeded_state();
        let app = build_router(state);
        for uri in [
            "/agent",
            "/overview",
            "/entities",
            &format!("/entity/{id}"),
            "/schema",
            "/llms.txt",
        ] {
            let (status, headers, _b) = get(&app, uri, None).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            assert_eq!(
                headers
                    .get(header::CACHE_CONTROL)
                    .and_then(|v| v.to_str().ok()),
                Some("no-store"),
                "{uri} must be Cache-Control: no-store"
            );
        }
    }

    #[tokio::test]
    async fn entity_is_html_with_relative_links_and_404s_unknown() {
        let (state, _tmp, id) = seeded_state();
        let app = build_router(state);
        let uri = format!("/entity/{id}");

        let (status, headers, body) = get(&app, &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(content_type(&headers).contains("text/html"));
        // Title, type, and body are present.
        assert!(body.contains("<h1>Widget</h1>"), "title heading: {body}");
        assert!(body.contains("Type: concept"), "type is shown: {body}");
        assert!(body.contains("self-contained"), "body content is shown: {body}");
        // Links back to /overview and /agent.
        for href in ["/overview", "/agent"] {
            assert!(body.contains(&format!("<a href=\"{href}\">")), "links to {href}: {body}");
        }

        // Unknown id → 404, not a 200 page.
        let (status, ..) = get(&app, "/entity/no--such", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Path-traversal and injection inputs on the entity route are answered as a
    /// not-found, never a 500 (verified-good surface — regression coverage). The
    /// id is canonicalised and looked up; a hostile id is simply a miss. Whether
    /// it resolves to the typed `ENTITY_NOT_FOUND` envelope (single-segment ids)
    /// or the router's static 404 (ids carrying encoded slashes), it is a 404
    /// and never a server error.
    #[tokio::test]
    async fn entity_traversal_and_injection_inputs_are_404_never_500() {
        let (state, _tmp, _id) = seeded_state();
        let app = build_router(state);
        for hostile in [
            "..%2F..%2F..%2Fetc%2Fpasswd",
            "%2e%2e%2f%2e%2e%2fsecret",
            "%27%20OR%201%3D1--",                 // ' OR 1=1--
            "%00null",                            // NUL injection
            "%3Cscript%3Ealert(1)%3C%2Fscript%3E", // <script>alert(1)</script>
            "....//....//etc/passwd",
        ] {
            let uri = format!("/entity/{hostile}");
            let (status, ..) = get(&app, &uri, None).await;
            assert_ne!(
                status,
                StatusCode::INTERNAL_SERVER_ERROR,
                "{uri} must never be a 500"
            );
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri} must be a 404");
        }
    }

    #[tokio::test]
    async fn schema_is_html_with_the_vocabulary() {
        let (state, _tmp) = test_state();
        let app = build_router(state);

        let (status, headers, body) = get(&app, "/schema", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type(&headers).contains("text/html"));
        // The type/relationship vocabulary, content as-is, rendered as HTML.
        assert!(body.contains("Available types"), "schema lists the types: {body}");
        // Reachable from /overview and /agent (the nav).
        for href in ["/overview", "/agent"] {
            assert!(body.contains(&format!("<a href=\"{href}\">")), "links to {href}: {body}");
        }
    }

    /// Schema self-consistency across the surviving read surfaces: the discovery
    /// manifest, the `/overview` map, and the `/schema` page all report the
    /// engine mem's pinned schema, and the overview marks the mem read-only
    /// rather than rendering "not writable" as "absent".
    #[tokio::test]
    async fn schema_is_consistent_across_authority_overview_and_schema_page() {
        let (state, _tmp) = test_state();
        let app = build_router(state);
        const SCHEMA: &str = "default@1.0.0";

        // Discovery manifest — reads the per-mem pin directly off the mount.
        let (status, _h, body) = get(&app, "/.well-known/memstead-authority.json", None).await;
        assert_eq!(status, StatusCode::OK);
        let manifest: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(manifest["mems"][0]["schema"].as_str(), Some(SCHEMA));

        // Overview — the schema appears in the rendered map, with no empty-schema
        // placeholder, and the mem is marked read-only.
        let (status, _h, body) = get(&app, "/overview", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(SCHEMA), "overview names the schema: {body}");
        assert!(
            !body.contains("(no schemas in use)"),
            "overview must not render the empty-schema placeholder when a schema is pinned: {body}"
        );
        assert!(
            body.contains("Access:") && body.contains("read-only"),
            "overview marks the mem read-only: {body}"
        );

        // Schema page — reachable and naming the same schema vocabulary.
        let (status, _h, body) = get(&app, "/schema", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Available types"), "schema page renders the vocabulary: {body}");
    }

    /// Overview layout under the sealed read-only mount: the rendered map opens
    /// with substantive, queryable content (the schema summary), not a run of
    /// empty write-oriented sections. The `Lifecycle Namespaces` placeholder is
    /// suppressed when there are no writable mems; nothing actionable is
    /// dropped (Schemas, Mems, Communities all remain reachable as headings).
    #[tokio::test]
    async fn overview_leads_with_content_under_sealed_mount() {
        let (state, _tmp) = test_state();
        let app = build_router(state);
        let (status, _h, body) = get(&app, "/overview", None).await;
        assert_eq!(status, StatusCode::OK);

        // No empty write-oriented lede: the lifecycle placeholder is gone.
        assert!(
            !body.contains("Lifecycle Namespaces"),
            "the empty lifecycle section must be suppressed under a sealed mount: {body}"
        );

        // The first heading is substantive content (Schemas), not an empty
        // write-oriented header.
        let first_heading = body
            .lines()
            .find(|l| l.starts_with("<h2>"))
            .expect("at least one section heading");
        assert_eq!(
            first_heading, "<h2>Schemas</h2>",
            "overview must lead with the schema summary: {body}"
        );

        // Nothing actionable dropped — Mems and Communities remain reachable.
        assert!(body.contains("<h2>Mems</h2>"), "Mems section remains: {body}");
        assert!(body.contains("<h2>Communities</h2>"), "Communities section remains: {body}");
    }

    /// Read-only invariant: every registered route is GET-only, so a POST
    /// (the shape any mutation would take) is refused at the router. No route
    /// reaches a mutating engine op — the handlers call only read methods.
    #[tokio::test]
    async fn mutations_are_refused_no_write_path() {
        let (state, _tmp) = test_state();
        let app = build_router(state);
        for uri in [
            "/",
            "/healthz",
            "/llms.txt",
            "/.well-known/memstead-authority.json",
            "/agent",
            "/overview",
            "/entities",
            "/entity/anything",
            "/schema",
        ] {
            let (status, ..) = send(&app, "POST", uri, None).await;
            assert_eq!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "POST {uri} must be refused — no write path"
            );
        }
    }

    /// The removed `/api/*` read channels are GONE — not redirected — on both
    /// `Accept: text/markdown` and `Accept: application/json`. Removal verified,
    /// not just addition.
    #[tokio::test]
    async fn removed_api_routes_are_404() {
        let (state, _tmp, _id) = seeded_state();
        let app = build_router(state);
        for path in [
            "/api/overview",
            "/api/html-overview",
            "/api/search?q=x",
            "/api/entities",
            "/api/entity/anything",
            "/api/schema",
            "/api/health",
        ] {
            for accept in [Some("text/markdown"), Some("application/json")] {
                let (status, ..) = get(&app, path, accept).await;
                assert_eq!(
                    status,
                    StatusCode::NOT_FOUND,
                    "{path} ({accept:?}) must be 404 — the /api channel is gone"
                );
            }
        }
    }

    /// Every new HTML page is pure: a syntactically framed `<!doctype html>` …
    /// `</html>` document with no `<style>`, no `<script>`, no `class=`
    /// attribute, and no absolute `https://<authority>/…` navigable href.
    #[tokio::test]
    async fn every_html_page_is_pure_and_framed() {
        let (state, _tmp, id) = seeded_state();
        let app = build_router(state);
        for uri in [
            "/agent".to_string(),
            "/overview".to_string(),
            "/entities".to_string(),
            format!("/entity/{id}"),
            "/schema".to_string(),
        ] {
            let (status, headers, body) = get(&app, &uri, None).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            assert!(content_type(&headers).contains("text/html"), "{uri} is html");
            assert!(
                body.starts_with("<!doctype html>") && body.contains("</html>"),
                "{uri} must be a framed HTML document: {body}"
            );
            assert!(!body.contains("<style"), "{uri} must carry no <style>: {body}");
            assert!(!body.contains("<script"), "{uri} must carry no <script>: {body}");
            assert!(!body.contains("class="), "{uri} must carry no class attribute: {body}");
            // No absolute authority href in the navigable body (the manifest is
            // exempt; these are graph pages).
            assert!(
                !body.contains("https://mem.example/"),
                "{uri} must emit no absolute authority href: {body}"
            );
        }
    }

    fn content_type(headers: &HeaderMap) -> String {
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    /// The `/agent` page carries all three jobs and links to its siblings: what
    /// this is, how to read it (naming /overview, /entities, /schema and that
    /// search/write require MCP), and how to talk to the user about it.
    #[tokio::test]
    async fn agent_page_carries_the_three_jobs_and_links() {
        let (state, _tmp) = test_state();
        let app = build_router(state);
        let (status, headers, body) = get(&app, "/agent", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type(&headers).contains("text/html"));
        // Job 1 — what this is.
        assert!(body.contains("This is Memstead"), "names what this is: {body}");
        // Job 2 — how to read it: names the read pages and that MCP is needed to
        // search or write.
        for href in ["/overview", "/entities", "/schema"] {
            assert!(
                body.contains(&format!("<a href=\"{href}\">")),
                "agent page links to {href}: {body}"
            );
        }
        assert!(
            body.contains("no search here") && body.contains("MCP"),
            "agent page states search/write require MCP: {body}"
        );
        // Job 3 — how to talk to the user about it.
        assert!(
            body.contains("How to tell the user about it"),
            "agent page carries the user-facing guidance: {body}"
        );
    }

    /// End-to-end naive-agent scenario over HTML alone (no MCP): given the bare
    /// base URL, an agent reads the runbook, follows it to `/agent`, then to
    /// `/overview`, follows an entity link, and reads the answer from
    /// `/entity/<id>` — every hop plain HTML, no MCP involved.
    #[tokio::test]
    async fn naive_agent_reaches_mem_content_via_html_alone() {
        let (state, _tmp, id) = seeded_state();
        let app = build_router(state);

        // 1. Bare URL → the runbook points at /agent.
        let (status, _h, runbook) = get(&app, "/", Some("*/*")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(runbook.contains("/agent"));

        // 2. /agent points at the map.
        let (status, _h, agent) = get(&app, "/agent", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(agent.contains("/overview"));

        // 3. The map links the entity relatively.
        let (status, _h, overview) = get(&app, "/overview", None).await;
        assert_eq!(status, StatusCode::OK);
        let href = format!("/entity/{id}");
        assert!(overview.contains(&format!("<a href=\"{href}\">")), "map links the entity: {overview}");

        // 4. Read the answer from the entity the map handed back.
        let (status, _h, entity) = get(&app, &href, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            entity.contains("self-contained"),
            "the agent reached the seeded entity's content via HTML alone: {entity}"
        );
    }

    /// Polarity: the `/mcp` tool list is exactly the five read tools, and the
    /// mutation tools the lean handler carries are absent from the list and
    /// refused on call. Tool-list scoping — not just a Sealed backend — is what
    /// the criterion demands.
    #[test]
    fn mcp_lists_exactly_five_read_tools_and_excludes_mutations() {
        let scoped: Vec<String> = ReadOnlyMcpServer::read_tools()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        let mut sorted = scoped.clone();
        sorted.sort();
        let mut expected: Vec<String> = MCP_READ_TOOLS.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(sorted, expected, "/mcp must list exactly the five read tools");

        let full: Vec<String> = FilesystemMcpServer::tool_router()
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        for mutating in [
            "memstead_create",
            "memstead_update",
            "memstead_delete",
            "memstead_relate",
            "memstead_rename",
        ] {
            assert!(
                full.contains(&mutating.to_string()),
                "lean handler must carry {mutating} (so filtering it is meaningful)"
            );
            assert!(
                !scoped.contains(&mutating.to_string()),
                "{mutating} must be absent from the /mcp tool list"
            );
            assert!(
                !ReadOnlyMcpServer::is_read_tool(mutating),
                "{mutating} must be refused on call"
            );
        }
    }

    /// The `/mcp` endpoint is mounted and completes the initialize handshake
    /// over the streamable-HTTP transport.
    #[tokio::test]
    async fn mcp_endpoint_responds_to_initialize() {
        let (state, _t1) = test_state();
        let (mcp, _t2) = mcp_server();
        let app = build_router(state).nest_service("/mcp", mcp_service(mcp));
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            // Real HTTP/1.1 clients always send Host; oneshot does not, and the
            // transport requires it present.
            .header("host", "mem.example")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(init))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "POST /mcp initialize must return 200"
        );
    }

    /// The `/mcp` handshake self-describes as read-only: the server identity is
    /// not the lean name, and the instructions state read-only, name the five
    /// available read tools, and name the unavailable mutation/lifecycle tool
    /// classes — so a cold client learns the surface cannot mutate from the
    /// handshake, before any tool call.
    #[test]
    fn mcp_handshake_self_describes_as_read_only() {
        let (mcp, _t) = mcp_server();
        let info = mcp.get_info();
        assert_eq!(
            info.server_info.name, "memstead-serve",
            "identity must not delegate to the lean (read-write) server name"
        );
        let instr = info
            .instructions
            .expect("read-only handshake carries instructions");
        assert!(
            instr.to_lowercase().contains("read-only"),
            "instructions state read-only: {instr}"
        );
        for absent in [
            "memstead_create",
            "memstead_update",
            "memstead_delete",
            "memstead_relate",
            "memstead_rename",
            "memstead_mem_create",
            "memstead_mem_delete",
        ] {
            assert!(
                instr.contains(absent),
                "instructions name unavailable tool {absent}: {instr}"
            );
        }
        for available in MCP_READ_TOOLS {
            assert!(
                instr.contains(available),
                "instructions name available read tool {available}: {instr}"
            );
        }
    }

    /// POST a JSON-RPC body to `/mcp`, optionally carrying a session id. Returns
    /// `(status, mcp-session-id, body)`. The body is the raw streamable-HTTP
    /// response (an SSE `event: message` frame for request methods); tool names
    /// are asserted by substring, no SSE parsing needed.
    async fn mcp_post(app: &Router, body: &str, session: Option<&str>) -> (StatusCode, Option<String>, String) {
        let mut req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "mem.example")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(s) = session {
            req = req.header("mcp-session-id", s);
        }
        let resp = app.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
        let status = resp.status();
        let sid = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, sid, String::from_utf8_lossy(&bytes).to_string())
    }

    /// The documented handshake completes end-to-end over the live transport:
    /// `initialize` issues an `Mcp-Session-Id` (the session contract is stateful —
    /// the id is required on every follow-up), the client posts
    /// `notifications/initialized`, then `tools/list` returns exactly the five
    /// read tools and none of the mutation tools. Exercising `tools/list` over
    /// the wire (not just the in-process `read_tools()`) is what the criterion
    /// demands.
    #[tokio::test]
    async fn mcp_initialize_then_tools_list_completes_over_transport() {
        let (mcp, _t) = mcp_server();
        let app = build_router(test_state().0).nest_service("/mcp", mcp_service(mcp));

        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;
        let (status, sid, body) = mcp_post(&app, init, None).await;
        assert_eq!(status, StatusCode::OK, "initialize must return 200");
        let sid = sid.expect("initialize issues an Mcp-Session-Id (stateful session contract)");
        // The read-only self-description rides the wire, not just `get_info()`.
        assert!(
            body.contains("read-only"),
            "initialize response self-describes as read-only: {body}"
        );

        // `notifications/initialized` on the established session is accepted.
        let initialized = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let (status, ..) = mcp_post(&app, initialized, Some(&sid)).await;
        assert!(
            status.is_success(),
            "notifications/initialized on the established session must be accepted, got {status}"
        );

        // `tools/list` over the wire returns exactly the five read tools.
        let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
        let (status, _sid, body) = mcp_post(&app, list, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK, "tools/list must return 200: {body}");
        for available in MCP_READ_TOOLS {
            assert!(body.contains(available), "tools/list names {available}: {body}");
        }
        for absent in ["memstead_create", "memstead_update", "memstead_delete", "memstead_relate", "memstead_rename"] {
            assert!(!body.contains(absent), "tools/list must not name {absent}: {body}");
        }
    }

    /// Rate limiting: requests beyond the per-IP budget are refused with a
    /// typed 429, never a 500.
    #[tokio::test]
    async fn rate_limit_refuses_with_429_not_500() {
        use axum::extract::ConnectInfo;
        use std::net::SocketAddr;

        let (state, _t1) = test_state();
        let (mcp, _t2) = mcp_server();
        // burst of 1 → the second request in the window is over budget.
        let app = build_app(state, mcp, 1, 1);
        let addr: SocketAddr = "203.0.113.7:5555".parse().unwrap();

        let mut got_429 = false;
        for _ in 0..5 {
            let mut req = Request::builder()
                .method("GET")
                .uri("/overview")
                .body(Body::empty())
                .unwrap();
            req.extensions_mut().insert(ConnectInfo(addr));
            let resp = app.clone().oneshot(req).await.unwrap();
            let status = resp.status();
            assert_ne!(
                status,
                StatusCode::INTERNAL_SERVER_ERROR,
                "over-budget must never be a 500"
            );
            if status == StatusCode::TOO_MANY_REQUESTS {
                got_429 = true;
            }
        }
        assert!(got_429, "burst-exceeding requests must be refused with 429");
    }

    /// `/` negotiates: a browser (`text/html`) gets the human landing carrying
    /// the one-paste bootstrap; agents (`text/markdown`, `*/*`, or no header)
    /// still get the plan-02 runbook — the human face did not break the agent
    /// face. One origin serves both.
    #[tokio::test]
    async fn root_negotiates_html_landing_vs_agent_runbook() {
        let (state, _tmp) = test_state();
        let app = build_router(state);

        // Browser → HTML landing. Its content differs by deployment: the real
        // embedded Astro build is the authority-agnostic agent-runbook surface,
        // while a Rust-only checkout serves the placeholder with the one-paste
        // bootstrap. Assert the marker for whichever is in play; the polarity
        // (HTML here, markdown below) is what this test guards.
        let (status, headers, html) = get(&app, "/", Some("text/html")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type(&headers).contains("text/html"), "browser gets HTML");
        if SITE.get_file("index.html").is_some() {
            // Embedded Astro build — the lean runbook surface (its full content
            // markers are asserted by the embedded-landing test).
            assert!(
                html.contains("id=\"mh-runbook\""),
                "embedded landing is the runbook surface: {html}"
            );
        } else {
            // Placeholder landing — the one-paste bootstrap and the bare
            // authority URL for a fetch agent.
            assert!(
                html.contains("claude mcp add --transport http mem.example mem.example/mcp"),
                "placeholder carries the MCP one-paste bootstrap line: {html}"
            );
            assert!(
                html.contains("mem.example"),
                "placeholder carries the bare URL to hand a fetch agent"
            );
            assert!(html.contains("mem-graph"), "placeholder carries the graph mount");
        }

        // Polarity: agent surfaces unchanged across markdown, */*, and absent.
        for accept in [Some("text/markdown"), Some("*/*"), None] {
            let (status, headers, body) = get(&app, "/", accept).await;
            assert_eq!(status, StatusCode::OK, "accept={accept:?}");
            assert!(
                content_type(&headers).contains("text/markdown"),
                "accept={accept:?} must return the runbook (markdown)"
            );
            assert!(
                body.contains("/agent") && body.contains("/mcp"),
                "accept={accept:?} must return the runbook body"
            );
        }
    }

    /// The runtime binary serves the static surface at one origin: an unmatched
    /// asset path falls through to the embedded site (here empty → 404, the
    /// same path that serves real `_astro/*` chunks once the build is embedded).
    #[tokio::test]
    async fn static_fallback_handles_unknown_asset() {
        let (state, _tmp) = test_state();
        let app = build_router(state);
        let (status, ..) = get(&app, "/_astro/does-not-exist.js", Some("text/html")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// When the Astro build is embedded (after `npm run build` in
    /// the configured site dist), the served browser landing carries the real
    /// interactive graph: the MemGraph data payload (the flagship mem's
    /// `serialize_graph_json` export) and the 3d-force-graph runtime. On a
    /// Rust-only checkout the embed is empty and this is a no-op — the
    /// placeholder, asserted above, is what ships then.
    #[tokio::test]
    async fn embedded_astro_landing_carries_the_interactive_graph() {
        if SITE.get_file("index.html").is_none() {
            return; // no Astro build embedded — covered by the placeholder test
        }
        let (state, _tmp) = test_state();
        let app = build_router(state);
        let (status, _h, html) = get(&app, "/", Some("text/html")).await;
        assert_eq!(status, StatusCode::OK);
        // The landing is the lean, classless agent runbook — the graph DOM is
        // no longer inlined; the human face injects it at runtime. So assert
        // the runbook surface
        // and that the human-face runtime + graph data asset are wired, not the
        // old inline `#mem-graph` markup.
        assert!(
            html.contains("id=\"mh-runbook\""),
            "real landing is the agent runbook surface"
        );
        assert!(
            html.contains("/agent"),
            "the runbook content ships in the body"
        );
        assert!(
            html.contains("data-graph-src"),
            "the graph data asset is wired for the human face to fetch"
        );
        assert!(
            html.contains("_astro/"),
            "the human-face runtime is wired into the landing"
        );
    }

    // -----------------------------------------------------------------------
    // Soft-launch gate — the serve binary's runtime toggle. `test_state()`
    // defaults the gate OFF (the public surface every other test asserts);
    // these flip it ON.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn gate_on_root_agent_is_dead_end() {
        let (state, _tmp) = test_state();
        let app = build_router(state.with_soft_launch(true));

        // The bare fetch agent (`*/*`, no Accept) gets a hint-free holding note,
        // not the runbook — no trail to any real path.
        for accept in [None, Some("*/*"), Some("text/markdown")] {
            let (status, _h, body) = get(&app, "/", accept).await;
            assert_eq!(status, StatusCode::OK);
            for needle in ["/agent", "/overview", "/llms.txt", "/mcp", "/try", "/.well-known"] {
                assert!(!body.contains(needle), "gated `/` ({accept:?}) leaks `{needle}`:\n{body}");
            }
            assert!(body.to_lowercase().contains("coming soon"), "{accept:?}: {body}");
        }
    }

    #[tokio::test]
    async fn gate_on_html_landing_drops_llms_link_header() {
        let (state, _tmp) = test_state();
        let app = build_router(state.with_soft_launch(true));
        let (status, headers, _body) = get(&app, "/", Some("text/html")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            headers.get(header::LINK).is_none(),
            "gated HTML landing must not advertise the runbook via Link header"
        );
    }

    #[tokio::test]
    async fn gate_off_keeps_public_agent_front_door() {
        let (state, _tmp) = test_state();
        let app = build_router(state); // soft_launch defaults OFF
        // Runbook is back at `/` for the agent.
        let (status, _h, body) = get(&app, "/", Some("*/*")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("/agent"), "public `/` is the runbook: {body}");
        // And the HTML landing re-advertises the runbook.
        let (_s, headers, _b) = get(&app, "/", Some("text/html")).await;
        assert!(
            headers.get(header::LINK).is_some(),
            "public HTML landing carries the discovery Link header"
        );
    }

    #[tokio::test]
    async fn gate_robots_is_toggle_aware() {
        // ON: allow `/`, name nothing real.
        let (state_on, _t1) = test_state();
        let app_on = build_router(state_on.with_soft_launch(true));
        let (status, _h, body) = get(&app_on, "/robots.txt", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body.contains("Disallow: /"), "ON robots must not block root: {body}");
        assert!(!body.contains("/try"), "ON robots must not name /try: {body}");

        // OFF: crawlable, not the pre-gate blanket ban.
        let (state_off, _t2) = test_state();
        let app_off = build_router(state_off);
        let (_s, _h, body_off) = get(&app_off, "/robots.txt", None).await;
        assert!(!body_off.contains("Disallow: /"), "OFF robots is crawlable: {body_off}");
    }

    #[tokio::test]
    async fn gate_on_read_surface_moves_under_try() {
        let (state, _tmp, id) = seeded_state();
        let app = build_router(state.with_soft_launch(true));

        // Top-level read routes are gone while gated.
        for path in ["/agent", "/overview", "/entities", "/schema", "/llms.txt",
                     "/.well-known/memstead-authority.json", &format!("/entity/{id}")] {
            let (status, headers, _b) = get(&app, path, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "top-level {path} must 404 while gated");
            assert!(headers.get("x-robots-tag").is_none(), "{path}");
        }

        // Under /try they answer, and carry noindex.
        for path in ["/try/agent", "/try/overview", "/try/entities", "/try/schema",
                     "/try/llms.txt", "/try/.well-known/memstead-authority.json",
                     &format!("/try/entity/{id}")] {
            let (status, headers, _b) = get(&app, path, None).await;
            assert_eq!(status, StatusCode::OK, "gated {path} must answer under /try");
            assert_eq!(
                headers.get("x-robots-tag").and_then(|v| v.to_str().ok()),
                Some("noindex"),
                "{path} must carry noindex"
            );
        }
    }

    #[tokio::test]
    async fn gate_on_read_pages_link_within_try() {
        let (state, _tmp, id) = seeded_state();
        let app = build_router(state.with_soft_launch(true));

        // The overview map links entities and nav within /try, never at top-level.
        let (_s, _h, overview) = get(&app, "/try/overview", None).await;
        assert!(overview.contains(&format!("/try/entity/{id}")), "overview links within /try: {overview}");
        assert!(overview.contains("\"/try/agent\""), "nav points within /try: {overview}");
        assert!(!overview.contains("\"/overview\""), "no bare top-level nav: {overview}");

        // The runbook (now at /try/llms.txt) breadcrumbs into /try.
        let (_s, _h, runbook) = get(&app, "/try/llms.txt", None).await;
        assert!(runbook.contains("/try/agent"), "runbook points within /try: {runbook}");

        // The manifest's endpoints carry the /try prefix.
        let (_s, _h, manifest) = get(&app, "/try/.well-known/memstead-authority.json", None).await;
        assert!(manifest.contains("/try/agent"), "manifest endpoints within /try: {manifest}");
    }

    #[tokio::test]
    async fn gate_off_read_surface_stays_top_level() {
        let (state, _tmp, id) = seeded_state();
        let app = build_router(state); // OFF
        for path in ["/agent", "/overview", "/llms.txt",
                     "/.well-known/memstead-authority.json", &format!("/entity/{id}")] {
            let (status, headers, _b) = get(&app, path, None).await;
            assert_eq!(status, StatusCode::OK, "public {path} answers at top-level");
            assert!(headers.get("x-robots-tag").is_none(), "public {path} is indexable");
        }
        // And nothing is mounted under /try.
        let (status, _h, _b) = get(&app, "/try/agent", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "no /try surface when public");
    }
}
