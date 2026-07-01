//! Writable multi-tenant remote-MCP session server.
//!
//! The read-only [`crate::ReadOnlyMcpServer`] serves one sealed archive,
//! read tools only. This module is the **additive** writable counterpart:
//! every visitor session gets its own ephemeral in-memory vault (the
//! [`memstead_base::MountStorage::InMemory`] backend) behind its own
//! remote-MCP endpoint, exposing **only the 10 entity tools** — the five
//! reads plus the five mutations — and withholding the other 13 of the
//! full MCP surface (vault lifecycle, workspace policy, admin).
//!
//! ## The allowlist is a security boundary
//!
//! A public, anonymous, writable endpoint must never hand vault-lifecycle
//! or workspace-policy mutation to arbitrary visitors. The exposed surface
//! is a hardcoded allowlist of exactly the 10 entity tools
//! ([`SESSION_ENTITY_TOOLS`]); [`SessionMcpServer`] scopes both
//! `list_tools` and `call_tool` to it, refusing everything else on call.
//! Crucially the posture is **fail-safe**: [`session_withheld_tools`]
//! derives the withheld set from the live tool router as
//! `all_known − the_10`, so a tool added to the engine later is withheld
//! by default rather than auto-exposed — the inverse of a hand-maintained
//! denylist, which fails open.
//!
//! ## Sessions
//!
//! [`SessionRegistry`] maps an opaque id to a per-session
//! [`rmcp`]-streamable-HTTP service over that session's own engine. The id
//! generator is injected (production wires a CSPRNG; tests a counter), so
//! the registry carries no opinion about unguessability. Sessions evict
//! after a TTL/idle window via [`SessionRegistry::sweep_expired`]; an
//! evicted or never-created id resolves to a typed [`SessionError`], never
//! a silently-minted fresh vault — eviction is observable.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use std::convert::Infallible;

use axum::Json;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tower::ServiceExt;

use memstead_base::backend::VaultBackend;
use memstead_base::storage::InMemoryBackend;
use memstead_base::{Engine, Mount, MountCapability, MountLifecycle, MountStorage};
use memstead_schema::SchemaRef;

use crate::graph::{GraphSnapshot, graph_projection};

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

/// The exact tool allowlist for a session endpoint — the five read
/// operations and the five mutations, and nothing else. Every lifecycle,
/// workspace-policy, and admin tool the basis handler carries is absent
/// from this set and refused on call. This is the public writable
/// surface's security boundary; treat changes to it as interface changes.
pub const SESSION_ENTITY_TOOLS: &[&str] = &[
    // reads
    "memstead_overview",
    "memstead_search",
    "memstead_entity",
    "memstead_schema",
    "memstead_health",
    // mutations
    "memstead_create",
    "memstead_update",
    "memstead_relate",
    "memstead_delete",
    "memstead_rename",
];

/// The writable vault every session engine mounts — the visitor's own
/// throwaway "sketch" graph. One in-memory vault per session; it is the
/// engine's only writable mount, so it is the default target for a create
/// that omits `vault`.
pub const SESSION_VAULT_NAME: &str = "sketch";

/// The read-only content vault mounted alongside every session's sketch —
/// the curated "what is Memstead" graph the agent reads to orient itself
/// (schema, patterns, a rich example) before sketching its own. Reads span
/// both mounts; writes reach only [`SESSION_VAULT_NAME`].
pub const CONTENT_VAULT_NAME: &str = "memstead";

/// Default per-session entity budget. A sketch session is a throwaway
/// scratch graph, not a production vault — a bounded budget caps the work
/// a single anonymous visitor can pile into one in-memory vault. Deletes
/// free room, so the cap bounds live size, not lifetime create count.
pub const DEFAULT_SESSION_ENTITY_CAP: usize = 200;

/// Default ceiling on concurrently-live sessions one registry will hold.
/// Each live session is its own in-memory engine (a writable sketch vault
/// plus a read-only content view), so the live-session count — not request
/// rate — is what bounds memory. Past this ceiling new sessions are refused
/// (page-first: a 503; connection-born: a typed `RESOURCE_CAP_EXCEEDED` on
/// the first tool call) so a traffic spike sheds load instead of OOM-ing the
/// process and dropping *every* session at once. Sized for a modest box;
/// raise it with `MEMSTEAD_SESSION_MAX` once the instance has the RAM
/// (rough budget: a few MB per live session).
pub const DEFAULT_SESSION_MAX: usize = 2000;

/// Write a seed `VaultConfig` (schema pin + version) into a fresh
/// in-memory backend *before* boot, so the vault is self-describing when
/// `from_mounts` reads it — and so a later `.mem` export projects a valid
/// published config (export refuses a vault whose config has no version).
fn in_memory_backend_with_config(
    schema: &SchemaRef,
    what: &str,
) -> Result<InMemoryBackend, memstead_base::EngineError> {
    let backend = InMemoryBackend::new();
    let config_bytes = format!(r#"{{"version":"0.1.0","schema":"{schema}"}}"#).into_bytes();
    backend
        .write_vault_config(&config_bytes)
        .map_err(|e| memstead_base::EngineError::Vault(format!("{what} config write: {e}")))?;
    Ok(backend)
}

/// Build a session's two-mount [`Engine`]: a **writable** in-memory
/// [`SESSION_VAULT_NAME`] sketch vault (the visitor's own throwaway graph)
/// alongside a **read-only** [`CONTENT_VAULT_NAME`] content vault (the
/// curated "what is Memstead" graph the agent reads to orient itself).
/// Reads span both mounts; a write reaches only the sketch vault — the
/// engine refuses a write to the read-only content mount at the capability
/// layer, a second guard beneath the tool allowlist. The sketch vault is
/// the engine's sole writable mount, so it is the default target for a
/// create that omits `vault`.
///
/// `content_storage` is the curated content source: [`MountStorage::Archive`]
/// (a sealed `.mem`) in production, [`MountStorage::Folder`] for tests, or
/// [`MountStorage::InMemory`] for an empty placeholder when no curated vault
/// is wired yet. `content_schema` pins the content mount (matching the
/// source's own pin for folder/archive sources); `sketch_schema` pins the
/// visitor's vault — a session-creation parameter, not hardcoded, so a
/// launch schema or a visitor-chosen one both flow through unchanged.
/// Provisions nothing on disk for the sketch vault; dropping the engine
/// reclaims it.
pub fn mount_session_engine(
    content_storage: MountStorage,
    content_schema: SchemaRef,
    content_vault_name: String,
    sketch_schema: SchemaRef,
) -> Result<Engine, memstead_base::EngineError> {
    // Writable sketch vault — declared first; its own empty in-memory backend.
    let sketch_mount = Mount {
        vault: SESSION_VAULT_NAME.to_string(),
        schema: Some(sketch_schema.clone()),
        storage: MountStorage::InMemory,
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let sketch_backend = in_memory_backend_with_config(&sketch_schema, "sketch")?;

    // Read-only content vault. Folder/archive sources carry their own
    // config; an in-memory placeholder is made self-describing here so
    // overview/schema still resolve over it until a curated vault is wired.
    let content_mount = Mount {
        vault: content_vault_name,
        schema: Some(content_schema.clone()),
        storage: content_storage,
        capability: MountCapability::ReadOnly,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let content_backend: Box<dyn VaultBackend> = match &content_mount.storage {
        MountStorage::InMemory => {
            Box::new(in_memory_backend_with_config(&content_schema, "content")?)
        }
        _ => memstead_base::workspace_store::instantiate_basis_backend(&content_mount)
            .map_err(|e| memstead_base::EngineError::Vault(e.to_string()))?,
    };

    Engine::from_mounts(vec![
        (sketch_mount, Box::new(sketch_backend) as Box<dyn VaultBackend>),
        (content_mount, content_backend),
    ])
}

/// The withheld tools: every tool the basis router carries that is **not**
/// in [`SESSION_ENTITY_TOOLS`]. Derived from the live router so a
/// newly-added engine tool lands here automatically (fail-safe), rather
/// than silently reaching the public endpoint. Returned sorted for stable
/// assertions.
pub fn session_withheld_tools() -> Vec<String> {
    let mut out: Vec<String> = FilesystemMcpServer::tool_router()
        .list_all()
        .into_iter()
        .map(|t| t.name.to_string())
        .filter(|n| !SESSION_ENTITY_TOOLS.contains(&n.as_str()))
        .collect();
    out.sort();
    out
}

/// The session handshake instructions: names the writable session surface,
/// lists the 10 available entity tools, and names the withheld tool
/// classes — so a cold MCP client learns the boundary from the handshake,
/// before it ever calls a withheld tool and gets a `TOOL_NOT_FOUND`.
pub fn session_instructions() -> String {
    format!(
        "Memstead sketch session: your own empty, writable, ephemeral knowledge \
graph exposed over MCP. This endpoint lists exactly ten entity tools ({tools}) and \
refuses every other tool on call with TOOL_NOT_FOUND. Vault-lifecycle tools \
(memstead_vault_create, memstead_vault_delete, memstead_vault_set_schema, \
memstead_vault_set_version), the workspace-policy tools (memstead_workspace_*), and \
admin tools (memstead_reload, memstead_diff, memstead_changes_since) are unavailable \
here. Cold-start: call memstead_overview for the schema catalogue and vault \
inventory; read a vault's schema via memstead_schema; then build the graph with \
memstead_create / memstead_relate / memstead_update / memstead_rename / \
memstead_delete and read it back with memstead_search / memstead_entity. The session \
vault is empty and yours alone; it is reclaimed when the session expires.",
        tools = SESSION_ENTITY_TOOLS.join(", "),
    )
}

/// A writable MCP `ServerHandler` that wraps the basis
/// [`FilesystemMcpServer`] — so each tool's schema and output bytes are
/// identical to the basis server — but scopes the advertised tool list to
/// [`SESSION_ENTITY_TOOLS`] and refuses any call outside that set.
/// Tool-list scoping (not backend capability alone) is the security
/// boundary: the backend is writable on purpose; the scoping is what keeps
/// lifecycle/policy/admin tools off the public surface.
/// A connection-born session's binding to its live-view endpoints.
///
/// Set for the connection-born flow; `None` (no binding) for the page-first
/// flow, which addresses sessions over HTTP and surfaces no link through MCP.
///
/// The binding is **registered lazily** — on the session's first tool call,
/// not when the rmcp factory mints the engine. rmcp invokes that factory once
/// per `initialize`, and a client may `initialize` more than once per logical
/// connection (a capability probe, a reconnect, an IDE that re-handshakes).
/// Registering eagerly would publish a resolvable, *empty* `/v/{id}` for every
/// such handshake — one of which the client might surface to a human as the
/// link, leaving them staring at a "live but empty" sibling of the vault the
/// agent is actually writing. Binding on first real work means only a session
/// that does something becomes resolvable, and the link is surfaced from that
/// same session's overview (see [`SessionMcpServer::call_tool`]).
#[derive(Clone)]
struct ViewBinding {
    /// Application-generated, unguessable id — the public `/v/{id}` handle.
    /// Deliberately NOT the rmcp transport session id, which is a bearer
    /// credential for the MCP session and must never appear in a shareable URL.
    id: String,
    /// Origin the link is composed against (e.g. `https://memstead.ai`); empty
    /// → a relative `/v/{id}`.
    base: String,
    /// The registry this session self-registers into on first tool call. The
    /// registry→entry→server→registry reference is a cycle, but a bounded one:
    /// it is broken when the session is swept (the entry, and with it this
    /// server clone and its registry handle, drops).
    registry: SessionRegistry,
    /// Tripped once the session has registered, so registration runs exactly
    /// once regardless of how many tools the agent calls.
    registered: Arc<OnceLock<()>>,
}

#[derive(Clone)]
pub struct SessionMcpServer {
    inner: FilesystemMcpServer,
    /// Max real (non-stub) entities this session may hold. A `create`
    /// at or above the cap is refused with a typed `RESOURCE_CAP_EXCEEDED`.
    entity_cap: usize,
    /// The live-view binding for the connection-born flow (see [`ViewBinding`]).
    /// `None` for the page-first `POST /sessions` flow, which returns URLs over
    /// HTTP and needs no MCP-channel link.
    view: Option<ViewBinding>,
}

impl SessionMcpServer {
    pub fn new(inner: FilesystemMcpServer, entity_cap: usize) -> Self {
        Self {
            inner,
            entity_cap,
            view: None,
        }
    }

    /// Build from a pre-mounted writable session [`Engine`] with a
    /// per-session entity cap.
    pub fn from_engine(engine: Engine, entity_cap: usize) -> Self {
        Self::new(FilesystemMcpServer::from_engine(engine, PathBuf::new()), entity_cap)
    }

    /// Attach a connection-born live-view binding: the public `view_id`, the
    /// `view_base` origin to compose the link against, and the `registry` to
    /// self-register into on the first tool call. The binding is NOT registered
    /// here — see [`ViewBinding`] and [`Self::ensure_view_registered`].
    pub fn with_view(
        mut self,
        view_id: impl Into<String>,
        view_base: impl Into<String>,
        registry: SessionRegistry,
    ) -> Self {
        self.view = Some(ViewBinding {
            id: view_id.into(),
            base: view_base.into(),
            registry,
            registered: Arc::new(OnceLock::new()),
        });
        self
    }

    /// This session's shareable read-only live-view link, if it has a view
    /// binding. `None` for the page-first flow.
    fn live_view_url(&self) -> Option<String> {
        self.view.as_ref().map(|v| format!("{}/v/{}", v.base, v.id))
    }

    /// Register this session under its view id the first time it is called
    /// (idempotent; a no-op without a view binding and on every later call).
    /// The registered clone shares this server's engine `Arc`, so the
    /// `/v/{id}` endpoints render exactly what the agent writes.
    ///
    /// Returns `false` only when the global session ceiling blocked
    /// registration — the caller then refuses the tool call so the session
    /// never does work it can't surface. `true` once registered, and always
    /// `true` for the page-first flow (no view binding). Not refused → the
    /// session is registered and resolvable at `/v/{id}`.
    fn ensure_view_registered(&self) -> bool {
        let Some(v) = &self.view else { return true };
        if v.registered.get().is_some() {
            return true;
        }
        if v.registry.register_view_session(v.id.clone(), self.clone()) {
            // Mark registered so later calls skip the ceiling check. A benign
            // race (two concurrent first calls) is harmless:
            // `register_view_session` is mutex-guarded and idempotent on the id.
            let _ = v.registered.set(());
            true
        } else {
            false
        }
    }

    /// The scoped tool list: the basis router's tools filtered to the 10
    /// entity tools, so each `Tool`'s schema/description is byte-identical.
    pub fn session_tools() -> Vec<Tool> {
        FilesystemMcpServer::tool_router()
            .list_all()
            .into_iter()
            .filter(|t| Self::is_session_tool(t.name.as_ref()))
            .collect()
    }

    pub fn is_session_tool(name: &str) -> bool {
        SESSION_ENTITY_TOOLS.contains(&name)
    }

    /// Seal this session's **sketch** vault into `.mem` archive bytes — the
    /// funnel exit. Targets [`SESSION_VAULT_NAME`] explicitly (not the
    /// engine's first vault), so the export bundles only the visitor's own
    /// graph and never the read-only content vault. The bytes are
    /// self-describing (schema embedded) and mount standalone in the real
    /// engine.
    pub fn export_bytes(&self) -> Result<Vec<u8>, memstead_base::EngineError> {
        self.inner
            .with_engine(|e| e.export_vault_to_bytes(SESSION_VAULT_NAME))
    }

    /// Count of real (non-stub) entities in this session's writable sketch
    /// vault. The per-session cap is scoped to the visitor's own graph —
    /// the read-only content vault's entities never count against it (the
    /// engine's `stats().entity_count` would sum both mounts).
    fn sketch_entity_count(&self) -> usize {
        self.inner.with_engine(|e| {
            e.store()
                .all_entities()
                .filter(|ent| ent.vault == SESSION_VAULT_NAME && !ent.stub)
                .count()
        })
    }

    /// The current `{nodes, edges, communities}` projection of this
    /// session's vault — the snapshot a viewer renders, recomputed from the
    /// live store on every call.
    pub fn graph_snapshot(&self) -> GraphSnapshot {
        self.inner.with_engine(|e| graph_projection(e, SESSION_VAULT_NAME))
    }

    /// Subscribe to this session's vault-change events — the trigger that
    /// drives a fresh projection per agent mutation. Returns the keep-alive
    /// handle (drop unsubscribes) and a broadcast receiver.
    pub fn subscribe_changes(
        &self,
    ) -> Result<
        (
            memstead_base::engine::SubscriptionHandle,
            tokio::sync::broadcast::Receiver<memstead_base::engine::VaultChangedEvent>,
        ),
        memstead_base::EngineError,
    > {
        self.inner
            .with_engine(|e| e.subscribe_vault_changes_broadcast(SESSION_VAULT_NAME))
    }
}

impl ServerHandler for SessionMcpServer {
    /// Self-describe as a writable session surface. Keeps the inner
    /// server's protocol version and capabilities (the tools are real,
    /// just scoped) but overrides identity + instructions so the handshake
    /// itself names the 10 available tools and the withheld classes.
    fn get_info(&self) -> ServerInfo {
        let mut info = self.inner.get_info();
        info.server_info.name = "memstead-session".to_string();
        info.server_info.title = Some("Memstead (sketch session)".to_string());
        info.server_info.description = Some(
            "Writable MCP surface over an ephemeral per-session Memstead vault — ten entity tools, \
no lifecycle/policy/admin path."
                .to_string(),
        );
        // Connection-born flow: the handshake names the live-view channel but
        // bakes NO URL. The link is delivered by the first `memstead_overview`
        // call instead — the only channel guaranteed to address the session
        // that carries the agent's writes. (rmcp invokes the service factory
        // per `initialize`, so a URL baked into the handshake can belong to a
        // different session than the one the client ends up driving; see
        // `ViewBinding`.)
        info.instructions = Some(match &self.view {
            Some(_) => format!(
                "{}\n\nLive view: call memstead_overview first — its response carries your \
private, read-only live-view link to hand to a human. The graph builds there as you work.",
                session_instructions()
            ),
            None => session_instructions(),
        });
        info
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        // Let the inner server do its client/peer bookkeeping, then return
        // *this* server's session self-description — the inner `initialize`
        // echoes the basis `get_info()`, which would defeat the override.
        self.inner.initialize(request, context).await?;
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: Self::session_tools(),
            meta: None,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if !Self::is_session_tool(request.name.as_ref()) {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "ERROR [TOOL_NOT_FOUND]: tool '{}' is not available on this session surface",
                request.name
            ))]));
        }
        // Per-session resource cap: a `create` at or above the entity
        // budget is refused with a typed code before it can grow the
        // vault. Deletes are never gated, so a capped session can free
        // room and continue. `relate`/`update` auto-stubs don't count
        // (stubs are excluded from the entity count), so only `create`
        // — the real-entity adder — is gated.
        if request.name.as_ref() == "memstead_create"
            && self.sketch_entity_count() >= self.entity_cap
        {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "ERROR [RESOURCE_CAP_EXCEEDED]: session entity cap of {} reached; \
delete entities to free space",
                self.entity_cap
            ))]));
        }
        // Bind the live view to THIS session the first time it does real work
        // (idempotent). Only a session that calls a tool becomes resolvable at
        // `/v/{id}`, so a bare handshake — a capability probe, a reconnect, an
        // IDE that re-initializes — never leaves a "live but empty" sibling
        // vault behind. The registered clone shares this session's engine
        // `Arc`, so the view renders exactly what the agent writes.
        //
        // If the global session ceiling is hit, registration is refused and we
        // shed this session with a typed code (rather than OOM the box and drop
        // everyone). Transient: a reconnect once idle sessions are evicted gets
        // in. Reads (overview/search/entity/schema/health) and writes alike are
        // refused here, because an unregistered session has no resolvable view.
        if !self.ensure_view_registered() {
            return Ok(CallToolResult::error(vec![Content::text(
                "ERROR [RESOURCE_CAP_EXCEEDED]: memstead.ai is at capacity (too many live \
sketch sessions right now). Reconnect and try again in a few minutes."
                    .to_string(),
            )]));
        }

        let is_overview = request.name.as_ref() == "memstead_overview";
        let tcc = ToolCallContext::new(&self.inner, request, context);
        let mut result = FilesystemMcpServer::tool_router().call(tcc).await?;

        // Surface the shareable live-view link through the mandated cold-start
        // read. This is the only channel that guarantees the link the agent
        // hands a human addresses the working session: the `initialize`
        // handshake cannot, because a client may surface instructions from a
        // different `initialize` than the one carrying the writes.
        if is_overview
            && let Some(url) = self.live_view_url()
        {
            result.content.push(Content::text(format!(
                "\n---\nLive view (read-only — share so a human can watch this graph build): {url}"
            )));
        }
        Ok(result)
    }
}

/// The per-session streamable-HTTP service type. Clone-cheap (an `Arc`
/// clone of the shared session engine), stored once per session and
/// dispatched to on each request.
pub type SessionService = StreamableHttpService<SessionMcpServer, LocalSessionManager>;

/// Build the streamable-HTTP MCP service for one session. The handler
/// factory clones the session server per MCP-protocol session, so every
/// request within a sketch session hits the same in-memory vault.
fn session_service(server: SessionMcpServer) -> SessionService {
    // A public service behind a proxy under its own hostname — rmcp's
    // loopback-only `allowed_hosts` default (a DNS-rebinding guard for
    // locally-run servers) would 400 every real client. The surface's
    // protection is tool-list scoping + per-session isolation, not Host
    // pinning.
    let config = StreamableHttpServerConfig::default().disable_allowed_hosts();
    StreamableHttpService::new(move || Ok(server.clone()), Default::default(), config)
}

/// Typed session-resolution failures. Surfaced to callers (HTTP handlers)
/// which map `code()` onto a transport status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// No live session under this id — never created, or already evicted.
    /// The two collapse to one refusal on purpose: a resolved id always
    /// means a live session, so eviction is observable (the id stops
    /// resolving) and never masked by minting a fresh empty vault.
    UnknownSession(String),
    /// The session vault engine failed to initialise.
    EngineInit(String),
    /// The session is live but sealing its vault to `.mem` failed.
    ExportFailed(String),
    /// The global live-session ceiling is reached — a new session is refused
    /// so the box sheds load instead of exhausting memory. Transient: a retry
    /// once sessions free up (idle eviction) succeeds.
    AtCapacity,
}

impl SessionError {
    pub fn code(&self) -> &'static str {
        match self {
            SessionError::UnknownSession(_) => "UNKNOWN_SESSION",
            SessionError::EngineInit(_) => "SESSION_INIT_FAILED",
            SessionError::ExportFailed(_) => "EXPORT_FAILED",
            SessionError::AtCapacity => "AT_CAPACITY",
        }
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::UnknownSession(id) => {
                write!(f, "no live session for id '{id}' (never created or evicted)")
            }
            SessionError::EngineInit(msg) => write!(f, "session engine init failed: {msg}"),
            SessionError::ExportFailed(msg) => write!(f, "session export failed: {msg}"),
            SessionError::AtCapacity => write!(
                f,
                "at capacity: too many live sketch sessions right now; retry in a few minutes"
            ),
        }
    }
}

impl std::error::Error for SessionError {}

struct SessionEntry {
    /// The scoped session server — held alongside the service so the
    /// registry can reach the engine (e.g. to export the vault) without
    /// going through the MCP transport. Shares the same `Arc`-wrapped
    /// engine as the service's per-request server clones.
    server: SessionMcpServer,
    /// The per-session MCP service for the page-first flow (`POST /sessions`
    /// → `/s/{id}/mcp`). `None` for a connection-born session: it was minted
    /// by the shared stable `/mcp` endpoint and has no per-session MCP route
    /// — only its view endpoints (`/v/{id}/...`) resolve through the registry.
    service: Option<SessionService>,
    /// Wall-clock of the last resolution. Drives idle eviction.
    last_active: Instant,
}

/// A boxed, shareable session-id generator. Production injects a CSPRNG
/// (an unguessable id is the only thing scoping a throwaway vault); tests
/// inject a deterministic counter.
pub type IdGen = Arc<dyn Fn() -> String + Send + Sync>;

/// In-process registry of live sessions: `session id → engine/service`.
/// Cheap to clone (shares one `Arc<Mutex<_>>`), so it doubles as axum
/// shared state. Each session is one writable in-memory vault, dropped on
/// eviction.
#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<Mutex<HashMap<String, SessionEntry>>>,
    /// The curated read-only content source mounted alongside every
    /// session's sketch vault (shared across sessions — each session mounts
    /// its own read-only view of it). `Folder`/`Archive` point at a real
    /// curated vault; `InMemory` is an empty placeholder until one is wired.
    content_storage: MountStorage,
    /// Schema pinned on the content mount (matches the source's own pin for
    /// folder/archive sources).
    content_schema: SchemaRef,
    /// Vault name the content mount registers under. Must match the content
    /// source's own vault name — an archive's ids are `<name>--slug`, so a
    /// mismatch makes its entities invisible. Configurable so any curated graph
    /// (not only the default [`CONTENT_VAULT_NAME`]) can back the read tier.
    content_vault_name: String,
    ttl: Duration,
    entity_cap: usize,
    /// Ceiling on concurrently-live sessions. Defaults to unlimited
    /// (`usize::MAX`) so tests and embedders are unaffected; the public
    /// binaries set a real cap from `MEMSTEAD_SESSION_MAX`. See
    /// [`Self::with_max_sessions`].
    max_sessions: usize,
    id_gen: IdGen,
}

impl SessionRegistry {
    /// Build a registry whose sessions each mount a writable sketch vault
    /// (idle-evicting after `ttl`, holding at most `entity_cap` real
    /// entities) plus a shared read-only content vault from
    /// `content_storage`/`content_schema`. Ids are drawn from `id_gen`.
    pub fn new(
        content_storage: MountStorage,
        content_schema: SchemaRef,
        content_vault_name: String,
        ttl: Duration,
        entity_cap: usize,
        id_gen: IdGen,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            content_storage,
            content_schema,
            content_vault_name,
            ttl,
            entity_cap,
            max_sessions: usize::MAX,
            id_gen,
        }
    }

    /// Cap the number of concurrently-live sessions this registry will hold.
    /// Beyond the cap, minting a new session is refused — page-first with a
    /// [`SessionError::AtCapacity`] (a 503), connection-born with a typed
    /// `RESOURCE_CAP_EXCEEDED` on the first tool call — so a spike sheds load
    /// gracefully instead of OOM-ing the process and dropping every live
    /// session at once. Idle eviction frees slots, so the cap bounds
    /// concurrency, not lifetime. Default (unset): unlimited.
    pub fn with_max_sessions(mut self, max_sessions: usize) -> Self {
        self.max_sessions = max_sessions;
        self
    }

    /// Mint a session: build its two-mount engine (writable sketch +
    /// read-only content), wrap it in a scoped session server +
    /// streamable-HTTP service, register it under a fresh id, and return
    /// the id. `sketch_schema` pins the new writable vault.
    pub fn create_session(&self, sketch_schema: SchemaRef) -> Result<String, SessionError> {
        // Shed before building the engine when the live-session ceiling is hit,
        // so a spike never pays the engine-mount cost just to be refused.
        if self.len() >= self.max_sessions {
            return Err(SessionError::AtCapacity);
        }
        let engine = mount_session_engine(
            self.content_storage.clone(),
            self.content_schema.clone(),
            self.content_vault_name.clone(),
            sketch_schema,
        )
        .map_err(|e| SessionError::EngineInit(e.to_string()))?;
        let server = SessionMcpServer::from_engine(engine, self.entity_cap);
        let service = session_service(server.clone());
        let id = (self.id_gen)();
        let mut map = self.inner.lock().expect("session registry mutex poisoned");
        // Re-check under the insert lock: the pre-check window above could have
        // filled. Keeps the ceiling exact under concurrent creates.
        if map.len() >= self.max_sessions {
            return Err(SessionError::AtCapacity);
        }
        map.insert(
            id.clone(),
            SessionEntry {
                server,
                service: Some(service),
                last_active: Instant::now(),
            },
        );
        Ok(id)
    }

    /// Register a pre-built connection-born session under `id`. The session
    /// was minted by the shared stable `/mcp` endpoint's handler factory (one
    /// engine per MCP connection), so it carries no per-session MCP service —
    /// only its view endpoints (`/v/{id}/...`) resolve through the registry.
    /// Idempotent on the id: re-registering replaces the entry (a fresh
    /// connection under a colliding id is vanishingly unlikely with a CSPRNG,
    /// but replacement keeps the registry single-valued).
    ///
    /// Returns `false` when the global session ceiling is reached and this is a
    /// genuinely new id — the caller then refuses the session's first tool
    /// call. Re-registering an id already present always succeeds (it does not
    /// grow the live set).
    pub fn register_view_session(&self, id: String, server: SessionMcpServer) -> bool {
        let mut map = self.inner.lock().expect("session registry mutex poisoned");
        if !map.contains_key(&id) && map.len() >= self.max_sessions {
            return false;
        }
        map.insert(
            id,
            SessionEntry {
                server,
                service: None,
                last_active: Instant::now(),
            },
        );
        true
    }

    /// Resolve a session's MCP service, refreshing its idle clock. A missing
    /// id (never created or already evicted) is a typed refusal — never a
    /// silently-minted fresh vault. A connection-born session (no per-session
    /// service) also refuses: its MCP surface is the shared `/mcp`, not
    /// `/s/{id}/mcp`.
    pub fn service_for(&self, id: &str) -> Result<SessionService, SessionError> {
        let mut map = self.inner.lock().expect("session registry mutex poisoned");
        match map.get_mut(id) {
            Some(entry) => match &entry.service {
                Some(service) => {
                    entry.last_active = Instant::now();
                    Ok(service.clone())
                }
                None => Err(SessionError::UnknownSession(id.to_string())),
            },
            None => Err(SessionError::UnknownSession(id.to_string())),
        }
    }

    /// Resolve a session's server, refreshing its idle clock. A missing id
    /// (never created or evicted) is a typed refusal. The shared seam for
    /// every server-reaching op (export, snapshot, subscribe).
    fn server_for(&self, id: &str) -> Result<SessionMcpServer, SessionError> {
        let mut map = self.inner.lock().expect("session registry mutex poisoned");
        match map.get_mut(id) {
            Some(entry) => {
                entry.last_active = Instant::now();
                Ok(entry.server.clone())
            }
            None => Err(SessionError::UnknownSession(id.to_string())),
        }
    }

    /// Seal a session's current vault into `.mem` archive bytes (the
    /// funnel exit). Scoped to the requesting session — it only ever
    /// bundles that session's vault. An unknown or evicted id is a typed
    /// refusal, never an empty or stale archive. Refreshes the idle clock.
    pub fn export_session(&self, id: &str) -> Result<Vec<u8>, SessionError> {
        self.server_for(id)?
            .export_bytes()
            .map_err(|e| SessionError::ExportFailed(e.to_string()))
    }

    /// The current graph projection for a session. Unknown/evicted id is a
    /// typed refusal (not an empty snapshot that looks live).
    pub fn snapshot(&self, id: &str) -> Result<GraphSnapshot, SessionError> {
        Ok(self.server_for(id)?.graph_snapshot())
    }

    /// Resolve a session and subscribe to its change stream. Returns the
    /// server (for per-frame snapshot recompute), the keep-alive handle,
    /// and the event receiver. Unknown/evicted id is a typed refusal —
    /// never a live-looking stream that never updates.
    pub fn subscribe_session(
        &self,
        id: &str,
    ) -> Result<
        (
            SessionMcpServer,
            memstead_base::engine::SubscriptionHandle,
            tokio::sync::broadcast::Receiver<memstead_base::engine::VaultChangedEvent>,
        ),
        SessionError,
    > {
        let server = self.server_for(id)?;
        let (handle, rx) = server
            .subscribe_changes()
            .map_err(|e| SessionError::EngineInit(e.to_string()))?;
        Ok((server, handle, rx))
    }

    /// Evict every session idle longer than the registry TTL relative to
    /// `now`, releasing its vault. Returns the number evicted. `now` is a
    /// parameter (not read from the clock) so eviction is deterministically
    /// testable.
    pub fn sweep_expired(&self, now: Instant) -> usize {
        let ttl = self.ttl;
        let mut map = self.inner.lock().expect("session registry mutex poisoned");
        let before = map.len();
        map.retain(|_, e| now.duration_since(e.last_active) < ttl);
        before - map.len()
    }

    /// Live session count.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("session registry mutex poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// HTTP surface — session creation + vault export
// ---------------------------------------------------------------------------

/// Optional `POST /sessions` body.
#[derive(serde::Deserialize, Default)]
struct CreateSessionBody {
    /// `<name>@<version>` schema pin for the session vault. Absent →
    /// `default@1.0.0`. (The schema-choice policy is plan 04's; this
    /// server just takes the pin.)
    schema: Option<String>,
}

/// `POST /sessions` — mint a session and return its id plus the URLs an
/// agent adds (MCP endpoint) and a visitor downloads (`.mem` export).
async fn create_session_handler(
    State(reg): State<SessionRegistry>,
    body: Option<Json<CreateSessionBody>>,
) -> Response {
    let schema = body
        .and_then(|Json(b)| b.schema)
        .and_then(|s| s.parse::<SchemaRef>().ok())
        .unwrap_or_else(|| "default@1.0.0".parse().expect("static default pin parses"));
    match reg.create_session(schema) {
        Ok(id) => {
            let payload = serde_json::json!({
                "session_id": id,
                "mcp_url": format!("/s/{id}/mcp"),
                "export_url": format!("/s/{id}/export"),
            });
            (StatusCode::CREATED, Json(payload)).into_response()
        }
        Err(e) => session_error_response(&e),
    }
}

/// `GET /s/{id}/export` — seal the session's vault and stream the `.mem`
/// archive as a download. An unknown or evicted id is a typed 404.
async fn export_handler(State(reg): State<SessionRegistry>, Path(id): Path<String>) -> Response {
    match reg.export_session(&id) {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"memstead-session.mem\"",
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => session_error_response(&e),
    }
}

/// Serialise a projection snapshot into an SSE `graph` event.
fn sse_graph_event(snapshot: &GraphSnapshot) -> Result<Event, Infallible> {
    let data = serde_json::to_string(snapshot).unwrap_or_else(|_| "{}".to_string());
    Ok(Event::default().event("graph").data(data))
}

/// `GET /s/{id}/graph` — the current graph projection as JSON. Unknown or
/// evicted id is a typed 404.
async fn graph_handler(State(reg): State<SessionRegistry>, Path(id): Path<String>) -> Response {
    match reg.snapshot(&id) {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(e) => session_error_response(&e),
    }
}

/// `GET /s/{id}/stream` — Server-Sent Events: a current snapshot on
/// subscribe (so a late joiner renders immediately), then a fresh snapshot
/// after each vault mutation. Each frame is recomputed from the live store,
/// so a deleted/renamed entity never lingers. Unknown/evicted id is a typed
/// 404 (not a live-looking stream that never updates).
async fn stream_handler(State(reg): State<SessionRegistry>, Path(id): Path<String>) -> Response {
    let (server, handle, rx) = match reg.subscribe_session(&id) {
        Ok(triple) => triple,
        Err(e) => return session_error_response(&e),
    };
    // Snapshot-on-subscribe, then one recomputed snapshot per change event.
    let initial = sse_graph_event(&server.graph_snapshot());
    // Keep the subscription handle alive for the stream's lifetime — dropping
    // it unsubscribes, which would silently stop updates.
    let handle = Arc::new(handle);
    let updates = BroadcastStream::new(rx).map(move |item| {
        let _alive = handle.clone();
        match item {
            Ok(_event) => sse_graph_event(&server.graph_snapshot()),
            // A lagged subscriber missed events; the next frame it does get
            // is still a full current snapshot, so correctness holds.
            Err(_lagged) => Ok(Event::default().event("lagged").data("{}")),
        }
    });
    let stream = tokio_stream::once(initial).chain(updates);
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

/// `/s/{id}/mcp` — the per-session remote-MCP endpoint. Resolves the
/// session's streamable-HTTP service and forwards the request to it. The
/// rmcp service routes on HTTP method (not path), so the `/s/{id}/mcp`
/// prefix needs no rewriting; an unknown or evicted id is a typed 404
/// before any MCP processing.
async fn mcp_handler(
    State(reg): State<SessionRegistry>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    match reg.service_for(&id) {
        // The service's error type is `Infallible`, so the dispatch never
        // fails at this layer; the response (including MCP-level errors)
        // comes straight back from the rmcp transport.
        Ok(svc) => svc
            .oneshot(req)
            .await
            .unwrap_or_else(|never| match never {})
            .into_response(),
        Err(e) => session_error_response(&e),
    }
}

fn session_error_response(e: &SessionError) -> Response {
    let status = match e.code() {
        "UNKNOWN_SESSION" => StatusCode::NOT_FOUND,
        // Temporary overload, not a client error — 503 invites a retry.
        "AT_CAPACITY" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    crate::error_response(status, e.code(), e.to_string(), serde_json::Value::Null)
}

/// The session HTTP routes over a shared [`SessionRegistry`], without the
/// rate limiter — the assembly seam the rate-limited [`build_session_app`]
/// wraps and tests drive directly. Mounts session creation
/// (`POST /sessions`), the per-session remote-MCP endpoint
/// (`/s/{id}/mcp`), the live graph snapshot + stream
/// (`GET /s/{id}/graph`, `GET /s/{id}/stream`), and vault export
/// (`GET /s/{id}/export`).
pub fn session_router(registry: SessionRegistry) -> Router {
    Router::new()
        .route("/sessions", post(create_session_handler))
        .route("/s/{id}/mcp", any(mcp_handler))
        .route("/s/{id}/graph", get(graph_handler))
        .route("/s/{id}/stream", get(stream_handler))
        .route("/s/{id}/export", get(export_handler))
        .with_state(registry)
}

/// The public session app: [`session_router`] plus a per-IP rate limiter
/// that refuses over-budget requests with a typed 429 (never a 500).
/// Serve with `into_make_service_with_connect_info::<SocketAddr>()` so the
/// limiter can key on the peer address.
pub fn build_session_app(registry: SessionRegistry, per_second: u64, burst: u32) -> Router {
    use tower_governor::GovernorLayer;
    use tower_governor::governor::GovernorConfigBuilder;
    use tower_governor::key_extractor::SmartIpKeyExtractor;

    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(per_second)
            .burst_size(burst)
            // Key on the real client IP. Behind a reverse proxy (Railway's edge)
            // the socket peer is the proxy, so the default peer-IP extractor
            // would lump every visitor into one bucket — a single shared limit.
            // `SmartIpKeyExtractor` reads `X-Forwarded-For` / `X-Real-Ip` /
            // `Forwarded` and falls back to the peer IP, so each developer gets
            // their own budget. (Trustworthy here: clients reach this only
            // through the proxy, which sets the header.)
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("rate-limit config must build"),
    );
    session_router(registry).layer(GovernorLayer::new(governor_conf))
}

// ---------------------------------------------------------------------------
// Connection-born flow — a single stable `/mcp` endpoint that mints a session
// per MCP connection and delivers the view URL through the handshake.
// ---------------------------------------------------------------------------

/// Build the **single, stable** streamable-HTTP MCP service for the
/// connection-born flow. Unlike [`session_service`] (one service per
/// already-minted session), this one service serves every visitor: rmcp calls
/// the handler factory once per `initialize`, and each call mints a fresh
/// two-mount engine (the visitor's own sketch vault + the shared read-only
/// content vault) and a fresh application-generated view id, then attaches a
/// [`ViewBinding`] carrying the registry. The session is born from the
/// connection — there is no separate session-creation API.
///
/// Registration is **deferred to the session's first tool call**, not done
/// here: rmcp invokes this factory per `initialize`, and a client may
/// `initialize` more than once per logical connection. Registering every
/// handshake eagerly is exactly what made a probe/duplicate `initialize`
/// publish a resolvable, empty `/v/{id}` that a client could surface as the
/// link — the "live but empty" failure. Binding on first real work, and
/// surfacing the link from that session's own `memstead_overview` response,
/// guarantees the link a human receives addresses the vault the agent writes.
///
/// The view id is application-generated rather than the rmcp transport session
/// id: that id is unavailable to the handler at `initialize` time, and it is a
/// bearer credential that must never appear in a shareable URL anyway.
pub fn build_sketch_mcp_service(
    registry: SessionRegistry,
    sketch_schema: SchemaRef,
    view_base: String,
) -> SessionService {
    // A public service behind a proxy under its own hostname — rmcp's
    // loopback-only `allowed_hosts` default would 400 every real client. The
    // surface's protection is tool-list scoping + per-connection isolation.
    let config = StreamableHttpServerConfig::default().disable_allowed_hosts();
    let factory = move || {
        let engine = mount_session_engine(
            registry.content_storage.clone(),
            registry.content_schema.clone(),
            registry.content_vault_name.clone(),
            sketch_schema.clone(),
        )
        .map_err(|e| std::io::Error::other(e.to_string()))?;
        let view_id = (registry.id_gen)();
        // Attach the view binding but DO NOT register here — the server
        // self-registers on its first tool call (see `ensure_view_registered`),
        // so a bare handshake leaves no resolvable empty vault behind.
        let server = SessionMcpServer::from_engine(engine, registry.entity_cap).with_view(
            view_id,
            view_base.clone(),
            registry.clone(),
        );
        Ok(server)
    };
    StreamableHttpService::new(factory, Default::default(), config)
}

/// The connection-born HTTP routes over a shared [`SessionRegistry`], without
/// the rate limiter. This is the agent/data plane only — it serves no HTML
/// page. It mounts the stable remote-MCP endpoint (`/mcp`) and the per-session
/// view *data* endpoints the agent's view URL points at — the live graph
/// snapshot + stream (`GET /v/{id}/graph`, `GET /v/{id}/stream`) and vault
/// export (`GET /v/{id}/export`). The view handlers are shared with the
/// page-first `/s/{id}/...` routes — same registry, same id resolution.
///
/// The human face that renders `/v/{id}` lives in the Astro `memstead.ai`
/// build, not here: `view_base` flows into the MCP service so the `initialize`
/// handshake hands the agent an absolute `/v/{id}` view URL, and the page that
/// URL opens is served by Astro.
pub fn sketch_router(
    registry: SessionRegistry,
    sketch_schema: SchemaRef,
    view_base: String,
    soft_launch: bool,
) -> Router {
    let mcp = build_sketch_mcp_service(registry.clone(), sketch_schema, view_base);
    // Soft-launch gate: the writable MCP connect endpoint is a real `.ai` surface
    // the plan keeps off the public top-level — while gated it answers only at
    // `/try/mcp` (the runbook + the `.ai` try page point there in lockstep). The
    // per-session view DATA endpoints (`/v/{id}/...`) carry unguessable ids, so
    // they are not a discoverable top-level surface and stay put.
    let mcp_path = if soft_launch { "/try/mcp" } else { "/mcp" };
    Router::new()
        .route("/v/{id}/graph", get(graph_handler))
        .route("/v/{id}/stream", get(stream_handler))
        .route("/v/{id}/export", get(export_handler))
        .with_state(registry)
        .nest_service(mcp_path, mcp)
}

/// The public unified session app: the connection-born surface (`/mcp` +
/// `/v/{id}/...`) merged with the page-first surface (`POST /sessions` +
/// `/s/{id}/...`), under one per-IP rate limiter (a typed 429, never a 500).
/// The two surfaces share the registry, so a session minted by either is
/// reachable by the view endpoints. Serve with
/// `into_make_service_with_connect_info::<SocketAddr>()`.
pub fn build_sketch_app(
    registry: SessionRegistry,
    sketch_schema: SchemaRef,
    view_base: String,
    per_second: u64,
    burst: u32,
    soft_launch: bool,
) -> Router {
    use tower_governor::GovernorLayer;
    use tower_governor::governor::GovernorConfigBuilder;
    use tower_governor::key_extractor::SmartIpKeyExtractor;

    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(per_second)
            .burst_size(burst)
            // Real client IP via forwarded headers (see `build_session_app`) —
            // behind the proxy the peer is the edge, so per-client keying needs
            // `X-Forwarded-For`, else all visitors share one bucket.
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("rate-limit config must build"),
    );
    session_router(registry.clone())
        .merge(sketch_router(registry, sketch_schema, view_base, soft_launch))
        .layer(GovernorLayer::new(governor_conf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tower::ServiceExt;

    fn default_schema() -> SchemaRef {
        SchemaRef::new("default", semver::Version::new(1, 0, 0))
    }

    /// Deterministic id generator — `sess-0`, `sess-1`, … — so tests assert
    /// exact ids. Production injects a CSPRNG instead.
    fn counter_id_gen() -> IdGen {
        let n = Arc::new(AtomicU64::new(0));
        Arc::new(move || format!("sess-{}", n.fetch_add(1, Ordering::Relaxed)))
    }

    fn registry(ttl: Duration) -> SessionRegistry {
        registry_with_cap(ttl, DEFAULT_SESSION_ENTITY_CAP)
    }

    fn registry_with_cap(ttl: Duration, entity_cap: usize) -> SessionRegistry {
        // Default fixture: an empty in-memory content placeholder. Tests that
        // exercise cross-vault content reads wire a seeded folder source via
        // `registry_with_content`.
        SessionRegistry::new(
            MountStorage::InMemory,
            default_schema(),
            CONTENT_VAULT_NAME.to_string(),
            ttl,
            entity_cap,
            counter_id_gen(),
        )
    }

    /// A registry whose read-only content vault is a seeded folder source —
    /// for the cross-vault-read and capability-split tests. `content_dir`
    /// must outlive the registry (the mount reads it lazily).
    fn registry_with_content(ttl: Duration, content_dir: &std::path::Path) -> SessionRegistry {
        SessionRegistry::new(
            MountStorage::Folder {
                path: content_dir.to_path_buf(),
            },
            default_schema(),
            CONTENT_VAULT_NAME.to_string(),
            ttl,
            DEFAULT_SESSION_ENTITY_CAP,
            counter_id_gen(),
        )
    }

    /// Write a one-entity curated content vault into `dir` (folder backend):
    /// a single `concept` entity. The content mount's schema comes from its
    /// `Mount.schema` pin, so no on-disk config is needed (mirrors the
    /// read-only serve's folder fixture).
    fn seed_content_vault(dir: &std::path::Path) {
        std::fs::write(
            dir.join("what-is-memstead.md"),
            "---\ntype: concept\n---\n# What Is Memstead\n\n## Definition\n\n\
A schema-agnostic graph engine over markdown + git.\n\n## Explanation\n\n\
Each vault keeps a typed model of a chosen subject.\n",
        )
        .unwrap();
    }

    /// POST a JSON-RPC body to a session's MCP service over the real
    /// streamable-HTTP transport, optionally carrying an established MCP
    /// session id. Returns `(status, mcp-session-id, body)`. The body is
    /// the raw streamable-HTTP response (an SSE `event: message` frame);
    /// assertions are by substring, no SSE parsing needed. The request
    /// path is irrelevant — the service routes on method, not path.
    async fn mcp_post(
        svc: &SessionService,
        body: &str,
        session: Option<&str>,
    ) -> (StatusCode, Option<String>, String) {
        let mut req = Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "session.example")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(s) = session {
            req = req.header("mcp-session-id", s);
        }
        let resp = svc
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let sid = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, sid, String::from_utf8_lossy(&bytes).to_string())
    }

    const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;
    const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

    /// Bring a freshly-created session to the post-handshake state and
    /// return its service + MCP session id, ready for tool calls.
    async fn handshaken(reg: &SessionRegistry) -> (SessionService, String) {
        let id = reg.create_session(default_schema()).unwrap();
        let svc = reg.service_for(&id).unwrap();
        let (status, sid, _) = mcp_post(&svc, INIT, None).await;
        assert_eq!(status, StatusCode::OK, "initialize must return 200");
        let sid = sid.expect("initialize issues an Mcp-Session-Id");
        let (status, ..) = mcp_post(&svc, INITIALIZED, Some(&sid)).await;
        assert!(status.is_success(), "notifications/initialized must be accepted");
        (svc, sid)
    }

    // ----- AC2: the allowlist is exactly the 10 entity tools, fail-safe ---

    #[test]
    fn session_lists_exactly_the_ten_entity_tools() {
        let mut scoped: Vec<String> = SessionMcpServer::session_tools()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        scoped.sort();
        let mut expected: Vec<String> =
            SESSION_ENTITY_TOOLS.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(scoped, expected, "session must list exactly the 10 entity tools");
        assert_eq!(SESSION_ENTITY_TOOLS.len(), 10);
    }

    #[test]
    fn withheld_tools_are_absent_and_refused() {
        let scoped: Vec<String> = SessionMcpServer::session_tools()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        for withheld in [
            "memstead_vault_create",
            "memstead_vault_delete",
            "memstead_vault_set_schema",
            "memstead_vault_set_version",
            "memstead_workspace_allow_create",
            "memstead_workspace_revoke_create",
            "memstead_workspace_allow_delete",
            "memstead_workspace_revoke_delete",
            "memstead_workspace_grant_cross_link",
            "memstead_workspace_revoke_cross_link",
            "memstead_reload",
            "memstead_diff",
            "memstead_changes_since",
        ] {
            assert!(
                !scoped.contains(&withheld.to_string()),
                "{withheld} must be absent from the session tool list"
            );
            assert!(
                !SessionMcpServer::is_session_tool(withheld),
                "{withheld} must be refused on call"
            );
        }
    }

    /// Fail-safe: the allowlist and the derived withheld set together
    /// exactly partition every tool the engine exposes. Any tool added to
    /// the engine that is not added to the allowlist lands in the withheld
    /// set — withheld by default, never auto-exposed.
    #[test]
    fn allowlist_and_withheld_partition_the_full_surface() {
        let all: std::collections::BTreeSet<String> = FilesystemMcpServer::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        let allowed: std::collections::BTreeSet<String> =
            SESSION_ENTITY_TOOLS.iter().map(|s| s.to_string()).collect();
        let withheld: std::collections::BTreeSet<String> =
            session_withheld_tools().into_iter().collect();

        // Allowlist is a real subset of the live surface (no stale names).
        assert!(allowed.is_subset(&all), "every allowlisted tool must exist on the router");
        // Disjoint, and together cover everything: a partition.
        assert!(allowed.is_disjoint(&withheld), "a tool is either allowed or withheld, not both");
        let union: std::collections::BTreeSet<String> =
            allowed.union(&withheld).cloned().collect();
        assert_eq!(union, all, "allowed ∪ withheld must equal the full tool surface");
        // The session server is hosted on the *basis* FilesystemMcpServer,
        // whose universe is the 10 entity tools plus two admin tools
        // (changes_since, diff). The vault-lifecycle and workspace-policy
        // tools are vault-repo-only and not present on this host at all —
        // defense in depth: they cannot be dispatched regardless of the
        // allowlist. So the derived withheld set is exactly those two
        // admin tools, and it is non-empty (filtering is meaningful).
        assert_eq!(withheld.len(), all.len() - allowed.len());
        assert!(!withheld.is_empty(), "the withheld set must be non-empty");
        assert!(withheld.contains("memstead_changes_since"));
        assert!(withheld.contains("memstead_diff"));
    }

    #[test]
    fn handshake_self_describes_as_writable_session() {
        let engine = mount_session_engine(
            MountStorage::InMemory,
            default_schema(),
            CONTENT_VAULT_NAME.to_string(),
            default_schema(),
        )
        .unwrap();
        let server = SessionMcpServer::from_engine(engine, DEFAULT_SESSION_ENTITY_CAP);
        let info = server.get_info();
        assert_eq!(
            info.server_info.name, "memstead-session",
            "identity must not delegate to the basis server name"
        );
        let instr = info.instructions.expect("session handshake carries instructions");
        for available in SESSION_ENTITY_TOOLS {
            assert!(instr.contains(available), "instructions name available tool {available}");
        }
        for absent in ["memstead_vault_create", "memstead_workspace_", "memstead_reload"] {
            assert!(instr.contains(absent), "instructions name withheld tool class {absent}");
        }
    }

    /// Build a bare session server (no view binding) for the registry-cap tests.
    fn cap_test_server() -> SessionMcpServer {
        let engine = mount_session_engine(
            MountStorage::InMemory,
            default_schema(),
            CONTENT_VAULT_NAME.to_string(),
            default_schema(),
        )
        .unwrap();
        SessionMcpServer::from_engine(engine, DEFAULT_SESSION_ENTITY_CAP)
    }

    #[test]
    fn create_session_refuses_past_the_global_cap() {
        let reg = registry(Duration::from_secs(60)).with_max_sessions(2);
        assert!(reg.create_session(default_schema()).is_ok());
        assert!(reg.create_session(default_schema()).is_ok());
        assert_eq!(reg.len(), 2);
        // The third is shed with a typed capacity error — never an OOM, never a 500.
        assert_eq!(reg.create_session(default_schema()), Err(SessionError::AtCapacity));
        assert_eq!(reg.create_session(default_schema()).unwrap_err().code(), "AT_CAPACITY");
        assert_eq!(reg.len(), 2, "a refused create must not grow the live set");
    }

    #[test]
    fn eviction_frees_a_capacity_slot() {
        // ttl 0 → any existing session is already idle-expired, so a sweep
        // clears the slot and a subsequent create gets in.
        let reg = registry(Duration::from_secs(0)).with_max_sessions(1);
        assert!(reg.create_session(default_schema()).is_ok());
        assert_eq!(reg.create_session(default_schema()), Err(SessionError::AtCapacity));
        reg.sweep_expired(Instant::now());
        assert_eq!(reg.len(), 0);
        assert!(
            reg.create_session(default_schema()).is_ok(),
            "a freed slot must admit a new session"
        );
    }

    #[test]
    fn register_view_session_enforces_the_global_cap() {
        let reg = registry(Duration::from_secs(60)).with_max_sessions(1);
        assert!(reg.register_view_session("a".to_string(), cap_test_server()));
        // A second, distinct id past the cap is refused (the connection-born
        // path turns this into a typed RESOURCE_CAP_EXCEEDED on first tool call).
        assert!(!reg.register_view_session("b".to_string(), cap_test_server()));
        assert_eq!(reg.len(), 1);
        // Re-registering an id already present never grows the set, so it is
        // always allowed even at the cap (idempotent reconnect).
        assert!(reg.register_view_session("a".to_string(), cap_test_server()));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn unlimited_by_default() {
        // Embedders/tests that never set a cap are unaffected.
        let reg = registry(Duration::from_secs(60));
        for _ in 0..50 {
            assert!(reg.create_session(default_schema()).is_ok());
        }
        assert_eq!(reg.len(), 50);
    }

    // ----- AC1: addressable session, list+call the 10, create reflected --

    #[tokio::test]
    async fn session_lists_ten_tools_over_transport_and_reflects_a_create() {
        let reg = registry(Duration::from_secs(3600));
        let (svc, sid) = handshaken(&reg).await;

        // tools/list over the wire names exactly the 10 and none withheld.
        let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
        let (status, _s, body) = mcp_post(&svc, list, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK, "tools/list must return 200: {body}");
        for available in SESSION_ENTITY_TOOLS {
            assert!(body.contains(available), "tools/list names {available}: {body}");
        }
        for absent in ["memstead_vault_create", "memstead_reload", "memstead_workspace_allow_create"] {
            assert!(!body.contains(absent), "tools/list must not name {absent}: {body}");
        }

        // A create issued over the session endpoint lands.
        let create = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memstead_create","arguments":{"title":"Alpha Note","entity_type":"spec","sections":{"identity":"the identity body","purpose":"the purpose body"}}}}"#;
        let (status, _s, body) = mcp_post(&svc, create, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK, "create must return 200: {body}");
        assert!(
            body.contains("sketch--alpha-note"),
            "create response carries the new id: {body}"
        );

        // A subsequent read on the same endpoint reflects it.
        let search = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memstead_search","arguments":{}}}"#;
        let (status, _s, body) = mcp_post(&svc, search, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK, "search must return 200: {body}");
        assert!(
            body.contains("alpha-note"),
            "the just-created entity is reflected by a read on the same session: {body}"
        );
    }

    /// Refusal complement: a withheld tool invoked by id over the wire is
    /// refused with TOOL_NOT_FOUND, not dispatched.
    #[tokio::test]
    async fn withheld_tool_is_refused_over_transport() {
        let reg = registry(Duration::from_secs(3600));
        let (svc, sid) = handshaken(&reg).await;
        let call = r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"memstead_vault_create","arguments":{}}}"#;
        let (status, _s, body) = mcp_post(&svc, call, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.contains("TOOL_NOT_FOUND"),
            "withheld tool must be refused with TOOL_NOT_FOUND: {body}"
        );
    }

    /// Refusal complement: session A and session B are different vaults. A
    /// create on A is invisible to a read on B.
    #[tokio::test]
    async fn sessions_are_isolated() {
        let reg = registry(Duration::from_secs(3600));
        let (svc_a, sid_a) = handshaken(&reg).await;
        let (svc_b, sid_b) = handshaken(&reg).await;

        let create = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memstead_create","arguments":{"title":"Secret A","entity_type":"spec","sections":{"identity":"i","purpose":"p"}}}}"#;
        let (status, _s, body) = mcp_post(&svc_a, create, Some(&sid_a)).await;
        assert_eq!(status, StatusCode::OK, "create on A must succeed: {body}");

        let search = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memstead_search","arguments":{}}}"#;
        let (_st, _s, body_b) = mcp_post(&svc_b, search, Some(&sid_b)).await;
        assert!(
            !body_b.contains("secret-a"),
            "session B must not see session A's entity: {body_b}"
        );
    }

    // ----- two-tier: read curated content, write only the sketch vault -----

    /// The two-tier shape over the MCP transport: a session mounts a writable
    /// `sketch` vault and a read-only `memstead` content vault. A create with
    /// no `vault` lands in `sketch`; an explicit create against `memstead` is
    /// refused with READ_ONLY_MOUNT (the engine capability layer, beneath the
    /// tool allowlist); and a read spans both mounts — the curated content is
    /// visible alongside the agent's own sketch.
    #[tokio::test]
    async fn session_reads_curated_content_and_writes_only_sketch() {
        let content_dir = tempfile::TempDir::new().unwrap();
        seed_content_vault(content_dir.path());
        let reg = registry_with_content(Duration::from_secs(3600), content_dir.path());
        let (svc, sid) = handshaken(&reg).await;

        // A create that omits `vault` lands in the writable sketch vault.
        let create = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memstead_create","arguments":{"title":"My Idea","entity_type":"spec","sections":{"identity":"i","purpose":"p"}}}}"#;
        let (status, _s, body) = mcp_post(&svc, create, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK, "create must succeed: {body}");
        assert!(
            body.contains("sketch--my-idea"),
            "an omitted-vault create lands in the writable sketch vault: {body}"
        );

        // An explicit create against the read-only content vault is refused —
        // not silently redirected to sketch.
        let create_ro = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memstead_create","arguments":{"vault":"memstead","title":"Sneaky","entity_type":"spec","sections":{"identity":"i","purpose":"p"}}}}"#;
        let (status, _s, body) = mcp_post(&svc, create_ro, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.contains("READ_ONLY_MOUNT"),
            "a write to the read-only content vault must refuse with READ_ONLY_MOUNT: {body}"
        );

        // A read spans both mounts: the curated content AND the agent's sketch.
        let search = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"memstead_search","arguments":{}}}"#;
        let (status, _s, body) = mcp_post(&svc, search, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK, "search must succeed: {body}");
        assert!(
            body.contains("what-is-memstead"),
            "reads surface the curated content vault: {body}"
        );
        assert!(
            body.contains("my-idea"),
            "reads surface the agent's own sketch: {body}"
        );
    }

    /// The live view scopes to the agent's own sketch graph — the read-only
    /// content vault's entities never appear in the projection (the viewer
    /// renders the visitor's work, not the curated reference graph).
    #[tokio::test]
    async fn live_projection_scopes_to_sketch_excluding_content() {
        let content_dir = tempfile::TempDir::new().unwrap();
        seed_content_vault(content_dir.path());
        let reg = registry_with_content(Duration::from_secs(3600), content_dir.path());
        let (id, svc, sid) = fresh_session(&reg).await;
        mcp_post(&svc, ALPHA_CREATE, Some(&sid)).await;

        let snap = reg.snapshot(&id).expect("snapshot for a live session");
        assert!(
            snap.nodes.iter().any(|n| n.id == "sketch--alpha-note"),
            "the agent's sketch entity is in the projection: {:?}",
            snap.nodes
        );
        assert!(
            !snap.nodes.iter().any(|n| n.id.starts_with("memstead--")),
            "the read-only content vault is excluded from the live view: {:?}",
            snap.nodes
        );
    }

    // ----- the curated "what is Memstead" content vault is real & conformant -

    /// The committed curated content vault mounts cleanly (schema-conformant),
    /// carries all its concept entities, has no dangling references, and is
    /// queryable — the public "what is Memstead" content the agent reads to
    /// orient itself. This validates the hand-authored `.md` asset against the
    /// live engine: a non-conformant entity fails the mount, and an unresolved
    /// `[[wiki-link]]` shows up as a stub. (Typed REFERENCES edges between the
    /// concepts require engine-authoring through the create path — see the plan
    /// session log; the body wiki-links are visible to a reader regardless.)
    #[test]
    fn curated_content_vault_loads_conformant_and_queryable() {
        let content_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("content/memstead");
        assert!(content_dir.is_dir(), "curated content dir exists: {content_dir:?}");

        // Mount it read-only alongside a sketch vault, exactly as a session does.
        let engine = mount_session_engine(
            MountStorage::Folder { path: content_dir },
            default_schema(),
            CONTENT_VAULT_NAME.to_string(),
            default_schema(),
        )
        .expect("the curated content vault mounts cleanly (schema-conformant)");

        // Every authored concept is present under the content vault.
        let slugs = [
            "memstead", "vault", "schema", "entity", "wikilink", "workspace", "mount",
            "storage-backend", "modal-flavour", "graph", "mcp-layer",
        ];
        for slug in slugs {
            let id = memstead_base::EntityId::new(CONTENT_VAULT_NAME, slug);
            assert!(
                engine.get_entity(&id).is_some(),
                "content entity {slug} loaded into the {CONTENT_VAULT_NAME} vault"
            );
        }

        // No stub (unresolved) entities: every body wiki-link points at a
        // concept that is actually authored, so the agent reads a coherent
        // vault, not one littered with dangling references.
        let stubs = engine.store().all_entities().filter(|e| e.stub).count();
        assert_eq!(stubs, 0, "no dangling wiki-links / stub entities in the curated vault");

        // The content is queryable: a search over the two-mount engine surfaces
        // the curated concepts (this is the "reads return the public content"
        // half of the two-tier experience).
        let scope = memstead_base::ops::SearchScope {
            query: Some(memstead_base::ops::Query {
                any: vec!["schema".to_string(), "vault".to_string()],
                not: Vec::new(),
                phrase: None,
                field: None,
            }),
            vault: Some(CONTENT_VAULT_NAME.to_string()),
            entity_type: None,
            limit: None,
            offset: None,
            filters: std::collections::HashMap::new(),
            range_filters: std::collections::HashMap::new(),
            edge_type: None,
            related_to: None,
            depth: None,
            expand_via: None,
            expand_depth: None,
            stub: None,
            token_budget: None,
        };
        let hits = engine.search(&scope).expect("search the content vault");
        assert!(
            hits.hits.len() >= 2,
            "search returns the curated content: {} hits",
            hits.hits.len()
        );
    }

    // ----- connection-born flow: stable /mcp + view URL via handshake -----

    /// The single connection-born MCP service over `reg` (empty view base →
    /// relative `/v/<id>` URLs).
    fn sketch_service(reg: &SessionRegistry) -> SessionService {
        build_sketch_mcp_service(reg.clone(), default_schema(), String::new())
    }

    /// POST a JSON-RPC body to an explicit path through the full app, threading
    /// the rmcp session id. Carries a `ConnectInfo` peer address so the
    /// rate-limiter layer can extract its key (the production app is served
    /// `into_make_service_with_connect_info`).
    async fn app_post_path(
        app: &Router,
        path: &str,
        body: &str,
        session: Option<&str>,
    ) -> (StatusCode, Option<String>, String) {
        use axum::extract::ConnectInfo;
        use std::net::SocketAddr;

        let mut req = Request::builder()
            .method("POST")
            .uri(path)
            .header("host", "session.example")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(s) = session {
            req = req.header("mcp-session-id", s);
        }
        let mut req = req.body(Body::from(body.to_string())).unwrap();
        let addr: SocketAddr = "203.0.113.7:4444".parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let sid = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, sid, String::from_utf8_lossy(&bytes).to_string())
    }

    /// The connection-born flow: one stable `/mcp` service mints a session per
    /// `initialize`. The handshake names the live-view channel but bakes no URL
    /// — the link arrives with the first `memstead_overview`, which also binds
    /// the (app-generated, not rmcp) view id; a create then lands in that
    /// connection's own sketch vault and the view reflects it.
    #[tokio::test]
    async fn connection_born_mcp_binds_view_to_the_working_session() {
        let reg = registry(Duration::from_secs(3600));
        let svc = sketch_service(&reg);

        // initialize: the handshake points the agent at overview, bakes no URL.
        let (status, rmcp_sid, body) = mcp_post(&svc, INIT, None).await;
        assert_eq!(status, StatusCode::OK, "initialize: {body}");
        assert!(
            body.contains("memstead_overview"),
            "handshake names overview as the live-link channel: {body}"
        );
        assert!(
            !body.contains("/v/"),
            "handshake bakes no view URL (it could belong to a different session): {body}"
        );
        let rmcp_sid = rmcp_sid.expect("initialize issues an rmcp session id");
        mcp_post(&svc, INITIALIZED, Some(&rmcp_sid)).await;

        // Lazy binding: a session that has only handshaked is NOT yet
        // resolvable — no "live but empty" vault exists until it does work.
        assert_eq!(
            reg.snapshot("sess-0").err().expect("unbound before first tool call").code(),
            "UNKNOWN_SESSION",
        );

        // The first overview binds the view AND carries the shareable link.
        let overview = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memstead_overview","arguments":{}}}"#;
        let (status, _s, obody) = mcp_post(&svc, overview, Some(&rmcp_sid)).await;
        assert_eq!(status, StatusCode::OK, "overview: {obody}");
        assert!(
            obody.contains("/v/sess-0"),
            "overview surfaces the working session's live-view link: {obody}"
        );
        assert!(
            reg.snapshot("sess-0").expect("view resolves after first tool call").nodes.is_empty(),
            "the freshly-bound session's sketch is empty"
        );

        // A create over the connection lands in this connection's sketch vault;
        // the view snapshot reflects it.
        let (cs, _s, cbody) = mcp_post(&svc, ALPHA_CREATE, Some(&rmcp_sid)).await;
        assert_eq!(cs, StatusCode::OK, "create: {cbody}");
        assert!(cbody.contains("sketch--alpha-note"), "create lands in sketch: {cbody}");
        let snap = reg.snapshot("sess-0").unwrap();
        assert!(
            snap.nodes.iter().any(|n| n.id == "sketch--alpha-note"),
            "the view reflects the agent's mutation: {:?}",
            snap.nodes
        );
    }

    /// Regression for the "live · vault empty" incident: the link the agent
    /// surfaces must address the session that carries its writes, never a
    /// sibling vault born from a different `initialize`. A bare "probe"
    /// handshake binds nothing (so its `/v/{id}` refuses instead of rendering a
    /// misleading live-but-empty graph), while the working session's overview
    /// carries the correct, resolvable link.
    #[tokio::test]
    async fn surfaced_link_tracks_the_working_session_not_a_probe_handshake() {
        let reg = registry(Duration::from_secs(3600));
        let svc = sketch_service(&reg);

        // initialize #1 (sess-0): a probe — it handshakes and is never used.
        let (_s, probe_sid, pbody) = mcp_post(&svc, INIT, None).await;
        let probe_sid = probe_sid.expect("probe rmcp session id");
        mcp_post(&svc, INITIALIZED, Some(&probe_sid)).await;
        assert!(!pbody.contains("/v/"), "the probe handshake surfaces no link: {pbody}");

        // initialize #2 (sess-1): the working session the client actually drives.
        let (_s, work_sid, _wbody) = mcp_post(&svc, INIT, None).await;
        let work_sid = work_sid.expect("working rmcp session id");
        mcp_post(&svc, INITIALIZED, Some(&work_sid)).await;

        // The agent works on #2: overview (binds + surfaces the link), then create.
        let overview = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memstead_overview","arguments":{}}}"#;
        let (_s, _x, obody) = mcp_post(&svc, overview, Some(&work_sid)).await;
        assert!(
            obody.contains("/v/sess-1") && !obody.contains("/v/sess-0"),
            "overview surfaces the WORKING session's link (sess-1), not the probe's: {obody}"
        );
        mcp_post(&svc, ALPHA_CREATE, Some(&work_sid)).await;

        // The probe's id never bound → it refuses, rather than masquerading as a
        // live-but-empty graph (the original bug).
        assert_eq!(
            reg.snapshot("sess-0").err().expect("probe id must refuse").code(),
            "UNKNOWN_SESSION",
            "a probe handshake leaves no resolvable empty vault behind",
        );
        // The surfaced link resolves and reflects the agent's writes.
        let snap = reg.snapshot("sess-1").expect("working view resolves");
        assert!(
            snap.nodes.iter().any(|n| n.id == "sketch--alpha-note"),
            "the surfaced link addresses the vault the agent actually wrote: {:?}",
            snap.nodes
        );
    }

    /// Two MCP connections to the one stable `/mcp` service get distinct view
    /// ids and isolated vaults — a create on one is invisible to the other.
    /// Each binds its view on its own first tool call (A creates, B reads).
    #[tokio::test]
    async fn connection_born_sessions_are_isolated() {
        let reg = registry(Duration::from_secs(3600));
        let svc = sketch_service(&reg);

        // First connection → first factory call → view id sess-0.
        let (_s, sid_a, _body_a) = mcp_post(&svc, INIT, None).await;
        let sid_a = sid_a.unwrap();
        mcp_post(&svc, INITIALIZED, Some(&sid_a)).await;

        // Second connection → second factory call → view id sess-1.
        let (_s, sid_b, _body_b) = mcp_post(&svc, INIT, None).await;
        let sid_b = sid_b.unwrap();
        mcp_post(&svc, INITIALIZED, Some(&sid_b)).await;

        // A writes (binds sess-0); B only reads (binds sess-1, empty).
        mcp_post(&svc, ALPHA_CREATE, Some(&sid_a)).await;
        let search = r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"memstead_search","arguments":{}}}"#;
        mcp_post(&svc, search, Some(&sid_b)).await;

        assert!(
            reg.snapshot("sess-0")
                .unwrap()
                .nodes
                .iter()
                .any(|n| n.id == "sketch--alpha-note"),
            "A's view has the entity"
        );
        assert!(
            reg.snapshot("sess-1").unwrap().nodes.is_empty(),
            "B's view is unaffected by A's mutation"
        );
    }

    /// Over the full unified app: a connection-born session minted by
    /// `initialize` over `/mcp` is reachable at `/v/{id}/graph`; the same id's
    /// `/s/{id}/mcp` refuses (its MCP surface is the shared `/mcp`, not a
    /// per-session route).
    #[tokio::test]
    async fn view_endpoint_resolves_connection_born_session_over_app() {
        let reg = registry(Duration::from_secs(3600));
        // soft_launch=false: this asserts the public-surface `/mcp` mount.
        let app = build_sketch_app(reg.clone(), default_schema(), String::new(), 1000, 1000, false);

        let (status, sid, body) = app_post_path(&app, "/mcp", INIT, None).await;
        assert_eq!(status, StatusCode::OK, "initialize over /mcp: {body}");
        let sid = sid.expect("rmcp session id");
        app_post_path(&app, "/mcp", INITIALIZED, Some(&sid)).await;
        // The create is this session's first tool call — it binds the view id.
        app_post_path(&app, "/mcp", ALPHA_CREATE, Some(&sid)).await;

        // /v/{id}/graph returns the projection.
        let mut req = Request::builder()
            .method("GET")
            .uri("/v/sess-0/graph")
            .body(Body::empty())
            .unwrap();
        {
            use axum::extract::ConnectInfo;
            use std::net::SocketAddr;
            let addr: SocketAddr = "203.0.113.7:4444".parse().unwrap();
            req.extensions_mut().insert(ConnectInfo(addr));
        }
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let bd = String::from_utf8_lossy(&bytes);
        assert!(bd.contains("sketch--alpha-note"), "view graph reflects the entity: {bd}");

        // The connection-born session has no per-session MCP route.
        let (st, _s, _b) = app_post_path(&app, "/s/sess-0/mcp", INIT, Some(&sid)).await;
        assert_eq!(
            st,
            StatusCode::NOT_FOUND,
            "connection-born session has no /s/{{id}}/mcp route"
        );
    }

    /// Soft-launch gate: with the gate ON the writable MCP endpoint answers
    /// only at `/try/mcp`; the public `/mcp` is gone.
    #[tokio::test]
    async fn gate_on_mcp_only_under_try() {
        let reg = registry(Duration::from_secs(3600));
        let app = build_sketch_app(reg, default_schema(), String::new(), 1000, 1000, true);

        // Public path is unmounted while gated.
        let (top, _s, _b) = app_post_path(&app, "/mcp", INIT, None).await;
        assert_eq!(top, StatusCode::NOT_FOUND, "/mcp must 404 while gated");

        // The handshake succeeds at the gated path.
        let (gated, sid, body) = app_post_path(&app, "/try/mcp", INIT, None).await;
        assert_eq!(gated, StatusCode::OK, "/try/mcp must answer the handshake: {body}");
        assert!(sid.is_some(), "gated handshake returns an mcp-session-id");
    }

    /// OFF keeps the writable MCP endpoint at the public `/mcp`.
    #[tokio::test]
    async fn gate_off_mcp_stays_public() {
        let reg = registry(Duration::from_secs(3600));
        let app = build_sketch_app(reg, default_schema(), String::new(), 1000, 1000, false);
        let (st, sid, body) = app_post_path(&app, "/mcp", INIT, None).await;
        assert_eq!(st, StatusCode::OK, "public /mcp answers: {body}");
        assert!(sid.is_some());
        let (gated, _s, _b) = app_post_path(&app, "/try/mcp", INIT, None).await;
        assert_eq!(gated, StatusCode::NOT_FOUND, "no /try/mcp when public");
    }

    // ----- AC3: eviction is observable, unknown/evicted is a typed refusal -

    #[test]
    fn unknown_session_is_a_typed_refusal() {
        let reg = registry(Duration::from_secs(3600));
        // `.err()` (not `unwrap_err`) — the Ok type is the rmcp service,
        // which isn't `Debug`.
        let err = reg.service_for("never-created").err().expect("unknown id must refuse");
        assert_eq!(err.code(), "UNKNOWN_SESSION");
    }

    #[test]
    fn idle_session_evicts_and_then_refuses_not_serves_fresh() {
        let reg = registry(Duration::from_secs(60));
        let id = reg.create_session(default_schema()).unwrap();
        assert_eq!(reg.len(), 1);

        // A sweep before the TTL keeps it.
        assert_eq!(reg.sweep_expired(Instant::now()), 0);
        assert!(reg.service_for(&id).is_ok());

        // A sweep well past the TTL evicts it.
        let evicted = reg.sweep_expired(Instant::now() + Duration::from_secs(3600));
        assert_eq!(evicted, 1);
        assert!(reg.is_empty());

        // The evicted id now refuses — it is NOT silently re-served a fresh
        // empty vault under the same id.
        let err = reg.service_for(&id).err().expect("evicted id must refuse");
        assert_eq!(err.code(), "UNKNOWN_SESSION");
    }

    // ----- AC5: the session vault is exportable as a standalone .mem -----

    /// A graph the agent builds over the session endpoint exports to a
    /// `.mem` archive that mounts standalone in the real engine — no
    /// network, no other inputs — to the same entities.
    #[tokio::test]
    async fn session_export_mounts_standalone_to_the_same_graph() {
        let reg = registry(Duration::from_secs(3600));
        let id = reg.create_session(default_schema()).unwrap();
        let svc = reg.service_for(&id).unwrap();
        let (_st, sid, _) = mcp_post(&svc, INIT, None).await;
        let sid = sid.expect("initialize issues a session id");
        mcp_post(&svc, INITIALIZED, Some(&sid)).await;
        let create = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memstead_create","arguments":{"title":"Alpha Note","entity_type":"spec","sections":{"identity":"i","purpose":"p"}}}}"#;
        let (status, _s, body) = mcp_post(&svc, create, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK, "create must succeed: {body}");

        let bytes = reg.export_session(&id).expect("export succeeds");
        let standalone =
            memstead_base::Engine::from_archive_bytes(bytes).expect("exported .mem mounts standalone");
        assert!(
            standalone
                .get_entity(&memstead_base::EntityId::new("sketch", "alpha-note"))
                .is_some(),
            "the exported .mem mounts standalone to the same graph the agent built"
        );
    }

    /// Durability honesty (Plan 02, Part A): the ephemeral in-memory
    /// sketch must mark itself non-durable on both the cold-start overview
    /// (pre-write visibility) and every mutation response (per-write echo),
    /// so an agent never reads the synthetic `commit_sha` as a durable ref.
    #[tokio::test]
    async fn sketch_marks_overview_and_writes_non_durable() {
        let reg = registry(Duration::from_secs(3600));
        let id = reg.create_session(default_schema()).unwrap();
        let svc = reg.service_for(&id).unwrap();
        let (_st, sid, _) = mcp_post(&svc, INIT, None).await;
        let sid = sid.expect("initialize issues a session id");
        mcp_post(&svc, INITIALIZED, Some(&sid)).await;

        // Pre-write: overview flags the vault ephemeral with no write yet.
        let overview = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memstead_overview","arguments":{}}}"#;
        let (status, _s, body) = mcp_post(&svc, overview, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK, "overview must succeed: {body}");
        assert!(
            body.contains("ephemeral") && body.contains("in-memory"),
            "sketch overview must flag the vault ephemeral before any write; got: {body}"
        );

        // Per-write: the create response echoes durable=false.
        let create = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memstead_create","arguments":{"title":"Alpha Note","entity_type":"spec","sections":{"identity":"i","purpose":"p"}}}}"#;
        let (status, _s, body) = mcp_post(&svc, create, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK, "create must succeed: {body}");
        assert!(
            body.contains("\"durable\":false"),
            "in-memory sketch write must echo durable=false; got: {body}"
        );
    }

    /// Refusal complement: an unknown or evicted session id refuses export
    /// with a typed code — not an empty or stale archive.
    #[test]
    fn export_unknown_session_is_a_typed_refusal() {
        let reg = registry(Duration::from_secs(3600));
        let err = reg.export_session("nope").err().expect("unknown id must refuse export");
        assert_eq!(err.code(), "UNKNOWN_SESSION");
    }

    /// Refusal complement: an export is scoped to the requesting session —
    /// session B's archive never bundles session A's vault.
    #[tokio::test]
    async fn export_is_scoped_to_the_requesting_session() {
        let reg = registry(Duration::from_secs(3600));
        let id_a = reg.create_session(default_schema()).unwrap();
        let id_b = reg.create_session(default_schema()).unwrap();
        let svc_a = reg.service_for(&id_a).unwrap();
        let (_st, sid, _) = mcp_post(&svc_a, INIT, None).await;
        let sid = sid.expect("initialize issues a session id");
        mcp_post(&svc_a, INITIALIZED, Some(&sid)).await;
        let create = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memstead_create","arguments":{"title":"Secret A","entity_type":"spec","sections":{"identity":"i","purpose":"p"}}}}"#;
        mcp_post(&svc_a, create, Some(&sid)).await;

        // B's export does not contain A's entity.
        let bytes_b = reg.export_session(&id_b).expect("export B succeeds");
        let standalone_b =
            memstead_base::Engine::from_archive_bytes(bytes_b).expect("B mounts standalone");
        assert!(
            standalone_b
                .get_entity(&memstead_base::EntityId::new("sketch", "secret-a"))
                .is_none(),
            "session B's export must not contain session A's entity"
        );
        // A's export does.
        let bytes_a = reg.export_session(&id_a).expect("export A succeeds");
        let standalone_a =
            memstead_base::Engine::from_archive_bytes(bytes_a).expect("A mounts standalone");
        assert!(
            standalone_a
                .get_entity(&memstead_base::EntityId::new("sketch", "secret-a"))
                .is_some(),
            "session A's export contains its own entity"
        );
    }

    // ----- AC4: per-session resource cap ---------------------------------

    async fn create_over_mcp(
        svc: &SessionService,
        sid: &str,
        title: &str,
        rpc_id: u32,
    ) -> (StatusCode, String) {
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":{rpc_id},"method":"tools/call","params":{{"name":"memstead_create","arguments":{{"title":"{title}","entity_type":"spec","sections":{{"identity":"i","purpose":"p"}}}}}}}}"#
        );
        let (status, _s, body) = mcp_post(svc, &body, Some(sid)).await;
        (status, body)
    }

    /// A `create` beyond the per-session entity cap is refused with a
    /// typed code and never lands; reads still work at the cap.
    #[tokio::test]
    async fn create_beyond_entity_cap_is_refused_with_typed_code() {
        let reg = registry_with_cap(Duration::from_secs(3600), 2);
        let (svc, sid) = handshaken(&reg).await;

        let (s1, b1) = create_over_mcp(&svc, &sid, "One", 10).await;
        assert_eq!(s1, StatusCode::OK);
        assert!(b1.contains("sketch--one"), "first create lands: {b1}");
        let (_s2, b2) = create_over_mcp(&svc, &sid, "Two", 11).await;
        assert!(b2.contains("sketch--two"), "second create lands: {b2}");

        // Third create is over the cap of 2 → typed refusal, not dispatched.
        let (s3, b3) = create_over_mcp(&svc, &sid, "Three", 12).await;
        assert_eq!(s3, StatusCode::OK);
        assert!(b3.contains("RESOURCE_CAP_EXCEEDED"), "over-cap create refused: {b3}");

        // Reads still work at the cap, and the refused entity never landed.
        let search = r#"{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"name":"memstead_search","arguments":{}}}"#;
        let (s4, _s, b4) = mcp_post(&svc, search, Some(&sid)).await;
        assert_eq!(s4, StatusCode::OK, "reads work at the cap: {b4}");
        assert!(!b4.contains("sketch--three"), "refused entity never landed: {b4}");
    }

    // ----- HTTP surface: session creation + export ------------------------

    /// `POST /sessions` mints a session and returns its id plus the MCP and
    /// export URLs.
    #[tokio::test]
    async fn post_sessions_returns_id_and_urls() {
        let reg = registry(Duration::from_secs(3600));
        let app = session_router(reg);
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = v["session_id"].as_str().expect("response carries a session id");
        assert_eq!(v["mcp_url"], format!("/s/{id}/mcp"));
        assert_eq!(v["export_url"], format!("/s/{id}/export"));
    }

    /// `GET /s/{id}/export` streams a `.mem` that mounts standalone to the
    /// graph the agent built over the session's MCP endpoint.
    #[tokio::test]
    async fn export_over_http_returns_mountable_mem() {
        let reg = registry(Duration::from_secs(3600));
        let id = reg.create_session(default_schema()).unwrap();
        let svc = reg.service_for(&id).unwrap();
        let (_st, sid, _) = mcp_post(&svc, INIT, None).await;
        let sid = sid.expect("initialize issues a session id");
        mcp_post(&svc, INITIALIZED, Some(&sid)).await;
        let create = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memstead_create","arguments":{"title":"Alpha Note","entity_type":"spec","sections":{"identity":"i","purpose":"p"}}}}"#;
        mcp_post(&svc, create, Some(&sid)).await;

        let app = session_router(reg.clone());
        let req = Request::builder()
            .method("GET")
            .uri(format!("/s/{id}/export"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
        let standalone = memstead_base::Engine::from_archive_bytes(bytes)
            .expect("downloaded .mem mounts standalone");
        assert!(
            standalone
                .get_entity(&memstead_base::EntityId::new("sketch", "alpha-note"))
                .is_some(),
            "the HTTP-downloaded archive holds the agent's entity"
        );
    }

    /// `GET /s/{id}/export` for an unknown id is a typed 404 — not an
    /// empty or stale archive.
    #[tokio::test]
    async fn export_unknown_session_over_http_is_404() {
        let reg = registry(Duration::from_secs(3600));
        let app = session_router(reg);
        let req = Request::builder()
            .method("GET")
            .uri("/s/does-not-exist/export")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("UNKNOWN_SESSION"), "typed code in body: {body}");
    }

    /// The public session surface is rate-limited: over-budget requests
    /// are refused with a typed 429, never a 500.
    #[tokio::test]
    async fn session_surface_is_rate_limited() {
        use axum::extract::ConnectInfo;
        use std::net::SocketAddr;

        let reg = registry(Duration::from_secs(3600));
        // burst of 1 → the second request in the window is over budget.
        let app = build_session_app(reg, 1, 1);
        let addr: SocketAddr = "203.0.113.9:5555".parse().unwrap();

        let mut got_429 = false;
        for _ in 0..5 {
            let mut req = Request::builder()
                .method("POST")
                .uri("/sessions")
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

    /// POST a JSON-RPC body to a session's `/s/{id}/mcp` route through the
    /// full axum app, threading the MCP session id.
    async fn app_mcp_post(
        app: &Router,
        id: &str,
        body: &str,
        session: Option<&str>,
    ) -> (StatusCode, Option<String>, String) {
        let mut req = Request::builder()
            .method("POST")
            .uri(format!("/s/{id}/mcp"))
            .header("host", "session.example")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(s) = session {
            req = req.header("mcp-session-id", s);
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let sid = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, sid, String::from_utf8_lossy(&bytes).to_string())
    }

    /// AC1 + AC6 end-to-end over the full HTTP app: `POST /sessions` yields
    /// an addressable URL; an MCP client driving `/s/{id}/mcp` initializes,
    /// lists the 10 tools, issues a create, and reads it back; after
    /// eviction the same URL refuses.
    #[tokio::test]
    async fn full_http_session_lifecycle() {
        let reg = registry(Duration::from_secs(3600));
        let app = session_router(reg.clone());

        // Create a session; learn its id from POST /sessions.
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = v["session_id"].as_str().unwrap().to_string();

        // initialize → tools/list over the addressable /s/{id}/mcp URL.
        let (status, sid, ibody) = app_mcp_post(&app, &id, INIT, None).await;
        assert_eq!(status, StatusCode::OK, "initialize over HTTP: {ibody}");
        let sid = sid.expect("HTTP MCP initialize issues a session id");
        app_mcp_post(&app, &id, INITIALIZED, Some(&sid)).await;

        let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
        let (_s, _x, lbody) = app_mcp_post(&app, &id, list, Some(&sid)).await;
        for t in SESSION_ENTITY_TOOLS {
            assert!(lbody.contains(t), "tools/list over HTTP names {t}: {lbody}");
        }

        // create + read-back over the URL.
        let create = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memstead_create","arguments":{"title":"Alpha Note","entity_type":"spec","sections":{"identity":"i","purpose":"p"}}}}"#;
        let (_s, _x, cbody) = app_mcp_post(&app, &id, create, Some(&sid)).await;
        assert!(cbody.contains("sketch--alpha-note"), "create over HTTP lands: {cbody}");
        let search = r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memstead_search","arguments":{}}}"#;
        let (_s, _x, sbody) = app_mcp_post(&app, &id, search, Some(&sid)).await;
        assert!(sbody.contains("alpha-note"), "create reflected by a read over HTTP: {sbody}");

        // Evict the session; the same URL now refuses with a typed 404.
        let evicted = reg.sweep_expired(std::time::Instant::now() + Duration::from_secs(7200));
        assert_eq!(evicted, 1, "the session evicts");
        let (status, _x, _b) = app_mcp_post(&app, &id, list, Some(&sid)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "an evicted session's URL refuses");
    }

    // ----- live graph stream + snapshot ----------------------------------

    const ALPHA_CREATE: &str = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memstead_create","arguments":{"title":"Alpha Note","entity_type":"spec","sections":{"identity":"i","purpose":"p"}}}}"#;

    /// Create a session, complete the MCP handshake, and return its registry
    /// id, service, and MCP session id.
    async fn fresh_session(reg: &SessionRegistry) -> (String, SessionService, String) {
        let id = reg.create_session(default_schema()).unwrap();
        let svc = reg.service_for(&id).unwrap();
        let (_st, sid, _) = mcp_post(&svc, INIT, None).await;
        let sid = sid.expect("initialize issues a session id");
        mcp_post(&svc, INITIALIZED, Some(&sid)).await;
        (id, svc, sid)
    }

    /// AC1: subscribing yields a current snapshot, then a fresh snapshot
    /// after a mutation reflecting it. (Drives the stream's content via the
    /// raw subscription so the assertions don't fight the open-ended SSE
    /// body.)
    #[tokio::test]
    async fn stream_emits_snapshot_then_update_per_mutation() {
        let reg = registry(Duration::from_secs(3600));
        let (id, svc, sid) = fresh_session(&reg).await;

        let (server, _handle, mut rx) = reg.subscribe_session(&id).unwrap();
        assert!(server.graph_snapshot().nodes.is_empty(), "snapshot-on-subscribe is the empty vault");

        let (status, _s, _b) = mcp_post(&svc, ALPHA_CREATE, Some(&sid)).await;
        assert_eq!(status, StatusCode::OK);

        // The mutation pushes a change event; the recomputed snapshot reflects it.
        let evt = rx.recv().await.expect("a change event follows the create");
        assert_eq!(evt.vault, "sketch");
        let after = server.graph_snapshot();
        assert!(
            after.nodes.iter().any(|n| n.id == "sketch--alpha-note"),
            "the snapshot updates after the agent's mutation: {:?}",
            after.nodes
        );
    }

    /// AC2: a subscriber joining AFTER a mutation gets a current snapshot
    /// reflecting it — not only future updates.
    #[tokio::test]
    async fn late_subscriber_gets_current_snapshot() {
        let reg = registry(Duration::from_secs(3600));
        let (id, svc, sid) = fresh_session(&reg).await;
        mcp_post(&svc, ALPHA_CREATE, Some(&sid)).await;

        // Subscribe only now — the initial snapshot already carries the entity.
        let (server, _handle, _rx) = reg.subscribe_session(&id).unwrap();
        assert!(
            server
                .graph_snapshot()
                .nodes
                .iter()
                .any(|n| n.id == "sketch--alpha-note"),
            "a late subscriber's first snapshot reflects prior mutations"
        );
    }

    /// AC3: the stream is session-scoped — a mutation on B reaches neither
    /// A's event stream nor A's snapshot. (The engines, hence the broadcast
    /// channels, are per-session.)
    #[tokio::test]
    async fn stream_is_session_scoped() {
        let reg = registry(Duration::from_secs(3600));
        let (id_a, _svc_a, _sid_a) = fresh_session(&reg).await;
        let (_id_b, svc_b, sid_b) = fresh_session(&reg).await;

        let (server_a, _handle, mut rx_a) = reg.subscribe_session(&id_a).unwrap();
        mcp_post(&svc_b, ALPHA_CREATE, Some(&sid_b)).await;

        assert!(rx_a.try_recv().is_err(), "session A's stream receives none of B's events");
        assert!(
            server_a.graph_snapshot().nodes.is_empty(),
            "session A's graph is unaffected by B's mutation"
        );
    }

    /// AC3 refusal complement: subscribing to an unknown/evicted session is
    /// a typed refusal, not a live-looking-but-dead stream.
    #[tokio::test]
    async fn stream_unknown_session_is_404() {
        let reg = registry(Duration::from_secs(3600));
        let app = session_router(reg);
        let req = Request::builder()
            .method("GET")
            .uri("/s/does-not-exist/stream")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&bytes).contains("UNKNOWN_SESSION"));
    }

    /// `GET /s/{id}/graph` returns the current projection as JSON, with no
    /// layout coordinates.
    #[tokio::test]
    async fn graph_endpoint_returns_current_projection() {
        let reg = registry(Duration::from_secs(3600));
        let (id, svc, sid) = fresh_session(&reg).await;
        mcp_post(&svc, ALPHA_CREATE, Some(&sid)).await;

        let app = session_router(reg.clone());
        let req = Request::builder()
            .method("GET")
            .uri(format!("/s/{id}/graph"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("sketch--alpha-note"), "graph reflects the entity: {body}");
        assert!(!body.contains("\"x\"") && !body.contains("\"y\""), "no layout coordinates: {body}");
    }

    /// `GET /s/{id}/stream` returns a Server-Sent Events response. (The body
    /// is open-ended; the stream's content is covered by the raw-subscription
    /// tests above — here we confirm the route serves an event-stream.)
    #[tokio::test]
    async fn stream_endpoint_serves_event_stream() {
        let reg = registry(Duration::from_secs(3600));
        let id = reg.create_session(default_schema()).unwrap();
        let app = session_router(reg);
        let req = Request::builder()
            .method("GET")
            .uri(format!("/s/{id}/stream"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(ct.starts_with("text/event-stream"), "content-type is SSE: {ct}");
    }
}
