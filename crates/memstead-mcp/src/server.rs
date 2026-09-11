//! MCP server — McpServer struct with Engine, tool router, and ServerHandler.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, InitializeRequestParams, InitializeResult,
    ListToolsResult, PaginatedRequestParams, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, tool, tool_handler, tool_router};

// `envelope` is the typed-error helper used across the file.
use memstead_git_branch::ops::envelope;

// Backend-neutral types live in memstead-base.
use memstead_base::chunking::{apply_chunking, estimate_tokens};
use memstead_base::render;
use memstead_base::vcs::{Actor, ClientId};
use memstead_base::{EntityId, SearchScope, ops::MemChangedNotice, ops::WarningHint};

use crate::error_envelope::{tool_error, tool_error_with_payload};
use crate::tools::admin::{ChangesSinceParams, DiffParams, HealthParams, ReloadParams};
use crate::tools::graph::{EntityParams, OverviewParams, SchemaParams, SearchParams};
use crate::tools::mutation::{
    CheckParams, CreateParams, DeleteParams, RelateParams, RenameParams, RetypeParams, UpdateParams,
};

/// The MCP server wrapping the Engine.
#[derive(Clone)]
pub struct McpServer {
    /// Unified engine handle. Held under `Arc<Mutex<>>` because the
    /// single-client stdio transport still produces sequential tool
    /// calls per server clone, but the rmcp router clones the server
    /// per request.
    unified_engine: Arc<Mutex<memstead_base::Engine>>,
    /// Per-response chunking budget in tokens. Plumbed from
    /// `EffectiveSettings::token_budget` (config file `[mcp] token_budget`,
    /// or `DEFAULT_TOKEN_BUDGET` when absent).
    token_budget: usize,
    /// Session-level default role, from the
    /// binary's `--role` flag. Per-call `role` params win; when both
    /// are absent, mutations record unspecified.
    default_role: memstead_base::vcs::Role,
    /// Session-level default identity, from the
    /// binary's `--identity` flag or the `MEMSTEAD_IDENTITY`
    /// environment variable (flag wins). Per-call `identity` params
    /// win over this default; when both are absent, operations record
    /// no identity.
    default_identity: Option<String>,
    /// Effective set of tool names hidden from this server. Resolved once
    /// from `[mcp].disabled_tools` at construction. Unknown names from
    /// the raw config are filtered out by `validate_disabled_tools` in
    /// `main.rs` before reaching this field — every entry here matches a
    /// compiled-in tool.
    ///
    /// Empty set is the "no filter" case and `list_tools` /
    /// `call_tool` / `get_tool` behave byte-identically to the macro's
    /// default implementation.
    disabled_tools: Arc<HashSet<String>>,
    /// Canonical path of the `.memstead/workspace.toml` that sourced the
    /// filter, for attribution in the `TOOL_DISABLED` error envelope.
    /// `None` when the server was constructed in a test or when no
    /// file was loaded — the envelope omits `details.config_source`
    /// in that case.
    config_source: Option<Arc<PathBuf>>,
    /// Resolved `[mutations]` section from the workspace's
    /// `.memstead/workspace.toml`. Surfaced under `memstead_health {
    /// include_config: true }` so plugins can read the configured
    /// posture without a trial-write round-trip. Default (section
    /// absent) is `MutationsSection { require_notes: None }`, which
    /// the mutation pipeline treats as `false`.
    mutations: Arc<crate::config::MutationsSection>,
    /// Resolved `[plugin.*]` namespace from the workspace's
    /// `.memstead/workspace.toml`. Surfaced verbatim under `memstead_health
    /// { include_config: true }`. Each value is an opaque
    /// `toml::Table`; the engine never inspects the contents — named
    /// plugins read their own sub-table (e.g. `[plugin.claude_code]`).
    plugin: Arc<HashMap<String, toml::Table>>,
    /// Process-scoped operator-mode posture. When `true`, the
    /// `memstead_mem_create` / `memstead_mem_delete` orchestrators bypass
    /// the workspace `[[mem_management.create]]` /
    /// `[[mem_management.delete]]` allowlists and the
    /// `MEM_REFERENCED_BY_POLICY` safeguard. Set only by the
    /// `memstead-mcp --operator-mode` boot path; agent-spawned servers
    /// (e.g. the Claude Code plugin) always boot with
    /// this `false` and have no in-band channel to flip it. Surfaced
    /// in `memstead_overview`'s `## Lifecycle Namespaces` section so the
    /// posture is observable to anyone reading the engine's outputs.
    operator_mode: bool,
    // SAFETY: single-client assumption — valid under stdio (one `memstead-mcp`
    // process per client). A future HTTP transport with concurrent clients
    // would require threading `RequestContext` through every handler
    // instead. `OnceLock` encodes write-once + lock-free reads; a second
    // `initialize` hits `set` → `Err` and the override logs the breach.
    client: Arc<OnceLock<ClientId>>,
}

impl McpServer {
    pub fn new(engine: memstead_base::Engine, token_budget: usize) -> Self {
        Self::new_with_filter(engine, token_budget, HashSet::new(), None)
    }

    /// Construct with an explicit disabled-tool filter. `disabled_tools`
    /// must already be validated against the compile-time tool-name
    /// registry (see `config::validate_disabled_tools`). `config_source`
    /// is the `.memstead/workspace.toml` path attributed in the
    /// `TOOL_DISABLED` envelope; pass `None` when there is no
    /// file-backed config.
    pub fn new_with_filter(
        engine: memstead_base::Engine,
        token_budget: usize,
        disabled_tools: HashSet<String>,
        config_source: Option<PathBuf>,
    ) -> Self {
        Self::new_with_config(
            engine,
            token_budget,
            disabled_tools,
            config_source,
            crate::config::MutationsSection::default(),
            HashMap::new(),
        )
    }

    /// Full-surface constructor. `mutations` and `plugin` come from
    /// `EffectiveSettings` and are surfaced verbatim under `memstead_health
    /// { include_config: true }`. The operator-mode posture defaults
    /// to `false` (agent-mode); flip it via [`Self::with_operator_mode`]
    /// before serving when boot established operator intent.
    pub fn new_with_config(
        mut engine: memstead_base::Engine,
        token_budget: usize,
        disabled_tools: HashSet<String>,
        config_source: Option<PathBuf>,
        mutations: crate::config::MutationsSection,
        plugin: HashMap<String, toml::Table>,
    ) -> Self {
        // `require_notes` is enforced once, inside the engine mutation
        // pipeline (every surface inherits the `NOTE_MISSING` warning
        // from the engine response). Mirror an explicitly-resolved
        // posture into the engine's settings so the engine and this
        // server can't disagree — but only when the caller passed an
        // explicit value. `new` / `new_with_filter` pass the default
        // (`None`), meaning "unspecified — keep whatever the engine
        // loaded from `.memstead/workspace.toml`"; clobbering that with
        // `None` would erase a policy the engine already knows.
        // Idempotent in production: both this `mutations` and
        // `engine.settings()` came from the same workspace.toml.
        if let Some(require_notes) = mutations.require_notes {
            let mut settings = engine.settings().clone();
            settings.mutations.require_notes = Some(require_notes);
            engine.set_settings(settings);
        }
        Self {
            unified_engine: Arc::new(Mutex::new(engine)),
            token_budget,
            default_role: memstead_base::vcs::Role::Unspecified,
            default_identity: None,
            disabled_tools: Arc::new(disabled_tools),
            config_source: config_source.map(Arc::new),
            client: Arc::new(OnceLock::new()),
            mutations: Arc::new(mutations),
            plugin: Arc::new(plugin),
            operator_mode: false,
        }
    }

    /// Builder-style setter for the operator-mode posture. Returns
    /// `self` for fluent chaining at the boot site. Only the
    /// `memstead-mcp --operator-mode` boot path is authorised to call
    /// this with `true`; agent-spawned servers leave it on the
    /// default `false`.
    /// Set the session-level default role (the binary's `--role`
    /// flag). Per-call `role` parameters win over this default.
    pub fn with_default_role(mut self, role: memstead_base::vcs::Role) -> Self {
        self.default_role = role;
        self
    }

    /// Set the session-level default identity (the binary's
    /// `--identity` flag / `MEMSTEAD_IDENTITY` environment variable,
    /// already normalised and length-checked at boot). Per-call
    /// `identity` parameters win over this default.
    pub fn with_default_identity(mut self, identity: Option<String>) -> Self {
        self.default_identity = identity;
        self
    }

    /// Resolve a per-call `identity` parameter against the session
    /// default. Per-call wins; an over-length
    /// value refuses typed (`INVALID_IDENTITY`) — the record is
    /// append-only, so nothing over the cap may reach it. An absent
    /// or whitespace-only value falls back to the session default;
    /// absence of both records as absence, never refused.
    fn resolve_identity(&self, raw: Option<&str>) -> Result<Option<String>, Box<CallToolResult>> {
        match memstead_base::vcs::normalise_identity(raw) {
            None => Ok(self.default_identity.clone()),
            Some(id) => {
                let len = id.chars().count();
                if len > memstead_base::vcs::IDENTITY_MAX_LEN {
                    let msg = format!(
                        "identity is {len} characters — the recorded identity is capped at {} \
                         (it is an opaque name or handle, not a description)",
                        memstead_base::vcs::IDENTITY_MAX_LEN
                    );
                    return Err(Box::new(tool_error_with_payload(
                        "INVALID_IDENTITY",
                        &msg,
                        envelope(
                            "INVALID_IDENTITY",
                            msg.clone(),
                            serde_json::json!({
                                "length": len,
                                "max": memstead_base::vcs::IDENTITY_MAX_LEN,
                            }),
                        ),
                    )));
                }
                Ok(Some(id))
            }
        }
    }

    /// Resolve a per-call `role` parameter against the session
    /// default. An unknown value refuses typed with the declarable
    /// vocabulary named.
    fn resolve_role(
        &self,
        raw: Option<&str>,
    ) -> Result<memstead_base::vcs::Role, Box<CallToolResult>> {
        match raw {
            None => Ok(self.default_role),
            Some(s) => memstead_base::vcs::Role::from_wire(s).ok_or_else(|| {
                let msg = format!(
                    "unknown role {s:?} — declarable roles: {}",
                    memstead_base::vcs::Role::DECLARABLE.join(", ")
                );
                Box::new(tool_error_with_payload(
                    "INVALID_ROLE",
                    &msg,
                    envelope(
                        "INVALID_ROLE",
                        msg.clone(),
                        serde_json::json!({
                            "role": s,
                            "allowed": memstead_base::vcs::Role::DECLARABLE,
                        }),
                    ),
                ))
            }),
        }
    }

    /// Resolve a call's `role` and `identity` against the session
    /// defaults, both before any mutation: the one resolver every
    /// mutating tool (entity and lifecycle alike) runs, so the
    /// per-call override, the fallback and the two typed refusals are
    /// the same on every write path.
    fn resolve_call_provenance(
        &self,
        role: Option<&str>,
        identity: Option<&str>,
    ) -> Result<(memstead_base::vcs::Role, Option<String>), Box<CallToolResult>> {
        let role = self.resolve_role(role)?;
        let identity = self.resolve_identity(identity)?;
        Ok((role, identity))
    }

    pub fn with_operator_mode(mut self, operator_mode: bool) -> Self {
        self.operator_mode = operator_mode;
        self
    }

    /// `true` when this server was booted with operator-mode bypass.
    /// Exposed so callers (tests, the overview surface) can observe
    /// the posture without reaching into private state.
    pub fn is_operator_mode(&self) -> bool {
        self.operator_mode
    }

    /// Borrow of the unified engine handle.
    pub fn unified_engine(&self) -> &Arc<Mutex<memstead_base::Engine>> {
        &self.unified_engine
    }

    /// Tool list filtered by the workspace's `disabled_tools` set. Used
    /// by the `list_tools` handler and directly by tests (which cannot
    /// easily synthesize a `RequestContext`).
    pub fn filtered_tool_list(&self) -> Vec<Tool> {
        let mut tools: Vec<Tool> = Self::tool_router().list_all();
        if !self.disabled_tools.is_empty() {
            tools.retain(|t| !self.disabled_tools.contains(t.name.as_ref()));
        }
        tools
    }

    /// `true` if the given tool name is disabled in this server's
    /// workspace. Public so tests can reason about the filter without
    /// reaching into private state.
    pub fn is_tool_disabled(&self, name: &str) -> bool {
        self.disabled_tools.contains(name)
    }

    /// `TOOL_DISABLED` error envelope. `details.config_source` is
    /// included only when a file-backed config was loaded — matches the
    /// crate's `serde(skip_serializing_if)` conventions.
    pub(crate) fn tool_disabled_response(&self, name: &str) -> CallToolResult {
        let msg = format!("Tool '{name}' is disabled in this workspace's MCP configuration.");
        let mut details = serde_json::Map::new();
        details.insert("tool".to_string(), serde_json::json!(name));
        if let Some(path) = &self.config_source {
            details.insert(
                "config_source".to_string(),
                serde_json::json!(path.display().to_string()),
            );
        }
        tool_error_with_payload(
            "TOOL_DISABLED",
            &msg,
            envelope(
                "TOOL_DISABLED",
                msg.clone(),
                serde_json::Value::Object(details),
            ),
        )
    }

    /// Get the default writable mem name from the unified engine.
    /// Returns `None` when the engine has no writable mems.
    ///
    /// Delegates to [`Engine::default_writable_mem`] — the first
    /// writable mount in declaration order (the stable seed mem), NOT
    /// `writable_mems().iter().next()` off an unordered set. Creating a
    /// second mem no longer silently retargets omitted-`mem` writes.
    fn primary_mem(&self) -> Option<String> {
        let engine = self.unified_engine.lock().ok()?;
        engine.default_writable_mem().map(|s| s.to_string())
    }

    /// Resolve a mem name, defaulting to primary.
    fn resolve_mem(&self, mem: Option<&str>) -> String {
        mem.map(|v| v.to_string())
            .or_else(|| self.primary_mem())
            .unwrap_or_else(|| "default".to_string())
    }
}

mod errors;
mod mem_tools;
mod read_tools;
mod responses;
mod write_tools;

use errors::*;
use responses::*;

// ==========================================================================
impl McpServer {
    /// The tool router every consumer uses: the macro-generated routes with
    /// each description stamped in from `descriptions::FULL`.
    ///
    /// The generated routers are `read_tool_router`, `write_tool_router` and
    /// `mem_tool_router`, one per tool-family module, and none is handed out:
    /// rmcp's `#[tool]` attribute takes a string literal and nothing else, so
    /// the prose cannot reach them, and this is the one seam where the text
    /// files become the served text. `#[tool_handler]` resolves to this
    /// function by name, so dispatch and listing agree.
    pub fn tool_router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        let mut router =
            Self::read_tool_router() + Self::write_tool_router() + Self::mem_tool_router();
        crate::descriptions::apply(&mut router, crate::descriptions::FULL);
        router
    }
}

// ServerHandler — wired up by #[tool_handler] macro
// ==========================================================================

/// The full server's session-start instructions — one named const so
/// the registry-honesty tests (`memstead-mcp/tests/tool_surface.rs`)
/// read the SAME string the macro serves, with no duplicated copy to
/// drift. Built with `concat!` so the engine version is baked in at
/// compile time; the trailing roster must name every registered tool
/// (bidirectionally test-enforced) and the CLI-companion note names
/// the verb families that deliberately do not live on this surface
/// (workspace policy among them — those are operator surfaces, the CLI
/// and the web API, so "not on MCP" must never be written "CLI only").
pub const SERVER_INSTRUCTIONS: &str = concat!(
    include_str!("../descriptions/full/server-instructions-head.md"),
    env!("CARGO_PKG_VERSION"),
    include_str!("../descriptions/full/server-instructions-tail.md"),
);

#[tool_handler]
impl ServerHandler for McpServer {
    /// Hand-written so `instructions` can be the named
    /// [`SERVER_INSTRUCTIONS`] const (the macro only accepts string
    /// literals) and the serverInfo version is the engine's full
    /// build version (semver + git build sha for dev builds) by
    /// construction — the historical hardcoded `"0.1.0"` cannot
    /// recur, and two dev builds between releases stay
    /// distinguishable. The const keeps its compile-time semver line;
    /// a short runtime "Build:" sentence is appended only when a sha
    /// exists. Mirrors the shape `#[tool_handler]` would generate.
    fn get_info(&self) -> rmcp::model::ServerInfo {
        let full_version = memstead_base::build_info::full_version();
        let instructions = if memstead_base::build_info::BUILD_SHA.is_empty() {
            SERVER_INSTRUCTIONS.to_string()
        } else {
            format!("{SERVER_INSTRUCTIONS} Build: {full_version}.")
        };
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::new("memstead", full_version))
        .with_instructions(instructions)
    }

    /// Capture the client's `clientInfo` from the initialize handshake so
    /// every agent-initiated mutation can tag its commit with a
    /// `Client: <name>@<version>` trailer.
    ///
    /// Mirrors the default `ServerHandler::initialize` body (set peer info +
    /// return `get_info()`) and additionally stashes the client identity
    /// into `McpServer::client`. `OnceLock::set` returning `Err` would mean
    /// a second `initialize` arrived on the same server instance —
    /// impossible under the stdio transport (one process per client) but
    /// worth logging if the transport ever changes.
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        let info = request.client_info.clone();
        let cid = ClientId {
            name: info.name.clone(),
            version: info.version.clone(),
        };
        // The commits this session causes as itself (config writes,
        // sync-state stamps, the anchor writers) carry the agent actor
        // and this client id like every entity mutation's commit.
        {
            let unified = self.unified_engine();
            let mut engine = unified.lock().unwrap_or_else(|p| p.into_inner());
            engine.set_actor(Actor::Agent);
            engine.set_client(Some(cid.clone()));
        }
        if let Err(existing) = self.client.set(cid) {
            tracing::warn!(
                existing = ?existing,
                incoming_name = info.name.as_str(),
                incoming_version = info.version.as_str(),
                "second initialize received on memstead-mcp server — single-client assumption violated; keeping first client identity",
            );
        }
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        Ok(self.get_info())
    }

    /// `list_tools` with the workspace's `disabled_tools` filter applied.
    /// Defining this method stops `#[tool_handler]` from generating the
    /// default body (see `has_method` gate in `rmcp-macros::tool_handler`).
    /// Behavior is byte-identical to the default when the filter is empty;
    /// otherwise matching tool records are omitted from the response.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.filtered_tool_list()))
    }

    /// `get_tool` short-circuits on disabled names to `None` so rmcp's
    /// validation path treats a disabled tool as non-existent. Matches
    /// `list_tools` — a disabled tool is neither listed nor discoverable
    /// by name.
    fn get_tool(&self, name: &str) -> Option<Tool> {
        if self.disabled_tools.contains(name) {
            return None;
        }
        Self::tool_router().get(name).cloned()
    }

    /// `call_tool` rejects disabled names with a `TOOL_DISABLED` envelope
    /// before dispatch. A client that kept a stale tool list or
    /// deliberately probes the bypass gets the same contract as the
    /// `list_tools` omission: this tool is not available here.
    ///
    /// This is also the friction ledger's one recording seam for the
    /// whole tool surface: every dispatched
    /// result that is a typed refusal appends one ledger entry whose
    /// values all come from closed engine-defined vocabularies (the
    /// module's privacy hard line — never parameters or payload text)
    /// — best-effort, after the response is already built, so
    /// recording can never perturb the refusal it measures.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, McpError> {
        let verb = request.name.to_string();
        if self.disabled_tools.contains(request.name.as_ref()) {
            let resp = self.tool_disabled_response(request.name.as_ref());
            self.record_friction(&verb, &resp);
            return Ok(resp.into());
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        let result = Self::tool_router().call(tcc).await;
        if let Ok(rmcp::model::CallToolResponse::Complete(r)) = &result {
            self.record_friction(&verb, r);
        }
        result
    }
}

impl McpServer {
    /// Append a friction-ledger entry when `result` is a typed refusal
    /// (`is_error` with a structured `code`). Untyped errors and
    /// successes append nothing; an unresolvable workspace root
    /// degrades to not-recording. Best-effort throughout — the
    /// response has already been built and is returned unchanged.
    fn record_friction(&self, verb: &str, result: &CallToolResult) {
        if !result.is_error.unwrap_or(false) {
            return;
        }
        let Some(code) = result
            .structured_content
            .as_ref()
            .and_then(|v| v.get("code"))
            .and_then(|c| c.as_str())
        else {
            return;
        };
        let root = match self.unified_engine().lock() {
            Ok(engine) => engine.workspace_root().map(|p| p.to_path_buf()),
            Err(_) => None,
        };
        if let Some(root) = root {
            let details = result
                .structured_content
                .as_ref()
                .and_then(|v| v.get("details"));
            memstead_base::friction::FrictionLedger::for_workspace(&root).record(
                "mcp",
                verb,
                code,
                memstead_base::friction::closed_reason(code, details),
            );
        }
    }
}

/// The unit tests, in `server/tests.rs`; the file gates itself with
/// `#![cfg(test)]`.
mod tests;
