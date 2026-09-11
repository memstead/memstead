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

/// Build a `_meta` map carrying `anthropic/alwaysLoad: true` so Claude
/// Code excludes the tagged tool from its `ToolSearch`-deferred set.
/// Applied to `memstead_overview` — the cold-start entry point that the
/// server `instructions` direct agents to call first. Without this,
/// agents pay an extra `ToolSearch` round-trip before they can reach
/// overview.
fn always_load_meta() -> rmcp::model::MetaObject {
    let mut m = rmcp::model::MetaObject::new();
    m.0.insert(
        "anthropic/alwaysLoad".to_string(),
        serde_json::Value::Bool(true),
    );
    m
}

/// Validate an entity ID before using it. Returns an error result if
/// invalid. Routes through [`tool_error_with_payload`] so the text
/// channel carries the `ERROR [INVALID_ENTITY_ID]: …` prefix and
/// `structured_content` carries the typed envelope — consistent with
/// the engine's `EngineError::InvalidEntityId` path for downstream
/// grammar violations.
fn validate_entity_id(id: &str) -> Option<CallToolResult> {
    if id.is_empty() {
        let msg = "Entity ID must not be empty.".to_string();
        return Some(tool_error_with_payload(
            "INVALID_ENTITY_ID",
            &msg,
            envelope(
                "INVALID_ENTITY_ID",
                msg.clone(),
                serde_json::json!({ "id": id, "reason": "empty" }),
            ),
        ));
    }
    if id.chars().count() > memstead_base::ENTITY_ID_MAX_LEN {
        let msg = format!(
            "Entity ID too long (max {} characters).",
            memstead_base::ENTITY_ID_MAX_LEN
        );
        return Some(tool_error_with_payload(
            "INVALID_ENTITY_ID",
            &msg,
            envelope(
                "INVALID_ENTITY_ID",
                msg.clone(),
                serde_json::json!({
                    "id": id,
                    "reason": "too_long",
                    "length": id.chars().count(),
                    "max": memstead_base::ENTITY_ID_MAX_LEN,
                }),
            ),
        ));
    }
    None
}

/// Validate the optional agent-authored `note` field on a mutation call.
/// Returns an `INVALID_INPUT` envelope when the note exceeds
/// `memstead_base::mem_management::NOTE_MAX_LEN` Unicode scalar values, matching the
/// mem-lifecycle orchestrators. Empty / absent values succeed.
/// Whitespace-only notes are allowed at the edge (the engine side
/// collapses them to "no body line" during commit-message assembly).
fn validate_note(note: Option<&str>) -> Option<CallToolResult> {
    let n = note?;
    if n.chars().count() > memstead_base::mem_management::NOTE_MAX_LEN {
        let max = memstead_base::mem_management::NOTE_MAX_LEN;
        let msg = format!(
            "note exceeds {max} characters — shorten the agent-authored \
             provenance line to one sentence."
        );
        let details = serde_json::json!({
            "max_chars": max,
            "got_chars": n.chars().count(),
        });
        return Some(tool_error_with_payload(
            "INVALID_INPUT",
            &msg,
            envelope("INVALID_INPUT", msg.clone(), details),
        ));
    }
    None
}

/// Helper: create a JSON tool response for mutation tools. Emits
/// pretty-printed JSON on the text channel and mirrors the typed value
/// onto `structured_content` so agents can branch on fields without
/// parsing the text. Read tools render pure Markdown without a JSON
/// sidecar; mutation tools keep the JSON envelope so agents can decode
/// the response shape deterministically. Serialization failures fall
/// back to an error body on the text channel and leave
/// `structured_content` empty.
fn json_response<T: serde::Serialize>(data: &T) -> CallToolResult {
    let text =
        serde_json::to_string_pretty(data).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"));
    let mut r = CallToolResult::success(vec![rmcp::model::ContentBlock::text(text)]);
    r.structured_content = serde_json::to_value(data).ok();
    r
}

/// #57: bound a health response's text channel so a multi-include report
/// can't overflow the response cap. `structured_content` always ships whole
/// (machine consumers read it). The text channel stays the pretty JSON when
/// the report fits the budget and no chunk was requested — byte-identical to
/// before, so a small call is unchanged; only when it would overflow (or a
/// chunk is explicitly requested) does the text become chunkable markdown
/// rendered from the structured payload, paged by `chunk`. A chunk index
/// past the end returns the chunker's `INVALID_INPUT` error verbatim.
/// Called last, after the post-processing that mutates `structured_content`.
fn finalize_health_text(
    mut res: CallToolResult,
    budget: usize,
    chunk: Option<usize>,
) -> CallToolResult {
    let Some(sc) = res.structured_content.as_ref() else {
        return res;
    };
    // The text channel currently holds the pretty JSON (from `json_response`
    // + the anchor/notice post-processing). Keep it verbatim while it fits.
    let json_text = serde_json::to_string_pretty(sc).unwrap_or_default();
    if chunk.is_none() && estimate_tokens(&json_text) <= budget {
        return res;
    }
    let md = memstead_base::ops::health_compose::render_health_markdown(sc);
    match apply_chunking(&md, budget, chunk, &[]) {
        Ok(text) => {
            res.content = vec![rmcp::model::ContentBlock::text(text)];
            res
        }
        Err(e) => tool_error("INVALID_INPUT", &e),
    }
}

/// Append a `WarningHint` envelope to the `warnings` array of an
/// already-serialized mutation response. Used by the `require_notes`
/// pipeline to attach a `NOTE_MISSING` warning without teaching every
/// `*Result` struct about the mutation-policy surface — the wire shape
/// already lists `warnings`, so we lift there and keep the engine
/// ignorant of the workspace policy.
///
/// Preserves the text channel (re-serialises the updated structured
/// value to pretty JSON). Silent no-op when the response has no
/// structured content or the structured content is not a JSON object
/// (e.g. serialization failed upstream) — the caller's original
/// response survives unchanged.
fn append_warning_hint(mut res: CallToolResult, warning: &WarningHint) -> CallToolResult {
    let Some(sc) = res.structured_content.as_mut() else {
        return res;
    };
    let Some(obj) = sc.as_object_mut() else {
        return res;
    };
    let entry = serde_json::to_value(warning).unwrap_or_else(
        |_| serde_json::json!({"code": warning.code(), "message": warning.message()}),
    );
    let warnings = obj
        .entry("warnings".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if let Some(arr) = warnings.as_array_mut() {
        arr.push(entry);
    } else {
        // Existing value was something other than an array — we can't
        // merge into it safely; leave the response untouched rather
        // than corrupting the wire shape.
        return res;
    }
    // Refresh the text channel so the JSON body lines up with the
    // updated structured content. On serialization failure leave the
    // existing text — a stale-but-readable mismatch beats an empty
    // body.
    if let Ok(text) = serde_json::to_string_pretty(&*sc) {
        res.content = vec![rmcp::model::ContentBlock::text(text)];
    }
    res
}

/// Helper: create a markdown tool response.
fn md_response(markdown: String) -> CallToolResult {
    CallToolResult::success(vec![rmcp::model::ContentBlock::text(markdown)])
}

/// Tool response that pairs rendered markdown on the text channel with
/// a structured envelope on `structured_content`. Tools whose response
/// has a
/// canonical human-readable form (entity, search) ship the markdown to
/// terminal/inline consumers and the typed JSON to branching agents in
/// one call, with no extra round-trip. The agent contract: branch on
/// `structured_content`; read the text channel for prose.
fn md_with_structured(markdown: String, structured: serde_json::Value) -> CallToolResult {
    let mut r = CallToolResult::success(vec![rmcp::model::ContentBlock::text(markdown)]);
    r.structured_content = Some(structured);
    r
}

/// Prepend a `> [!warning]` admonition block describing drift events
/// (`MemReloaded` warnings from [`Engine::reload_if_stale`])
/// to a markdown response body. Visible to agents inline at the top
/// of the rendered output so a reasoning loop reading e.g.
/// `memstead_entity` notices the snapshot shifted under it without
/// having to inspect a sidecar field.
///
/// No-op when `drift_warnings` is empty so the common (single-engine)
/// path produces byte-identical markdown to pre-multi-engine-coherence.
/// Attach the structured `mem_changed` notices a just-completed
/// operation accumulated (reload-before-operation) to a JSON response
/// body under the `mem_changed` key. No-op when `notices` is empty,
/// so the common single-engine path leaves the body byte-identical.
fn attach_mem_changed(body: &mut serde_json::Value, notices: Vec<MemChangedNotice>) {
    if notices.is_empty() {
        return;
    }
    body["mem_changed"] = serde_json::to_value(&notices).unwrap_or(serde_json::Value::Null);
}

/// Attach the target mem's durability marker to a mutation response.
/// `durable: false` means the mem's storage is volatile (in-memory) —
/// the accompanying `write_id` is shaped like a git SHA but denotes
/// nothing that survives process restart or session-TTL eviction. This is
/// the per-write echo of the same per-mem marker `overview` / `health`
/// carry, derived from the same `MountStorage::is_durable()`; it is
/// orthogonal to `write_id.is_empty()` (which says only whether a commit
/// happened), so an agent never has to conflate "no commit" with "commit
/// in RAM". Defaults to `false` for an unresolvable mem — the engine
/// never claims a durability it cannot vouch for.
fn attach_durability(body: &mut serde_json::Value, engine: &memstead_base::Engine, mem: &str) {
    let durable = engine
        .mounts()
        .iter()
        .find(|m| m.mem == mem)
        .map(|m| m.storage.is_durable())
        .unwrap_or(false);
    // The marker stays; what travels with it is what it was derived from, so
    // a caller cannot read the mount-kind answer as a stronger one.
    let basis = engine
        .mounts()
        .iter()
        .find(|m| m.mem == mem)
        .map(|m| {
            m.storage
                .durability_basis(engine.mem_head_sha(mem).ok().flatten().as_deref())
                .as_wire()
        })
        .unwrap_or("inferred-from-mount-kind");
    if let Some(obj) = body.as_object_mut() {
        obj.insert("durable".into(), serde_json::json!(durable));
        obj.insert("durability_basis".into(), serde_json::json!(basis));
    }
}

/// Attach `mem_changed` notices to a response's `structured_content`
/// envelope (success or error — anything whose `structured_content` is
/// a JSON object). A mutation that reloaded then refused (e.g.
/// `HASH_MISMATCH`) carries the notice alongside the refusal; a read
/// whose structured envelope is built separately gets it the same way.
/// Draining here also keeps the engine stash from leaking into the next
/// operation. No-op when `notices` is empty or the envelope is not a
/// JSON object.
fn attach_mem_changed_to_result(
    mut res: CallToolResult,
    notices: Vec<MemChangedNotice>,
) -> CallToolResult {
    if notices.is_empty() {
        return res;
    }
    if let Some(obj) = res
        .structured_content
        .as_mut()
        .and_then(|sc| sc.as_object_mut())
    {
        obj.insert(
            "mem_changed".to_string(),
            serde_json::to_value(&notices).unwrap_or(serde_json::Value::Null),
        );
    }
    res
}

/// Reconstruct one `MemReloaded` warning per stashed notice. Used on
/// error paths that hold only the drained `mem_changed` notices and
/// no `drift_warnings` Vec — mutation handlers, whose reload happens
/// inside the engine and surfaces solely as a stashed notice. The
/// `entities_loaded` count comes from the notice's own delta size so
/// the synthesised warning Display matches what a read handler's
/// `WarningHint::MemReloaded` would render for the same reload.
fn notices_as_reload_warnings(notices: &[MemChangedNotice]) -> Vec<WarningHint> {
    notices
        .iter()
        .map(|n| WarningHint::MemReloaded {
            mem: n.mem.clone(),
            old_head: n.from_head.clone(),
            new_head: n.to_head.clone(),
            entities_loaded: n.entity_count(),
        })
        .collect()
}

/// Attach reload drift to an *error* response on the same channel split
/// a successful response uses: the full per-entity `mem_changed`
/// notice on `structured_content`, plus the `MEM_RELOADED` admonition
/// the read success path prepends, on the text channel. Never carries
/// the serialised notice on the text channel — that would make an error
/// response richer than the matching success (a new asymmetry); the
/// text channel gets only the warning line.
///
/// Every error early-return reachable *after* a
/// `OperationScope::finish()` hand-out routes through this so a reload
/// that happened during the operation reaches the agent whether the
/// operation then succeeded or failed. No-op when both inputs are empty,
/// so the common no-drift path stays byte-identical to pre-fix.
fn attach_drift_to_error(
    res: CallToolResult,
    drift_warnings: &[WarningHint],
    notices: Vec<MemChangedNotice>,
) -> CallToolResult {
    let res = prepend_drift_warnings_to_result_text(res, drift_warnings);
    attach_mem_changed_to_result(res, notices)
}

/// Prepend the drift admonition (the same `> [!warning]` block the read
/// success path renders) to the first text content of an already-built
/// response — typically an `ERROR [<CODE>]: …` error envelope. No-op
/// when `drift_warnings` is empty or the response has no text content,
/// so the no-drift path is byte-identical.
fn prepend_drift_warnings_to_result_text(
    mut res: CallToolResult,
    drift_warnings: &[WarningHint],
) -> CallToolResult {
    if drift_warnings.is_empty() {
        return res;
    }
    let Some(existing) = res
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
    else {
        return res;
    };
    let prefixed = prepend_drift_warnings_md(existing, drift_warnings);
    res.content[0] = rmcp::model::ContentBlock::text(prefixed);
    res
}

fn prepend_drift_warnings_md(md: String, drift_warnings: &[WarningHint]) -> String {
    if drift_warnings.is_empty() {
        return md;
    }
    let mut prefix = String::new();
    prefix.push_str(
        "> [!warning] Engine snapshot reloaded — sibling writer advanced an on-disk mem HEAD\n",
    );
    for w in drift_warnings {
        prefix.push_str(&format!(">\n> - **{}**: {}\n", w.code(), w));
    }
    prefix.push('\n');
    prefix.push_str(&md);
    prefix
}

// Per-mem schema pin / workspace policy / cross-catalogue schema
// lookup helpers live in `memstead_base::overview` so the shared
// composer (this MCP tool + the CLI) and the rest of this server
// reach the same canonical implementation. The wrappers below stay so
// the existing ~14 call sites in this file continue to compile
// unchanged; their bodies just forward.

/// Format the canonical schema pin for a known mem as `name@version`,
/// or `None` if the mem is not registered. Source of truth is the
/// engine's `MemState.schema_ref`, which always reflects the schema
/// actually loaded — never a stale on-disk pin.
fn mem_schema_ref_unified(engine: &memstead_base::Engine, mem_name: &str) -> Option<String> {
    memstead_base::overview::mem_schema_ref(engine, mem_name)
}

fn find_schema_unified<'a>(
    engine: &'a memstead_base::Engine,
    sref: &memstead_schema::SchemaRef,
) -> Option<&'a std::sync::Arc<memstead_schema::Schema>> {
    memstead_base::overview::find_schema(engine, sref)
}

/// Resolve a schema by bare name across mem-pinned, workspace, and
/// built-in catalogues. Mirrors `find_schema_unified`'s precedence:
/// mem-pinned first, then workspace, then built-ins. Used by
/// `memstead_schema(name="<bare>")` for the path that doesn't carry a
/// `@<version>` pin — picks the first matching schema by name.
fn find_schema_by_name<'a>(
    engine: &'a memstead_base::Engine,
    name: &str,
) -> Option<&'a std::sync::Arc<memstead_schema::Schema>> {
    if let Some(s) = engine.schemas().values().find(|s| s.manifest.name == name) {
        return Some(s);
    }
    if let Some(s) = engine
        .workspace_schemas()
        .iter()
        .find(|s| s.manifest.name == name)
    {
        return Some(s);
    }
    engine
        .builtin_schemas()
        .iter()
        .find(|s| s.manifest.name == name)
}

/// Inject `_mem_schema: <ref>` as the first line inside the YAML
/// frontmatter block of a rendered markdown response. No-op when the
/// markdown does not start with `---\n` (defensive — a future renderer
/// without frontmatter must not get a malformed prefix). Idempotent: if
/// `_mem_schema:` is already present at the top of the frontmatter the
/// second call is a silent no-op so chunked responses do not double-stamp.
fn inject_md_mem_schema(md: &mut String, schema_ref: &str) {
    // The core's split decides whether there is frontmatter at all and
    // where it starts (a CRLF fence is five bytes, a byte-order mark is
    // skipped); the meta slice borrows `md`, so its offset is the insert
    // point.
    let Some((meta, _)) = memstead_base::frontmatter_parts(md) else {
        return;
    };
    let at = meta.as_ptr() as usize - md.as_ptr() as usize;
    if md[at..].starts_with("_mem_schema:") {
        return;
    }
    md.insert_str(at, &format!("_mem_schema: {schema_ref}\n"));
}

/// Insert `_mem_schema: <ref>` at the top level of a JSON-shaped tool
/// response built via [`json_response`]. Refreshes the text channel so
/// pretty-printed JSON stays in lockstep with `structured_content`. Silent
/// no-op when the response has no structured content or the structured
/// content is not a JSON object — the original response survives unchanged.
fn with_mem_schema_anchor(mut res: CallToolResult, schema_ref: &str) -> CallToolResult {
    let Some(sc) = res.structured_content.as_mut() else {
        return res;
    };
    let Some(obj) = sc.as_object_mut() else {
        return res;
    };
    obj.insert(
        "_mem_schema".to_string(),
        serde_json::Value::String(schema_ref.to_string()),
    );
    if let Ok(text) = serde_json::to_string_pretty(&*sc) {
        res.content = vec![rmcp::model::ContentBlock::text(text)];
    }
    res
}

/// Convert an `EngineError` to a tool error response. Exhaustive over every
/// variant — each case produces a `{code, message, details}` envelope on
/// `structured_content` so agents can branch on the stable `UPPER_SNAKE_CASE`
/// Typed-envelope translator for the unified `memstead_base::EngineError`.
/// Mutation handlers' unified branches return this when the engine
/// surfaces an error so the wire shape carries a stable `code` and
/// (where applicable) the recovery `details` payload full callers
/// already branch on.
///
/// The match is exhaustive — no wildcard arm. Every `EngineError`
/// variant maps to a typed envelope; adding a new variant fails
/// compilation here until an arm picks a code (and, when applicable,
/// a structured details payload). The compiler is the forcing
/// function. An earlier shape carried
/// a `_ => INTERNAL` arm that silently swallowed `DescriptionNotPermitted`,
/// `WikiLinkWithoutRelation`, `MissingRequiredDescription`, and several
/// rename/parse-path variants — trained agents to ignore `INTERNAL`
/// even when the underlying error was user-recoverable. The exhaustive
/// match makes that regression class structurally impossible: every
/// future `EngineError` variant must declare its wire shape here
/// before it can land.
fn engine_err_unified(
    e: memstead_base::EngineError,
    engine: &memstead_base::Engine,
) -> CallToolResult {
    use memstead_base::EngineError as E;
    // The text-channel message uses the rich-prose renderer so an agent
    // reading only `result.content[0].text` sees the full recovery
    // payload inline rather than a "+N more — see details.X" pointer
    // to a structured channel the MCP client doesn't surface. The
    // structured payload below is unchanged.
    let message = e.prose_render();
    match &e {
        E::NotFound { id } => {
            // #55: attach `suggestions` so this generic mapper carries the
            // same recovery detail as the dedicated `not_found_error`, no
            // matter which internal path raised the error.
            let suggestions = suggest_similar(engine.store(), id);
            tool_error_with_payload(
                "ENTITY_NOT_FOUND",
                &message,
                envelope(
                    "ENTITY_NOT_FOUND",
                    message.clone(),
                    serde_json::json!({ "id": id, "suggestions": suggestions }),
                ),
            )
        }
        E::EntityIdMissingMem { .. } => tool_error_with_payload(
            "ENTITY_ID_MISSING_MEM",
            &message,
            envelope("ENTITY_ID_MISSING_MEM", message.clone(), e.details()),
        ),
        E::AlreadyExists { .. } => tool_error_with_payload(
            "ENTITY_ALREADY_EXISTS",
            &message,
            // Payload comes from `details()` so the occupying title
            // cannot drift between the CLI and MCP envelopes.
            envelope("ENTITY_ALREADY_EXISTS", message.clone(), e.details()),
        ),
        // Block-tier declared-constraint refusals — code and recovery
        // payload come from the error itself (`code()` / `details()`),
        // so the envelope stays aligned with the CLI `--json` shape.
        // Code and payload from the error itself, so the
        // buried-section list reaches the agent identically on both surfaces.
        E::UnterminatedFenceInStoredBody { .. } => tool_error_with_payload(
            "UNTERMINATED_FENCE_IN_STORED_BODY",
            &message,
            envelope(
                "UNTERMINATED_FENCE_IN_STORED_BODY",
                message.clone(),
                e.details(),
            ),
        ),
        E::ConstraintUnsatisfied { .. }
        | E::RequiredOutgoingUnsatisfied { .. }
        | E::SectionFormatRefused { .. } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(e.code(), message.clone(), e.details()),
        ),
        E::HashMismatch {
            id,
            current,
            is_stub,
        } => tool_error_with_payload(
            "HASH_MISMATCH",
            &message,
            envelope(
                "HASH_MISMATCH",
                message.clone(),
                serde_json::json!({
                    "id": id,
                    "current": current,
                    "is_stub": is_stub,
                }),
            ),
        ),
        E::UnknownMem(name) => {
            // #55: attach `known_mems` so this generic mapper matches the
            // dedicated mem-not-found handler regardless of which path
            // raised the error.
            let known_mems: Vec<String> = engine.mounts().iter().map(|m| m.mem.clone()).collect();
            tool_error_with_payload(
                "UNKNOWN_MEM",
                &message,
                envelope(
                    "UNKNOWN_MEM",
                    message.clone(),
                    serde_json::json!({ "name": name, "known_mems": known_mems }),
                ),
            )
        }
        E::MemUnmounted { mem } => tool_error_with_payload(
            "MEM_UNMOUNTED",
            &message,
            envelope(
                "MEM_UNMOUNTED",
                message.clone(),
                serde_json::json!({ "mem": mem }),
            ),
        ),
        E::MemQuarantined {
            mem,
            reason_code,
            reason_message,
        } => tool_error_with_payload(
            "MEM_QUARANTINED",
            &message,
            envelope(
                "MEM_QUARANTINED",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "reason_code": reason_code,
                    "reason_message": reason_message,
                }),
            ),
        ),
        E::UnknownRef(raw) => tool_error_with_payload(
            "UNKNOWN_REF",
            &message,
            envelope(
                "UNKNOWN_REF",
                message.clone(),
                serde_json::json!({ "ref": raw }),
            ),
        ),
        E::BranchResetHeadMoved {
            mem,
            expected,
            current,
        } => tool_error_with_payload(
            "BRANCH_RESET_HEAD_MOVED",
            &message,
            envelope(
                "BRANCH_RESET_HEAD_MOVED",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "expected": expected,
                    "current": current,
                }),
            ),
        ),
        E::PushedCommitsProtected {
            mem,
            target_sha,
            pushed_shas,
        } => tool_error_with_payload(
            "PUSHED_COMMITS_PROTECTED",
            &message,
            envelope(
                "PUSHED_COMMITS_PROTECTED",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "target_sha": target_sha,
                    "pushed_shas": pushed_shas,
                }),
            ),
        ),
        E::UnknownRemote(name) => tool_error_with_payload(
            "UNKNOWN_REMOTE",
            &message,
            envelope(
                "UNKNOWN_REMOTE",
                message.clone(),
                serde_json::json!({ "remote": name }),
            ),
        ),
        E::LocalDivergence { mem, remote_ref } => tool_error_with_payload(
            "LOCAL_DIVERGENCE",
            &message,
            envelope(
                "LOCAL_DIVERGENCE",
                message.clone(),
                serde_json::json!({ "mem": mem, "remote_ref": remote_ref }),
            ),
        ),
        E::NonFastForward { mem, remote } => tool_error_with_payload(
            "NON_FAST_FORWARD",
            &message,
            envelope(
                "NON_FAST_FORWARD",
                message.clone(),
                serde_json::json!({ "mem": mem, "remote": remote }),
            ),
        ),
        E::LocalInvalidState {
            mem,
            remote,
            detail,
        } => tool_error_with_payload(
            "LOCAL_INVALID_STATE",
            &message,
            envelope(
                "LOCAL_INVALID_STATE",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "remote": remote,
                    "detail": detail,
                }),
            ),
        ),
        E::SchemaViolationInFetch {
            mem,
            ref_name,
            violations,
        } => tool_error_with_payload(
            "SCHEMA_VIOLATION_IN_FETCH",
            &message,
            envelope(
                "SCHEMA_VIOLATION_IN_FETCH",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "ref": ref_name,
                    "violations": violations,
                }),
            ),
        ),
        E::ReadOnlyMount(mem) => tool_error_with_payload(
            "READ_ONLY_MOUNT",
            &message,
            envelope(
                "READ_ONLY_MOUNT",
                message.clone(),
                serde_json::json!({ "mem": mem }),
            ),
        ),
        E::CheckNotRecorded { reason } => tool_error_with_payload(
            "CHECK_NOT_RECORDED",
            &message,
            envelope(
                "CHECK_NOT_RECORDED",
                message.clone(),
                serde_json::json!({ "reason": reason }),
            ),
        ),
        E::UnknownType {
            name,
            schema_ref,
            declared,
            suggestion,
        } => tool_error_with_payload(
            "UNKNOWN_ENTITY_TYPE",
            &message,
            envelope(
                "UNKNOWN_ENTITY_TYPE",
                message.clone(),
                serde_json::json!({
                    "name": name,
                    "schema_ref": schema_ref,
                    "declared": declared,
                    "suggestion": suggestion,
                }),
            ),
        ),
        E::HasIncomingRefs { id, referrers } => {
            // Project each ReferrerInfo into the wire shape the
            // memstead_delete description advertises: `{ from_id,
            // rel_types, mem, capability: "write" }`. The capability
            // is constant on this path — only Write-Mem referrers
            // ever surface here (ReadOnly referrers ride the
            // residual-stub demotion path). Per-source dedup happens
            // upstream: `rel_types` carries every edge
            // type from this source to the deletion target.
            let referrers_json: Vec<_> = referrers
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "from_id": r.from_id,
                        "rel_types": r.rel_types,
                        "mem": r.mem,
                        "capability": "write",
                    })
                })
                .collect();
            tool_error_with_payload(
                e.code(),
                &message,
                envelope(
                    e.code(),
                    message.clone(),
                    serde_json::json!({ "id": id, "referrers": referrers_json }),
                ),
            )
        }
        E::MemHasIncomingRefs { mem, referrers } => {
            // Mem-level mirror of HasIncomingRefs (F15 / CLI F8): the
            // mem-delete edge-graph check. Same `{from_id,
            // rel_types, mem}` projection; capability is omitted
            // because the mem-level check already filtered to
            // Write-Mem sources upstream.
            let referrers_json: Vec<_> = referrers
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "from_id": r.from_id,
                        "rel_types": r.rel_types,
                        "mem": r.mem,
                    })
                })
                .collect();
            tool_error_with_payload(
                e.code(),
                &message,
                envelope(
                    e.code(),
                    message.clone(),
                    serde_json::json!({ "mem": mem, "referrers": referrers_json }),
                ),
            )
        }
        E::CrossMemLinkNotAllowed { from_mem, to_mem } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(
                e.code(),
                message.clone(),
                serde_json::json!({
                    "from_mem": from_mem,
                    "to_mem": to_mem,
                }),
            ),
        ),
        E::CrossMemTargetNotFound {
            target_id,
            target_mem,
        } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(
                e.code(),
                message.clone(),
                serde_json::json!({
                    "target_id": target_id,
                    "target_mem": target_mem,
                }),
            ),
        ),
        E::CrossMemEdgeNotDeclared {
            source_schema,
            target_schema,
            rel_type,
            from_id,
            to_id,
        } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(
                e.code(),
                message.clone(),
                serde_json::json!({
                    "source_schema": source_schema,
                    "target_schema": target_schema,
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                }),
            ),
        ),
        E::RepairNotNeeded { id, recovery } => tool_error_with_payload(
            "REPAIR_NOT_NEEDED",
            &message,
            envelope(
                "REPAIR_NOT_NEEDED",
                message.clone(),
                serde_json::json!({ "id": id, "recovery": recovery }),
            ),
        ),
        E::ConflictingSectionModes { section, modes } => tool_error_with_payload(
            "CONFLICTING_SECTION_MODES",
            &message,
            envelope(
                "CONFLICTING_SECTION_MODES",
                message.clone(),
                serde_json::json!({ "section": section, "modes": modes }),
            ),
        ),
        E::RelationshipCycle {
            rel_type,
            from,
            to,
            existing_path,
            path_truncated,
            acyclic_set,
            existing_path_rel_types,
        } => {
            let existing_path_json: Vec<String> =
                existing_path.iter().map(|id| id.to_string()).collect();
            let mut details = serde_json::json!({
                "rel_type": rel_type,
                "from": from.to_string(),
                "to": to.to_string(),
                "existing_path": existing_path_json,
                "path_truncated": path_truncated,
            });
            // Additive set-refusal extras; single-rel-type refusals
            // keep their byte-identical payload.
            if let Some(set) = &acyclic_set {
                details["acyclic_set"] = serde_json::json!(set);
            }
            if let Some(rels) = &existing_path_rel_types {
                details["existing_path_rel_types"] = serde_json::json!(rels);
            }
            tool_error_with_payload(
                "RELATIONSHIP_CYCLE",
                &message,
                envelope("RELATIONSHIP_CYCLE", message.clone(), details),
            )
        }
        E::RequiredFieldUnset {
            field,
            entity_type,
            field_description,
            enum_values,
            type_write_rules,
            // Path-aware prose flows through `prose_render()`; the
            // structured payload below is the same on both paths so
            // `on_create` is intentionally not surfaced here.
            on_create: _,
            missing,
        } => {
            // `details.missing[]` carries every required-no-default
            // field unset on the create path. Each entry echoes
            // the type-level `write_rules` for self-containment.
            let missing_json: Vec<_> = missing
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "field": m.key,
                        "description": m.description,
                        "enum_values": m.enum_values,
                        "write_rules": type_write_rules,
                    })
                })
                .collect();
            tool_error_with_payload(
                "REQUIRED_FIELD_UNSET",
                &message,
                envelope(
                    "REQUIRED_FIELD_UNSET",
                    message.clone(),
                    serde_json::json!({
                        "field": field,
                        "entity_type": entity_type,
                        "field_description": field_description,
                        "enum_values": enum_values,
                        "type_write_rules": type_write_rules,
                        "missing": missing_json,
                    }),
                ),
            )
        }
        E::MissingRequiredSection {
            entity_type,
            missing_count,
            sections,
            type_guidance,
            pre_announced_missing_fields,
        } => {
            let sections_json: Vec<_> = sections
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "entity_type": s.entity_type,
                        "key": s.key,
                        "heading": s.heading,
                        "write_rules": s.write_rules,
                    })
                })
                .collect();
            let mut details = serde_json::json!({
                "entity_type": entity_type,
                "missing_count": missing_count,
                "sections": sections_json,
                "type_guidance": type_guidance,
            });
            // Cross-gate pre-announcement — additive, only when
            // non-empty; element shape mirrors REQUIRED_FIELD_UNSET's
            // details.missing[] so one decoder reads both.
            if !pre_announced_missing_fields.is_empty() {
                let type_rules = type_guidance.get(entity_type).cloned().unwrap_or_default();
                let missing_json: Vec<_> = pre_announced_missing_fields
                    .iter()
                    .map(|m| {
                        serde_json::json!({
                            "field": m.key,
                            "description": m.description,
                            "enum_values": m.enum_values,
                            "write_rules": type_rules,
                        })
                    })
                    .collect();
                details["pre_announced"] = serde_json::json!({
                    "required_field_unset": { "missing": missing_json }
                });
            }
            tool_error_with_payload(
                "MISSING_REQUIRED_SECTION",
                &message,
                envelope("MISSING_REQUIRED_SECTION", message.clone(), details),
            )
        }
        E::SetAndUnsetConflict { keys } => tool_error_with_payload(
            "SET_AND_UNSET_CONFLICT",
            &message,
            envelope(
                "SET_AND_UNSET_CONFLICT",
                message.clone(),
                serde_json::json!({ "keys": keys }),
            ),
        ),
        E::PatchSectionEmpty { section } => tool_error_with_payload(
            "PATCH_SECTION_EMPTY",
            &message,
            envelope(
                "PATCH_SECTION_EMPTY",
                message.clone(),
                serde_json::json!({ "section": section }),
            ),
        ),
        E::PatchOldNotFound {
            section,
            current_content,
            truncated,
            found_in_sections,
        } => tool_error_with_payload(
            "PATCH_OLD_NOT_FOUND",
            &message,
            envelope(
                "PATCH_OLD_NOT_FOUND",
                message.clone(),
                serde_json::json!({
                    "section": section,
                    "current_content": current_content,
                    "truncated": truncated,
                    "found_in_sections": found_in_sections,
                }),
            ),
        ),
        E::InvalidTitle(slug_err) => {
            use memstead_base::SlugError;
            let reason = slug_err.reason();
            let details = match &slug_err {
                SlugError::IdTooLong { input, length, max } => serde_json::json!({
                    "reason": reason,
                    "input": input,
                    "length": length,
                    "max": max,
                }),
                SlugError::TitleEmpty { input } => serde_json::json!({
                    "reason": reason,
                    "input": input,
                }),
                SlugError::TitleHasControlChars {
                    input,
                    control_chars,
                    proposed_slug,
                } => {
                    let control_chars_str: Vec<String> = control_chars
                        .iter()
                        .map(|c| c.escape_default().to_string())
                        .collect();
                    serde_json::json!({
                        "reason": reason,
                        "input": input,
                        "control_chars": control_chars_str,
                        "proposed_slug": proposed_slug,
                    })
                }
            };
            tool_error_with_payload(
                "INVALID_TITLE",
                &message,
                envelope("INVALID_TITLE", message.clone(), details),
            )
        }
        E::StubCannotRelate { id } => tool_error_with_payload(
            "STUB_CANNOT_RELATE",
            &message,
            envelope(
                "STUB_CANNOT_RELATE",
                message.clone(),
                serde_json::json!({ "id": id }),
            ),
        ),
        E::StubNotUpdatable { id } => tool_error_with_payload(
            "STUB_NOT_UPDATABLE",
            &message,
            envelope(
                "STUB_NOT_UPDATABLE",
                message.clone(),
                serde_json::json!({ "id": id }),
            ),
        ),
        E::StubNotRenamable { id } => tool_error_with_payload(
            "STUB_NOT_RENAMABLE",
            &message,
            envelope(
                "STUB_NOT_RENAMABLE",
                message.clone(),
                serde_json::json!({ "id": id }),
            ),
        ),
        E::InvalidEntityId { id, reason } => tool_error_with_payload(
            "INVALID_ENTITY_ID",
            &message,
            envelope(
                "INVALID_ENTITY_ID",
                message.clone(),
                serde_json::json!({ "id": id, "reason": reason }),
            ),
        ),
        E::InvalidWikiLinkTarget {
            raw,
            suggested,
            section,
            link_source,
            reason,
        } => tool_error_with_payload(
            "INVALID_WIKI_LINK_TARGET",
            &message,
            envelope(
                "INVALID_WIKI_LINK_TARGET",
                message.clone(),
                serde_json::json!({
                    "raw": raw,
                    "suggested": suggested,
                    "section": section,
                    "source": link_source,
                    "reason": reason,
                }),
            ),
        ),
        E::InvalidWikiLinkMem {
            raw,
            section,
            reason,
        } => tool_error_with_payload(
            "INVALID_MEM_NAME",
            &message,
            envelope(
                "INVALID_MEM_NAME",
                message.clone(),
                serde_json::json!({
                    "raw": raw,
                    "section": section,
                    "reason": reason,
                }),
            ),
        ),
        E::RelationHasBodyLinks {
            from_id,
            to_id,
            rel_type,
            body_links,
        } => tool_error_with_payload(
            "RELATION_HAS_BODY_LINKS",
            &message,
            envelope(
                "RELATION_HAS_BODY_LINKS",
                message.clone(),
                serde_json::json!({
                    "from_id": from_id,
                    "to_id": to_id,
                    "rel_type": rel_type,
                    "body_links": body_links,
                }),
            ),
        ),
        E::Validation(verr) => unified_validation_envelope(verr.clone()),
        // Lifecycle envelopes: the unified
        // `mem_management::create_mem` / `delete_mem` paths
        // surface these on the same wire contract.
        E::MemNameCollision {
            name,
            source_origin,
        } => tool_error_with_payload(
            "MEM_NAME_COLLISION",
            &message,
            envelope(
                "MEM_NAME_COLLISION",
                message.clone(),
                serde_json::json!({
                    "name": name,
                    "source": source_origin,
                }),
            ),
        ),
        e @ E::SchemaNotFound { .. } => tool_error_with_payload(
            "SCHEMA_NOT_FOUND",
            &message,
            envelope("SCHEMA_NOT_FOUND", message.clone(), e.details()),
        ),
        E::EmbeddedSchemaInvalid { mem, pin, reason } => tool_error_with_payload(
            "EMBEDDED_SCHEMA_INVALID",
            &message,
            envelope(
                "EMBEDDED_SCHEMA_INVALID",
                message.clone(),
                serde_json::json!({ "mem": mem, "schema": pin, "error": reason }),
            ),
        ),
        E::SchemaPackageInvalid {
            name,
            version,
            message: detail,
        } => tool_error_with_payload(
            "SCHEMA_VALIDATION_FAILED",
            &message,
            envelope(
                "SCHEMA_VALIDATION_FAILED",
                message.clone(),
                serde_json::json!({
                    "schema": format!("{name}@{version}"),
                    "error": detail,
                }),
            ),
        ),
        E::SchemaResolverInit(detail) => tool_error_with_payload(
            "SCHEMA_RESOLVER_INIT_FAILED",
            &message,
            envelope(
                "SCHEMA_RESOLVER_INIT_FAILED",
                message.clone(),
                serde_json::json!({ "detail": detail }),
            ),
        ),
        E::Mem(detail) => tool_error_with_payload(
            "MEM_ERROR",
            &message,
            envelope(
                "MEM_ERROR",
                message.clone(),
                serde_json::json!({ "detail": detail }),
            ),
        ),
        E::InvalidInput(msg) => tool_error_with_payload(
            "INVALID_INPUT",
            &message,
            envelope(
                "INVALID_INPUT",
                message.clone(),
                serde_json::json!({ "message": msg }),
            ),
        ),
        E::MergeConflictUnsupportedBackend { mem } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(e.code(), message.clone(), serde_json::json!({ "mem": mem })),
        ),
        E::NotConflicted { id } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(e.code(), message.clone(), serde_json::json!({ "id": id })),
        ),
        E::RenameSimilarityOutOfRange {
            requested,
            allowed_min,
            allowed_max,
        } => tool_error_with_payload(
            "INVALID_INPUT",
            &message,
            envelope(
                "INVALID_INPUT",
                message.clone(),
                serde_json::json!({
                    "field": "rename_similarity",
                    "requested": requested,
                    "allowed_range": [allowed_min, allowed_max],
                }),
            ),
        ),
        E::MemConfigIncomplete {
            mem,
            missing_fields,
        } => tool_error_with_payload(
            "MEM_CONFIG_INCOMPLETE",
            &message,
            envelope(
                "MEM_CONFIG_INCOMPLETE",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "missing_fields": missing_fields,
                    "set_via": format!("memstead mem set-version {mem} <version>"),
                }),
            ),
        ),
        E::WikiLinkWithoutRelation { from_id, missing } => tool_error_with_payload(
            "WIKILINK_WITHOUT_RELATION",
            &message,
            envelope(
                "WIKILINK_WITHOUT_RELATION",
                message.clone(),
                serde_json::json!({
                    "from_id": from_id,
                    "missing": missing,
                }),
            ),
        ),
        E::DescriptionNotPermitted {
            rel_type,
            from_id,
            to_id,
        } => tool_error_with_payload(
            "DESCRIPTION_NOT_PERMITTED",
            &message,
            envelope(
                "DESCRIPTION_NOT_PERMITTED",
                message.clone(),
                serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                }),
            ),
        ),
        E::MissingRequiredDescription {
            rel_type,
            from_id,
            to_id,
        } => tool_error_with_payload(
            "MISSING_REQUIRED_DESCRIPTION",
            &message,
            envelope(
                "MISSING_REQUIRED_DESCRIPTION",
                message.clone(),
                serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                }),
            ),
        ),
        E::RelationManualAuthoringForbidden {
            rel_type,
            from_id,
            to_id,
            guidance,
        } => tool_error_with_payload(
            "RELATION_MANUAL_AUTHORING_FORBIDDEN",
            &message,
            envelope(
                "RELATION_MANUAL_AUTHORING_FORBIDDEN",
                message.clone(),
                serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                    "guidance": guidance,
                }),
            ),
        ),
        // Retype refusals carry their report-all payload from `details()`
        // so the CLI and MCP envelopes cannot drift; the code is the
        // shared problem code or `RETYPE_REFUSED` (see `EngineError::code`).
        E::RetypeRefused { .. }
        | E::RetypeNoOp { .. }
        | E::RetypeReferrerUnprobeable { .. }
        | E::InvalidCheckFinding { .. } => tool_error_with_payload(
            e.code(),
            &message,
            envelope(e.code(), message.clone(), e.details()),
        ),
        E::RenameNoOp { id, new_title } => tool_error_with_payload(
            "RENAME_NO_OP",
            &message,
            envelope(
                "RENAME_NO_OP",
                message.clone(),
                serde_json::json!({
                    "id": id,
                    "new_title": new_title,
                }),
            ),
        ),
        E::RenameBlockedByCrossMemPolicy {
            from_mem,
            blocked_referrers,
        } => {
            let entries: Vec<_> = blocked_referrers
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "from_mem": r.from_mem,
                        "to_mem": r.to_mem,
                        "count": r.count,
                    })
                })
                .collect();
            tool_error_with_payload(
                "RENAME_BLOCKED_BY_CROSS_MEM_POLICY",
                &message,
                envelope(
                    "RENAME_BLOCKED_BY_CROSS_MEM_POLICY",
                    message.clone(),
                    serde_json::json!({
                        "from_mem": from_mem,
                        "blocked_referrers": entries,
                    }),
                ),
            )
        }
        E::RenamePartialFailure {
            committed_mems,
            failed_mem,
            failure_cause,
        } => tool_error_with_payload(
            "RENAME_PARTIAL_FAILURE",
            &message,
            envelope(
                "RENAME_PARTIAL_FAILURE",
                message.clone(),
                serde_json::json!({
                    "committed_mems": committed_mems,
                    "failed_mem": failed_mem,
                    "failure_cause": failure_cause,
                }),
            ),
        ),
        E::DuplicateMem(name) => tool_error_with_payload(
            "DUPLICATE_MEM",
            &message,
            envelope(
                "DUPLICATE_MEM",
                message.clone(),
                serde_json::json!({ "name": name }),
            ),
        ),
        // Parse-after-write / parse / backend: typed code, free-form
        // detail string so callers can render the underlying message
        // without grepping it back out of the envelope's `message`.
        E::ParseAfterWrite(detail) => tool_error_with_payload(
            "PARSE_ERROR",
            &message,
            envelope(
                "PARSE_ERROR",
                message.clone(),
                serde_json::json!({ "detail": detail }),
            ),
        ),
        E::Parse(inner) => tool_error_with_payload(
            "PARSE_ERROR",
            &message,
            envelope(
                "PARSE_ERROR",
                message.clone(),
                serde_json::json!({ "detail": inner.to_string() }),
            ),
        ),
        E::Backend(inner) => tool_error_with_payload(
            "MEM_ERROR",
            &message,
            envelope(
                "MEM_ERROR",
                message.clone(),
                serde_json::json!({ "detail": inner.to_string() }),
            ),
        ),
        E::SearchUnavailable => tool_error_with_payload(
            "SEARCH_UNAVAILABLE_IN_WASM",
            &message,
            envelope(
                "SEARCH_UNAVAILABLE_IN_WASM",
                message.clone(),
                serde_json::json!({}),
            ),
        ),
        // Typed refusal when
        // `export_markdown` targets a mem whose active backend
        // doesn't support markdown regeneration.
        E::MarkdownExportUnsupportedBackend {
            mem,
            active_backend,
            supported_backends,
        } => tool_error_with_payload(
            "MARKDOWN_EXPORT_UNSUPPORTED_BACKEND",
            &message,
            envelope(
                "MARKDOWN_EXPORT_UNSUPPORTED_BACKEND",
                message.clone(),
                serde_json::json!({
                    "mem": mem,
                    "active_backend": active_backend,
                    "supported_backends": supported_backends,
                }),
            ),
        ),
        E::EmptyUpdate { id } => tool_error_with_payload(
            "EMPTY_UPDATE",
            &message,
            envelope(
                "EMPTY_UPDATE",
                message.clone(),
                serde_json::json!({
                    "id": id,
                    // The engine's own list, not a copy.
                    "recognised_keys":
                        memstead_base::engine::error::RECOGNISED_MUTATION_KEYS,
                }),
            ),
        ),
        E::InvalidChangesCursor { mem, since } | E::InvalidTimestampCursor { mem, since } => {
            tool_error_with_payload(
                "INVALID_CURSOR",
                &message,
                envelope(
                    "INVALID_CURSOR",
                    message.clone(),
                    serde_json::json!({ "mem": mem, "since": since }),
                ),
            )
        }
        // Review-mark diff on a markless mem — typed refusal so agents
        // never equate "no mark" with "no changes".
        E::ReviewMarkNotSet { mem } => tool_error_with_payload(
            "REVIEW_MARK_NOT_SET",
            &message,
            envelope(
                "REVIEW_MARK_NOT_SET",
                message.clone(),
                serde_json::json!({ "mem": mem }),
            ),
        ),
        // Malformed `anchors[]` element on create/update: typed
        // `INVALID_ANCHOR` with the wrapped anchor error's recovery detail
        // (offending field, bad value, allowed set). The whole mutation
        // refused and the entity was not written.
        E::InvalidAnchor(anchor_err) => tool_error_with_payload(
            memstead_base::anchor::INVALID_ANCHOR_CODE,
            &message,
            envelope(
                memstead_base::anchor::INVALID_ANCHOR_CODE,
                message.clone(),
                serde_json::Value::Object(anchor_err.detail().into_iter().collect()),
            ),
        ),
    }
}

/// Typed-envelope translator for `FullEngineError`. Delegates wrapped
/// base-engine errors to [`engine_err_unified`]; constructs the lifecycle-
/// specific envelopes (`MEM_PATH_NOT_ALLOWED`,
/// `MEM_REFERENCED_BY_POLICY`, `MEM_SCHEMA_NOT_ALLOWED`,
/// `CONFIG_ERROR`) here. The wire shape is
/// bit-identical to what `engine_err_unified` produced for the same
/// variants before the lifecycle variants moved off
/// `memstead_base::EngineError`; the move is pure plumbing.
fn full_engine_err_unified(
    e: memstead_base::FullEngineError,
    engine: &memstead_base::Engine,
) -> CallToolResult {
    use memstead_base::FullEngineError as PE;
    // The text-channel message uses the rich-prose renderer so lifecycle
    // refusals (MEM_PATH_NOT_ALLOWED, MEM_SCHEMA_NOT_ALLOWED,
    // MEM_REFERENCED_BY_POLICY) inline their full recovery payload
    // inline rather than relying on the structured channel for
    // recovery context.
    let message = e.prose_render();
    // Shared structured payload — computed before the match so the
    // lifecycle arms cannot drift from the CLI envelope, which lifts
    // the same `details()`.
    let shared_details = e.details();
    match e {
        // #55: thread the engine so the wrapped base-engine path enriches
        // not-found envelopes the same as every other call site.
        PE::Engine(inner) => engine_err_unified(inner, engine),
        PE::MemPathNotAllowed { .. } => tool_error_with_payload(
            "MEM_PATH_NOT_ALLOWED",
            &message,
            envelope("MEM_PATH_NOT_ALLOWED", message.clone(), shared_details),
        ),
        PE::InvalidMemName { name, reason } => tool_error_with_payload(
            "INVALID_MEM_NAME",
            &message,
            envelope(
                "INVALID_MEM_NAME",
                message.clone(),
                serde_json::json!({
                    "name": name,
                    "reason": reason,
                }),
            ),
        ),
        PE::MemSchemaNotAllowed {
            candidate,
            matched_pattern,
            requested_schema,
            allowed_schemas,
        } => tool_error_with_payload(
            "MEM_SCHEMA_NOT_ALLOWED",
            &message,
            envelope(
                "MEM_SCHEMA_NOT_ALLOWED",
                message.clone(),
                serde_json::json!({
                    "candidate": candidate,
                    "matched_pattern": matched_pattern,
                    "requested_schema": requested_schema,
                    "allowed_schemas": allowed_schemas,
                }),
            ),
        ),
        PE::MemReferencedByPolicy {
            name,
            referring_mems,
        } => tool_error_with_payload(
            "MEM_REFERENCED_BY_POLICY",
            &message,
            envelope(
                "MEM_REFERENCED_BY_POLICY",
                message.clone(),
                serde_json::json!({
                    "name": name,
                    "referring_mems": referring_mems,
                }),
            ),
        ),
        PE::ConfigAlreadyExists { path } => tool_error_with_payload(
            "CONFIG_ERROR",
            &message,
            envelope(
                "CONFIG_ERROR",
                message.clone(),
                serde_json::json!({
                    "path": path.display().to_string(),
                    "reason": "config_already_exists",
                }),
            ),
        ),
        PE::MemStorageResidueDetected {
            branch_ref,
            config_blob,
            entity_count,
        } => tool_error_with_payload(
            "MEM_STORAGE_RESIDUE_DETECTED",
            &message,
            envelope(
                "MEM_STORAGE_RESIDUE_DETECTED",
                message.clone(),
                serde_json::json!({
                    "branch_ref": branch_ref,
                    "config_blob": config_blob,
                    "entity_count": entity_count,
                    "recovery": ["reattach", "force_overwrite", "hard_cleanup_first"],
                }),
            ),
        ),
    }
}

/// Map a runtime [`memstead_base::runtime_validator::ValidationError`] to
/// the MCP wire envelope. Thin delegation to the shared
/// [`crate::error_envelopes::validation_envelope`] so the unified
/// engine's mutation handlers emit the same wire shape full's
/// filesystem-server already does.
fn unified_validation_envelope(
    err: memstead_base::runtime_validator::ValidationError,
) -> CallToolResult {
    crate::error_envelopes::validation_envelope(err)
}

/// Find entity IDs that end with the given suffix (slug or medium--slug).
/// Returns up to `max` suggestions for "did you mean?" messages.
///
/// Takes `&Store` directly so the helper composes against any
/// engine — `memstead_base::Engine::store()` exposes a `memstead_base::Store`.
fn suggest_similar(store: &memstead_base::Store, input: &str) -> Vec<String> {
    let needle = input.trim_start_matches("@memstead/");
    store
        .all_ids()
        .filter(|id| {
            let haystack = id.as_ref();
            // Match if the stored ID ends with the input (e.g. "mcp-server" matches "...specs--mcp-server")
            haystack.ends_with(needle)
                || haystack.ends_with(&format!("--{needle}"))
                // Also match if the slug portion contains the input
                || haystack.rsplit_once("--").is_some_and(|(_, slug)| slug.contains(needle))
        })
        .take(5)
        .map(|id| id.to_string())
        .collect()
}

/// Build a "not found" error with suggestions.
///
/// Routes through [`tool_error_with_payload`] so the text channel
/// carries the `ERROR [ENTITY_NOT_FOUND]: …` prefix and
/// `structured_content` carries the `{ code, message, details }`
/// envelope — matching every other not-found return on the surface.
/// Takes `&Store` so the helper works for both full and unified engines.
fn not_found_error(store: &memstead_base::Store, id: &EntityId) -> CallToolResult {
    let suggestions = suggest_similar(store, id.as_ref());
    let msg = if suggestions.is_empty() {
        format!("Entity not found: {id}")
    } else {
        format!(
            "Entity not found: \"{id}\". Did you mean: {}",
            suggestions.join(", ")
        )
    };
    tool_error_with_payload(
        "ENTITY_NOT_FOUND",
        &msg,
        envelope(
            "ENTITY_NOT_FOUND",
            msg.clone(),
            serde_json::json!({
                "id": id.as_ref(),
                "suggestions": suggestions,
            }),
        ),
    )
}

// ==========================================================================
// Tool implementations
// ==========================================================================

#[tool_router(router = tool_router_undescribed, vis = "pub(crate)")]
impl McpServer {
    // ----------------------------------------------------------------------
    // Read-only graph tools
    // ----------------------------------------------------------------------

    #[tool(
        name = "memstead_entity",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn memstead_entity(&self, Parameters(p): Parameters<EntityParams>) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.id) {
            return err;
        }
        let id = EntityId::canonical(&p.id);

        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        // Cross-mem forms take the FULL lazy-mount load before answering:
        // `include_relations` renders INCOMING edges, which can originate
        // in any mem, and `include_context` computes the workspace-global
        // community clustering — either one over a partial store is a
        // silently incomplete answer, the failure class the lazy-mount
        // observability contract forbids. Declared signals and labelling
        // are cross-mem forms too (an `in`-direction signal reads
        // incoming edges, a neighbour pair reads the counterpart record,
        // and the labelling view counts the cross-mem edges it excludes),
        // so a mem whose schema declares either also takes the full
        // load. The plain read against a schema declaring neither stays
        // scoped to the target mem (the reload below), preserving the
        // lazy win.
        let declares_cross_mem_serving = engine.schema_for(id.mem()).is_some_and(|s| {
            s.manifest.relationships.labelling.is_some()
                || s.types.values().any(|td| !td.signals.is_empty())
        });
        if p.include_relations.unwrap_or(false)
            || p.include_context.unwrap_or(false)
            || declares_cross_mem_serving
        {
            engine.ensure_mems_loaded(None);
        }
        let drift_warnings = engine.reload_if_stale(Some(id.mem()));
        // Drain the stashed structured notices: attached to the
        // response's `structured_content` below (and the markdown
        // `MEM_RELOADED` warning still rides the text channel).
        let (engine, mem_changed_notices) = engine.finish();
        let entity = match engine.get_entity(&id) {
            Some(e) => e.clone(),
            // Drift must survive the error path too: a sibling that
            // deleted X advanced this engine's head during the reload
            // above, so a bare `not_found_error` would consume the
            // drained notice and silently swallow the whole reload
            // window. Attach it on the success channel split.
            None => {
                // A quarantined mem's entities are deliberately not in
                // the store; refusing ENTITY_NOT_FOUND there would be
                // dishonest ("honest absence beats partial truth" —
                // the read names the quarantine, not a phantom miss).
                // Likewise a mem that left the roster: MEM_UNMOUNTED.
                if engine.quarantine_reason(id.mem()).is_some()
                    || engine.recently_unmounted(id.mem())
                {
                    let err = engine.unknown_mem_error(id.mem());
                    return attach_drift_to_error(
                        engine_err_unified(err, &engine),
                        &drift_warnings,
                        mem_changed_notices,
                    );
                }
                return attach_drift_to_error(
                    not_found_error(engine.store(), &id),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
        };
        let schema_anchor = mem_schema_ref_unified(&engine, id.mem());

        let sections_filter = p.sections.as_deref();
        // Declared aggregate signals — computed once, served on both
        // channels (frontmatter headline + `## Signals` append on the
        // text channel, `_signals` on the envelope). `None` for types
        // declaring none keeps both channels byte-identical.
        let computed_signals = engine.computed_signals(&entity);
        let computed_labelling = engine.computed_labelling(&entity);
        let mut md = render::render_entity_markdown_with_signals(
            &entity,
            sections_filter,
            computed_signals.as_deref(),
            computed_labelling.as_ref(),
        );

        if p.include_relations.unwrap_or(false) {
            let outgoing = engine.store().outgoing(&id).to_vec();
            let incoming = engine.store().incoming(&id).to_vec();
            md.push_str(&render::render_relations_markdown(
                id.as_ref(),
                &outgoing,
                &incoming,
            ));
        }

        if p.include_context.unwrap_or(false)
            && let Some(ctx) = engine.context(&id)
        {
            let cluster_id = ctx.community.clone().unwrap_or_else(|| "unknown".into());
            md.push_str(&render::render_community_context_section(&ctx, &cluster_id));
        }

        if let Some(ref s) = schema_anchor {
            inject_md_mem_schema(&mut md, s);
        }

        let mut extra_fm: Vec<(&str, &str)> = vec![("_hash", &entity.content_hash)];
        if let Some(ref s) = schema_anchor {
            extra_fm.push(("_mem_schema", s.as_str()));
        }

        // Structured envelope
        // rides alongside the chunked markdown text channel. Built
        // off the *unchunked* entity so consumers can branch on full
        // field shapes regardless of which chunk the text channel
        // ships; sections-filtering still applies so a narrowed read
        // narrows both channels. `_tokens` reflects the rendered body
        // (post-filter, post-opt-in); `_tokens_unfiltered_body`
        // surfaces when the filter dropped any sections, matching
        // the markdown renderer's signal that there is "more entity"
        // to read. Renamed from `_tokens_full` because the
        // previous name implied a monotonic relationship the opt-in
        // path can invert.
        let rendered_body_tokens = estimate_tokens(&md);
        let full_tokens = if sections_filter.is_some() {
            let full_body = render::render_entity_markdown(&entity, None);
            Some(estimate_tokens(&full_body))
        } else {
            None
        };
        // Origin now rides inside the shared envelope builder (the
        // structural fix for cold-start 0-8-0 F9/F13 — every surface
        // that composes an entity read carries it, not just this one).
        // Incoming edges join the envelope's `relationships` array when
        // the caller opted into relations, mirroring the text channel's
        // `## Relations` section (F15).
        let incoming_for_envelope = if p.include_relations.unwrap_or(false) {
            Some(engine.store().incoming(&id).to_vec())
        } else {
            None
        };
        let mut structured = render::build_entity_envelope(
            &entity,
            rendered_body_tokens,
            full_tokens,
            sections_filter,
            schema_anchor.as_deref(),
            engine.mem_origin_class(id.mem()),
            engine.store().outgoing(&id),
            incoming_for_envelope.as_deref(),
            computed_signals.as_deref(),
            computed_labelling.as_ref(),
        );
        if let Some(obj) = structured.as_object_mut() {
            // Authoring provenance carried in the installed archive. Emitted
            // only when the mem ships a provenance payload; `history`
            // makes the "full commit history not shipped" decision
            // observable, and `rationale` is `null` when this entity was
            // authored without a note — absence reported as absence, never
            // a fabricated value. A mem with no payload omits the field.
            if let Some(prov) = engine.archive_provenance_for(id.mem()) {
                let mut block = serde_json::Map::new();
                block.insert("history".into(), serde_json::json!(prov.history));
                let rec = prov.entity(id.path());
                block.insert(
                    "rationale".into(),
                    rec.and_then(|r| r.rationale.as_ref())
                        .map(|s| serde_json::json!(s))
                        .unwrap_or(serde_json::Value::Null),
                );
                if let Some(r) = rec {
                    if let Some(kind) = &r.kind {
                        block.insert("kind".into(), serde_json::json!(kind));
                    }
                    if let Some(ts) = &r.timestamp {
                        block.insert("timestamp".into(), serde_json::json!(ts));
                    }
                    if let Some(actor) = &r.actor {
                        block.insert("actor".into(), serde_json::json!(actor));
                    }
                }
                obj.insert("provenance".into(), serde_json::Value::Object(block));
            }
            // Mutation provenance, opt-in:
            // created-by / last-modified-by with actor, client,
            // declared role, and timestamp — derived from the
            // append-only mutation record, which no verb can edit.
            // Distinct key from the archive-provenance block above
            // (that one describes an installed archive's authoring
            // payload). Default responses are byte-unchanged; on a
            // mount whose seam records no history (archives) the
            // block states unavailability instead of fabricating.
            if p.include_provenance.unwrap_or(false) {
                let block = match engine.entity_provenance(id.mem(), id.as_ref()) {
                    Ok(prov) => serde_json::to_value(&prov).unwrap_or(serde_json::Value::Null),
                    Err(e) => serde_json::json!({
                        "unavailable": e.to_string(),
                    }),
                };
                obj.insert("mutation_provenance".into(), block);
            }
            // Provenance anchors. Additive, emitted only when the
            // entity has anchors so a reader that predates anchors is unaffected. Carries
            // the stored anchor records plus their class/grain composition
            // (derived inputs; tree-grain fan-out on its own axis) and, for a
            // path-medium mem, each anchor's live resolution `state`
            // (resolves / drifted / recheck / orphaned — additive per-anchor
            // field). A present hash-bearing anchor adjudicates its recorded
            // prepared-content hash against the observed one, so `drifted` is
            // deterministic on a stable medium; `state` is absent when the
            // source is unobserved (a `url` grain, or an `entity` grain whose
            // mem is not mounted), never fabricated.
            // An unreadable sidecar is a condition on the read, never an
            // absent `anchors` key: the entity's anchors are unknown.
            if let Some(why) = engine.anchors_sidecar_error(id.mem()) {
                obj.insert(
                    "anchors_sidecar_error".into(),
                    serde_json::json!({
                        "code": "ANCHORS_SIDECAR_UNREADABLE",
                        "mem": id.mem(),
                        "reason": &why,
                    }),
                );
                md.push_str(&format!(
                    "\n\n> **ANCHORS_SIDECAR_UNREADABLE** — mem `{}`: {why}. This entity's \
                     provenance anchors are unknown, not absent.\n",
                    id.mem()
                ));
            }
            let resolved = engine.entity_anchors_resolved(&id);
            if !resolved.is_empty() {
                let anchors: Vec<memstead_base::anchor::Anchor> =
                    resolved.iter().map(|r| r.anchor.clone()).collect();
                let composition = memstead_base::anchor::compose_entity_anchors(&anchors);
                obj.insert(
                    "anchors".into(),
                    serde_json::to_value(&resolved).unwrap_or(serde_json::Value::Null),
                );
                obj.insert(
                    "anchor_composition".into(),
                    serde_json::to_value(&composition).unwrap_or(serde_json::Value::Null),
                );
            }
        }

        let budget = p.token_budget.unwrap_or(self.token_budget);
        attach_mem_changed_to_result(
            match apply_chunking(&md, budget, p.chunk, &extra_fm) {
                Ok(result) => md_with_structured(
                    prepend_drift_warnings_md(result, &drift_warnings),
                    structured,
                ),
                Err(e) => prepend_drift_warnings_to_result_text(
                    tool_error("INVALID_INPUT", &e),
                    &drift_warnings,
                ),
            },
            mem_changed_notices,
        )
    }

    #[tool(
        name = "memstead_search",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn memstead_search(&self, Parameters(p): Parameters<SearchParams>) -> CallToolResult {
        let filters = p.filters.clone().unwrap_or_default();

        // Telemetry: one line per invocation with flags only — no
        // query strings, no entity content. Enables Tier 2 (regex / fuzzy /
        // nested / per-term field) to be decided from real usage data.
        let q = p.query.as_ref();
        tracing::info!(
            any_term_count = q.map(|q| q.any.len()).unwrap_or(0),
            has_phrase = q.is_some_and(|q| q.phrase.is_some()),
            has_not = q.is_some_and(|q| !q.not.is_empty()),
            has_field = q.is_some_and(|q| q.field.is_some()),
            has_expand = p.expand_via.as_ref().is_some_and(|v| !v.is_empty()),
            mem_scope = if p.mem.is_some() { "one" } else { "all" },
            "memstead_search invoked"
        );

        // Snapshot the mem filter for drift detection before the scope
        // construction below moves `p.mem`.
        let mem_filter = p.mem.clone();

        let scope = SearchScope {
            query: p.query,
            mem: p.mem,
            entity_type: p.entity_type,
            limit: p.limit,
            offset: p.offset,
            filters,
            // Thread the
            // agent's `range_filters` input into the engine arg.
            // The engine's `collect_range_filter_warnings` was
            // ready-but-unreachable before this wiring.
            range_filters: p.range_filters.unwrap_or_default(),
            edge_type: p.edge_type,
            related_to: p.related_to.map(EntityId),
            depth: p.depth,
            expand_via: p.expand_via,
            expand_depth: p.expand_depth,
            direction: p.direction.unwrap_or_default(),
            stub: p.stub,
            token_budget: p.token_budget,
        };
        let offset = scope.offset.unwrap_or(0);

        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        // Graph-walking forms cross mem boundaries — `related_to` is a
        // BFS over the whole graph and `expand_via` follows edges
        // wherever they lead — so they take the full lazy-mount load
        // rather than walking a partial store. A plain mem-filtered
        // text search stays scoped: its answer lives in one mem.
        if scope.related_to.is_some() || scope.expand_via.is_some() {
            engine.ensure_mems_loaded(None);
        }
        let drift_warnings = engine.reload_if_stale(mem_filter.as_deref());
        let (engine, mem_changed_notices) = engine.finish();
        let result = match engine.search(&scope) {
            Ok(r) => r,
            Err(e) => {
                return attach_drift_to_error(
                    engine_err_unified(e, &engine),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
        };

        let md = render::render_search_markdown(&result, offset);
        // Structured envelope
        // on `structured_content`, rendered markdown on the text
        // channel. Search results have a useful human-readable
        // canonical form (the rendered prose with score lines) and
        // a typed branching shape — both ship in one call.
        // Every hit carries `origin` (first-party / third-party), stamped
        // by the shared envelope builder so the CLI's `--json` and this
        // `structured_content` agree key for key.
        let envelope =
            render::build_search_envelope(&result, offset, &|m| engine.mem_origin_class(m));
        let structured = serde_json::to_value(&envelope).unwrap_or(serde_json::Value::Null);
        attach_mem_changed_to_result(
            md_with_structured(prepend_drift_warnings_md(md, &drift_warnings), structured),
            mem_changed_notices,
        )
    }

    // ----------------------------------------------------------------------
    // Community detection + schema tools
    // ----------------------------------------------------------------------

    #[tool(
        name = "memstead_overview",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        meta = always_load_meta()
    )]
    fn memstead_overview(&self, Parameters(p): Parameters<OverviewParams>) -> CallToolResult {
        // The schemas catalogue includes rule-referenced unpinned
        // schemas via `engine.workspace_schemas()`.
        let unified = self.unified_engine();
        self.memstead_overview_unified(p, unified.clone())
    }

    /// Unified-engine path for [`Self::memstead_overview`]. Body lifted to
    /// [`memstead_base::overview::compose_overview`] so the full CLI
    /// surfaces the same rich-content output via the same composer.
    /// This wrapper handles drift-warning collection, error-envelope
    /// mapping, and response-cap chunking; the composer produces the
    /// markdown body + warnings + extra-frontmatter.
    fn memstead_overview_unified(
        &self,
        p: OverviewParams,
        unified: Arc<Mutex<memstead_base::Engine>>,
    ) -> CallToolResult {
        let mut engine = crate::lock_engine!(unified);
        // Overview's community detection is workspace-global BY CONTRACT
        // (a `mem` filter only scopes which clusters are reported, never
        // the partition), so even a mem-scoped overview takes the full
        // lazy-mount load — cluster ids computed over a partial store
        // would be a different partition presented as the global one.
        engine.ensure_mems_loaded(None);
        let drift_warnings = engine.reload_if_stale(p.mem.as_deref());

        let include = p.include.clone().unwrap_or_default();
        let args = memstead_base::overview::OverviewArgs {
            include: &include,
            mem: p.mem.as_deref(),
            rebuild: p.rebuild.unwrap_or(false) && p.chunk.unwrap_or(1) <= 1,
            token_budget: p
                .token_budget
                .unwrap_or(memstead_base::overview::DEFAULT_OVERVIEW_BUDGET),
            operator_mode: self.operator_mode,
            // The section is a truthful function of which tools this
            // server exposes: an embedder (or a workspace's
            // `[mcp].disabled_tools`) that withholds both lifecycle tools
            // gets no section naming them.
            suppress_lifecycle: self.disabled_tools.contains("memstead_mem_create")
                && self.disabled_tools.contains("memstead_mem_delete"),
        };

        let out = match memstead_base::overview::compose_overview(
            &mut engine,
            args,
            memstead_base::overview::Surface::Mcp,
        ) {
            Ok(o) => o,
            Err(memstead_base::overview::ComposeOverviewError::InvalidIncludeKeySchemaTypes) => {
                let msg = "include key 'schema_types' was removed; \
                           call memstead_schema(name=...) for full schema bodies."
                    .to_string();
                return tool_error_with_payload(
                    "INVALID_INPUT",
                    &msg,
                    envelope(
                        "INVALID_INPUT",
                        msg.clone(),
                        serde_json::json!({ "message": msg }),
                    ),
                );
            }
            Err(memstead_base::overview::ComposeOverviewError::MemQuarantined(name)) => {
                let err = engine.unknown_mem_error(&name);
                return engine_err_unified(err, &engine);
            }
            Err(memstead_base::overview::ComposeOverviewError::UnknownMem {
                name,
                writable_mems,
            }) => {
                let msg = format!(
                    "unknown mem: \"{name}\". Writable mems: [{}]",
                    writable_mems.join(", ")
                );
                return tool_error_with_payload(
                    "UNKNOWN_MEM",
                    &msg,
                    envelope(
                        "UNKNOWN_MEM",
                        msg.clone(),
                        serde_json::json!({
                            "name": name,
                            "writable_mems": writable_mems,
                        }),
                    ),
                );
            }
        };

        // Promote the composer's extra-frontmatter into the
        // `apply_chunking` shape (`Vec<(&str, &str)>`). The slots stay
        // alive through `out` for the duration of this call.
        let extra_fm: Vec<(&str, &str)> = out
            .extra_frontmatter
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        match apply_chunking(&out.markdown, self.token_budget, p.chunk, &extra_fm) {
            Ok(r) => md_response(prepend_drift_warnings_md(r, &drift_warnings)),
            Err(e) => tool_error("INVALID_INPUT", &e),
        }
    }

    #[tool(
        name = "memstead_schema",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn memstead_schema(&self, Parameters(p): Parameters<SchemaParams>) -> CallToolResult {
        // The unified engine exposes `schemas()` as a HashMap keyed by
        // mem name (one schema per mem per V1). suggest_name is
        // not available on the unified surface (the per-mem HashMap
        // has no fuzzy index); not-found errors carry an empty
        // suggestions list — wire shape stays consistent.
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        let drift_warnings = engine.reload_if_stale(None);
        let (engine, mem_changed_notices) = engine.finish();

        // Resolve the effective schema name. Accept exactly one of
        // `name` (canonical) or `mem` (mount-roster lookup); the
        // pair `(Some, Some)` and `(None, None)` are typed input
        // errors so an agent that misreads the API gets a precise
        // failure rather than silent fallback. `mem` resolves
        // through the same cascade as `name` once the engine maps
        // the mem to its pinned `schema_ref`.
        let effective_name: String = match (p.name.as_deref(), p.mem.as_deref()) {
            (Some(_), Some(_)) => {
                let msg =
                    "memstead_schema accepts exactly one of `name` or `mem`, not both.".to_string();
                return attach_drift_to_error(
                    tool_error_with_payload(
                        "INVALID_INPUT",
                        &msg,
                        envelope(
                            "INVALID_INPUT",
                            msg.clone(),
                            serde_json::json!({ "message": msg }),
                        ),
                    ),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
            (None, None) => {
                let msg = "memstead_schema requires either `name` or `mem`.".to_string();
                return attach_drift_to_error(
                    tool_error_with_payload(
                        "INVALID_INPUT",
                        &msg,
                        envelope(
                            "INVALID_INPUT",
                            msg.clone(),
                            serde_json::json!({ "message": msg }),
                        ),
                    ),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
            (Some(name), None) => name.to_string(),
            (None, Some(mem)) => match engine.mount(mem) {
                Some(m) => m.schema.as_ref().map(|s| s.to_string()).unwrap_or_default(),
                None if engine.quarantine_reason(mem).is_some() => {
                    let err = engine.unknown_mem_error(mem);
                    return attach_drift_to_error(
                        engine_err_unified(err, &engine),
                        &drift_warnings,
                        mem_changed_notices,
                    );
                }
                None => {
                    let known_mems: Vec<String> =
                        engine.mounts().iter().map(|m| m.mem.clone()).collect();
                    let msg = format!("unknown mem: \"{mem}\"");
                    return attach_drift_to_error(
                        tool_error_with_payload(
                            "UNKNOWN_MEM",
                            &msg,
                            envelope(
                                "UNKNOWN_MEM",
                                msg.clone(),
                                serde_json::json!({
                                    "name": mem,
                                    "known_mems": known_mems,
                                }),
                            ),
                        ),
                        &drift_warnings,
                        mem_changed_notices,
                    );
                }
            },
        };

        // Lookup: name@version path uses parsed pin; bare-name path
        // picks the first matching schema by name. Cascade covers
        // mem-pinned, workspace-loaded, and embedded built-in
        // catalogues so any pin `memstead_mem_create` would accept also
        // resolves through `memstead_schema` — agents reading
        // `memstead_overview`'s lifecycle namespaces can introspect their
        // schemas without first creating a mem.
        let schema_arc: Option<std::sync::Arc<memstead_schema::Schema>> =
            if effective_name.contains('@') {
                match effective_name.parse::<memstead_schema::SchemaRef>() {
                    Ok(parsed) => find_schema_unified(&engine, &parsed).cloned(),
                    Err(_) => None,
                }
            } else {
                find_schema_by_name(&engine, &effective_name).cloned()
            };
        let schema = match schema_arc {
            Some(s) => s,
            None => {
                let msg = format!("schema not found: \"{effective_name}\"");
                return attach_drift_to_error(
                    tool_error_with_payload(
                        "ENTITY_NOT_FOUND",
                        &msg,
                        envelope(
                            "ENTITY_NOT_FOUND",
                            msg.clone(),
                            serde_json::json!({
                                "id": effective_name,
                                "suggestions": Vec::<String>::new(),
                            }),
                        ),
                    ),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
        };

        // `used_by` — every writable mem whose pinned schema
        // resolves to this one. Iterate mounts(), compare each
        // mount.schema with the matched schema's canonical pin.
        let canon = format!("{}@{}", schema.manifest.name, schema.version);
        let mut used_by: Vec<String> = engine
            .mounts()
            .iter()
            .filter(|m| m.schema.as_ref().map(|s| s.to_string()).as_deref() == Some(canon.as_str()))
            .map(|m| m.mem.clone())
            .collect();
        used_by.sort();

        // Resolve the optional `verbosity` toggle. Absent → lite: a fresh
        // session following the schema-discovery contract pays the
        // skeleton price (~7 KB), not the full-prose price (~52 KB); the
        // full body stays one explicit `verbosity: "full"` away. An
        // unrecognized value is a typed `INVALID_INPUT` naming the bad
        // value rather than a silent fallback to full/lite — the same
        // anti-silent-no-op principle the write-path plans enforce.
        let verbosity = match p.verbosity.as_deref() {
            None => render::SchemaVerbosity::Lite,
            Some(v) => match render::SchemaVerbosity::from_wire(v) {
                Some(sv) => sv,
                None => {
                    let msg = format!("unknown verbosity: \"{v}\" — expected \"full\" or \"lite\"");
                    return attach_drift_to_error(
                        tool_error_with_payload(
                            "INVALID_INPUT",
                            &msg,
                            envelope(
                                "INVALID_INPUT",
                                msg.clone(),
                                serde_json::json!({
                                    "value": v,
                                    "allowed": ["full", "lite"],
                                }),
                            ),
                        ),
                        &drift_warnings,
                        mem_changed_notices,
                    );
                }
            },
        };
        // Trust origin governs de-framing: a third-party schema is served
        // structural-only regardless of the requested `verbosity` (the
        // prose-instruction fields never reach the agent as instructions).
        let origin = engine.schema_origin(&schema);
        // Serving-shape controls (an earlier plana): `types`
        // scopes the per-type prose; the token budget guards the
        // unscoped full reply with a visible degrade instead of a
        // response-cap overflow.
        let payload = match render::build_schema_payload_scoped(
            &schema,
            used_by,
            verbosity,
            origin,
            p.types.as_deref(),
            Some(p.token_budget.unwrap_or(render::DEFAULT_SCHEMA_FULL_BUDGET)),
        ) {
            Ok(v) => v,
            Err(unknown) => {
                let msg = format!(
                    "unknown entity type(s) in `types`: [{}] — valid types: [{}]",
                    unknown.unknown.join(", "),
                    unknown.known.join(", ")
                );
                return attach_drift_to_error(
                    tool_error_with_payload(
                        "UNKNOWN_ENTITY_TYPE",
                        &msg,
                        envelope(
                            "UNKNOWN_ENTITY_TYPE",
                            msg.clone(),
                            serde_json::json!({
                                "unknown": unknown.unknown,
                                "known_types": unknown.known,
                            }),
                        ),
                    ),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
        };
        let mut res = json_response(&payload);
        for w in &drift_warnings {
            res = append_warning_hint(res, w);
        }
        attach_mem_changed_to_result(res, mem_changed_notices)
    }

    // ----------------------------------------------------------------------
    // Write tools
    // ----------------------------------------------------------------------

    #[tool(
        name = "memstead_create",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    fn memstead_create(&self, Parameters(p): Parameters<CreateParams>) -> CallToolResult {
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let mem = self.resolve_mem(p.mem.as_deref());
        let dry_run = p.dry_run.unwrap_or(false);
        let role = match self.resolve_role(p.role.as_deref()) {
            Ok(r) => r,
            Err(resp) => return *resp,
        };
        let identity = match self.resolve_identity(p.identity.as_deref()) {
            Ok(i) => i,
            Err(resp) => return *resp,
        };

        // Wire JSON matches the `CreateResult` contract.
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        engine.set_role(role);
        engine.set_identity(identity);
        let relations: Vec<memstead_base::ops::RelateArg> = p
            .relations
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|r| memstead_base::ops::RelateArg {
                target: EntityId(r.target),
                rel_type: r.rel_type,
                description: r.description,
            })
            .collect();
        let anchors: Vec<memstead_base::anchor::AnchorInput> = p
            .anchors
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.into_engine())
            .collect();
        let args = memstead_base::CreateEntityArgs {
            mem: mem.clone(),
            title: p.title.clone(),
            entity_type: p.entity_type.clone(),
            sections: p.sections.unwrap_or_default(),
            metadata: p.metadata.unwrap_or_default(),
            relations,
            anchors,
            dry_run,
        };
        let client = self.client.get().cloned();
        match engine.create_entity(args, Actor::Agent, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                // Skip empty `incoming` / `None` `incoming_count`
                // manually to match full's
                // `#[serde(skip_serializing_if=...)]`.
                let mut body = serde_json::json!({
                    "id": outcome.id.to_string(),
                    "title": outcome.title,
                    "mem": outcome.mem,
                    "file_path": outcome.file_path,
                    "created_date": outcome.created_date,
                    "_hash": outcome.content_hash,
                    "write_id": outcome.write_id,
                    "warnings": outcome.warnings,
                    "type_guidance": outcome.type_guidance,
                });
                if let Some(count) = outcome.incoming_count {
                    body["incoming_count"] = serde_json::json!(count);
                }
                if !outcome.incoming.is_empty() {
                    body["incoming"] =
                        serde_json::to_value(&outcome.incoming).unwrap_or(serde_json::Value::Null);
                }
                if !outcome.relations_declared.is_empty() {
                    body["relations_declared"] = serde_json::to_value(&outcome.relations_declared)
                        .unwrap_or(serde_json::Value::Null);
                }
                attach_durability(&mut body, &engine, &outcome.mem);
                let (engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                let res = json_response(&body);
                match mem_schema_ref_unified(&engine, &mem) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => {
                // The notice already rode `structured_content` here;
                // the text channel lacked the `MEM_RELOADED` line a
                // successful response carries. A mutation reloads inside
                // the engine, so reconstruct the warning from the
                // drained notices to match the success channel split —
                // collision (`HASH_MISMATCH`) is the path drift matters
                // most, since it lands on the very entity being written.
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    #[tool(
        name = "memstead_update",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    fn memstead_update(&self, Parameters(p): Parameters<UpdateParams>) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.id) {
            return err;
        }
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let id = EntityId::canonical(&p.id);
        let dry_run = p.dry_run.unwrap_or(false);
        let role = match self.resolve_role(p.role.as_deref()) {
            Ok(r) => r,
            Err(resp) => return *resp,
        };
        let identity = match self.resolve_identity(p.identity.as_deref()) {
            Ok(i) => i,
            Err(resp) => return *resp,
        };

        // Wire JSON matches the `UpdateResult` contract.
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        engine.set_role(role);
        engine.set_identity(identity);
        // The mem the anchors ride on is the resolved id's mem: a bare
        // slug the engine resolves to one mem must not stage its
        // anchors under the empty mem name. The verb itself receives
        // the id as given and announces the resolution.
        let mem_for_anchor = match engine.resolve_entity_id(&id) {
            Ok((resolved, _)) => resolved.mem().to_string(),
            Err(e) => return engine_err_unified(e, &engine),
        };
        let patch_sections: indexmap::IndexMap<String, Vec<memstead_base::ops::PatchArg>> = p
            .patch_sections
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.into_vec()
                        .into_iter()
                        .map(|v| memstead_base::ops::PatchArg {
                            old: v.old,
                            new: v.new,
                            all: v.all.unwrap_or(false),
                        })
                        .collect(),
                )
            })
            .collect();
        let declare_relations: Vec<memstead_base::ops::RelateArg> = p
            .declare_relations
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|r| memstead_base::ops::RelateArg {
                target: EntityId(r.target),
                rel_type: r.rel_type,
                description: r.description,
            })
            .collect();
        let anchors: Vec<memstead_base::anchor::AnchorInput> = p
            .anchors
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.into_engine())
            .collect();
        let anchors_unset: Vec<memstead_base::anchor::AnchorUnsetInput> = p
            .anchors_unset
            .unwrap_or_default()
            .into_iter()
            .map(|u| u.into_engine())
            .collect();
        let mut args = memstead_base::UpdateEntityArgs {
            anchors,
            anchors_unset,
            id: id.clone(),
            expected_hash: p.expected_hash.clone(),
            sections: p.sections.unwrap_or_default(),
            append_sections: p.append_sections.unwrap_or_default(),
            patch_sections,
            sections_unset: p.sections_unset.clone().unwrap_or_default(),
            metadata: p.metadata.unwrap_or_default(),
            metadata_unset: p.metadata_unset.unwrap_or_default(),
            dry_run,
            declare_relations,
            relations_unset: p
                .relations_unset
                .unwrap_or_default()
                .into_iter()
                .map(|r| memstead_base::ops::RelationUnsetArg {
                    rel_type: r.rel_type,
                    target: EntityId(r.target),
                })
                .collect(),
        };
        // The compare-and-swap gate, enforced HERE rather than by the engine
        // core, which has always checked only when a caller supplies a token
        // (consistency-sweep 03/04). Content-changing updates still demand it;
        // an anchors-only update does not, because the sidecar is outside
        // `_hash` and the token would compare a value the write cannot move.
        // `changes_content` is the engine's own predicate, so this surface and
        // the CLI cannot come to disagree about whether a write is safe.
        // An EMPTY token is no token on an anchors-only write, where the CLI
        // already discards it: leaving it Some("") sent it to the engine,
        // which compared "" against a real hash and refused HASH_MISMATCH for
        // a write the CLI accepted.
        //
        // Narrowed to that shape on purpose. Normalizing unconditionally made
        // this gate fire BEFORE the engine's existence and stub gates, so an
        // empty token on a stub reported a missing hash instead of
        // `STUB_NOT_UPDATABLE`, which is the more specific and more actionable
        // refusal. A content-changing payload therefore keeps its empty token
        // and keeps the engine's own ordering.
        if !args.changes_content() {
            args.expected_hash = args.expected_hash.filter(|h| !h.is_empty());
        }
        if args.expected_hash.is_none() && !args.dry_run && args.changes_content() {
            let message = format!(
                "`expected_hash` is required for an update that changes content. Read `{id}` \
                 first and pass its `_hash`, or pass `dry_run: true` to preview. Only an \
                 anchors-only update (`anchors` / `anchors_unset` and nothing else) may omit \
                 it, because anchors are outside the content hash."
            );
            return tool_error_with_payload(
                "EXPECTED_HASH_REQUIRED",
                &message,
                envelope(
                    "EXPECTED_HASH_REQUIRED",
                    message.clone(),
                    serde_json::json!({ "field": "expected_hash", "id": id.to_string() }),
                ),
            );
        }
        let client = self.client.get().cloned();
        match engine.update_entity(args, Actor::Agent, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                // ModifiedSections / ModifiedMetadata serialise
                // with `#[serde(skip_serializing_if = "Vec::is_empty")]`
                // on each inner vec — matching full's UpdateResult
                // wire shape.
                let mut body = serde_json::json!({
                    "id": outcome.id.to_string(),
                    "title": outcome.title,
                    "modified_sections": outcome.modified_sections,
                    "modified_metadata": outcome.modified_metadata,
                    "modified_date": outcome.modified_date,
                    "_hash": outcome.content_hash,
                    "write_id": outcome.write_id,
                    "warnings": outcome.warnings,
                    // Orphan-stub GC: removing a body wiki-link that was
                    // a stub target's last referrer GC's the stub and
                    // lists it here. Always present (empty array when
                    // nothing orphaned), matching the relate-remove
                    // always-emit shape so agents branch uniformly on
                    // the field rather than its presence.
                    "orphan_stubs_removed": outcome
                        .orphan_stubs_removed
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>(),
                });
                // Add `prospective_hash` only on the dry_run path
                // (matches full's `#[serde(skip_serializing_if = "Option::is_none")]`).
                if let Some(hash) = outcome.prospective_hash {
                    body["prospective_hash"] = serde_json::json!(hash);
                }
                // Present only when the update carried anchors or unsets:
                // whether the sidecar changed (a restated row writes
                // nothing and says so; an earlier plan).
                if let Some(changed) = outcome.anchors_changed {
                    body["anchors_changed"] = serde_json::json!(changed);
                }
                // Surface `relations_declared` when the agent used
                // `declare_relations`. Always-present-when-non-empty
                // wire shape so consumers branch on `.len()` rather
                // than on key presence; `serde(skip_serializing_if =
                // "Vec::is_empty")` on the outcome keeps the
                // no-batch case bytes-identical to pre-feature.
                if !outcome.relations_declared.is_empty() {
                    body["relations_declared"] = serde_json::to_value(&outcome.relations_declared)
                        .unwrap_or(serde_json::Value::Null);
                }
                attach_durability(&mut body, &engine, outcome.id.mem());
                let (engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                let res = json_response(&body);
                match mem_schema_ref_unified(&engine, &mem_for_anchor) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => {
                // The notice already rode `structured_content` here;
                // the text channel lacked the `MEM_RELOADED` line a
                // successful response carries. A mutation reloads inside
                // the engine, so reconstruct the warning from the
                // drained notices to match the success channel split —
                // collision (`HASH_MISMATCH`) is the path drift matters
                // most, since it lands on the very entity being written.
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    #[tool(
        name = "memstead_relate",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn memstead_relate(&self, Parameters(p): Parameters<RelateParams>) -> CallToolResult {
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let role = match self.resolve_role(p.role.as_deref()) {
            Ok(r) => r,
            Err(resp) => return *resp,
        };
        let identity = match self.resolve_identity(p.identity.as_deref()) {
            Ok(i) => i,
            Err(resp) => return *resp,
        };
        if p.relations.is_empty() {
            let msg = "relations must carry at least one operation";
            return tool_error_with_payload(
                "INVALID_INPUT",
                msg,
                envelope(
                    "INVALID_INPUT",
                    msg.to_string(),
                    serde_json::json!({ "message": msg }),
                ),
            );
        }

        // The whole list is one engine batch: all-or-nothing in one
        // commit per touched mem, in-order validation, report-all
        // refusals. A list of one routes through the same path and
        // carries the same per-entry body today's single call did.
        let ops: Vec<(memstead_base::RelateEntityArgs, Option<String>)> = p
            .relations
            .iter()
            .map(|op| {
                (
                    memstead_base::RelateEntityArgs {
                        source: EntityId::canonical(&op.from),
                        target: EntityId::canonical(&op.to),
                        rel_type: op.rel_type.clone(),
                        remove: op.remove.unwrap_or(false),
                        expected_hash: None,
                        description: op.description.clone(),
                        dry_run: p.dry_run.unwrap_or(false),
                    },
                    p.note.clone(),
                )
            })
            .collect();
        let mem_for_anchor = ops[0].0.source.mem().to_string();

        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        let client = self.client.get().cloned();
        engine.set_role(role);
        engine.set_identity(identity);

        // A list of one routes through the single-op engine path so it
        // behaves byte-identically to the historical single call —
        // full error enrichment (recovery payloads, message text),
        // full warning parity — wrapped in the same plural envelope
        // larger lists produce.
        if p.relations.len() == 1 {
            let (args, note) = {
                let mut it = ops.into_iter();
                it.next().expect("len checked above")
            };
            return match engine.relate_entity(args, Actor::Agent, client.as_ref(), note.as_deref())
            {
                Ok(outcome) => {
                    let action = match outcome.action {
                        memstead_base::RelateAction::Added => "added",
                        memstead_base::RelateAction::Removed => "removed",
                        memstead_base::RelateAction::NoOpAlreadyPresent
                        | memstead_base::RelateAction::NoOpAbsent => "noop",
                    };
                    let mut body = serde_json::json!({
                        "results": [{
                            "from": outcome.from.to_string(),
                            "to": outcome.to.to_string(),
                            "rel_type": outcome.rel_type,
                            "action": action,
                            "source": outcome.source,
                            "_hash": outcome.content_hash,
                        }],
                        "write_id": outcome.write_id,
                        "warnings": outcome.warnings,
                        "orphan_stubs_removed": outcome
                            .orphan_stubs_removed
                            .iter()
                            .map(|i| i.to_string())
                            .collect::<Vec<_>>(),
                    });
                    attach_durability(&mut body, &engine, outcome.from.mem());
                    let (engine, finished_notices) = engine.finish();
                    attach_mem_changed(&mut body, finished_notices);
                    let res = json_response(&body);
                    match mem_schema_ref_unified(&engine, &mem_for_anchor) {
                        Some(sr) => with_mem_schema_anchor(res, &sr),
                        None => res,
                    }
                }
                Err(e) => {
                    let (engine, notices) = engine.finish();
                    let warnings = notices_as_reload_warnings(&notices);
                    attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
                }
            };
        }

        // Snapshot which targets are absent pre-call so applied
        // auto-stubs can surface the same AUTO_STUB_CREATED warning
        // the single call emitted.
        let absent_targets: std::collections::HashSet<String> = p
            .relations
            .iter()
            .filter(|op| !op.remove.unwrap_or(false))
            .map(|op| EntityId::canonical(&op.to))
            .filter(|to| engine.store().get(to).is_none())
            .map(|to| to.to_string())
            .collect();
        let dry_run = p.dry_run.unwrap_or(false);
        let result = match engine.batch_relate(ops, Actor::Agent, client.as_ref(), dry_run) {
            Ok(r) => r,
            Err(e) => {
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                return attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices);
            }
        };

        if !result.applied {
            // Report-all refusal: nothing committed; every failing
            // entry ships its typed envelope. A list of one surfaces
            // its single entry's own code top-level so error-branching
            // callers keep working; larger lists wrap under
            // BATCH_REFUSED with per-entry envelopes in
            // `details.entries`.
            let entries: Vec<serde_json::Value> = result
                .results
                .iter()
                .zip(p.relations.iter())
                .enumerate()
                .map(|(i, (entry, op))| {
                    let mut e = serde_json::json!({
                        "index": i,
                        "from": op.from,
                        "to": op.to,
                        "rel_type": op.rel_type,
                        "action": entry.action,
                    });
                    if let Some(err) = &entry.error {
                        e["code"] = serde_json::json!(err.code);
                        e["message"] = serde_json::json!(err.message);
                        e["details"] = err.details.clone();
                    }
                    e
                })
                .collect();
            let (_engine, notices) = engine.finish();
            let drift = notices_as_reload_warnings(&notices);
            let msg = format!(
                "batch refused — {} of {} operation(s) failed, nothing committed",
                result.failed,
                p.relations.len(),
            );
            let payload = envelope(
                "BATCH_REFUSED",
                msg.clone(),
                serde_json::json!({
                    "entries": entries,
                    "failed": result.failed,
                    "errors_suppressed": result.errors_suppressed,
                }),
            );
            return attach_drift_to_error(
                tool_error_with_payload("BATCH_REFUSED", &msg, payload),
                &drift,
                notices,
            );
        }

        // Applied: enrich every entry with the same fields today's
        // single response carried — canonical rel_type comes back via
        // the store edge, `source` labels body_link vs explicit, and
        // `_hash` is the source entity's post-commit hash (the next
        // valid expected_hash for that entity). Noop entries
        // additionally synthesize the typed no-op warning the single
        // call emitted (DUPLICATE_RELATIONSHIP / NO_SUCH_RELATIONSHIP).
        let mut warnings: Vec<memstead_base::ops::WarningHint> = Vec::new();
        let entries: Vec<serde_json::Value> = result
            .results
            .iter()
            .zip(p.relations.iter())
            .map(|(entry, op)| {
                let from = EntityId::canonical(&op.from);
                let to = EntityId::canonical(&op.to);
                let canonical_type = op.rel_type.to_uppercase();
                if entry.action == "noop" {
                    if op.remove.unwrap_or(false) {
                        warnings.push(memstead_base::ops::WarningHint::NoSuchRelationship {
                            rel_type: canonical_type.clone(),
                            from: from.clone(),
                            to: to.clone(),
                        });
                    } else {
                        warnings.push(memstead_base::ops::WarningHint::DuplicateRelationship {
                            rel_type: canonical_type.clone(),
                            from: from.clone(),
                            to: to.clone(),
                        });
                    }
                }
                // Real batch: the stub exists post-commit. Rehearsal:
                // the staged state rolled back, so a still-absent
                // target of a validated add entry IS the would-be stub
                // — same warning, reported instead of created.
                let stubbed = engine.store().get(&to).map(|e| e.stub).unwrap_or(false);
                let would_stub =
                    dry_run && entry.action == "added" && engine.store().get(&to).is_none();
                if !op.remove.unwrap_or(false)
                    && absent_targets.contains(&to.to_string())
                    && (stubbed || would_stub)
                {
                    // `pending` only on the rehearsal branch — the
                    // rolled-back stub was never written, so the
                    // warning must not claim a performed effect.
                    warnings.push(memstead_base::ops::WarningHint::AutoStubCreated {
                        stub_id: to.clone(),
                        pending: would_stub,
                    });
                }
                let source_label = engine
                    .store()
                    .outgoing(&from)
                    .iter()
                    .find(|e| e.target == to && e.rel_type.eq_ignore_ascii_case(&op.rel_type))
                    .map(|e| match e.source {
                        memstead_base::EdgeSource::BodyLink => "body_link",
                        memstead_base::EdgeSource::Hierarchy => "hierarchy",
                        memstead_base::EdgeSource::Explicit => "explicit",
                    })
                    .unwrap_or("explicit");
                // Real batch: post-commit hash (the next valid
                // `expected_hash`). Rehearsal: the rolled-back store
                // serves the CURRENT on-disk hash — still the value a
                // follow-up real call validates against.
                let hash = engine
                    .store()
                    .get(&from)
                    .map(|e| e.content_hash.clone())
                    .unwrap_or_default();
                serde_json::json!({
                    "from": from.to_string(),
                    "to": to.to_string(),
                    "rel_type": canonical_type,
                    "action": entry.action,
                    "source": source_label,
                    "_hash": hash,
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "results": entries,
            "write_id": result.write_id,
            "warnings": warnings,
            "orphan_stubs_removed": result
                .orphan_stubs_removed
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>(),
        });
        attach_durability(&mut body, &engine, mem_for_anchor.as_str());
        let (engine, finished_notices) = engine.finish();
        attach_mem_changed(&mut body, finished_notices);
        let res = json_response(&body);
        match mem_schema_ref_unified(&engine, &mem_for_anchor) {
            Some(s) => with_mem_schema_anchor(res, &s),
            None => res,
        }
    }

    #[tool(
        name = "memstead_delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    fn memstead_delete(&self, Parameters(p): Parameters<DeleteParams>) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.id) {
            return err;
        }
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let id = EntityId::canonical(&p.id);
        // Empty `expected_hash` is the stub-delete escape hatch —
        // stubs have an empty content_hash so the hash check is
        // skipped. Convert empty string to None at this boundary so
        // engine semantics stay opaque to the request shape.
        let expected_hash_opt = if p.expected_hash.is_empty() {
            None
        } else {
            Some(p.expected_hash.clone())
        };

        let role = match self.resolve_role(p.role.as_deref()) {
            Ok(r) => r,
            Err(resp) => return *resp,
        };
        let identity = match self.resolve_identity(p.identity.as_deref()) {
            Ok(i) => i,
            Err(resp) => return *resp,
        };
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        engine.set_role(role);
        engine.set_identity(identity);
        // Capture the schema-ref BEFORE the delete — mem may
        // drop with the last entity.
        let mem_for_anchor = match engine.resolve_entity_id(&id) {
            Ok((resolved, _)) => mem_schema_ref_unified(&engine, resolved.mem()),
            Err(e) => return engine_err_unified(e, &engine),
        };
        let args = memstead_base::DeleteEntityArgs {
            id: id.clone(),
            expected_hash: expected_hash_opt,
        };
        let client = self.client.get().cloned();
        match engine.delete_entity(args, Actor::Agent, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                let mut body = serde_json::json!({
                    "id": outcome.id.to_string(),
                    "relations_removed": outcome.relations_removed,
                    "write_id": outcome.write_id,
                });
                // Full skip-serialises empty `orphan_stubs_removed`;
                // mirror by only adding the field when populated.
                if !outcome.orphan_stubs_removed.is_empty() {
                    body["orphan_stubs_removed"] = serde_json::json!(
                        outcome
                            .orphan_stubs_removed
                            .iter()
                            .map(|i| i.to_string())
                            .collect::<Vec<_>>()
                    );
                }
                attach_durability(&mut body, &engine, id.mem());
                let (_engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                let mut res = json_response(&body);
                if let Some(s) = mem_for_anchor.as_deref() {
                    res = with_mem_schema_anchor(res, s);
                }
                // Surface every engine-emitted warning on the outcome
                // (residual-stub demotion, and the `NOTE_MISSING`
                // provenance nudge the engine now emits when
                // `require_notes` is set and a commit landed).
                for w in &outcome.warnings {
                    res = append_warning_hint(res, w);
                }
                res
            }
            Err(e) => {
                // The notice already rode `structured_content` here;
                // the text channel lacked the `MEM_RELOADED` line a
                // successful response carries. A mutation reloads inside
                // the engine, so reconstruct the warning from the
                // drained notices to match the success channel split —
                // collision (`HASH_MISMATCH`) is the path drift matters
                // most, since it lands on the very entity being written.
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    #[tool(
        name = "memstead_check",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    fn memstead_check(&self, Parameters(p): Parameters<CheckParams>) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.entity) {
            return err;
        }
        let id = EntityId::canonical(&p.entity);
        let Some(verdict) = memstead_base::check::Verdict::from_wire(&p.verdict) else {
            let msg = format!(
                "unknown verdict {:?} — the vocabulary is: {}",
                p.verdict,
                memstead_base::check::VERDICTS.join(", ")
            );
            return tool_error_with_payload(
                "INVALID_VERDICT",
                &msg,
                envelope(
                    "INVALID_VERDICT",
                    msg.clone(),
                    serde_json::json!({ "allowed": memstead_base::check::VERDICTS }),
                ),
            );
        };
        let kind = match p.kind.as_deref() {
            None => memstead_base::check::RecordKind::Engine(
                memstead_base::check::CheckKind::Verification,
            ),
            Some(s) => match memstead_base::check::RecordKind::from_wire(s) {
                Some(k) => k,
                None => {
                    let msg = format!(
                        "unknown check kind {s:?} — the vocabulary is: {}",
                        memstead_base::check::RecordKind::vocabulary_hint()
                    );
                    return tool_error_with_payload(
                        "INVALID_CHECK_KIND",
                        &msg,
                        envelope(
                            "INVALID_CHECK_KIND",
                            msg.clone(),
                            serde_json::json!({
                                "allowed": memstead_base::check::CHECK_KINDS,
                                "foreign_prefix": memstead_base::check::FOREIGN_KIND_PREFIX,
                            }),
                        ),
                    );
                }
            },
        };
        let finding = match p.finding.as_ref() {
            None => None,
            Some(f) => match memstead_base::check::CheckFinding::from_json(
                serde_json::to_value(f).unwrap_or(serde_json::Value::Null),
            ) {
                Ok(f) => Some(f),
                Err(reason) => {
                    return tool_error_with_payload(
                        memstead_base::check::INVALID_CHECK_FINDING_CODE,
                        &reason,
                        envelope(
                            memstead_base::check::INVALID_CHECK_FINDING_CODE,
                            reason.clone(),
                            serde_json::json!({ "shape": memstead_base::check::CheckFinding::SHAPE }),
                        ),
                    );
                }
            },
        };
        let role = match self.resolve_role(p.role.as_deref()) {
            Ok(r) => r,
            Err(resp) => return *resp,
        };
        let identity = match self.resolve_identity(p.identity.as_deref()) {
            Ok(i) => i,
            Err(resp) => return *resp,
        };

        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        let _drift = engine.reload_if_stale(Some(id.mem()));
        engine.set_role(role);
        engine.set_identity(identity);
        let client = self.client.get().cloned();
        match engine.record_check_with(
            id.mem(),
            id.as_ref(),
            verdict,
            &kind,
            p.method.as_deref(),
            finding,
            Actor::Agent,
            client.as_ref(),
        ) {
            Ok(record) => {
                // A foreign kind moves no state: the response reports the
                // verification state, unchanged by this record.
                let state_result = match kind.engine_kind() {
                    Some(memstead_base::check::CheckKind::Conformance) => {
                        engine.entity_conformance_state(id.mem(), id.as_ref())
                    }
                    _ => engine.entity_check_state(id.mem(), id.as_ref()),
                };
                let (state, _) = match state_result {
                    Ok(pair) => pair,
                    Err(e) => return engine_err_unified(e, &engine),
                };
                let body = serde_json::json!({
                    "entity": record.entity,
                    "verdict": record.verdict,
                    "check_state": state.as_str(),
                    "kind": record.kind.as_deref().unwrap_or("verification"),
                    "schema_ref": record.schema_ref,
                    "role": record.role,
                    "identity": record.identity,
                    "ts": record.ts,
                    "method": record.method,
                    "finding": record.finding,
                });
                md_with_structured(
                    format!(
                        "Check recorded: {} — kind {}, verdict {}, state {}",
                        record.entity,
                        record.kind.as_deref().unwrap_or("verification"),
                        record.verdict,
                        state.as_str()
                    ),
                    body,
                )
            }
            Err(e) => engine_err_unified(e, &engine),
        }
    }

    #[tool(
        name = "memstead_rename",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    fn memstead_rename(&self, Parameters(p): Parameters<RenameParams>) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.id) {
            return err;
        }
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let id = EntityId::canonical(&p.id);
        let role = match self.resolve_role(p.role.as_deref()) {
            Ok(r) => r,
            Err(resp) => return *resp,
        };
        let identity = match self.resolve_identity(p.identity.as_deref()) {
            Ok(i) => i,
            Err(resp) => return *resp,
        };

        // Unified outcome exposes `old_path` / `new_path` directly
        // (matching full's `RenameResult` shape).
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        engine.set_role(role);
        engine.set_identity(identity);
        let mem_for_anchor = match engine.resolve_entity_id(&id) {
            Ok((resolved, _)) => resolved.mem().to_string(),
            Err(e) => return engine_err_unified(e, &engine),
        };
        let args = memstead_base::RenameEntityArgs {
            id: id.clone(),
            expected_hash: Some(p.expected_hash.clone()),
            new_title: p.new_title.clone(),
        };
        let client = self.client.get().cloned();
        match engine.rename_entity(args, Actor::Agent, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                let mut body = serde_json::json!({
                    "old_id": outcome.old_id.to_string(),
                    "new_id": outcome.new_id.to_string(),
                    "old_path": outcome.old_path,
                    "new_path": outcome.new_path,
                    "_hash": outcome.content_hash,
                    "write_id": outcome.write_id,
                    "warnings": outcome.warnings,
                });
                attach_durability(&mut body, &engine, id.mem());
                let (engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                let res = json_response(&body);
                match mem_schema_ref_unified(&engine, &mem_for_anchor) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => {
                // The notice already rode `structured_content` here;
                // the text channel lacked the `MEM_RELOADED` line a
                // successful response carries. A mutation reloads inside
                // the engine, so reconstruct the warning from the
                // drained notices to match the success channel split —
                // collision (`HASH_MISMATCH`) is the path drift matters
                // most, since it lands on the very entity being written.
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    #[tool(
        name = "memstead_retype",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    fn memstead_retype(&self, Parameters(p): Parameters<RetypeParams>) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.id) {
            return err;
        }
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        let dry_run = p.dry_run.unwrap_or(false);
        if !dry_run && p.expected_hash.as_deref().is_none_or(str::is_empty) {
            let msg = "`expected_hash` is required for a real retype (read the entity first and \
                       pass its `_hash`); only `dry_run: true` may omit it";
            return tool_error_with_payload(
                "INVALID_INPUT",
                msg,
                envelope(
                    "INVALID_INPUT",
                    msg.to_string(),
                    serde_json::json!({ "field": "expected_hash" }),
                ),
            );
        }
        let id = EntityId::canonical(&p.id);
        let role = match self.resolve_role(p.role.as_deref()) {
            Ok(r) => r,
            Err(resp) => return *resp,
        };
        let identity = match self.resolve_identity(p.identity.as_deref()) {
            Ok(i) => i,
            Err(resp) => return *resp,
        };
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        engine.set_role(role);
        engine.set_identity(identity);
        let mem_for_anchor = match engine.resolve_entity_id(&id) {
            Ok((resolved, _)) => resolved.mem().to_string(),
            Err(e) => return engine_err_unified(e, &engine),
        };
        let args = memstead_base::RetypeEntityArgs {
            id: id.clone(),
            expected_hash: p.expected_hash.clone(),
            target_type: p.target_type.clone(),
            section_map: p
                .section_map
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            drop_metadata: p.drop_metadata.clone().unwrap_or_default(),
            dry_run,
        };
        let client = self.client.get().cloned();
        match engine.retype_entity(args, Actor::Agent, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                let mut body = serde_json::to_value(&outcome).unwrap_or(serde_json::Value::Null);
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("dry_run".into(), serde_json::json!(dry_run));
                }
                attach_durability(&mut body, &engine, id.mem());
                let (engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                let res = json_response(&body);
                match mem_schema_ref_unified(&engine, &mem_for_anchor) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => {
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    // ----------------------------------------------------------------------
    // Admin tools
    // ----------------------------------------------------------------------

    #[tool(
        name = "memstead_health",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn memstead_health(&self, Parameters(p): Parameters<HealthParams>) -> CallToolResult {
        // include_config: true is served end-to-end via the unified
        // accessors (gitdir_for / worktree_for / mem_head_sha /
        // mem_config_for). `mutations` + `plugin` remain server
        // state on `self`.
        //
        // Caveat: git-branch mounts return `None` from
        // `mem_config_for` until the backend's config-read path
        // lifts; under include_config: true their per-mem entries
        // emit without `write_guidance` / `extra` (the `vcs` block
        // is present instead).
        let unified = self.unified_engine();
        self.memstead_health_unified(p, unified.clone())
    }

    /// Body of [`Self::memstead_health`]. Default shape (`summary`,
    /// totals, distributions, rosters, `mem_schemas`) plus the
    /// eight `include` detail sections (orphans, stubs,
    /// most_connected, missing_fields, stale, dangling_links, tags,
    /// missing_required_outgoing). `include_config: true` adds
    /// `mutations`, `plugin`, and per-mem `vcs` / `write_guidance`
    /// / `extra` — the vcs subobject for git-branch mounts uses
    /// the worktree heuristic from
    /// [`memstead_base::Engine::worktree_for`].
    fn memstead_health_unified(
        &self,
        p: HealthParams,
        unified: Arc<Mutex<memstead_base::Engine>>,
    ) -> CallToolResult {
        let mut engine = crate::lock_engine!(unified);
        // Health always takes the FULL lazy-mount load, mirroring
        // overview: a `mem` filter scopes what is REPORTED, but the
        // answer's cross-mem components — dangling-link adjudication
        // (targets live anywhere; an unloaded real target reads as a
        // stub and would be reported as a broken link) and the
        // workspace-global community partition — are only truthful over
        // a complete store. The second final grade demonstrated 28
        // false dangling links on a mem-scoped health over a lazy
        // workspace; a partial-store count presented as truth is the
        // forbidden rendering.
        engine.ensure_mems_loaded(None);
        let drift_warnings = engine.reload_if_stale(p.mem.as_deref());
        let (mut engine, mem_changed_notices) = engine.finish();

        let include = p.include.unwrap_or_default();
        let args = memstead_base::ops::health_compose::HealthArgs {
            mem: p.mem.as_deref(),
            include: &include,
            limit: p.limit,
            target_schema: p.target_schema.as_deref(),
            include_config: p.include_config,
            strict: false,
            today: None,
        };
        // Server-owned config the engine does not carry — prebuilt here so the
        // composer inserts the bytes verbatim (and stays free of the MCP
        // server's config types).
        let plugin_json: serde_json::Map<String, serde_json::Value> = self
            .plugin
            .iter()
            .map(|(k, v)| {
                let json = serde_json::to_value(v).unwrap_or(serde_json::Value::Null);
                (k.clone(), json)
            })
            .collect();
        let config = memstead_base::ops::health_compose::HealthConfig {
            mutations: serde_json::json!({ "require_notes": self.mutations.require_notes }),
            plugin: serde_json::Value::Object(plugin_json),
        };

        let result = match memstead_projection::health::compose_health(
            &mut engine,
            &args,
            drift_warnings,
            &config,
        ) {
            Ok(v) => v,
            Err(memstead_base::ops::health_compose::ComposeHealthError::MemQuarantined(name)) => {
                let err = engine.unknown_mem_error(&name);
                return engine_err_unified(err, &engine);
            }
            Err(memstead_base::ops::health_compose::ComposeHealthError::UnknownMem {
                name,
                writable_mems,
            }) => {
                let msg = format!(
                    "unknown mem: \"{name}\". Writable mems: [{}]",
                    writable_mems.join(", ")
                );
                return tool_error_with_payload(
                    "UNKNOWN_MEM",
                    &msg,
                    envelope(
                        "UNKNOWN_MEM",
                        msg.clone(),
                        serde_json::json!({
                            "name": name,
                            "writable_mems": writable_mems,
                        }),
                    ),
                );
            }
            Err(memstead_base::ops::health_compose::ComposeHealthError::InvalidTargetSchema {
                raw,
                reason,
            }) => {
                let msg = format!("invalid target_schema {raw:?}: {reason}");
                return tool_error_with_payload(
                    "INVALID_INPUT",
                    &msg,
                    envelope(
                        "INVALID_INPUT",
                        msg.clone(),
                        serde_json::json!({ "target_schema": raw, "reason": reason }),
                    ),
                );
            }
            Err(memstead_base::ops::health_compose::ComposeHealthError::Engine(e)) => {
                return engine_err_unified(e, &engine);
            }
        };

        let res = json_response(&result);
        let res = match p
            .mem
            .as_deref()
            .and_then(|v| mem_schema_ref_unified(&engine, v))
        {
            Some(s) => with_mem_schema_anchor(res, &s),
            None => res,
        };
        let res = attach_mem_changed_to_result(res, mem_changed_notices);
        // #57: the text channel is chunkable markdown rendered from the
        // final structured payload (which ships whole), so a multi-include
        // report can't overflow the response cap. Done last — after the
        // anchor / mem-changed post-processing that mutates
        // `structured_content`.
        finalize_health_text(res, p.token_budget.unwrap_or(self.token_budget), p.chunk)
    }

    #[tool(
        name = "memstead_changes_since",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn memstead_changes_since(
        &self,
        Parameters(p): Parameters<ChangesSinceParams>,
    ) -> CallToolResult {
        // The engine's
        // `changes_since` populates `notes` and `memstead_ref` on every
        // git-branch call — the rename map is note-driven, so the
        // walk happens regardless. `include_notes` becomes a
        // renderer-side filter on the wire response: when `false`,
        // strip the fields so the wire shape matches the
        // `include_notes: false` contract.
        let include_notes = p.include_notes;
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        let drift_warnings = engine.reload_if_stale(Some(&p.mem));
        let (engine, mem_changed_notices) = engine.finish();
        let mem_for_anchor = p.mem.clone();
        let res = match engine.changes_since(&p.mem, &p.since, p.rename_similarity) {
            Ok(mut report) => {
                if !include_notes {
                    report.notes = None;
                    report.memstead_ref = None;
                }
                let mut res = json_response(&report);
                for w in &drift_warnings {
                    res = append_warning_hint(res, w);
                }
                match mem_schema_ref_unified(&engine, &mem_for_anchor) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => {
                // Delegate to the typed-envelope translator so the wire
                // `code` matches `EngineError::code()` for the underlying
                // variant. A bad `since` cursor now arrives as the typed
                // `EngineError::InvalidChangesCursor` (code `INVALID_CURSOR`,
                // `details.mem` + untruncated `details.since`) — lifted
                // from the backend's typed marker in `Engine::changes_since`
                // rather than sniffed out of a raw backend message string here.
                // Genuine backend faults still surface `MEM_ERROR`.
                // The structured notice rides via the shared
                // `attach_mem_changed_to_result` below; prepend the
                // `MEM_RELOADED` text line here so the error path
                // carries the same channel split a success carries.
                prepend_drift_warnings_to_result_text(
                    engine_err_unified(e, &engine),
                    &drift_warnings,
                )
            }
        };
        attach_mem_changed_to_result(res, mem_changed_notices)
    }

    #[tool(
        name = "memstead_diff",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn memstead_diff(&self, Parameters(p): Parameters<DiffParams>) -> CallToolResult {
        let unified = self.unified_engine();
        let engine = crate::lock_engine!(unified);
        let config = memstead_base::ops::DiffConfig {
            rename_similarity: p
                .rename_similarity
                .unwrap_or(memstead_base::ops::RENAME_SIMILARITY_DEFAULT),
            include_content: p.include_content,
            include_ripple: p.include_ripple,
        };
        match engine.diff(&p.mem, &p.ref_a, &p.ref_b, Some(config)) {
            Ok(diff) => json_response(&diff),
            Err(e) => engine_err_unified(e, &engine),
        }
    }

    #[tool(
        name = "memstead_reload",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn memstead_reload(&self, Parameters(p): Parameters<ReloadParams>) -> CallToolResult {
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        // Full mode: the additive schema-source + mount-manifest
        // re-scan runs FIRST (so newly registered mems join the
        // content sweep below), then the ordinary workspace-wide
        // content reload — coherence (MEM_RELOADED, expected_hash
        // discipline) rides the same content-reload path it always
        // did. Per-item refresh failures ride the `refresh` block;
        // they never abort the content reload.
        let refresh = if p.full.unwrap_or(false) {
            if p.mem.is_some() {
                let msg = "`full: true` is workspace-scoped — omit `mem`".to_string();
                return tool_error_with_payload(
                    "INVALID_INPUT",
                    &msg,
                    envelope(
                        "INVALID_INPUT",
                        msg.clone(),
                        serde_json::json!({ "message": msg }),
                    ),
                );
            }
            Some(engine.full_refresh())
        } else {
            None
        };
        let result = match p.mem.as_deref() {
            Some(name) => engine.reload_one_mem_report(name).map(|r| vec![r]),
            None => engine.reload_each_writable_mem_reports(),
        };
        match result {
            Ok(reports) => {
                let mut payload = serde_json::json!({ "reports": reports });
                if let Some(refresh) = refresh {
                    payload["refresh"] =
                        serde_json::to_value(&refresh).unwrap_or(serde_json::Value::Null);
                }
                json_response(&payload)
            }
            // `engine_err_unified` reads `EngineError::code()` for the
            // underlying variant so the wire envelope here carries the
            // same typed token the rest of the surface emits for the
            // same fire condition — `MEM_ERROR` for backend wraps,
            // `UNKNOWN_MEM` for missing-mem, etc.
            Err(e) => engine_err_unified(e, &engine),
        }
    }

    // ----------------------------------------------------------------------
    // Mem lifecycle tools
    // ----------------------------------------------------------------------

    #[tool(
        name = "memstead_mem_create",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    fn memstead_mem_create(
        &self,
        Parameters(p): Parameters<crate::lifecycle::MemCreateParams>,
    ) -> CallToolResult {
        // Collisions surface via the snapshot probe with the
        // `{name, source}` envelope (callers branch on `code` only).
        let unified = self.unified_engine();
        self.memstead_mem_create_unified(p, unified.clone())
    }

    #[tool(
        name = "memstead_mem_delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    fn memstead_mem_delete(
        &self,
        Parameters(p): Parameters<crate::lifecycle::MemDeleteParams>,
    ) -> CallToolResult {
        let unified = self.unified_engine();
        self.memstead_mem_delete_unified(p, unified.clone())
    }

    #[tool(
        name = "memstead_mem_set_version",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    fn memstead_mem_set_version(
        &self,
        Parameters(p): Parameters<crate::lifecycle::MemSetVersionParams>,
    ) -> CallToolResult {
        let new_version = match semver::Version::parse(&p.version) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("version {:?} is not a valid semver: {e}", p.version);
                return tool_error_with_payload(
                    "INVALID_INPUT",
                    &msg,
                    envelope(
                        "INVALID_INPUT",
                        msg.clone(),
                        serde_json::json!({ "message": msg }),
                    ),
                );
            }
        };
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        match engine.set_mem_version(&p.name, new_version, p.note.as_deref()) {
            Ok(outcome) => {
                let mut body = serde_json::json!({
                    "mem": outcome.mem,
                    "old_version": outcome.old_version.map(|v| v.to_string()),
                    "new_version": outcome.new_version.to_string(),
                    "warnings": outcome.warnings,
                });
                let (_engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                json_response(&body)
            }
            Err(e) => {
                // The notice already rode `structured_content` here;
                // the text channel lacked the `MEM_RELOADED` line a
                // successful response carries. A mutation reloads inside
                // the engine, so reconstruct the warning from the
                // drained notices to match the success channel split —
                // collision (`HASH_MISMATCH`) is the path drift matters
                // most, since it lands on the very entity being written.
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    #[tool(
        name = "memstead_mem_configure",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn memstead_mem_configure(
        &self,
        Parameters(p): Parameters<crate::lifecycle::MemConfigureParams>,
    ) -> CallToolResult {
        if let Some(err) = validate_note(p.note.as_deref()) {
            return err;
        }
        if p.subject.is_some() && p.clear_subject {
            let msg = "`subject` and `clear_subject` are mutually exclusive — pass one";
            return tool_error_with_payload(
                "INVALID_INPUT",
                msg,
                envelope(
                    "INVALID_INPUT",
                    msg.to_string(),
                    serde_json::json!({ "message": msg }),
                ),
            );
        }
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        let mut warnings: Vec<memstead_base::ops::WarningHint> = Vec::new();

        // "Set what is present": each Some field routes through the
        // engine setter the CLI verb uses; empty string clears.
        if let Some(t) = p.title.clone() {
            let value = if t.is_empty() { None } else { Some(t) };
            match engine.set_mem_title(&p.name, value, p.note.as_deref()) {
                Ok(o) => warnings.extend(o.warnings),
                Err(e) => {
                    let (engine, notices) = engine.finish();
                    let drift = notices_as_reload_warnings(&notices);
                    return attach_drift_to_error(engine_err_unified(e, &engine), &drift, notices);
                }
            }
        }
        if let Some(d) = p.description.clone() {
            let value = if d.is_empty() { None } else { Some(d) };
            match engine.set_mem_description(&p.name, value, p.note.as_deref()) {
                Ok(o) => warnings.extend(o.warnings),
                Err(e) => {
                    let (engine, notices) = engine.finish();
                    let drift = notices_as_reload_warnings(&notices);
                    return attach_drift_to_error(engine_err_unified(e, &engine), &drift, notices);
                }
            }
        }
        if p.subject.is_some() || p.clear_subject {
            let value = p.subject.clone().map(|s| s.into_engine());
            match engine.set_mem_subject(&p.name, value, p.note.as_deref()) {
                Ok(o) => warnings.extend(o.warnings),
                Err(e) => {
                    let (engine, notices) = engine.finish();
                    let drift = notices_as_reload_warnings(&notices);
                    return attach_drift_to_error(engine_err_unified(e, &engine), &drift, notices);
                }
            }
        }

        // No-field calls still validate the mem exists (a pure no-op
        // against an unknown name would be a silent lie).
        let no_field =
            p.title.is_none() && p.description.is_none() && p.subject.is_none() && !p.clear_subject;
        if no_field && engine.mount(&p.name).is_none() {
            let e = engine.unknown_mem_error(&p.name);
            let (engine, notices) = engine.finish();
            let drift = notices_as_reload_warnings(&notices);
            return attach_drift_to_error(engine_err_unified(e, &engine), &drift, notices);
        }

        // Post-call state from the loaded config — the stable response
        // shape regardless of which fields were touched.
        let config = engine
            .mem_configs_named()
            .find(|(name, _)| *name == p.name)
            .map(|(_, c)| c.clone());
        let mut body = serde_json::json!({
            "mem": p.name,
            "title": config.as_ref().and_then(|c| c.title.clone()),
            "description": config.as_ref().and_then(|c| c.description.clone()),
            "subject": config.as_ref().and_then(|c| c.subject.as_ref().map(|sub| serde_json::json!({
                "scope": sub.scope,
                "method": sub.method,
                "exclusions": sub.exclusions,
            }))),
            "warnings": warnings,
        });
        let (_engine, finished_notices) = engine.finish();
        attach_mem_changed(&mut body, finished_notices);
        json_response(&body)
    }

    #[tool(
        name = "memstead_mem_set_schema",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    fn memstead_mem_set_schema(
        &self,
        Parameters(p): Parameters<crate::lifecycle::MemSetSchemaParams>,
    ) -> CallToolResult {
        let target = match p.schema.parse::<memstead_schema::SchemaRef>() {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("invalid schema ref {:?}: {e}", p.schema);
                return tool_error_with_payload(
                    "INVALID_INPUT",
                    &msg,
                    envelope(
                        "INVALID_INPUT",
                        msg.clone(),
                        serde_json::json!({ "message": msg }),
                    ),
                );
            }
        };
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        match engine.set_mem_schema(&p.mem, &target) {
            Ok(outcome) => {
                let mut body = serde_json::to_value(&outcome).expect("SetSchemaOutcome serialises");
                let (_engine, finished_notices) = engine.finish();
                attach_mem_changed(&mut body, finished_notices);
                json_response(&body)
            }
            Err(e) => {
                let (engine, notices) = engine.finish();
                let warnings = notices_as_reload_warnings(&notices);
                attach_drift_to_error(engine_err_unified(e, &engine), &warnings, notices)
            }
        }
    }

    /// Body of [`Self::memstead_mem_create`].
    fn memstead_mem_create_unified(
        &self,
        p: crate::lifecycle::MemCreateParams,
        unified: Arc<Mutex<memstead_base::Engine>>,
    ) -> CallToolResult {
        let mut engine = crate::lock_engine!(unified);
        // The session's declared role and identity ride the seed
        // commit like every entity mutation's commit (see the delete
        // wrapper for the same rule).
        engine.set_role(self.default_role);
        engine.set_identity(self.default_identity.clone());

        let schema_ref = match p.schema.parse::<memstead_schema::SchemaRef>() {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("invalid schema ref {:?}: {e}", p.schema);
                return tool_error_with_payload(
                    "INVALID_INPUT",
                    &msg,
                    envelope(
                        "INVALID_INPUT",
                        msg.clone(),
                        serde_json::json!({ "message": msg }),
                    ),
                );
            }
        };

        // Resolve the inlined-schema verbosity up front — *before* the
        // create side-effect — so a bad value refuses cleanly rather than
        // after the mem has already landed on disk. Only meaningful when
        // `include_schema` is set; ignored otherwise per the param
        // contract (so a moot typo doesn't sink an otherwise-valid create).
        let schema_verbosity = if p.include_schema {
            match p.schema_verbosity.as_deref() {
                // Absent → lite, mirroring `memstead_schema`'s default so
                // the inlined body stays byte-identical to that tool's
                // default reply.
                None => render::SchemaVerbosity::Lite,
                Some(v) => match render::SchemaVerbosity::from_wire(v) {
                    Some(sv) => sv,
                    None => {
                        let msg = format!(
                            "unknown schema_verbosity: \"{v}\" — expected \"full\" or \"lite\""
                        );
                        return tool_error_with_payload(
                            "INVALID_INPUT",
                            &msg,
                            envelope(
                                "INVALID_INPUT",
                                msg.clone(),
                                serde_json::json!({
                                    "value": v,
                                    "allowed": ["full", "lite"],
                                }),
                            ),
                        );
                    }
                },
            }
        } else {
            render::SchemaVerbosity::Full
        };

        // Curation fields ride the create call and are applied through
        // the same setters the CLI verbs use, after the create lands.
        let cur_title = p.title.clone().filter(|t| !t.is_empty());
        let cur_description = p.description.clone().filter(|d| !d.is_empty());
        let cur_subject = p.subject.clone();
        let cur_note = p.note.clone();

        // Hierarchical paths are first-class. The separate `path`
        // wire-shape field retired; `name` carries the full
        // identifier (`team/sub-mem`) verbatim.
        let params = memstead_base::mem_management::MemCreateParams {
            name: p.name,
            location: std::path::PathBuf::from(p.location),
            schema_ref,
            vcs: p.vcs.map(Into::into),
            note: p.note,
            operator_mode: self.operator_mode,
            // Forward the optional
            // recovery action from the MCP wire shape. Bare creates
            // pass `None` and route via the tombstone-driven
            // default; explicit values (`reattach` /
            // `force_overwrite` / `hard_cleanup_first`) override.
            recovery: p.recovery.map(Into::into),
            // Optional per-instance writing guidance from the wire
            // shape, forwarded opaquely into the seed config.
            write_guidance: p.write_guidance,
            actor: Actor::Agent,
            client: self.client.get().cloned(),
            // The MCP wire shape does not expose the storage override
            // yet — the workspace-shape heuristic keeps behaviour
            // identical.
            storage: None,
        };

        match memstead_base::mem_management::create_mem(&mut engine, params) {
            Ok(response) => {
                // Apply curation at creation — same setters, same
                // validation, same storage as the CLI verbs. Each is
                // its own config commit; a failure here surfaces after
                // the mem exists (the create itself has no implicit
                // rollback, matching the seed-commit contract).
                let mut curation_warnings: Vec<memstead_base::ops::WarningHint> = Vec::new();
                if let Some(t) = cur_title {
                    match engine.set_mem_title(&response.name, Some(t), cur_note.as_deref()) {
                        Ok(o) => curation_warnings.extend(o.warnings),
                        Err(e) => return engine_err_unified(e, &engine),
                    }
                }
                if let Some(d) = cur_description {
                    match engine.set_mem_description(&response.name, Some(d), cur_note.as_deref()) {
                        Ok(o) => curation_warnings.extend(o.warnings),
                        Err(e) => return engine_err_unified(e, &engine),
                    }
                }
                if let Some(subj) = cur_subject {
                    match engine.set_mem_subject(
                        &response.name,
                        Some(subj.into_engine()),
                        cur_note.as_deref(),
                    ) {
                        Ok(o) => curation_warnings.extend(o.warnings),
                        Err(e) => return engine_err_unified(e, &engine),
                    }
                }

                // Build wire response with the same shape full emits.
                let body = serde_json::json!({
                    "name": response.name,
                    "location": response.location,
                    "schema_ref": response.schema_ref.to_string(),
                    "seed_write_id": response.seed_write_id,
                });
                // The engine ships the
                // `MEM_REATTACHED_AFTER_UNREGISTER` warning on the
                // create response. Surface every response-side warning
                // via `append_warning_hint` so MCP callers see the
                // structured envelope alongside the success payload.
                let mut create_warnings: Vec<memstead_base::ops::WarningHint> =
                    response.warnings.clone();
                create_warnings.extend(curation_warnings);
                let res = json_response(&body);
                let res = create_warnings.iter().fold(res, append_warning_hint);
                // Inline the
                // full schema body only when the caller opts in via
                // `include_schema: true`. Otherwise every successful
                // create would ship ~25 KB of schema body, even for the
                // agent's second+ mem on the same schema where the
                // value is workspace-stable and already cached.
                let schema_payload = if p.include_schema {
                    engine.schemas().get(&response.name).cloned().map(|s| {
                        let origin = engine.schema_origin(&s);
                        render::build_schema_payload(
                            &s,
                            vec![response.name.clone()],
                            schema_verbosity,
                            origin,
                        )
                    })
                } else {
                    None
                };
                let res = if let Some(payload) = schema_payload {
                    let mut res = res;
                    if let Some(sc) = res.structured_content.as_mut()
                        && let Some(obj) = sc.as_object_mut()
                    {
                        obj.insert("schema".to_string(), payload);
                        if let Ok(text) = serde_json::to_string_pretty(&*sc) {
                            res.content = vec![rmcp::model::ContentBlock::text(text)];
                        }
                    }
                    res
                } else {
                    res
                };
                match mem_schema_ref_unified(&engine, &response.name) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => full_engine_err_unified(e, &engine),
        }
    }

    /// Unified-engine path for [`Self::memstead_mem_delete`].
    fn memstead_mem_delete_unified(
        &self,
        p: crate::lifecycle::MemDeleteParams,
        unified: Arc<Mutex<memstead_base::Engine>>,
    ) -> CallToolResult {
        let mut engine = crate::lock_engine!(unified);
        // The session's declared role and identity ride the prune
        // commit like every entity mutation's commit; without this the
        // engine carried whatever the previous tool call left on it.
        engine.set_role(self.default_role);
        engine.set_identity(self.default_identity.clone());

        // MCP `memstead_mem_delete`
        // always means destructive. The wire shape no longer exposes
        // `delete_files`; the wrapper hardcodes `true` so the engine
        // runs both refusal gates (`MEM_REFERENCED_BY_POLICY`,
        // `MEM_HAS_INCOMING_REFS`) and the policy scrub on success.
        let params = memstead_base::mem_management::MemDeleteParams {
            name: p.name,
            delete_files: true,
            note: p.note,
            actor: Actor::Agent,
            client: self.client.get().cloned(),
            operator_mode: self.operator_mode,
            detach_incoming: false,
        };

        // Snapshot the schema-ref BEFORE the delete so the response
        // can still anchor to the now-departed mem's schema.
        let mem_for_anchor = mem_schema_ref_unified(&engine, &params.name);

        match memstead_base::mem_management::delete_mem(&mut engine, params) {
            Ok(response) => {
                let body = serde_json::json!({
                    "name": response.name,
                    "deleted_from_router": response.deleted_from_router,
                    "files_deleted": response.files_deleted,
                    // Surface scrubbed
                    // `.memstead/workspace.toml` entries so the agent
                    // doesn't have to re-read `workspace show` to
                    // learn the policy side effects of the delete.
                    "allowlist_entries_removed": &response.allowlist_entries_removed,
                });
                let res = json_response(&body);
                let res = match mem_for_anchor {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                };
                // Surface every engine-emitted warning: disk-cleanup
                // outcome (rmdir_failed / backend_prune_failed) and the
                // `NOTE_MISSING` provenance nudge the engine now emits
                // when `require_notes` is set.
                let mut res = res;
                for w in &response.warnings {
                    res = append_warning_hint(res, w);
                }
                res
            }
            Err(e) => full_engine_err_unified(e, &engine),
        }
    }
}

// ==========================================================================
impl McpServer {
    /// The tool router every consumer uses: the macro-generated routes with
    /// each description stamped in from `descriptions::FULL`.
    ///
    /// The generated router is `tool_router_undescribed` and is never handed
    /// out: rmcp's `#[tool]` attribute takes a string literal and nothing
    /// else, so the prose cannot reach it, and this is the one seam where the
    /// text files become the served text. `#[tool_handler]` resolves to this
    /// function by name, so dispatch and listing agree.
    pub fn tool_router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        let mut router = Self::tool_router_undescribed();
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
