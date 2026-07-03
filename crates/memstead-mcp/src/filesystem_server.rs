//! filesystem-mem MCP server — parallel module to [`crate::server`].
//!
//! Boots when `memstead-mcp/src/main.rs` runs without the `mem-repo`
//! feature against a `.memstead/workspace.toml` workspace that carries
//! only folder + archive mounts (no `mem-repo/.git/`). Wraps the
//! unified [`memstead_base::Engine`] behind the same rmcp ServerHandler
//! shape the mem-repo `McpServer` uses, but ports only the subset
//! of tools that make sense in a single-mem history-free context.
//!
//! Per-mutation provenance lands in `.memstead/changes.jsonl` (the
//! filesystem-mem analogue of the commit-body trailer the
//! mem-repo server writes).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, InitializeRequestParams, InitializeResult,
    ListToolsResult, PaginatedRequestParams, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, tool, tool_handler, tool_router};

use indexmap::IndexMap;

use memstead_base::EntityId;
use memstead_base::ops::SearchScope;
use memstead_base::render::{render_entity_markdown, render_search_markdown};
use memstead_base::vcs::{Actor, ClientId};
use memstead_base::{
    BootError, CreateEntityArgs, DeleteEntityArgs, Engine, EngineError, RelateAction,
    RelateEntityArgs, RenameEntityArgs, UpdateEntityArgs,
};
use std::path::Path;

use crate::tools::admin::{ChangesSinceParams, DiffParams, HealthParams};
use crate::tools::graph::{EntityParams, OverviewParams, SchemaParams, SearchParams};
use crate::tools::mutation::{CreateParams, DeleteParams, RelateParams, RenameParams, UpdateParams};

/// MCP server backed by the unified [`memstead_base::Engine`].
///
/// Constructed via [`Self::from_workspace_root`].
#[derive(Clone)]
pub struct FilesystemMcpServer {
    /// Persistent unified engine. Mutations invalidate its memo
    /// caches via the engine's own hooks; reads see fresh state on
    /// every lock without needing a re-init from disk.
    engine: Arc<Mutex<Engine>>,
    /// Workspace root captured at construction time. Used by
    /// `memstead_changes_since` (which still reads JSONL directly off
    /// disk) — the unified engine does not expose a workspace_root
    /// accessor because mounts can be heterogeneous.
    workspace_root: PathBuf,
    /// Captured `clientInfo` from the initialize handshake. Used to
    /// stamp the changelog `client` field on every mutation. Same
    /// `OnceLock` shape as `crate::server::McpServer::client`.
    client: Arc<OnceLock<ClientId>>,
}

impl FilesystemMcpServer {
    /// Construct from a workspace root. Boots the unified
    /// [`Engine`] via [`Engine::from_workspace_root`] (lean path —
    /// folder + archive backends only). The error envelope wraps
    /// every layer (layout dispatch, store load, backend
    /// instantiation, engine construction) under one [`BootError`].
    ///
    /// Production callers (main.rs, every test fixture) reach the
    /// server through this constructor directly.
    pub fn from_workspace_root(workspace_root: &Path) -> Result<Self, BootError> {
        let workspace_root = workspace_root.to_path_buf();
        let engine = Engine::from_workspace_root(&workspace_root)?;
        Ok(Self {
            engine: Arc::new(Mutex::new(engine)),
            workspace_root,
            client: Arc::new(OnceLock::new()),
        })
    }

    /// Construct directly from a pre-built [`Engine`] — e.g. a sealed
    /// read-only archive mount stood up by an embedding service. `workspace_root`
    /// is consulted only by `memstead_changes_since` (which reads JSONL off
    /// disk); surfaces that do not expose that tool may pass any path.
    pub fn from_engine(engine: Engine, workspace_root: PathBuf) -> Self {
        Self {
            engine: Arc::new(Mutex::new(engine)),
            workspace_root,
            client: Arc::new(OnceLock::new()),
        }
    }

    /// Export the server's single mem as `.mem` archive bytes. Used by
    /// embedding services (the session server) to hand a visitor a
    /// self-describing copy of the mem their agent built. The server's
    /// engine carries exactly one mem (filesystem / session mems are
    /// single-mem by design); this exports it.
    pub fn export_mem_to_bytes(&self) -> Result<Vec<u8>, memstead_base::EngineError> {
        let engine = self.engine.lock().expect("filesystem MCP engine mutex poisoned");
        let mem = engine
            .mem_names()
            .into_iter()
            .next()
            .map(String::from)
            .ok_or_else(|| {
                memstead_base::EngineError::InvalidInput("no mem to export".to_string())
            })?;
        engine.export_mem_to_bytes(&mem)
    }

    /// Count of real (non-stub) entities across the server's mem. Used
    /// by embedding services (the session server) to enforce a
    /// per-session resource cap before admitting a create.
    pub fn entity_count(&self) -> usize {
        self.engine
            .lock()
            .expect("filesystem MCP engine mutex poisoned")
            .stats()
            .entity_count
    }

    /// Run a read closure against the locked engine. The escape hatch for
    /// embedding services that need engine reads the tool surface does not
    /// expose — e.g. the session server's live graph projection and its
    /// change-event subscription. Keeps the engine itself private; callers
    /// get a borrow only for the duration of `f`.
    pub fn with_engine<R>(&self, f: impl FnOnce(&Engine) -> R) -> R {
        let engine = self.engine.lock().expect("filesystem MCP engine mutex poisoned");
        f(&engine)
    }

    fn actor_and_client(&self) -> (Actor, Option<ClientId>) {
        match self.client.get() {
            Some(c) => (Actor::Agent, Some(c.clone())),
            None => (Actor::Agent, None),
        }
    }
}

/// Build a typed tool-error envelope. The text channel is
/// `ERROR [<CODE>]: <message>` (consumers reading only
/// `result.content[0].text` recover the code with one regex) and the
/// `structured_content` channel carries `{code, message}` so agents
/// branching on the structured shape get the typed code without parsing
/// text. Mirror of `crate::error_envelope::tool_error_with_payload`'s
/// payload-less shape — the per-flavour symmetry is what makes the
/// wire-byte contract uniform across lean and full. Pre-fix the text
/// channel emitted a JSON-stringified `{code, message}` payload; that
/// form parsed for machine consumers but missed the documented
/// prefix-form contract.
fn tool_error(code: &str, message: &str) -> CallToolResult {
    tool_error_with_details(code, message, None)
}

/// Same as [`tool_error`] but additionally embeds a structured `details`
/// payload under `structured_content.details`. Text channel format is
/// identical (`ERROR [<CODE>]: <message>`); recovery payloads (current
/// hash, declared sections, referrer list) live exclusively on the
/// structured channel.
fn tool_error_with_details(
    code: &str,
    message: &str,
    details: Option<serde_json::Value>,
) -> CallToolResult {
    let payload = match details {
        Some(d) => serde_json::json!({ "code": code, "message": message, "details": d }),
        None => serde_json::json!({ "code": code, "message": message }),
    };
    let text = format!("ERROR [{code}]: {message}");
    let mut result = CallToolResult::error(vec![Content::text(text)]);
    result.structured_content = Some(payload);
    result
}

fn md_response(markdown: String) -> CallToolResult {
    CallToolResult::success(vec![Content::text(markdown)])
}

/// Pair rendered markdown on the text channel with a structured
/// envelope on `structured_content`:
/// tools whose response has a canonical human-readable form (entity,
/// search) ship the markdown to terminal/inline consumers and the
/// typed JSON to branching agents in one call.
fn md_with_structured(markdown: String, structured: serde_json::Value) -> CallToolResult {
    let mut result = CallToolResult::success(vec![Content::text(markdown)]);
    result.structured_content = Some(structured);
    result
}

fn json_response<T: serde::Serialize>(data: &T) -> CallToolResult {
    let value = serde_json::to_value(data).unwrap_or(serde_json::Value::Null);
    let text = serde_json::to_string_pretty(&value).unwrap_or_default();
    let mut result = CallToolResult::success(vec![Content::text(text)]);
    result.structured_content = Some(value);
    result
}

/// Whether the named mem's storage is durable (persists past restart /
/// session-TTL eviction), derived from its mount's `MountStorage` kind.
/// On the ephemeral in-memory sketch this returns `false` — the per-write
/// `durable` echo on every mutation response is how an agent learns its
/// `commit_sha` denotes nothing durable. Defaults to `false` for an
/// unresolvable mem: the engine never claims a durability it can't vouch
/// for.
fn mem_is_durable(engine: &memstead_base::Engine, mem: &str) -> bool {
    engine
        .mounts()
        .iter()
        .find(|m| m.mem == mem)
        .map(|m| m.storage.is_durable())
        .unwrap_or(false)
}

/// Refuse params the filesystem-mem MCP surface does not honour rather
/// than silently dropping them (Plan 03, Part B). Each `(name, meaningful)`
/// pair flags a param this surface hardwires off; `meaningful` is true when
/// the caller passed it with an effect they expect (a non-empty map/list, or
/// `dry_run: true`). When any such param was meaningfully supplied the call
/// refuses UP FRONT — before any mutation — with `UNSUPPORTED_PARAM` naming
/// every dropped param in `details.params`, so an agent can never believe a
/// no-op succeeded (the worst case being `dry_run: true`, which would
/// otherwise commit a real write the agent thought was a preview). A
/// defaulted-empty / absent / `false` param is left alone — the caller
/// intended no effect, so the call proceeds unchanged (backward-compatible).
/// Returns `None` when nothing meaningful was dropped.
fn reject_unsupported_params(params: &[(&str, bool)]) -> Option<CallToolResult> {
    let dropped: Vec<&str> = params
        .iter()
        .filter_map(|(name, meaningful)| meaningful.then_some(*name))
        .collect();
    if dropped.is_empty() {
        return None;
    }
    let msg = format!(
        "the filesystem-mem surface does not implement: {}. These params were \
         refused, not silently ignored — pass them only to the unified engine \
         (mem-repo MCP / CLI), or omit them.",
        dropped.join(", ")
    );
    Some(tool_error_with_details(
        "UNSUPPORTED_PARAM",
        &msg,
        Some(serde_json::json!({ "params": dropped })),
    ))
}

/// Map an [`EngineError`] to an MCP error envelope. Codes match the
/// mem-repo error-code vocabulary (`HASH_MISMATCH`, `ENTITY_NOT_FOUND`,
/// `ENTITY_ALREADY_EXISTS`, etc.) so agents that handle mem-repo
/// errors get the same shape here.
///
/// Variants that should never trip on a single-mem filesystem-mem
/// boot path (`DuplicateMem`, `UnknownMem`, `ReadOnlyMount`) still
/// fall into a generic `INTERNAL` envelope so the wire shape is total —
/// a future bug that produced one of those wouldn't crash the handler.
/// Schema-resolution failures (`SchemaNotFound`, `SchemaResolverInit`)
/// surface as their own typed codes via [`EngineError::code()`] so
/// callers see the same wire contract here as on the mem-repo server.
fn engine_op_error(err: EngineError) -> CallToolResult {
    // Pre-compute the canonical Display string. Variants that take the
    // engine's Display rendering verbatim use this — variants that build
    // their own customised message (e.g. stub-aware `HashMismatch`) keep
    // doing so in-arm.
    let display = err.to_string();
    match err {
        EngineError::UnknownType {
            name,
            schema_ref,
            declared,
            suggestion,
        } => {
            let hint = suggestion
                .as_deref()
                .map(|s| format!(". Did you mean '{s}'?"))
                .unwrap_or_default();
            tool_error(
                "UNKNOWN_ENTITY_TYPE",
                &format!(
                    "unknown entity type '{name}' in schema '{schema_ref}'. \
                     Declared types: [{}]{hint}",
                    declared.join(", ")
                ),
            )
        }
        EngineError::InvalidTitle(slug_err) => {
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
                SlugError::TitleHasInvalidChars { input, invalid_chars, proposed_slug } => {
                    let invalid_chars_str: Vec<String> =
                        invalid_chars.iter().map(|c| c.to_string()).collect();
                    serde_json::json!({
                        "reason": reason,
                        "input": input,
                        "invalid_chars": invalid_chars_str,
                        "proposed_slug": proposed_slug,
                    })
                }
                SlugError::TitleHasControlChars { input, control_chars, proposed_slug } => {
                    let control_chars_str: Vec<String> =
                        control_chars.iter().map(|c| c.escape_default().to_string()).collect();
                    serde_json::json!({
                        "reason": reason,
                        "input": input,
                        "control_chars": control_chars_str,
                        "proposed_slug": proposed_slug,
                    })
                }
            };
            tool_error_with_details(
                "INVALID_TITLE",
                &format!("title is invalid: {slug_err}"),
                Some(details),
            )
        }
        EngineError::AlreadyExists { id } => {
            tool_error("ENTITY_ALREADY_EXISTS", &format!("entity already exists: {id}"))
        }
        EngineError::NotFound { id } => {
            tool_error("ENTITY_NOT_FOUND", &format!("entity not found: {id}"))
        }
        EngineError::HashMismatch { id, current, is_stub } => {
            // Stub-aware message — pre-fix code printed `current is `
            // with an empty trailing value when the entity was a stub
            // and misdirected toward hash-recovery. Surface
            // `details.is_stub` and a corrective-action message
            // ("pass expected_hash: \"\"") for stubs; the prior
            // contract holds for real entities.
            let message = if is_stub {
                format!(
                    "hash mismatch for {id} — entity is a stub (no content_hash); pass expected_hash: \"\" to operate on stubs"
                )
            } else {
                format!("hash mismatch for {id}: current is {current}")
            };
            let payload = serde_json::json!({
                "code": "HASH_MISMATCH",
                "message": message.clone(),
                "details": {
                    "id": id,
                    "current": current,
                    "is_stub": is_stub,
                },
            });
            // Same text-channel format as `tool_error_with_payload`:
            // `ERROR [<CODE>]: <message>`. Pre-Item-01 this site emitted
            // a JSON-stringified payload on the text channel.
            let text = format!("ERROR [HASH_MISMATCH]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::HasIncomingRefs { id, referrers } => {
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
            let message = display;
            let payload = serde_json::json!({
                "code": "HAS_INCOMING_REFS",
                "message": message.clone(),
                "details": { "id": id, "referrers": referrers_json },
            });
            let text = format!("ERROR [HAS_INCOMING_REFS]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::MemHasIncomingRefs { mem, referrers } => {
            // Single-mem filesystem boot path never produces
            // MemHasIncomingRefs in practice — mem-delete is a
            // full-only operation. The arm is here for exhaustiveness;
            // the envelope shape matches the full-side mapping so
            // wire-byte parity holds if the filesystem flavour ever
            // gains a mem-delete surface.
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
            let message = display;
            let payload = serde_json::json!({
                "code": "MEM_HAS_INCOMING_REFS",
                "message": message.clone(),
                "details": { "mem": mem, "referrers": referrers_json },
            });
            let text = format!("ERROR [MEM_HAS_INCOMING_REFS]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::CrossMemLinkNotAllowed { from_mem, to_mem } => tool_error(
            "CROSS_MEM_LINK_NOT_ALLOWED",
            &format!(
                "cross-mem link from `{from_mem}` to `{to_mem}` is not allowed by the workspace `[cross_mem_links]` policy"
            ),
        ),
        EngineError::CrossMemTargetNotFound { target_id, target_mem } => tool_error(
            "CROSS_MEM_TARGET_NOT_FOUND",
            &format!(
                "cross-mem target `{target_id}` is absent in read-only mem `{target_mem}` — auto-stub is unavailable across the read-only boundary"
            ),
        ),
        EngineError::CrossMemEdgeNotDeclared {
            source_schema,
            target_schema,
            rel_type,
            from_id,
            to_id,
        } => {
            let message = display;
            let payload = serde_json::json!({
                "code": "CROSS_MEM_EDGE_NOT_DECLARED",
                "message": message.clone(),
                "details": {
                    "source_schema": source_schema,
                    "target_schema": target_schema,
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                },
            });
            let text = format!("ERROR [CROSS_MEM_EDGE_NOT_DECLARED]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::RepairNotNeeded { id, recovery } => {
            let message = display;
            let payload = serde_json::json!({
                "code": "REPAIR_NOT_NEEDED",
                "message": message.clone(),
                "details": { "id": id, "recovery": recovery },
            });
            let text = format!("ERROR [REPAIR_NOT_NEEDED]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::RenameNoOp { id, new_title } => tool_error(
            "RENAME_NO_OP",
            &format!(
                "rename would not change the id of {id} — new title {new_title:?} produces the same slug"
            ),
        ),
        EngineError::WikiLinkWithoutRelation { from_id, missing } => {
            let message = display;
            let payload = serde_json::json!({
                "code": "WIKILINK_WITHOUT_RELATION",
                "message": message.clone(),
                "details": {
                    "from_id": from_id,
                    "missing": missing,
                },
            });
            let text = format!("ERROR [WIKILINK_WITHOUT_RELATION]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::RelationHasBodyLinks { from_id, to_id, rel_type, body_links } => {
            let message = display;
            let payload = serde_json::json!({
                "code": "RELATION_HAS_BODY_LINKS",
                "message": message.clone(),
                "details": {
                    "from_id": from_id,
                    "to_id": to_id,
                    "rel_type": rel_type,
                    "body_links": body_links,
                },
            });
            let text = format!("ERROR [RELATION_HAS_BODY_LINKS]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::RenamePartialFailure { committed_mems, failed_mem, failure_cause } => {
            let message = format!(
                "rename partial-failure: mem `{failed_mem}` aborted with cause {failure_cause:?} after {committed_mems:?} already committed — reload and retry, or reconcile manually"
            );
            let payload = serde_json::json!({
                "code": "RENAME_PARTIAL_FAILURE",
                "message": message.clone(),
                "details": {
                    "committed_mems": committed_mems,
                    "failed_mem": failed_mem,
                    "failure_cause": failure_cause,
                },
            });
            let text = format!("ERROR [RENAME_PARTIAL_FAILURE]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::RenameBlockedByCrossMemPolicy { ref from_mem, ref blocked_referrers } => {
            let message = err.to_string();
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
            let payload = serde_json::json!({
                "code": "RENAME_BLOCKED_BY_CROSS_MEM_POLICY",
                "message": message.clone(),
                "details": {
                    "from_mem": from_mem,
                    "blocked_referrers": entries,
                },
            });
            let text = format!("ERROR [RENAME_BLOCKED_BY_CROSS_MEM_POLICY]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::StubCannotRelate { id } => {
            let message = format!(
                "source entity {id} is a stub — promote it to a real entity via memstead_create first"
            );
            let payload = serde_json::json!({
                "code": "STUB_CANNOT_RELATE",
                "message": message.clone(),
                "details": { "id": id },
            });
            let text = format!("ERROR [STUB_CANNOT_RELATE]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::StubNotUpdatable { id } => {
            let message = format!(
                "entity {id} is a stub — promote it to a real entity via memstead_create first"
            );
            let payload = serde_json::json!({
                "code": "STUB_NOT_UPDATABLE",
                "message": message.clone(),
                "details": { "id": id },
            });
            let text = format!("ERROR [STUB_NOT_UPDATABLE]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::StubNotRenamable { id } => {
            let message = format!(
                "entity {id} is a stub — promote it to a real entity via memstead_create before renaming"
            );
            let payload = serde_json::json!({
                "code": "STUB_NOT_RENAMABLE",
                "message": message.clone(),
                "details": { "id": id },
            });
            let text = format!("ERROR [STUB_NOT_RENAMABLE]: {message}");
            let mut result = CallToolResult::error(vec![Content::text(text)]);
            result.structured_content = Some(payload);
            result
        }
        EngineError::InvalidEntityId { id, reason } => {
            let message = format!("entity id '{id}' is malformed: {reason}");
            tool_error_with_details(
                "INVALID_ENTITY_ID",
                &message,
                Some(serde_json::json!({ "id": id, "reason": reason })),
            )
        }
        EngineError::InvalidWikiLinkTarget {
            raw,
            suggested,
            section,
            link_source,
            reason,
        } => {
            let message = format!(
                "body wiki-link target '{raw}' in section '{section}' is not slug-form: {reason}"
            );
            tool_error_with_details(
                "INVALID_WIKI_LINK_TARGET",
                &message,
                Some(serde_json::json!({
                    "raw": raw,
                    "suggested": suggested,
                    "section": section,
                    "source": link_source,
                    "reason": reason,
                })),
            )
        }
        EngineError::InvalidWikiLinkMem { raw, section, reason } => {
            let message = format!(
                "body wiki-link mem prefix '{raw}' in section '{section}' is not a valid mem name: {reason}"
            );
            tool_error_with_details(
                "INVALID_MEM_NAME",
                &message,
                Some(serde_json::json!({
                    "raw": raw,
                    "section": section,
                    "reason": reason,
                })),
            )
        }
        EngineError::ConflictingSectionModes { section, modes } => {
            let message =
                format!("section {section:?} appears in multiple mutation modes: {modes:?}");
            tool_error_with_details(
                "CONFLICTING_SECTION_MODES",
                &message,
                Some(serde_json::json!({ "section": section, "modes": modes })),
            )
        }
        EngineError::RelationshipCycle {
            rel_type,
            from,
            to,
            existing_path,
            path_truncated,
        } => {
            let existing_path_json: Vec<String> =
                existing_path.iter().map(|id| id.to_string()).collect();
            let message = format!(
                "creating edge {rel_type} from '{from}' to '{to}' would close a cycle in the {rel_type} subgraph"
            );
            tool_error_with_details(
                "RELATIONSHIP_CYCLE",
                &message,
                Some(serde_json::json!({
                    "rel_type": rel_type,
                    "from": from.to_string(),
                    "to": to.to_string(),
                    "existing_path": existing_path_json,
                    "path_truncated": path_truncated,
                })),
            )
        }
        EngineError::SetAndUnsetConflict { keys } => {
            let message = format!("metadata keys appear in both set and unset: {keys:?}");
            tool_error_with_details(
                "SET_AND_UNSET_CONFLICT",
                &message,
                Some(serde_json::json!({ "keys": keys })),
            )
        }
        EngineError::RequiredFieldUnset {
            field,
            entity_type,
            field_description,
            enum_values,
            type_write_rules,
            on_create,
            missing,
        } => {
            // Path-aware wording — create path renders "not provided"
            // (caller never supplied the field), update path renders
            // "cannot unset" (caller asked to remove a required field).
            // The typed code stays `REQUIRED_FIELD_UNSET` on both
            // paths so code-key consumers branch unchanged.
            let message = if on_create {
                format!(
                    "required metadata field '{field}' not provided — type '{entity_type}' declares the field as required and has no default for it"
                )
            } else {
                format!("cannot unset required field '{field}' for type '{entity_type}'")
            };
            // `details.missing[]` carries every required-no-default
            // field unset on the create path.
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
            tool_error_with_details(
                "REQUIRED_FIELD_UNSET",
                &message,
                Some(serde_json::json!({
                    "field": field,
                    "entity_type": entity_type,
                    "field_description": field_description,
                    "enum_values": enum_values,
                    "type_write_rules": type_write_rules,
                    "missing": missing_json,
                })),
            )
        }
        EngineError::MissingRequiredSection {
            entity_type,
            missing_count,
            sections,
            type_guidance,
        } => {
            let message = format!(
                "missing {missing_count} required section(s) for type '{entity_type}'"
            );
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
            tool_error_with_details(
                "MISSING_REQUIRED_SECTION",
                &message,
                Some(serde_json::json!({
                    "entity_type": entity_type,
                    "missing_count": missing_count,
                    "sections": sections_json,
                    "type_guidance": type_guidance,
                })),
            )
        }
        EngineError::PatchSectionEmpty { section } => tool_error(
            "PATCH_SECTION_EMPTY",
            &format!("patch target section is empty: {section}"),
        ),
        EngineError::PatchOldNotFound {
            section,
            current_content,
            truncated,
        } => {
            let message = format!("patch `old` substring not found in {section}");
            tool_error_with_details(
                "PATCH_OLD_NOT_FOUND",
                &message,
                Some(serde_json::json!({
                    "section": section,
                    "current_content": current_content,
                    "truncated": truncated,
                })),
            )
        }
        // Codes follow `EngineError::code()` — the single wire-code
        // source every surface (full MCP, CLI, wasm) shares. These two
        // historically drifted (`MEM_WRITER_ERROR` / `PARSE_AFTER_WRITE`);
        // `lean_backend_and_parse_after_write_codes_follow_code_contract`
        // pins them to `code()` so a re-divergence fails the build.
        EngineError::Backend(e) => tool_error("MEM_ERROR", &e.to_string()),
        EngineError::ParseAfterWrite(e) => {
            tool_error("PARSE_ERROR", &format!("parse-after-write failed: {e}"))
        }
        EngineError::Parse(e) => tool_error("PARSE_ERROR", &e.to_string()),
        EngineError::Validation(v) => validation_envelope(v),
        // Schema-resolution, boot-path, and lifecycle variants surface
        // as their own typed codes so the wire contract matches the
        // mem-repo server. Pre-fix this set collapsed to `INTERNAL`
        // — a lean-fireable variant (multi-folder workspaces can trip
        // DuplicateMem / UnknownMem; cross_mem_links policy can
        // trip ReadOnlyMount; generic input validation produces
        // InvalidInput) shipped as INTERNAL instead of its typed code,
        // breaking the agent contract that the structured code matches
        // `EngineError::code()`.
        // Description-posture variants ship structured details so MCP
        // callers branch on `details.rel_type`/`details.from_id`/
        // `details.to_id` instead of parsing the message — bit-identical
        // wire shape with the full-server typed envelope.
        ref err @ EngineError::MissingRequiredDescription { ref rel_type, ref from_id, ref to_id } => {
            tool_error_with_details(
                "MISSING_REQUIRED_DESCRIPTION",
                &err.to_string(),
                Some(serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                })),
            )
        }
        ref err @ EngineError::DescriptionNotPermitted { ref rel_type, ref from_id, ref to_id } => {
            tool_error_with_details(
                "DESCRIPTION_NOT_PERMITTED",
                &err.to_string(),
                Some(serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                })),
            )
        }
        ref err @ EngineError::RelationManualAuthoringForbidden {
            ref rel_type,
            ref from_id,
            ref to_id,
            ref guidance,
        } => tool_error_with_details(
            "RELATION_MANUAL_AUTHORING_FORBIDDEN",
            &err.to_string(),
            Some(serde_json::json!({
                "rel_type": rel_type,
                "from_id": from_id,
                "to_id": to_id,
                "guidance": guidance,
            })),
        ),
        e @ EngineError::SchemaNotFound { .. }
        | e @ EngineError::SchemaResolverInit(_)
        | e @ EngineError::DuplicateMem(_)
        | e @ EngineError::UnknownMem(_)
        | e @ EngineError::UnknownRef(_)
        | e @ EngineError::UnknownRemote(_)
        | e @ EngineError::LocalDivergence { .. }
        | e @ EngineError::NonFastForward { .. }
        | e @ EngineError::LocalInvalidState { .. }
        | e @ EngineError::SchemaViolationInFetch { .. }
        | e @ EngineError::PushedCommitsProtected { .. }
        | e @ EngineError::ReadOnlyMount(_)
        | e @ EngineError::Mem(_)
        | e @ EngineError::MemNameCollision { .. }
        | e @ EngineError::InvalidInput(_) => tool_error(e.code(), &e.to_string()),
        EngineError::RenameSimilarityOutOfRange {
            requested,
            allowed_min,
            allowed_max,
        } => tool_error_with_details(
            "INVALID_INPUT",
            &format!(
                "rename_similarity {requested} outside allowed range [{allowed_min}, {allowed_max}]"
            ),
            Some(serde_json::json!({
                "field": "rename_similarity",
                "requested": requested,
                "allowed_range": [allowed_min, allowed_max],
            })),
        ),
        EngineError::MemConfigIncomplete { mem, missing_fields } => {
            let message = format!(
                "mem `{mem}` config is missing required field(s) {missing_fields:?} — \
                 set via `memstead mem set-version {mem} <version>` (e.g. 0.1.0)"
            );
            tool_error_with_details(
                "MEM_CONFIG_INCOMPLETE",
                &message,
                Some(serde_json::json!({
                    "mem": mem,
                    "missing_fields": missing_fields,
                    "set_via": format!("memstead mem set-version {mem} <version>"),
                })),
            )
        }
        EngineError::SearchUnavailable => {
            tool_error_with_details(
                "SEARCH_UNAVAILABLE_IN_WASM",
                &display,
                Some(serde_json::json!({})),
            )
        }
        // Typed refusal when
        // `export_markdown` targets a mem whose active backend
        // doesn't support markdown regeneration. Filesystem-backed
        // workspaces don't reach this arm today (single folder mount),
        // but the variant must be handled to keep the match exhaustive.
        ref err @ EngineError::MarkdownExportUnsupportedBackend {
            ref mem,
            ref active_backend,
            ref supported_backends,
        } => tool_error_with_details(
            "MARKDOWN_EXPORT_UNSUPPORTED_BACKEND",
            &err.to_string(),
            Some(serde_json::json!({
                "mem": mem,
                "active_backend": active_backend,
                "supported_backends": supported_backends,
            })),
        ),
        ref err @ EngineError::EmptyUpdate { ref id } => tool_error_with_details(
            "EMPTY_UPDATE",
            &err.to_string(),
            Some(serde_json::json!({
                "id": id,
                "recognised_keys": [
                    "sections", "append_sections", "patch_sections",
                    "metadata", "metadata_unset", "declare_relations",
                ],
            })),
        ),
        // A bad `since` cursor on `memstead_changes_since`. The folder backend
        // keys `since` off timestamps rather than commit SHAs, so this
        // arm isn't reached on a filesystem-mem workspace today, but the
        // variant must be handled to keep the match exhaustive.
        ref err @ EngineError::InvalidChangesCursor { ref mem, ref since } => {
            tool_error_with_details(
                "INVALID_CURSOR",
                &err.to_string(),
                Some(serde_json::json!({ "mem": mem, "since": since })),
            )
        }
    }
}

/// Map a runtime [`memstead_base::runtime_validator::ValidationError`] to
/// the MCP wire envelope. Thin delegation to the shared
/// [`crate::error_envelopes::validation_envelope`] so the wire shape
/// stays bit-identical with the mem-repo `server.rs` handlers.
fn validation_envelope(
    err: memstead_base::runtime_validator::ValidationError,
) -> CallToolResult {
    crate::error_envelopes::validation_envelope(err)
}

#[tool_router(vis = "pub")]
impl FilesystemMcpServer {
    #[tool(
        name = "memstead_entity",
        description = "Read one entity as markdown (filesystem-mem flavour). Same JSON shape as the mem-repo `memstead_entity`. Frontmatter carries `_hash` (content hash) for optimistic locking on follow-up mutations.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn memstead_entity(&self, Parameters(p): Parameters<EntityParams>) -> CallToolResult {
        let engine = self.engine.lock().unwrap();
        let id = EntityId::canonical(&p.id);
        let entity = match engine.get_entity(&id) {
            Some(e) => e.clone(),
            None => {
                return tool_error("ENTITY_NOT_FOUND", &format!("entity not found: {id}"));
            }
        };
        let sections_filter = p.sections.as_deref();
        let mut md = render_entity_markdown(&entity, sections_filter);
        // Inject `_hash` as the first frontmatter field so callers
        // can pin it to `expected_hash` on the next mutation.
        if let Some(idx) = md.find("---\n") {
            let inject_at = idx + 4;
            let line = format!("_hash: {}\n", entity.content_hash);
            md.insert_str(inject_at, &line);
        }

        // Append `## Relations` when the caller asked for it. Mirrors
        // the mem-repo entity handler — outgoing + incoming edges
        // grouped by direction, rendered as a Markdown table.
        if p.include_relations.unwrap_or(false) {
            let outgoing = engine.store().outgoing(&id).to_vec();
            let incoming = engine.store().incoming(&id).to_vec();
            md.push_str(&memstead_base::render::render_relations_markdown(
                id.as_ref(),
                &outgoing,
                &incoming,
            ));
        }

        // Append `## Community Context` when the caller asked for it
        // — the entity's cluster + neighbour list. The community
        // detection is lazy (memoised per engine) and invalidated on
        // every successful mutation, so this is cheap on a static
        // graph and pays the Louvain cost once after each write.
        if p.include_context.unwrap_or(false)
            && let Some(ctx) = engine.context(&id)
        {
            let cluster_id = ctx
                .community
                .clone()
                .unwrap_or_else(|| "unknown".into());
            md.push_str(&memstead_base::render::render_community_context_section(
                &ctx,
                &cluster_id,
            ));
        }

        // Structured envelope
        // alongside the markdown text channel; same shape both MCP
        // flavours emit so agents branch on `structured_content`
        // uniformly regardless of which backend served the read.
        let rendered_body_tokens = memstead_base::chunking::estimate_tokens(&md);
        let full_tokens = if sections_filter.is_some() {
            let full_body = render_entity_markdown(&entity, None);
            Some(memstead_base::chunking::estimate_tokens(&full_body))
        } else {
            None
        };
        let structured = memstead_base::render::build_entity_envelope(
            &entity,
            rendered_body_tokens,
            full_tokens,
            sections_filter,
            None,
            engine.store().outgoing(&entity.id),
        );
        md_with_structured(md, structured)
    }

    #[tool(
        name = "memstead_create",
        description = "Create a new entity in the filesystem-mem workspace. Required: `title`, `entity_type`. Optional `sections`, `metadata`, `note`, `mem`. `mem` selects the target mount; omit it to land in the default writable mem (the first writable mount in declaration order). A create aimed at a read-only mount is refused with READ_ONLY_MOUNT. The `note` lands in `.memstead/changes.jsonl` (the filesystem-mem analogue of the mem-repo commit body). `relations` and `dry_run` are not implemented on this surface: passing a non-empty `relations` or `dry_run: true` is REFUSED up front with `UNSUPPORTED_PARAM` (`details.params` names them), never silently ignored — so a `dry_run` preview can never accidentally land a real write. Omit them, or use the unified engine (mem-repo MCP / CLI) which honours both.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    fn memstead_create(&self, Parameters(p): Parameters<CreateParams>) -> CallToolResult {
        // Part B: this surface hardwires `dry_run` off and ignores inline
        // `relations`. Refuse up front when either was meaningfully supplied
        // rather than committing a real write the agent thought was a preview
        // (or dropping edges it thought it wired).
        if let Some(err) = reject_unsupported_params(&[
            ("dry_run", p.dry_run == Some(true)),
            (
                "relations",
                p.relations.as_ref().is_some_and(|r| !r.is_empty()),
            ),
        ]) {
            return err;
        }
        let mut engine = self.engine.lock().unwrap();
        let (actor, client) = self.actor_and_client();
        // Resolve the target mem. An explicit, non-empty `mem` is
        // honoured verbatim — so a multi-mount engine (e.g. a read-only
        // content mem alongside a writable sketch mem) can be targeted
        // by name, and a create aimed at a read-only mount surfaces the
        // engine's READ_ONLY_MOUNT refusal rather than being silently
        // redirected to the writable mount. Omitted → the default writable
        // mem (first writable mount in declaration order), falling back to
        // the first mount so a read-only-only engine still resolves a name
        // (the create then refuses with READ_ONLY_MOUNT, never panics on an
        // empty mem). Single-mem filesystem workspaces are unaffected:
        // the sole mem is both the first and the default writable one.
        let mem = match p.mem.as_deref() {
            Some(v) if !v.is_empty() => v.to_string(),
            _ => engine
                .default_writable_mem()
                .or_else(|| engine.mem_names().into_iter().next())
                .map(String::from)
                .unwrap_or_default(),
        };
        let args = CreateEntityArgs {
            mem,
            title: p.title,
            entity_type: p.entity_type,
            sections: p.sections.unwrap_or_default(),
            metadata: p.metadata.unwrap_or_default(),
            // The filesystem-mem MCP surface doesn't (yet)
            // accept inline relations on the wire — pass empty;
            // operators wire edges via memstead_relate post-create.
            relations: Vec::new(),
            // dry_run not exposed on the filesystem-mem tool
            // surface; operators preview changes by reading first
            // and inspecting on the agent side.
            dry_run: false,
        };
        match engine.create_entity(args, actor, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                // WarningHint's Serialize impl produces the same
                // `{code, message, details}` envelope the manual
                // synthesis used to emit. commit_sha + title +
                // mem are now first-class on the outcome.
                let durable = mem_is_durable(&engine, &outcome.mem);
                let body = serde_json::json!({
                    "id": outcome.id.to_string(),
                    "title": outcome.title,
                    "mem": outcome.mem,
                    "file_path": outcome.file_path,
                    "_hash": outcome.content_hash,
                    "commit_sha": outcome.commit_sha,
                    "durable": durable,
                    "warnings": outcome.warnings,
                    "type_guidance": outcome.type_guidance,
                });
                json_response(&body)
            }
            Err(e) => engine_op_error(e),
        }
    }

    #[tool(
        name = "memstead_update",
        description = "Update an existing entity in the filesystem-mem workspace. `expected_hash` (from a previous memstead_entity read) is required — mismatch returns code HASH_MISMATCH with details.current carrying the live hash. This surface honours `sections` (replace) + `metadata` (set) + `metadata_unset` + `declare_relations` + `relations_unset`. The mem-repo `append_sections` / `patch_sections` / `dry_run` shapes are NOT implemented here: passing a non-empty `append_sections` / `patch_sections`, or `dry_run: true`, is REFUSED up front with `UNSUPPORTED_PARAM` (`details.params` names them), never silently ignored — an agent that patches is told its patch was dropped instead of believing it applied. Omit them, or use the unified engine (mem-repo MCP / CLI) which honours all three.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    fn memstead_update(&self, Parameters(p): Parameters<UpdateParams>) -> CallToolResult {
        // Part B: this surface hardwires `append_sections` / `patch_sections`
        // off and `dry_run` off. Refuse up front when any was meaningfully
        // supplied rather than dropping the edit silently (an agent that
        // patched believes it patched). `sections` (replace), `metadata`,
        // `metadata_unset`, `declare_relations`, and `relations_unset` ARE
        // honoured and pass through untouched.
        if let Some(err) = reject_unsupported_params(&[
            ("dry_run", p.dry_run == Some(true)),
            (
                "append_sections",
                p.append_sections.as_ref().is_some_and(|m| !m.is_empty()),
            ),
            (
                "patch_sections",
                p.patch_sections.as_ref().is_some_and(|m| !m.is_empty()),
            ),
        ]) {
            return err;
        }
        let mut engine = self.engine.lock().unwrap();
        let (actor, client) = self.actor_and_client();
        let args = UpdateEntityArgs {
            relations_unset: p
                .relations_unset
                .unwrap_or_default()
                .into_iter()
                .map(|r| memstead_base::ops::RelationUnsetArg {
                    rel_type: r.rel_type,
                    target: memstead_base::EntityId(r.target),
                })
                .collect(),
            id: EntityId(p.id),
            expected_hash: Some(p.expected_hash),
            sections: p.sections.unwrap_or_default(),
            // The filesystem-mem tool doesn't expose
            // append_sections / patch_sections on the wire yet;
            // pass empty.
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            metadata: p.metadata.unwrap_or_default(),
            metadata_unset: p.metadata_unset.unwrap_or_default(),
            declare_relations: p
                .declare_relations
                .unwrap_or_default()
                .into_iter()
                .map(|r| memstead_base::ops::RelateArg {
                    rel_type: r.r#type,
                    to: EntityId(r.to),
                    description: r.description,
                })
                .collect(),
            dry_run: false,
        };
        match engine.update_entity(args, actor, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                let durable = mem_is_durable(&engine, outcome.id.mem());
                let body = serde_json::json!({
                    "id": outcome.id.to_string(),
                    "file_path": outcome.file_path,
                    "_hash": outcome.content_hash,
                    "durable": durable,
                    "modified_sections": outcome.modified_sections.replaced,
                    "modified_metadata_set": outcome.modified_metadata.set,
                    "modified_metadata_unset": outcome.modified_metadata.unset,
                    // Typed warnings ride out on `outcome.warnings` —
                    // the engine emits `NOTE_MISSING` here under
                    // `[mutations].require_notes`, matching create/relate.
                    "warnings": outcome.warnings,
                    // Orphan-stub GC: when this update removed a body
                    // wiki-link that was a stub target's last referrer,
                    // the engine GC'd the stub and lists it here. Always
                    // present (empty array when nothing orphaned),
                    // matching the relate-remove and delete shape so
                    // consumers don't branch on field presence.
                    "orphan_stubs_removed": outcome
                        .orphan_stubs_removed
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>(),
                });
                json_response(&body)
            }
            Err(e) => engine_op_error(e),
        }
    }

    #[tool(
        name = "memstead_delete",
        description = "Delete an entity from the filesystem-mem workspace. `expected_hash` is required (read first via memstead_entity); mismatch returns HASH_MISMATCH. Refuses entities with incoming references unless the agent passes `force` via the workspace-level config (v1 has no per-call force toggle on the MCP surface — use `memstead delete --force` on the CLI). The `note` lands in `.memstead/changes.jsonl`.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    fn memstead_delete(&self, Parameters(p): Parameters<DeleteParams>) -> CallToolResult {
        let mut engine = self.engine.lock().unwrap();
        let (actor, client) = self.actor_and_client();
        let args = DeleteEntityArgs {
            id: EntityId(p.id),
            // The MCP delete shape carries a required `expected_hash` String;
            // an empty string is the documented "stub delete" path. The
            // engine takes Option — pass `None` only for empty
            // strings to preserve the no-hash-check semantics.
            expected_hash: if p.expected_hash.is_empty() {
                None
            } else {
                Some(p.expected_hash)
            },
        };
        match engine.delete_entity(args, actor, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                let durable = mem_is_durable(&engine, outcome.id.mem());
                let body = serde_json::json!({
                    "id": outcome.id.to_string(),
                    "file_path": outcome.file_path,
                    "removed_incoming": outcome.removed_incoming,
                    "durable": durable,
                    // Engine-emitted warnings (residual-stub demotion,
                    // and `NOTE_MISSING` under `require_notes`).
                    "warnings": outcome.warnings,
                });
                json_response(&body)
            }
            Err(e) => engine_op_error(e),
        }
    }

    #[tool(
        name = "memstead_relate",
        description = "Add or remove a typed relationship between two entities in the same filesystem-mem. Cross-mem targets are rejected with CROSS_MEM_RELATION (filesystem-mem is single-mem by design). `remove: true` drops the matching pair if present; otherwise the call appends. No-op paths (already present add, absent remove) succeed silently and do not append a changelog line.",
        // idempotent_hint = true: relate's duplicate-add and
        // remove-nonexistent paths are typed-warning no-ops, so a retry
        // converges. Matches the mem-repo server's annotation —
        // `relate_annotation_is_idempotent_on_both_flavours` pins parity.
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn memstead_relate(&self, Parameters(p): Parameters<RelateParams>) -> CallToolResult {
        let mut engine = self.engine.lock().unwrap();
        let (actor, client) = self.actor_and_client();
        // Relate is hash-stable: the source's content_hash does not
        // change under add/remove. The mem-repo tool surface omits
        // `expected_hash` for relate, so we pass `None` and let the
        // engine's single-writer assumption stand.
        let args = RelateEntityArgs {
            source: EntityId(p.from),
            expected_hash: None,
            rel_type: p.r#type,
            target: EntityId(p.to),
            remove: p.remove.unwrap_or(false),
            description: p.description,
        };
        match engine.relate_entity(args, actor, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                let action = match outcome.action {
                    RelateAction::Added => "added",
                    RelateAction::Removed => "removed",
                    RelateAction::NoOpAlreadyPresent => "no_op_already_present",
                    RelateAction::NoOpAbsent => "no_op_absent",
                };
                // Typed warnings ride out on `outcome.warnings` —
                // open-mode admissions, duplicate-add no-ops,
                // remove-nonexistent no-ops, and auto-stub creation
                // (the `AUTO_STUB_CREATED` entry retired the
                // deprecated top-level `stub_warning` field in Item
                // 03). WarningHint's Serialize impl produces the
                // `{ code, message, details }` envelope shared with
                // the mem-repo `RelateResult`.
                // Surface `orphan_stubs_removed` so the lean surface
                // matches full on the relate response shape.
                let durable = mem_is_durable(&engine, outcome.from.mem());
                let body = serde_json::json!({
                    "from": outcome.from.to_string(),
                    "to": outcome.to.to_string(),
                    "type": outcome.rel_type,
                    "action": action,
                    "source": outcome.source,
                    "_hash": outcome.content_hash,
                    "commit_sha": outcome.commit_sha,
                    "durable": durable,
                    "warnings": outcome.warnings,
                    "orphan_stubs_removed": outcome
                        .orphan_stubs_removed
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>(),
                });
                json_response(&body)
            }
            Err(e) => engine_op_error(e),
        }
    }

    #[tool(
        name = "memstead_search",
        description = "Search entities by lexical content + structural filters. Same JSON shape as the mem-repo `memstead_search`. The first call after engine init or any mutation pays a one-time search-index build (scales with entity count); subsequent calls reuse the cache. Pass an empty `query: {}` (or omit it) for a metadata-only structural filter — the list shape folds in here. Filters: `mem`, `entity_type`, `edge_type` (first-class engine axes), `stub`, plus `filters: { <field>: <value> }` for any schema-declared `filterable: equality` field (e.g. `{\"level\": \"M0\", \"tags\": \"auth\"}`). Strict type-narrowing: an entity whose type doesn't declare a *filterable* field is excluded (warning `FILTER_TYPE_SCOPED`); a field declared but not filterable on any reachable type is ignored — the result equals the same search without it (warning `FIELD_NOT_FILTERABLE`), never emptied, in both the scoped and unscoped case; a key no schema declares is ignored (`UNKNOWN_FILTER_KEY`). Pagination via `limit` / `offset`. Section bodies are not shipped per hit — read them with `memstead_entity`. A page is bounded to `token_budget` (default 12000): an overflowing page returns the highest-ranked hits that fit with a `SEARCH_RESULTS_TRUNCATED` warning (`kept`/`budget`) while `_total` stays the full count — page on with `offset` or raise `token_budget`.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn memstead_search(&self, Parameters(p): Parameters<SearchParams>) -> CallToolResult {
        let engine = self.engine.lock().unwrap();
        let filters = p.filters.unwrap_or_default();
        let scope = SearchScope {
            query: p.query,
            mem: p.mem,
            entity_type: p.entity_type,
            limit: p.limit,
            offset: p.offset,
            filters,
            // Thread the
            // agent's `range_filters` through to the engine arg —
            // mirrors the full server's wiring so both servers expose the
            // typed range-filter warnings the engine already produces.
            range_filters: p.range_filters.unwrap_or_default(),
            edge_type: p.edge_type,
            related_to: p.related_to.map(EntityId),
            depth: p.depth,
            expand_via: p.expand_via,
            expand_depth: p.expand_depth,
            stub: p.stub,
            token_budget: p.token_budget,
        };
        let offset = scope.offset.unwrap_or(0);
        let result = match engine.search(&scope) {
            Ok(r) => r,
            Err(e) => return engine_op_error(e),
        };
        let md = render_search_markdown(&result, offset);
        // Structured envelope
        // on `structured_content`, rendered markdown on the text
        // channel; lean MCP mirrors full's split so cross-flavour
        // agents see the same wire contract.
        let envelope = memstead_base::render::build_search_envelope(&result, offset);
        let structured =
            serde_json::to_value(&envelope).unwrap_or(serde_json::Value::Null);
        md_with_structured(md, structured)
    }

    #[tool(
        name = "memstead_health",
        description = "Health summary for the filesystem-mem workspace: orphans, stubs, missing required fields, stale entities. Returns the same JSON shape as the mem-repo `memstead_health` (single-mem, so `writable_mems` carries one entry). Detail sections are produced via the kernel's `compute_health`. `include` accepts the shared health key set — today the lean surface dispatches `dangling_links` (matching the mem-repo response shape: `{from, target_id, target_path, section}`) and validates every key against the allowed set, emitting `UNKNOWN_INCLUDE_KEY` on the response's `warnings[]` for typos. `conformance` / `integrity` are dispatched too: `conformance` lints every entity against the effective schema (the pin, or `target_schema` when given) into a `findings` array of `{id, axis, code, detail}` with write-time typed codes; `integrity` adds the consistency axis (DANGLING_LINK, ORPHAN_STUB) to the same list. Other detail keys (`orphans`, `stubs`, …) are accepted but the v1 surface returns the full report regardless — narrowing is a follow-up.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn memstead_health(&self, Parameters(p): Parameters<HealthParams>) -> CallToolResult {
        let engine = self.engine.lock().unwrap();
        let mut health = engine.health();
        let include = p.include.unwrap_or_default();

        // Validate include keys against the shared catalogue. Unknown
        // keys surface as a typed `UNKNOWN_INCLUDE_KEY` warning — the
        // same shape full emits and the same shape the CLI consumes.
        for key in &include {
            if !memstead_base::ops::health::HEALTH_INCLUDE_KEYS.contains(&key.as_str()) {
                health.warnings.push(memstead_base::WarningHint::UnknownIncludeKey {
                    key: key.clone(),
                    allowed: memstead_base::ops::health::HEALTH_INCLUDE_KEYS
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                });
            }
        }

        // `dangling_links` opt-in: the engine's `HealthSummary` carries
        // a `dangling_links: Option<...>` slot that handlers populate
        // (the kernel's `compute_health` leaves it `None`). Populating
        // it from the lean surface gives agents the documented
        // include-key without forcing them through the full engine.
        if include.iter().any(|s| s == "dangling_links") {
            let dangling = memstead_base::ops::health::collect_dangling_links(engine.store(), None);
            health.dangling_links = Some(dangling);
        }

        // Conformance axis (`conformance`), or both axes
        // (`integrity`) — same `findings` slot and `{ id, axis, code,
        // detail }` shape as the mem-repo flavour. Filesystem-mem
        // is single-mem: the scan covers the one mounted mem.
        if include.iter().any(|s| s == "conformance" || s == "integrity") {
            let target: Option<memstead_schema::SchemaRef> = match p.target_schema.as_deref() {
                None => None,
                Some(raw) => match raw.parse::<memstead_schema::SchemaRef>() {
                    Ok(r) => Some(r),
                    Err(reason) => {
                        return tool_error(
                            "INVALID_INPUT",
                            &format!("invalid target_schema {raw:?}: {reason}"),
                        );
                    }
                },
            };
            let mut mem_names: Vec<String> = engine.schemas().keys().cloned().collect();
            mem_names.sort();
            let mut findings = Vec::new();
            for v in &mem_names {
                match engine.conformance_findings(v, target.as_ref()) {
                    Ok(f) => findings.extend(f),
                    Err(e) => {
                        return tool_error(e.code(), &e.to_string());
                    }
                }
                if include.iter().any(|s| s == "integrity") {
                    match engine.consistency_findings(v) {
                        Ok(f) => findings.extend(f),
                        Err(e) => {
                            return tool_error(e.code(), &e.to_string());
                        }
                    }
                }
            }
            health.findings = Some(findings);
        }

        json_response(&health)
    }

    #[tool(
        name = "memstead_schema",
        description = "Read the workspace's pinned schema as a JSON document — `ref` (canonical `name@version`), `relationship_mode`, the relationship vocabulary, `community`, `used_by[]`, top-level `origin` (`first-party` / `third-party`; a third-party schema is served structural-only with its prose-instruction fields omitted), top-level `alias_target_rel_type` (when authored — the rel-type body wiki-links `[[target]]` auto-emit), and per-type section/field detail. Accepts either `name` (bare name or canonical pin) or `mem` (the workspace's single mem). Passing both is `INVALID_INPUT`. v1 surface returns the engine's pinned schema regardless of which form is used (filesystem-mem is single-mem, single-schema). Pass `verbosity: \"lite\"` for a cheap cold-start skeleton — entity-type names + section keys + field shapes, relationship names + endpoints, the alias pointer, prose dropped (heavy arrays ship as `types_summary`/`relationships_summary`); default `\"full\"`. An unrecognized `verbosity` returns `INVALID_INPUT`. Returns `ENTITY_NOT_FOUND` when `name` explicitly mismatches the pinned schema; `UNKNOWN_MEM` when `mem` is not the workspace's mem.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn memstead_schema(&self, Parameters(p): Parameters<SchemaParams>) -> CallToolResult {
        let engine = self.engine.lock().unwrap();
        // filesystem-mem is single-mem by design; the new engine
        // carries one schemas[] entry. Pick it.
        let Some((mem_name, schema)) = engine.schemas().iter().next() else {
            // Genuinely-systemic: the filesystem boot path always
            // mounts exactly one mem and pins exactly one schema. An
            // empty `schemas()` map means engine construction itself
            // is inconsistent — no agent-side recovery applies, so
            // `INTERNAL` is the honest wire code (this class of
            // genuinely-systemic failure is the legitimate `INTERNAL`
            // use case).
            return tool_error(
                "INTERNAL",
                "engine has no schemas — workspace mount list is empty",
            );
        };
        let pinned_name = &schema.manifest.name;
        let pinned_version = schema.version.to_string();
        let canon = format!("{pinned_name}@{pinned_version}");

        // Validate (`name`, `mem`) input shape; either resolves to a
        // string the lookup below checks against the pinned schema.
        // The `mem` path is provided for parity with the mem-repo
        // flavour; in filesystem-mem the single mem always maps
        // to the single schema.
        let want_owned: String = match (p.name.as_deref(), p.mem.as_deref()) {
            (Some(_), Some(_)) => {
                return tool_error(
                    "INVALID_INPUT",
                    "memstead_schema accepts exactly one of `name` or `mem`, not both.",
                );
            }
            (Some(name), None) => name.trim().to_string(),
            (None, Some(mem)) => {
                if mem != mem_name.as_str() {
                    return tool_error(
                        "UNKNOWN_MEM",
                        &format!(
                            "unknown mem: {mem:?} — workspace mounts {mem_name:?}"
                        ),
                    );
                }
                String::new() // matches pinned schema by default
            }
            (None, None) => String::new(),
        };
        let want = want_owned.as_str();
        let matches = want.is_empty()
            || want == pinned_name.as_str()
            || want == canon.as_str();
        if !matches {
            return tool_error(
                "ENTITY_NOT_FOUND",
                &format!(
                    "schema not found: {want:?} — workspace pins {canon}"
                ),
            );
        }

        // Verbosity toggle — `full` (default) or the `lite` skeleton.
        // An unrecognized value refuses with a typed INVALID_INPUT
        // naming the bad value rather than silently falling back.
        let verbosity = match p.verbosity.as_deref() {
            None => memstead_base::render::SchemaVerbosity::Full,
            Some(v) => match memstead_base::render::SchemaVerbosity::from_wire(v) {
                Some(sv) => sv,
                None => {
                    return tool_error(
                        "INVALID_INPUT",
                        &format!("unknown verbosity: {v:?} — expected \"full\" or \"lite\""),
                    );
                }
            },
        };

        // One shared, transport-neutral builder for every schema-read
        // surface (mem-repo MCP, the HTTP `/api/schema` endpoint, and
        // this filesystem-mem flavour) — no second divergent renderer
        // to drift. `ref` carries the canonical `name@version`, and
        // `alias_target_rel_type` rides along, so the public surface now
        // advertises the body-wiki-link edge-authoring rule the full
        // schema response always carried.
        // Trust origin governs de-framing: a third-party schema is served
        // structural-only regardless of the requested `verbosity`.
        let origin = engine.schema_origin(schema);
        let payload = memstead_base::render::build_schema_payload(
            schema,
            vec![mem_name.to_string()],
            verbosity,
            origin,
        );
        json_response(&payload)
    }


    #[tool(
        name = "memstead_diff",
        description = "Two-ref structural diff at entity granularity. **Filesystem-mem flavour:** folder mounts carry no git refs, so this tool refuses with `INVALID_INPUT` against folder-backed mems. Use the mem-repo flavour for the real diff; the surface stays for cross-flavour clients that hit either server.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn memstead_diff(&self, Parameters(p): Parameters<DiffParams>) -> CallToolResult {
        let engine = self.engine.lock().unwrap();
        let config = memstead_base::ops::DiffConfig {
            rename_similarity: p
                .rename_similarity
                .unwrap_or(memstead_base::ops::RENAME_SIMILARITY_DEFAULT),
            include_content: p.include_content,
            include_ripple: p.include_ripple,
        };
        match engine.diff(&p.mem, &p.ref_a, &p.ref_b, Some(config)) {
            Ok(diff) => json_response(&diff),
            Err(e) => engine_op_error(e),
        }
    }

    #[tool(
        name = "memstead_changes_since",
        description = "Read the per-mutation changelog at `.memstead/changes.jsonl` since a given RFC 3339 timestamp. **Diverges from the mem-repo flavour** — filesystem-mem has no commit history, so `since` is a timestamp string (e.g. `\"2026-05-08T15:30:00.000Z\"`) and the response yields the JSONL entries with `ts > since` as a structured array. Pass an empty string or the UNIX epoch (`\"1970-01-01T00:00:00.000Z\"`) for a full dump. The `mem` field is accepted for shape compatibility with the mem-repo flavour but ignored — single-mem. `rename_similarity` and `include_notes` are also accepted but ignored.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn memstead_changes_since(&self, Parameters(p): Parameters<ChangesSinceParams>) -> CallToolResult {
        // The unified engine doesn't expose a workspace_root accessor
        // (mounts can be heterogeneous); use the captured field.
        let log_path = self
            .workspace_root
            .join(memstead_base::MEM_META_DIR)
            .join("changes.jsonl");
        let raw = match std::fs::read_to_string(&log_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return tool_error("CHANGELOG_ERROR", &e.to_string());
            }
        };

        let since = p.since.trim();
        let mut entries: Vec<serde_json::Value> = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: serde_json::Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue, // skip malformed lines silently
            };
            let ts_match = value
                .get("ts")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            if !since.is_empty() && ts_match.as_str() <= since {
                continue;
            }
            entries.push(value);
        }
        let payload = serde_json::json!({
            "since": since,
            "count": entries.len(),
            "entries": entries,
        });
        json_response(&payload)
    }

    #[tool(
        name = "memstead_rename",
        description = "Rename an entity by changing its title. The slug, id, and on-disk file path follow. `expected_hash` is required. Atomic referrer rewrite: every Write-Mem entity whose relationships or section bodies point at the old id has its `[[old-slug]]` tokens rewritten in one per-mem commit; ReadOnly referrers leave a residual stub at the old id holding the surviving incoming edges.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    fn memstead_rename(&self, Parameters(p): Parameters<RenameParams>) -> CallToolResult {
        let mut engine = self.engine.lock().unwrap();
        let (actor, client) = self.actor_and_client();
        let args = RenameEntityArgs {
            id: EntityId(p.id),
            expected_hash: Some(p.expected_hash),
            new_title: p.new_title,
        };
        match engine.rename_entity(args, actor, client.as_ref(), p.note.as_deref()) {
            Ok(outcome) => {
                let durable = mem_is_durable(&engine, outcome.new_id.mem());
                let body = serde_json::json!({
                    "old_id": outcome.old_id.to_string(),
                    "new_id": outcome.new_id.to_string(),
                    "old_file_path": outcome.old_path,
                    "new_file_path": outcome.new_path,
                    "_hash": outcome.content_hash,
                    "durable": durable,
                    // Engine-emitted warnings (slug-noop, and
                    // `NOTE_MISSING` under `require_notes`).
                    "warnings": outcome.warnings,
                });
                json_response(&body)
            }
            Err(e) => engine_op_error(e),
        }
    }

    #[tool(
        name = "memstead_overview",
        description = "Cold-start entry point for filesystem-mem workspaces. Returns the schema catalogue, the (single) mem entry, and the community clusters as Markdown. Schemas list as `{ref, description}` only — call `memstead_schema(name=<ref>)` for full per-type bodies. Token-budget-driven: hard-required content (mem, schema, community titles) always ships; heavy content greedy-fills the remaining budget by default-priority. Anything that didn't fit is advertised under `## Hints` with `estimated_tokens`; re-query by passing `key` into `include[]`. Allowed `include` keys: `community_members`, `community_bridges`, `mem_distribution`, `dangling_links`. `mem` parameter is accepted for shape compatibility but only the workspace's single mem matches; an unknown name returns an error. Set `rebuild: true` to invalidate the community memo before computing — it recomputes the whole-graph Louvain partition (detection is global; there is no per-subgraph scoping). A small or disconnected subgraph may surface as no cluster: sparsely-connected / edge-less nodes collapse into a single catch-all rather than forming their own cluster, so building a handful of loosely-linked entities and expecting a distinct cluster will come back empty. The `## Mems` entry carries a `durable` flag and `storage` kind — on this in-memory sketch it reads `durable: false` / `storage: in-memory`, i.e. writes are volatile and evicted on session-TTL / restart. The mem-repo `## Lifecycle Namespaces` section is omitted — filesystem-mem has no mem-creation rules. Frontmatter `_overview_mode` is `\"complete\"`, `\"reduced\"`, or `\"overbudget\"`.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    fn memstead_overview(&self, Parameters(p): Parameters<OverviewParams>) -> CallToolResult {
        const DEFAULT_OVERVIEW_BUDGET: usize = 8_000;
        const ALLOWED_OVERVIEW_INCLUDE: &[&str] = &[
            "community_members",
            "community_bridges",
            "mem_distribution",
            "dangling_links",
        ];

        // Schema-tool split: the legacy `schema_types` include key is gone;
        // surface the same actionable error the mem-repo flavour does so an agent that
        // still passes it gets a clear hint to call memstead_schema instead.
        if let Some(include) = p.include.as_ref()
            && include.iter().any(|k| k == "schema_types")
        {
            let msg = "include key 'schema_types' was removed; \
                       call memstead_schema(name=...) for full schema bodies."
                .to_string();
            return tool_error("INVALID_INPUT", &msg);
        }

        let mut engine = self.engine.lock().unwrap();
        if p.rebuild.unwrap_or(false) && p.chunk.unwrap_or(1) <= 1 {
            engine.invalidate_communities();
        }

        // --- Mem filter validation ---
        // filesystem-mem has exactly one mem. A non-matching `mem`
        // is a typed error — same shape as the mem-repo surface.
        let mem_name = engine
            .mem_names()
            .into_iter()
            .next()
            .map(String::from)
            .unwrap_or_default();
        if let Some(v) = p.mem.as_deref()
            && v != mem_name
        {
            return tool_error(
                "INVALID_INPUT",
                &format!(
                    "unknown mem: \"{v}\". Writable mems: [{mem_name}]"
                ),
            );
        }

        // --- include validation ---
        let budget = p.token_budget.unwrap_or(DEFAULT_OVERVIEW_BUDGET);
        let include: Vec<String> = p.include.clone().unwrap_or_default();
        let mut warnings: Vec<(String, String)> = Vec::new();
        for key in &include {
            if !ALLOWED_OVERVIEW_INCLUDE.contains(&key.as_str()) {
                warnings.push((
                    "UNKNOWN_INCLUDE_KEY".to_string(),
                    format!(
                        "unknown include key: \"{key}\". Allowed: {}",
                        ALLOWED_OVERVIEW_INCLUDE.join(", ")
                    ),
                ));
            }
        }
        let include_set: std::collections::BTreeSet<&'static str> = include
            .iter()
            .filter_map(|k| {
                ALLOWED_OVERVIEW_INCLUDE
                    .iter()
                    .find(|a| **a == k.as_str())
                    .copied()
            })
            .collect();

        // --- Schema (single entry) ---
        let Some((_, schema)) = engine.schemas().iter().next() else {
            // Genuinely-systemic: the filesystem boot path always
            // mounts exactly one mem and pins exactly one schema. An
            // empty `schemas()` map means engine construction itself
            // is inconsistent — no agent-side recovery applies, so
            // `INTERNAL` is the honest wire code (this class of
            // genuinely-systemic failure is the legitimate `INTERNAL`
            // use case).
            return tool_error(
                "INTERNAL",
                "engine has no schemas — workspace mount list is empty",
            );
        };
        let schema_canon = format!(
            "{}@{}",
            schema.manifest.name, schema.version
        );
        let schemas_slim = vec![serde_json::json!({
            "ref": schema_canon,
            "description": schema.manifest.description,
        })];

        // --- Mem (single entry, lite + full variants) ---
        let mut entity_count: usize = 0;
        let mut type_dist: std::collections::BTreeMap<String, usize> = Default::default();
        for e in engine.store().all_entities() {
            if e.stub || e.mem != mem_name {
                continue;
            }
            entity_count += 1;
            *type_dist.entry(e.entity_type.clone()).or_default() += 1;
        }
        // Per-mem storage backend → durability marker, derived from the
        // mount's `MountStorage` kind. On the ephemeral sketch this reads
        // `in-memory` / `durable: false`, so an agent learns the mem is
        // volatile (writes evicted on session-TTL / restart) before its
        // first write rather than after a reset.
        let (storage, durable) = engine
            .mounts()
            .iter()
            .find(|m| m.mem == mem_name)
            .map(|m| (m.storage.backend_id(), m.storage.is_durable()))
            .unwrap_or(("unknown", false));
        let mems_lite = vec![serde_json::json!({
            "name": mem_name,
            "schema": schema_canon,
            "entity_count": entity_count,
            "storage": storage,
            "durable": durable,
        })];
        let mems_full = vec![serde_json::json!({
            "name": mem_name,
            "schema": schema_canon,
            "entity_count": entity_count,
            "type_distribution": type_dist,
            "storage": storage,
            "durable": durable,
        })];

        // --- Communities ---
        let output = engine.communities();
        let cluster_count = output.count;
        let entity_count_total: usize = output.clusters.values().map(|c| c.entities.len()).sum();
        let modularity = output.modularity;

        let mut cluster_ids: Vec<String> = output.clusters.keys().cloned().collect();
        cluster_ids.sort();

        let mut communities_lite: Vec<serde_json::Value> = Vec::with_capacity(cluster_ids.len());
        let mut communities_full: Vec<serde_json::Value> = Vec::with_capacity(cluster_ids.len());
        for cid in &cluster_ids {
            let info = &output.clusters[cid];
            let summary = memstead_base::graph::community::generate_auto_summary(
                engine.store(),
                &info.entities,
            );
            communities_lite.push(serde_json::json!({
                "cluster_id": cid,
                "entity_count": info.entities.len(),
                "summary": summary,
            }));
            communities_full.push(serde_json::json!({
                "cluster_id": cid,
                "entity_count": info.entities.len(),
                "summary": summary,
                "members": info.entities,
            }));
        }

        // --- Bridges / dangling links ---
        let bridges_component: serde_json::Value = serde_json::to_value(
            memstead_base::graph::community::aggregate_bridges(engine.store(), output, None),
        )
        .unwrap_or(serde_json::Value::Array(Vec::new()));
        let dangling_links_component = serde_json::to_value(
            memstead_base::ops::health::collect_dangling_links(engine.store(), None),
        )
        .unwrap_or(serde_json::Value::Array(Vec::new()));

        // --- Costs ---
        let estimate_tokens = memstead_base::chunking::estimate_tokens;
        let hard_required_cost = estimate_tokens(
            &serde_json::to_string(&schemas_slim).unwrap_or_default(),
        ) + estimate_tokens(
            &serde_json::to_string(&mems_lite).unwrap_or_default(),
        ) + estimate_tokens(
            &serde_json::to_string(&communities_lite).unwrap_or_default(),
        );
        let overbudget = hard_required_cost > budget;

        let mem_distribution_component =
            serde_json::to_value(&mems_full).unwrap_or(serde_json::Value::Array(Vec::new()));
        let community_members_component = serde_json::to_value(&communities_full)
            .unwrap_or(serde_json::Value::Array(Vec::new()));

        let mem_distribution_cost = estimate_tokens(
            &serde_json::to_string(&mem_distribution_component).unwrap_or_default(),
        )
        .saturating_sub(estimate_tokens(
            &serde_json::to_string(&mems_lite).unwrap_or_default(),
        ));
        let community_members_cost = estimate_tokens(
            &serde_json::to_string(&community_members_component).unwrap_or_default(),
        )
        .saturating_sub(estimate_tokens(
            &serde_json::to_string(&communities_lite).unwrap_or_default(),
        ));
        let bridges_cost =
            estimate_tokens(&serde_json::to_string(&bridges_component).unwrap_or_default());
        let dangling_links_cost = estimate_tokens(
            &serde_json::to_string(&dangling_links_component).unwrap_or_default(),
        );

        // --- Greedy fill ---
        let candidates: [(&'static str, usize, serde_json::Value); 4] = [
            (
                "mem_distribution",
                mem_distribution_cost,
                mem_distribution_component,
            ),
            (
                "community_members",
                community_members_cost,
                community_members_component,
            ),
            ("community_bridges", bridges_cost, bridges_component),
            (
                "dangling_links",
                dangling_links_cost,
                dangling_links_component,
            ),
        ];

        let mut emitted: std::collections::BTreeMap<&'static str, serde_json::Value> =
            Default::default();
        let mut hints: Vec<(String, usize)> = Vec::new();
        let mut used = hard_required_cost;
        let mut remaining = budget.saturating_sub(hard_required_cost);

        for (key, cost, component) in candidates {
            let forced = include_set.contains(key);
            if forced {
                emitted.insert(key, component);
                used += cost;
                remaining = remaining.saturating_sub(cost);
            } else if !overbudget && remaining >= cost {
                emitted.insert(key, component);
                used += cost;
                remaining -= cost;
            } else {
                hints.push((key.to_string(), cost));
            }
        }

        let overview_mode = if overbudget {
            "overbudget"
        } else if hints.is_empty() {
            "complete"
        } else {
            "reduced"
        };

        let schemas_out = schemas_slim.clone();
        let mems_out = if emitted.contains_key("mem_distribution") {
            mems_full.clone()
        } else {
            mems_lite.clone()
        };

        // --- Markdown render ---
        let mod_str = if modularity == 0.0 {
            "0".to_string()
        } else {
            format!("{modularity:.4}")
        };

        let mut md = String::new();
        md.push_str("---\n");
        md.push_str(&format!("_mem_schema: {schema_canon}\n"));
        md.push_str(&format!("_overview_mode: {overview_mode}\n"));
        md.push_str(&format!("_budget_requested: {budget}\n"));
        md.push_str(&format!("_budget_used: {used}\n"));
        md.push_str(&format!("_cluster_count: {cluster_count}\n"));
        md.push_str(&format!("_entity_count: {entity_count_total}\n"));
        md.push_str(&format!("_modularity: {mod_str}\n"));
        md.push_str("---\n\n");

        // Schemas
        md.push_str("## Schemas\n\n");
        if schemas_out.is_empty() {
            md.push_str("_(no schemas)_\n\n");
        } else {
            md.push_str(
                "_(call `memstead_schema(name=<ref>)` for the full per-type catalogue, sections, fields, and relationship vocabulary)_\n\n",
            );
            for s in &schemas_out {
                let r = s["ref"].as_str().unwrap_or("?");
                md.push_str(&format!("### {r}\n\n"));
                if let Some(desc) = s["description"].as_str()
                    && !desc.is_empty()
                {
                    md.push_str(&format!("{desc}\n\n"));
                }
            }
        }

        // Mems
        let emit_mem_distribution = emitted.contains_key("mem_distribution");
        md.push_str("## Mems\n\n");
        for v in &mems_out {
            let name = v["name"].as_str().unwrap_or("?");
            let schema_ref = v["schema"].as_str().unwrap_or("(unspecified)");
            let count = v["entity_count"].as_u64().unwrap_or(0);
            md.push_str(&format!("### {name}\n\n"));
            md.push_str(&format!("- **Schema:** {schema_ref}\n"));
            // Flag ephemeral storage loudly; durable-on-disk mems keep
            // their lines unchanged. On the sketch this is the line that
            // tells an agent its `commit_sha` is volatile before it writes.
            if v["durable"].as_bool() == Some(false) {
                let storage = v["storage"].as_str().unwrap_or("in-memory");
                md.push_str(&format!(
                    "- **Storage:** {storage} (ephemeral — writes are volatile, evicted on restart/TTL; `commit_sha` is not durable)\n"
                ));
            }
            md.push_str(&format!("- **Entities:** {count}\n"));
            if emit_mem_distribution
                && let Some(td) = v["type_distribution"].as_object()
                && !td.is_empty()
            {
                let pairs: Vec<String> = td
                    .iter()
                    .map(|(k, v)| format!("{k}={}", v.as_u64().unwrap_or(0)))
                    .collect();
                md.push_str(&format!("- **By type:** {}\n", pairs.join(", ")));
            }
            md.push('\n');
        }

        // Communities
        let emit_community_members = emitted.contains_key("community_members");
        md.push_str("## Communities\n\n");
        if cluster_ids.is_empty() {
            md.push_str("_(no communities — graph is empty or has no edges)_\n");
        } else {
            for cid in &cluster_ids {
                let info = &output.clusters[cid];
                let summary = memstead_base::graph::community::generate_auto_summary(
                    engine.store(),
                    &info.entities,
                );
                md.push_str(&format!(
                    "### Cluster {cid} ({} entities)\n",
                    info.entities.len()
                ));
                if !summary.is_empty() {
                    md.push_str(&format!("{summary}\n"));
                }
                if emit_community_members {
                    for eid in &info.entities {
                        md.push_str(&format!("- {eid}\n"));
                    }
                } else {
                    md.push_str(
                        "_(call with include=[\"community_members\"] to see member lists)_\n",
                    );
                }
                md.push('\n');
            }
        }

        // Community bridges
        if emitted.contains_key("community_bridges")
            && let Some(bridges) = emitted["community_bridges"].as_array()
            && !bridges.is_empty()
        {
            md.push_str("## Community Bridges\n\n");
            for b in bridges {
                let from_c = b["from_cluster"].as_str().unwrap_or("?");
                let to_c = b["to_cluster"].as_str().unwrap_or("?");
                let n = b["edge_count"].as_u64().unwrap_or(0);
                md.push_str(&format!("### {from_c} ↔ {to_c} ({n} edges)\n"));
                if let Some(types) = b["edge_types"].as_array() {
                    let list: Vec<String> = types
                        .iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect();
                    if !list.is_empty() {
                        md.push_str(&format!("- **Edge types:** {}\n", list.join(", ")));
                    }
                }
                if let Some(samples) = b["sample_edges"].as_array() {
                    for s in samples {
                        let rel = s["rel_type"].as_str().unwrap_or("?");
                        let from = s["from"].as_str().unwrap_or("?");
                        let to = s["to"].as_str().unwrap_or("?");
                        md.push_str(&format!("  - `{rel}` {from} → {to}\n"));
                    }
                }
                md.push('\n');
            }
        }

        // Dangling links
        if emitted.contains_key("dangling_links")
            && let Some(links) = emitted["dangling_links"].as_array()
            && !links.is_empty()
        {
            md.push_str("## Dangling Links\n\n");
            for link in links {
                let from = link["from"].as_str().unwrap_or("?");
                let target = link["target_id"].as_str().unwrap_or("?");
                let section = link["section"].as_str();
                if let Some(s) = section {
                    md.push_str(&format!("- `{from}` → `{target}` (in `{s}`)\n"));
                } else {
                    md.push_str(&format!("- `{from}` → `{target}`\n"));
                }
            }
            md.push('\n');
        }

        // Hints
        if !hints.is_empty() {
            md.push_str("## Hints\n\n");
            md.push_str("_(keys not included — re-query with `include: [\"<key>\"]`)_\n\n");
            for (key, tokens) in &hints {
                md.push_str(&format!("- `{key}` — estimated_tokens: {tokens}\n"));
            }
            md.push('\n');
        }

        // Warnings
        if !warnings.is_empty() {
            md.push_str("## Warnings\n\n");
            for (code, message) in &warnings {
                md.push_str(&format!("- **{code}** — {message}\n"));
            }
            md.push('\n');
        }

        md_response(md)
    }
}

#[tool_handler(
    router = FilesystemMcpServer::tool_router(),
    name = "memstead-mcp",
    version = "0.1.0",
    instructions = "Memstead: schema-agnostic graph engine over typed, interconnected markdown entities. Each mem is a typed model of a chosen subject — its modal flavour (knowledge, planning, inquiry, spec, or any mix) follows from the schema the mem pins. Granularity: a mem is the packaged unit — a whole typed model, typically 1,000-5,000 entities; an entity is never called a mem (a mem is not one 'memory'/fact). Cold-start: call memstead_overview first for the schema catalogue and mem inventory; read a mem's schema via memstead_schema before mutating."
)]
impl ServerHandler for FilesystemMcpServer {
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
        let _ = self.client.set(cid);
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools: Vec<Tool> = Self::tool_router().list_all();
        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        Self::tool_router().call(tcc).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use memstead_base::filesystem::config::{WorkspaceConfig, write_workspace_config};
    use memstead_schema::SchemaRef;
    use rmcp::handler::server::wrapper::Parameters;
    use tempfile::TempDir;

    /// The lean MCP error map must emit the same wire code as
    /// `EngineError::code()` — the single source every surface (full MCP,
    /// CLI, wasm) follows. `Backend` and `ParseAfterWrite` historically
    /// shipped `MEM_WRITER_ERROR` / `PARSE_AFTER_WRITE` here, diverging
    /// from `code()`'s `MEM_ERROR` / `PARSE_ERROR`. Pin them so a
    /// re-divergence fails the build.
    #[test]
    fn lean_backend_and_parse_after_write_codes_follow_code_contract() {
        let cases: Vec<EngineError> = vec![
            EngineError::Backend(memstead_base::backend::BackendError::Other("disk".into())),
            EngineError::ParseAfterWrite("boom".into()),
        ];
        for err in cases {
            let expected = err.code();
            let result = engine_op_error(err);
            let code = result
                .structured_content
                .as_ref()
                .and_then(|v| v.get("code"))
                .and_then(|c| c.as_str())
                .expect("error envelope carries structured.code")
                .to_string();
            assert_eq!(
                code, expected,
                "lean error-map code drifted from EngineError::code()"
            );
        }
    }

    /// `memstead_relate` is idempotent on the lean surface, matching the
    /// mem-repo server: duplicate-add and remove-nonexistent are
    /// typed-warning no-ops, so a retry converges. The full-side
    /// annotation meta-test is `mem-repo`-gated; this lean-side test
    /// pins the parity so the two flavours cannot silently re-diverge.
    #[test]
    fn relate_annotation_is_idempotent_on_lean() {
        let tools = FilesystemMcpServer::tool_router().list_all();
        let relate = tools
            .iter()
            .find(|t| t.name == "memstead_relate")
            .expect("memstead_relate is on the lean surface");
        let ann = relate
            .annotations
            .as_ref()
            .expect("memstead_relate sets annotation hints");
        assert_eq!(
            ann.idempotent_hint,
            Some(true),
            "lean memstead_relate idempotent_hint must match the mem-repo server's `true`"
        );
    }

    fn write_workspace(tmp: &TempDir, name: &str) {
        let pin: SchemaRef = "default@1.0.0".parse().unwrap();
        let cfg = WorkspaceConfig::new(name, pin.clone());
        write_workspace_config(tmp.path(), &cfg).unwrap();
        // Two-layer file adapter markers — `Engine::from_workspace_root`
        // recognises a workspace by `.memstead/workspace.toml` plus the
        // mount list in `.memstead/state/mounts.json`.
        let memstead = tmp.path().join(".memstead");
        std::fs::write(
            memstead.join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        let workspace = memstead_base::Workspace {
            mounts: vec![memstead_base::Mount {
                mem: name.to_string(),
                schema: Some(pin),
                storage: memstead_base::MountStorage::Folder {
                    path: tmp.path().to_path_buf(),
                },
                capability: memstead_base::MountCapability::Write,
                lifecycle: memstead_base::MountLifecycle::Eager,
                cross_linkable: true,
            migration_target: None,
        }],
            settings: memstead_base::WorkspaceSettings::default(),
        };
        use memstead_base::WorkspaceStoreAdapter;
        memstead_base::FileWorkspaceStore::new()
            .save_state(tmp.path(), &workspace)
            .unwrap();
    }

    #[test]
    fn create_then_entity_round_trip() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        // memstead_create — the engine refuses on missing required
        // sections, so seed `identity` + `purpose` so the spec lands.
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "first identity".to_string());
        sections.insert("purpose".to_string(), "first purpose".to_string());
        let create_params = CreateParams {
            title: "First".to_string(),
            entity_type: "spec".to_string(),
            mem: None,
            sections: Some(sections),
            metadata: None,
            relations: None,
            dry_run: None,
            note: Some("first via mcp".to_string()),
        };
        let create_result = server.memstead_create(Parameters(create_params));
        assert!(
            !create_result.is_error.unwrap_or(false),
            "create must succeed: {:?}", create_result.structured_content,
        );
        let create_body = create_result
            .structured_content
            .as_ref()
            .expect("structured content");
        let id = create_body["id"].as_str().unwrap().to_string();
        assert_eq!(id, "demo--first");

        // Changelog has the note.
        let log = std::fs::read_to_string(
            tmp.path().join(".memstead").join("changes.jsonl"),
        )
        .unwrap();
        assert!(log.contains("\"note\":\"first via mcp\""));

        // memstead_entity
        let entity_params = EntityParams {
            id: id.clone(),
            sections: None,
            include_relations: None,
            include_context: None,
            token_budget: None,
            chunk: None,
        };
        let entity_result = server.memstead_entity(Parameters(entity_params));
        assert!(!entity_result.is_error.unwrap_or(false));
        let text = match entity_result.content.first() {
            Some(c) => match c.as_text() {
                Some(t) => t.text.clone(),
                None => panic!("expected text"),
            },
            None => panic!("expected at least one content"),
        };
        assert!(text.contains("# First"));
        assert!(text.contains("_hash:"));
    }

    /// Build a two-mount engine — a writable in-memory `sketch` mem
    /// (declared first) and a read-only in-memory `content` mem — so the
    /// create handler's multi-mount mem resolution is exercised at this
    /// layer. Mirrors the session server's two-tier shape.
    fn two_mount_engine() -> memstead_base::Engine {
        use memstead_base::backend::MemBackend;
        use memstead_base::storage::InMemoryBackend;
        use memstead_base::{Mount, MountCapability, MountLifecycle, MountStorage};
        let pin: SchemaRef = "default@1.0.0".parse().unwrap();
        let build = |name: &str, cap: MountCapability| -> (Mount, Box<dyn MemBackend>) {
            let backend = InMemoryBackend::new();
            let cfg = format!(r#"{{"version":"0.1.0","schema":"{pin}"}}"#).into_bytes();
            backend.write_mem_config(&cfg).unwrap();
            let mount = Mount {
                mem: name.to_string(),
                schema: Some(pin.clone()),
                storage: MountStorage::InMemory,
                capability: cap,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            };
            (mount, Box::new(backend) as Box<dyn MemBackend>)
        };
        memstead_base::Engine::from_mounts(vec![
            build("sketch", MountCapability::Write),
            build("content", MountCapability::ReadOnly),
        ])
        .unwrap()
    }

    fn spec_sections() -> Option<IndexMap<String, String>> {
        let mut s = IndexMap::new();
        s.insert("identity".to_string(), "i".to_string());
        s.insert("purpose".to_string(), "p".to_string());
        Some(s)
    }

    fn create_params(title: &str, mem: Option<&str>) -> CreateParams {
        CreateParams {
            title: title.to_string(),
            entity_type: "spec".to_string(),
            mem: mem.map(String::from),
            sections: spec_sections(),
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
        }
    }

    /// Multi-mount create-targeting: with a writable `sketch` and a
    /// read-only `content` mem mounted together, an omitted `mem` lands
    /// in the writable mount (not the alphabetically-first read-only one);
    /// an explicit read-only target is refused with READ_ONLY_MOUNT rather
    /// than silently redirected; an explicit writable target is honoured.
    #[test]
    fn create_resolves_target_mem_across_multiple_mounts() {
        let server = FilesystemMcpServer::from_engine(two_mount_engine(), std::path::PathBuf::new());

        // Omitted mem → default writable mount (`sketch`).
        let r = server.memstead_create(Parameters(create_params("Default Target", None)));
        assert!(
            !r.is_error.unwrap_or(false),
            "omitted-mem create must land in the writable mount: {:?}",
            r.structured_content
        );
        assert_eq!(
            r.structured_content.as_ref().unwrap()["id"].as_str(),
            Some("sketch--default-target"),
            "create defaults to the writable mem, not the read-only one"
        );

        // Explicit read-only mem → typed refusal, not a redirect.
        let r = server.memstead_create(Parameters(create_params("Into Content", Some("content"))));
        assert!(r.is_error.unwrap_or(false), "write to a read-only mount must refuse");
        assert_eq!(
            r.structured_content.unwrap()["code"], "READ_ONLY_MOUNT",
            "the engine capability layer refuses the read-only target"
        );

        // Explicit writable mem → honoured.
        let r = server.memstead_create(Parameters(create_params("Explicit Sketch", Some("sketch"))));
        assert!(
            !r.is_error.unwrap_or(false),
            "explicit writable target must land: {:?}",
            r.structured_content
        );
        assert_eq!(
            r.structured_content.as_ref().unwrap()["id"].as_str(),
            Some("sketch--explicit-sketch")
        );
    }

    /// Plan 03, Part B: params this surface hardwires off are REFUSED with
    /// a typed `UNSUPPORTED_PARAM` naming them — never silently dropped. The
    /// `dry_run` case is the load-bearing one: silently treating it as a real
    /// write would land an entity the agent thought was a preview.
    #[test]
    fn unsupported_write_params_refuse_rather_than_silently_drop() {
        use crate::tools::mutation::RelationInput;
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let dropped = |r: &CallToolResult| -> Vec<String> {
            r.structured_content.as_ref().unwrap()["details"]["params"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        let is_unsupported = |r: &CallToolResult| {
            r.is_error.unwrap_or(false)
                && r.structured_content.as_ref().unwrap()["code"] == "UNSUPPORTED_PARAM"
        };

        // create + dry_run: true → refused up front; nothing lands.
        let mut p = create_params("Preview Me", None);
        p.dry_run = Some(true);
        let r = server.memstead_create(Parameters(p));
        assert!(is_unsupported(&r), "dry_run create must refuse: {r:?}");
        assert!(dropped(&r).contains(&"dry_run".to_string()));
        // The refusal precedes the engine, so the preview entity never lands.
        let entity = server.memstead_entity(Parameters(EntityParams {
            id: "demo--preview-me".into(),
            include_relations: None,
            include_context: None,
            sections: None,
            token_budget: None,
            chunk: None,
        }));
        assert!(
            entity.is_error.unwrap_or(false),
            "a refused dry_run must NOT have created the entity"
        );

        // create + non-empty relations → refused, naming relations.
        let mut p = create_params("With Edges", None);
        p.relations = Some(vec![RelationInput {
            to: "demo--target".into(),
            r#type: "REFERENCES".into(),
            description: None,
        }]);
        let r = server.memstead_create(Parameters(p));
        assert!(is_unsupported(&r));
        assert!(dropped(&r).contains(&"relations".to_string()));

        // update + each unsupported param → refused naming it.
        let base = || UpdateParams {
            id: "demo--anything".into(),
            expected_hash: "deadbeef".into(),
            sections: None,
            append_sections: None,
            patch_sections: None,
            metadata: None,
            metadata_unset: None,
            dry_run: None,
            declare_relations: None,
            relations_unset: None,
            note: None,
        };
        let mut u = base();
        u.append_sections = Some(IndexMap::from([("purpose".to_string(), "x".to_string())]));
        let r = server.memstead_update(Parameters(u));
        assert!(is_unsupported(&r));
        assert!(dropped(&r).contains(&"append_sections".to_string()));

        let mut u = base();
        u.patch_sections = Some(IndexMap::from([(
            "purpose".to_string(),
            crate::tools::mutation::PatchInput {
                old: "a".into(),
                new: "b".into(),
                all: None,
            },
        )]));
        let r = server.memstead_update(Parameters(u));
        assert!(is_unsupported(&r));
        assert!(dropped(&r).contains(&"patch_sections".to_string()));

        let mut u = base();
        u.dry_run = Some(true);
        let r = server.memstead_update(Parameters(u));
        assert!(is_unsupported(&r));
        assert!(dropped(&r).contains(&"dry_run".to_string()));
    }

    /// Refusal complement (Part B): a defaulted-empty / absent unsupported
    /// param is left alone — the surface stays backward-compatible for a
    /// caller that harmlessly passes nothing. A plain create/update with the
    /// supported params still succeeds.
    #[test]
    fn absent_unsupported_params_do_not_refuse() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        // create with dry_run: None / relations: None → no UNSUPPORTED_PARAM.
        let r = server.memstead_create(Parameters(create_params("Plain Create", None)));
        assert!(
            !r.is_error.unwrap_or(false),
            "a default create must not be refused: {r:?}"
        );
        // dry_run: Some(false) is the caller intending no preview — also fine.
        let mut p = create_params("Explicit No Preview", None);
        p.dry_run = Some(false);
        let r = server.memstead_create(Parameters(p));
        assert!(
            !r.is_error.unwrap_or(false),
            "dry_run: false must not be refused: {r:?}"
        );
    }

    #[test]
    fn create_rejects_unknown_type_with_typed_code() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let result = server.memstead_create(Parameters(CreateParams {
            title: "X".into(),
            entity_type: "totally-not-a-type".into(),
            mem: None,
            sections: None,
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "UNKNOWN_ENTITY_TYPE");
    }

    /// Smoke-test probe A: a memo created with sections that belong
    /// to a different type (here `identity` + `purpose`, which are
    /// `spec` sections, not `memo`'s `claim` + `context`) must reject
    /// with `UNKNOWN_SECTION` before any disk write lands.
    #[test]
    fn create_rejects_unknown_section_keys() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "Some text".to_string());
        sections.insert("purpose".to_string(), "Other text".to_string());

        let result = server.memstead_create(Parameters(CreateParams {
            title: "Stray Memo".into(),
            entity_type: "memo".into(),
            mem: None,
            sections: Some(sections),
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "UNKNOWN_SECTION");
        // The first offender hits first; either key would be valid.
        let bad_key = body["details"]["key"].as_str().unwrap();
        assert!(
            bad_key == "identity" || bad_key == "purpose",
            "expected identity/purpose, got {bad_key}"
        );
        // No file should have been written.
        assert!(
            !tmp.path().join("stray-memo.md").exists(),
            "stray memo should not have been persisted"
        );
    }

    /// Smoke-test probe B: a memo with no sections at all should
    /// succeed (required-section gaps are Tier-2 warnings, not hard
    /// errors), but the response must carry a `MISSING_REQUIRED_SECTION`
    /// warning per missing required section so the agent can
    /// self-correct.
    #[test]
    fn create_refuses_missing_required_section_with_typed_envelope() {
        // `memstead_create` refuses on missing required sections instead
        // of emitting warnings. Pre-fix the entity landed with empty
        // placeholders for each missing required section and the
        // install-time strict validator could later refuse the
        // resulting archive — the export-then-install round-trip
        // broke. Now the refusal fires at the write boundary.
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let result = server.memstead_create(Parameters(CreateParams {
            title: "Empty Memo".into(),
            entity_type: "memo".into(),
            mem: None,
            sections: Some(IndexMap::new()),
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
        }));
        assert!(result.is_error.unwrap_or(false), "create must refuse");
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "MISSING_REQUIRED_SECTION");
        assert_eq!(body["details"]["entity_type"], "memo");
        assert!(
            body["details"]["sections"].as_array().map_or(0, |s| s.len()) >= 1,
            "details.sections must list at least one missing key, got: {body}"
        );
        assert!(
            body["details"]["type_guidance"].is_object(),
            "details.type_guidance must be a map, got: {body}"
        );
    }

    /// Smoke-test probe C: an out-of-enum metadata value (`level: "Z3"`
    /// against the `M0|M1|M2|M3` allowed set) must reject with
    /// `INVALID_ENUM_VALUE`.
    #[test]
    fn create_rejects_out_of_enum_metadata_value() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let mut metadata = IndexMap::new();
        metadata.insert("level".to_string(), "Z3".to_string());

        let result = server.memstead_create(Parameters(CreateParams {
            title: "Bad Level".into(),
            entity_type: "spec".into(),
            mem: None,
            sections: None,
            metadata: Some(metadata),
            relations: None,
            dry_run: None,
            note: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "INVALID_ENUM_VALUE");
        assert_eq!(body["details"]["field"], "level");
        assert_eq!(body["details"]["value"], "Z3");
    }

    /// Unknown metadata fields surface `UNKNOWN_METADATA_FIELD`. The
    /// mem-repo path emits the same code; this guard pins the
    /// filesystem-mem contract so future refactors don't silently
    /// drop the gate.
    #[test]
    fn create_rejects_unknown_metadata_field() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let mut metadata = IndexMap::new();
        metadata.insert("nonsense".to_string(), "value".to_string());

        let result = server.memstead_create(Parameters(CreateParams {
            title: "Stray Field".into(),
            entity_type: "spec".into(),
            mem: None,
            sections: None,
            metadata: Some(metadata),
            relations: None,
            dry_run: None,
            note: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "UNKNOWN_METADATA_FIELD");
        assert_eq!(body["details"]["key"], "nonsense");
    }

    #[test]
    fn entity_not_found_returns_typed_code() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let result = server.memstead_entity(Parameters(EntityParams {
            id: "demo--ghost".into(),
            sections: None,
            include_relations: None,
            include_context: None,
            token_budget: None,
            chunk: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "ENTITY_NOT_FOUND");
    }

    fn seed_via_mcp(server: &FilesystemMcpServer, title: &str) -> (String, String) {
        // The
        // engine refuses on missing required sections. Seed the
        // `spec` type's required `identity` + `purpose` sections so
        // wire-shape tests using this helper continue to land valid
        // entities.
        let mut seeded_sections = IndexMap::new();
        seeded_sections.insert("identity".to_string(), "seed identity".to_string());
        seeded_sections.insert("purpose".to_string(), "seed purpose".to_string());
        let result = server.memstead_create(Parameters(CreateParams {
            title: title.into(),
            entity_type: "spec".into(),
            mem: None,
            sections: Some(seeded_sections),
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
        }));
        assert!(
            !result.is_error.unwrap_or(false),
            "seed_via_mcp must succeed; got error: {:?}",
            result.structured_content,
        );
        let body = result.structured_content.unwrap();
        (
            body["id"].as_str().unwrap().to_string(),
            body["_hash"].as_str().unwrap().to_string(),
        )
    }

    #[test]
    fn update_replaces_section_and_returns_new_hash() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (id, hash) = seed_via_mcp(&server, "Updatable");

        let mut sections = indexmap::IndexMap::new();
        sections.insert("identity".to_string(), "Updated body.".to_string());
        let result = server.memstead_update(Parameters(UpdateParams {
            relations_unset: None,
            id: id.clone(),
            expected_hash: hash.clone(),
            sections: Some(sections),
            append_sections: None,
            patch_sections: None,
            metadata: None,
            metadata_unset: None,
            dry_run: None,
            note: Some("touched body".into()),
            declare_relations: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        let new_hash = body["_hash"].as_str().unwrap();
        assert_ne!(new_hash, hash);
        assert_eq!(body["modified_sections"][0], "identity");
    }

    #[test]
    fn update_rejects_stale_hash_with_typed_code() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (id, _hash) = seed_via_mcp(&server, "Pinned");

        let result = server.memstead_update(Parameters(UpdateParams {
            relations_unset: None,
            id,
            expected_hash: "0000000000".into(),
            sections: None,
            append_sections: None,
            patch_sections: None,
            metadata: None,
            metadata_unset: None,
            dry_run: None,
            note: None,
            declare_relations: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "HASH_MISMATCH");
        assert!(body["details"]["current"].is_string());
    }

    /// `memstead_update` must reject any attempt to mutate the read-only
    /// metadata triple (`mem`, `id`, `type`) on either set or unset
    /// — the entity-id contract depends on those staying stable.
    #[test]
    fn update_rejects_read_only_metadata_set() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (id, hash) = seed_via_mcp(&server, "Locked");

        for field in ["mem", "id", "type"] {
            let mut metadata = IndexMap::new();
            metadata.insert(field.to_string(), "garbage".to_string());
            let result = server.memstead_update(Parameters(UpdateParams {
                relations_unset: None,
                id: id.clone(),
                expected_hash: hash.clone(),
                sections: None,
                append_sections: None,
                patch_sections: None,
                metadata: Some(metadata),
                metadata_unset: None,
                dry_run: None,
                note: None,
                declare_relations: None,
            }));
            assert!(
                result.is_error.unwrap_or(false),
                "set of {field} should error"
            );
            let body = result.structured_content.unwrap();
            assert_eq!(body["code"], "READ_ONLY_FIELD");
            assert_eq!(body["details"]["field"], field);
        }
    }

    /// Same gate applies on `metadata_unset` — agents can't sneak the
    /// triple out by removing it instead of overwriting.
    #[test]
    fn update_rejects_read_only_metadata_unset() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (id, hash) = seed_via_mcp(&server, "Unsettable");

        let result = server.memstead_update(Parameters(UpdateParams {
            relations_unset: None,
            id: id.clone(),
            expected_hash: hash,
            sections: None,
            append_sections: None,
            patch_sections: None,
            metadata: None,
            metadata_unset: Some(vec!["type".to_string()]),
            dry_run: None,
            note: None,
            declare_relations: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "READ_ONLY_FIELD");
        assert_eq!(body["details"]["field"], "type");
    }

    /// The virtual `relationships` surface is managed by `memstead_relate`,
    /// not `memstead_update`. Writes there reject with
    /// `SECTION_NOT_UPDATABLE` so an agent does not bypass the
    /// rel-validation pipeline by treating relationships as a section.
    #[test]
    fn update_rejects_relationships_section_as_not_updatable() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (id, hash) = seed_via_mcp(&server, "Sneaky");

        let mut sections = IndexMap::new();
        sections.insert(
            "relationships".to_string(),
            "- KIND: target".to_string(),
        );
        let result = server.memstead_update(Parameters(UpdateParams {
            relations_unset: None,
            id,
            expected_hash: hash,
            sections: Some(sections),
            append_sections: None,
            patch_sections: None,
            metadata: None,
            metadata_unset: None,
            dry_run: None,
            note: None,
            declare_relations: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        // The relationships sneak path is gated by validate_section_keys
        // first (the schema doesn't declare a `relationships` section),
        // so it fires UNKNOWN_SECTION. Either gate is correct — both
        // close the bypass.
        let code = body["code"].as_str().unwrap();
        assert!(
            code == "SECTION_NOT_UPDATABLE" || code == "UNKNOWN_SECTION",
            "expected SECTION_NOT_UPDATABLE or UNKNOWN_SECTION, got {code}"
        );
    }

    #[test]
    fn delete_removes_entity_and_logs_change() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (id, hash) = seed_via_mcp(&server, "Doomed");

        let result = server.memstead_delete(Parameters(DeleteParams {
            id: id.clone(),
            expected_hash: hash,
            note: Some("retired".into()),
        }));
        assert!(!result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["id"], id);

        let log = std::fs::read_to_string(
            tmp.path().join(".memstead").join("changes.jsonl"),
        )
        .unwrap();
        assert!(log.contains("\"kind\":\"delete\""));
        assert!(log.contains("\"note\":\"retired\""));
    }

    #[test]
    fn relate_appends_then_no_op_on_duplicate() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (from, _) = seed_via_mcp(&server, "Source");
        let (to, _) = seed_via_mcp(&server, "Target");

        let added = server.memstead_relate(Parameters(RelateParams {
            from: from.clone(),
            to: to.clone(),
            r#type: "USES".into(),
            remove: None,
            note: Some("first".into()),
            description: None,
        }));
        assert!(!added.is_error.unwrap_or(false));
        assert_eq!(added.structured_content.unwrap()["action"], "added");

        let dup = server.memstead_relate(Parameters(RelateParams {
            from,
            to,
            r#type: "USES".into(),
            remove: None,
            note: None,
            description: None,
        }));
        assert!(!dup.is_error.unwrap_or(false));
        assert_eq!(
            dup.structured_content.unwrap()["action"],
            "no_op_already_present"
        );
    }

    /// Strict-mode schemas (the default) reject undeclared
    /// relationship names with `INVALID_REL_TYPE`. The recovery
    /// envelope carries the canonical vocabulary on
    /// `details.allowed[]` so the agent can self-correct in one
    /// round trip without a follow-up `memstead_overview`.
    #[test]
    fn relate_rejects_undeclared_rel_type() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (from, _) = seed_via_mcp(&server, "Source");
        let (to, _) = seed_via_mcp(&server, "Target");

        let result = server.memstead_relate(Parameters(RelateParams {
            from,
            to,
            r#type: "TOTALLY_MADE_UP".into(),
            remove: None,
            note: None,
            description: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "INVALID_REL_TYPE");
        assert_eq!(body["details"]["input"], "TOTALLY_MADE_UP");
        let allowed = body["details"]["allowed"]
            .as_array()
            .expect("allowed should be array");
        assert!(!allowed.is_empty(), "allowed[] must list real edges");
    }

    #[test]
    fn relate_rejects_cross_mem_target() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (from, _) = seed_via_mcp(&server, "Source");

        let result = server.memstead_relate(Parameters(RelateParams {
            from,
            to: "other--thing".into(),
            r#type: "USES".into(),
            remove: None,
            note: None,
            description: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "CROSS_MEM_LINK_NOT_ALLOWED"
        );
    }

    #[test]
    fn search_with_empty_query_returns_seeded_entities() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        for title in ["Alpha", "Beta", "Gamma"] {
            seed_via_mcp(&server, title);
        }

        let result = server.memstead_search(Parameters(SearchParams {
            query: None,
            mem: None,
            entity_type: None,
            expand_via: None,
            expand_depth: None,
            related_to: None,
            depth: None,
            edge_type: None,
            limit: None,
            offset: None,
            filters: None,
            range_filters: None,
            stub: Some(false),
            token_budget: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let md = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(md.contains("_total: 3"), "expected _total: 3 in: {md}");
        for title in ["Alpha", "Beta", "Gamma"] {
            assert!(md.contains(title), "expected title {title} in: {md}");
        }
    }

    /// Malformed `range_filters` key (no `min_`/`max_`/`*_before`/`*_after`
    /// shape) refuses with `RANGE_FILTER_KEY_MALFORMED`. An earlier
    /// MCP `SearchParams` had no `range_filters` field at all so the
    /// engine never saw the input — silent no-op.
    #[test]
    fn search_range_filter_malformed_key_surfaces_typed_warning() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        seed_via_mcp(&server, "Seed");

        let mut range_filters = std::collections::HashMap::new();
        range_filters.insert("malformedkey".to_string(), "10".to_string());

        let result = server.memstead_search(Parameters(SearchParams {
            query: None,
            mem: None,
            entity_type: None,
            expand_via: None,
            expand_depth: None,
            related_to: None,
            depth: None,
            edge_type: None,
            limit: None,
            offset: None,
            filters: None,
            range_filters: Some(range_filters),
            stub: Some(false),
            token_budget: None,
        }));
        let sc = result
            .structured_content
            .as_ref()
            .expect("range-filter warning must ride on structured_content");
        let warnings = sc["warnings"]
            .as_array()
            .expect("warnings array on the structured envelope");
        assert!(
            warnings
                .iter()
                .any(|w| w["code"] == "RANGE_FILTER_KEY_MALFORMED"),
            "expected RANGE_FILTER_KEY_MALFORMED warning, got: {warnings:?}",
        );
    }

    /// Unknown range-filter field surfaces
    /// `UNKNOWN_RANGE_FILTER_FIELD` with the derived field name.
    #[test]
    fn search_range_filter_unknown_field_surfaces_typed_warning() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        seed_via_mcp(&server, "Seed");

        let mut range_filters = std::collections::HashMap::new();
        range_filters.insert("min_fake_field".to_string(), "10".to_string());

        let result = server.memstead_search(Parameters(SearchParams {
            query: None,
            mem: None,
            entity_type: Some("spec".to_string()),
            expand_via: None,
            expand_depth: None,
            related_to: None,
            depth: None,
            edge_type: None,
            limit: None,
            offset: None,
            filters: None,
            range_filters: Some(range_filters),
            stub: Some(false),
            token_budget: None,
        }));
        let sc = result.structured_content.as_ref().unwrap();
        let warnings = sc["warnings"].as_array().unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| w["code"] == "UNKNOWN_RANGE_FILTER_FIELD"),
            "expected UNKNOWN_RANGE_FILTER_FIELD warning, got: {warnings:?}",
        );
    }

    /// Range-filter against a field that exists on the
    /// type's schema but is not declared `filterable: range` surfaces
    /// `FIELD_NOT_RANGE_FILTERABLE`. `level` exists on the default
    /// `spec` type with `filterable: equality` — perfect probe.
    #[test]
    fn search_range_filter_field_not_range_filterable_surfaces_typed_warning() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        seed_via_mcp(&server, "Seed");

        let mut range_filters = std::collections::HashMap::new();
        range_filters.insert("min_level".to_string(), "M0".to_string());

        let result = server.memstead_search(Parameters(SearchParams {
            query: None,
            mem: None,
            entity_type: Some("spec".to_string()),
            expand_via: None,
            expand_depth: None,
            related_to: None,
            depth: None,
            edge_type: None,
            limit: None,
            offset: None,
            filters: None,
            range_filters: Some(range_filters),
            stub: Some(false),
            token_budget: None,
        }));
        let sc = result.structured_content.as_ref().unwrap();
        let warnings = sc["warnings"].as_array().unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| w["code"] == "FIELD_NOT_RANGE_FILTERABLE"),
            "expected FIELD_NOT_RANGE_FILTERABLE warning, got: {warnings:?}",
        );
    }

    /// Omitting `range_filters` produces no
    /// range-filter warnings (the parameter is optional).
    #[test]
    fn search_without_range_filters_produces_no_range_warnings() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        seed_via_mcp(&server, "Seed");

        let result = server.memstead_search(Parameters(SearchParams {
            query: None,
            mem: None,
            entity_type: None,
            expand_via: None,
            expand_depth: None,
            related_to: None,
            depth: None,
            edge_type: None,
            limit: None,
            offset: None,
            filters: None,
            range_filters: None,
            stub: Some(false),
            token_budget: None,
        }));
        let sc = result.structured_content.as_ref().unwrap();
        let warnings = sc["warnings"].as_array().unwrap_or(&Vec::new()).clone();
        for w in warnings {
            let code = w["code"].as_str().unwrap_or("");
            assert!(
                !code.starts_with("RANGE_FILTER_") && code != "UNKNOWN_RANGE_FILTER_FIELD" && code != "FIELD_NOT_RANGE_FILTERABLE",
                "no range-filter warning expected when range_filters omitted, got: {w}",
            );
        }
    }

    #[test]
    fn search_filters_by_entity_type() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        seed_via_mcp(&server, "OnlySpec");

        // Filter to a non-existent type → empty hits.
        let result = server.memstead_search(Parameters(SearchParams {
            query: None,
            mem: None,
            entity_type: Some("totally-not-a-type".into()),
            expand_via: None,
            expand_depth: None,
            related_to: None,
            depth: None,
            edge_type: None,
            limit: None,
            offset: None,
            filters: None,
            range_filters: None,
            stub: Some(false),
            token_budget: None,
        }));
        let md = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(md.contains("_total: 0"), "expected _total: 0 in: {md}");
    }

    #[test]
    fn health_returns_workspace_summary() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        seed_via_mcp(&server, "Healthy");

        let result = server.memstead_health(Parameters(HealthParams {
            include: None,
            limit: None,
            mem: None,
            include_config: false,
            target_schema: None,
            token_budget: None,
            chunk: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        // The summary structure carries totals and a per-mem map.
        // We do not pin field names here (memstead-base owns the shape),
        // just assert the response is a non-empty JSON object.
        assert!(body.is_object());
        assert!(!body.as_object().unwrap().is_empty());
    }

    #[test]
    fn health_via_new_engine_reflects_post_boot_mutations() {
        // Pins the migration template's "boot fresh per call" property:
        // seed entities through the legacy engine after server boot,
        // then assert memstead_health (which now routes through a fresh
        // memstead_base::Engine) reflects them. Without the per-call boot
        // the health response would be stale relative to the legacy
        // engine's mutations.
        //
        // `memstead_create` refuses on missing required sections,
        // so `seed_via_mcp` seeds `identity` + `purpose`. The
        // seeded entity no longer surfaces as missing_fields, but
        // the broader invariant — health reflects post-boot
        // mutations — still holds via the total entity count.
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        seed_via_mcp(&server, "Post Boot Spec");

        // Health must reflect the post-boot mutation — the seeded
        // entity is visible via the stats projection (stub_count and
        // missing_fields can both be 0 on a fresh mem with a
        // well-formed seed). Use a query for the entity directly:
        // memstead_search by title term proves the engine re-boot sees
        // the new entity. If `health` had been boot-cached, the
        // search index would lag.
        let result = server.memstead_search(Parameters(SearchParams {
            query: Some(memstead_base::ops::Query {
                any: vec!["post-boot".into()],
                not: vec![],
                phrase: None,
                field: None,
            }),
            mem: None,
            entity_type: None,
            expand_via: None,
            expand_depth: None,
            related_to: None,
            depth: None,
            edge_type: None,
            limit: None,
            offset: None,
            filters: None,
            range_filters: None,
            stub: None,
            token_budget: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let text = result.content.first().unwrap().as_text().unwrap().text.clone();
        assert!(
            text.contains("demo--post-boot-spec") || text.contains("Post Boot"),
            "post-seed search must surface the seeded entity, got: {text}"
        );
    }

    #[test]
    fn schema_returns_pinned_schema_when_name_matches() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        // Bare-name and canonical pin both work.
        for name in ["default", "default@1.0.0"] {
            let result = server.memstead_schema(Parameters(SchemaParams {
            verbosity: None,
                name: Some(name.into()),
                mem: None,
            }));
            assert!(!result.is_error.unwrap_or(false), "name={name:?}");
            let body = result.structured_content.unwrap();
            // Converged onto the shared `build_schema_payload`: the
            // canonical `ref` subsumes the former top-level `name`/`version`.
            assert_eq!(body["ref"], "default@1.0.0");
            assert!(body["types"].is_array());
            assert!(body["used_by"].is_array());
            assert_eq!(body["used_by"][0], "demo");
        }

        // `mem` shortcut resolves the same schema.
        let result = server.memstead_schema(Parameters(SchemaParams {
            verbosity: None,
            name: None,
            mem: Some("demo".into()),
        }));
        assert!(!result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["ref"], "default@1.0.0");
    }

    /// Surface parity: the public filesystem `/mcp` flavour honours the
    /// same `verbosity` toggle as the mem-repo surface (Plan 01) — it
    /// converged onto the shared `build_schema_payload`, so lite is the
    /// identical structural skeleton and an unknown value refuses typed.
    #[test]
    fn schema_honours_verbosity_toggle() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let lite = server.memstead_schema(Parameters(SchemaParams {
            verbosity: Some("lite".into()),
            name: None,
            mem: Some("demo".into()),
        }));
        assert!(!lite.is_error.unwrap_or(false));
        let lite_body = lite.structured_content.unwrap();
        assert!(
            lite_body["types_summary"].is_array(),
            "lite skeleton present"
        );
        assert!(lite_body.get("types").is_none(), "lite omits rich types");
        assert!(lite_body.get("description").is_none(), "lite drops prose");
        assert_eq!(lite_body["ref"], "default@1.0.0");

        let unknown = server.memstead_schema(Parameters(SchemaParams {
            verbosity: Some("brief".into()),
            name: None,
            mem: Some("demo".into()),
        }));
        assert!(
            unknown.is_error.unwrap_or(false),
            "unknown verbosity refuses"
        );
        let env = unknown.structured_content.unwrap();
        assert_eq!(env["code"], "INVALID_INPUT");
    }

    #[test]
    fn schema_rejects_both_name_and_mem() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let result = server.memstead_schema(Parameters(SchemaParams {
            verbosity: None,
            name: Some("default".into()),
            mem: Some("demo".into()),
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.content[0]
            .as_text()
            .map(|t| t.text.clone())
            .unwrap_or_default();
        assert!(body.contains("INVALID_INPUT"), "got: {body}");
    }

    #[test]
    fn schema_rejects_unknown_mem() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let result = server.memstead_schema(Parameters(SchemaParams {
            verbosity: None,
            name: None,
            mem: Some("nope".into()),
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.content[0]
            .as_text()
            .map(|t| t.text.clone())
            .unwrap_or_default();
        assert!(body.contains("UNKNOWN_MEM"), "got: {body}");
    }

    #[test]
    fn schema_rejects_unknown_name_with_typed_code() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let result = server.memstead_schema(Parameters(SchemaParams {
            verbosity: None,
            name: Some("totally-not-a-schema".into()),
            mem: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "ENTITY_NOT_FOUND");
    }

    #[test]
    fn changes_since_returns_entries_after_timestamp() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        // Three creates → three changelog lines.
        for title in ["A", "B", "C"] {
            seed_via_mcp(&server, title);
        }

        // Empty `since` → all three entries.
        let result = server.memstead_changes_since(Parameters(ChangesSinceParams {
            mem: "demo".into(),
            since: "".into(),
            rename_similarity: None,
            include_notes: false,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["count"], 3);
        let entries = body["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        for entry in entries {
            assert_eq!(entry["kind"], "create");
        }

        // `since` set to a far-future timestamp → empty.
        let result = server.memstead_changes_since(Parameters(ChangesSinceParams {
            mem: "demo".into(),
            since: "9999-01-01T00:00:00.000Z".into(),
            rename_similarity: None,
            include_notes: false,
        }));
        let body = result.structured_content.unwrap();
        assert_eq!(body["count"], 0);
    }

    #[test]
    fn changes_since_handles_missing_changelog_file() {
        // Fresh workspace with no mutations → no changelog file
        // exists. Must return an empty result, not an error.
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let result = server.memstead_changes_since(Parameters(ChangesSinceParams {
            mem: "demo".into(),
            since: "".into(),
            rename_similarity: None,
            include_notes: false,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["count"], 0);
    }

    #[test]
    fn entity_includes_relations_when_flag_set() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (from, _) = seed_via_mcp(&server, "Source");
        let (to, _) = seed_via_mcp(&server, "Target");

        // Add a relation.
        server.memstead_relate(Parameters(RelateParams {
            from: from.clone(),
            to,
            r#type: "USES".into(),
            remove: None,
            note: None,
            description: None,
        }));

        // Read with include_relations.
        let result = server.memstead_entity(Parameters(EntityParams {
            id: from,
            sections: None,
            include_relations: Some(true),
            include_context: None,
            token_budget: None,
            chunk: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let text = result.content.first().unwrap().as_text().unwrap().text.clone();
        assert!(text.contains("## Relations"));
    }

    #[test]
    fn entity_includes_context_when_flag_set() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (id, _) = seed_via_mcp(&server, "Lonely");

        // Read with include_context. The community cache runs Louvain
        // on first call; a single-node graph has a trivial cluster.
        let result = server.memstead_entity(Parameters(EntityParams {
            id,
            sections: None,
            include_relations: None,
            include_context: Some(true),
            token_budget: None,
            chunk: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let text = result.content.first().unwrap().as_text().unwrap().text.clone();
        assert!(text.contains("## Community Context"));
    }

    #[test]
    fn search_returns_results_for_text_query() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        // Seed two entities — one matches the query body, one does not.
        // The
        // engine refuses on missing required sections, so seed both
        // `identity` + `purpose` for each create.
        let mut secs = indexmap::IndexMap::new();
        secs.insert(
            "identity".to_string(),
            "Discusses the architecture of the universe.".to_string(),
        );
        secs.insert("purpose".to_string(), "match purpose".to_string());
        server.memstead_create(Parameters(CreateParams {
            title: "Match".into(),
            entity_type: "spec".into(),
            mem: None,
            sections: Some(secs),
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
        }));
        let mut other_secs = indexmap::IndexMap::new();
        other_secs.insert("identity".to_string(), "other identity".to_string());
        other_secs.insert("purpose".to_string(), "other purpose".to_string());
        server.memstead_create(Parameters(CreateParams {
            title: "Other".into(),
            entity_type: "spec".into(),
            mem: None,
            sections: Some(other_secs),
            metadata: None,
            relations: None,
            dry_run: None,
            note: None,
        }));

        // Issue a search using the structured query shape.
        use memstead_base::ops::Query;
        let result = server.memstead_search(Parameters(SearchParams {
            query: Some(Query {
                any: vec!["architecture".into()],
                not: vec![],
                phrase: None,
                field: None,
            }),
            mem: None,
            entity_type: None,
            expand_via: None,
            expand_depth: None,
            related_to: None,
            depth: None,
            edge_type: None,
            limit: None,
            offset: None,
            filters: None,
            range_filters: None,
            stub: Some(false),
            token_budget: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let text = result.content.first().unwrap().as_text().unwrap().text.clone();
        // Markdown response — rendered by `render_search_markdown`.
        // The matching entity's title or id appears; the
        // non-matching one should not (asserts that the text
        // predicate actually filtered).
        assert!(text.contains("demo--match") || text.contains("Match"));
        assert!(!text.contains("demo--other"));
    }

    #[test]
    fn search_metadata_only_returns_seeded_entities() {
        // Empty query → falls through to metadata-only / list semantics.
        // Asserts the no-text-predicate branch returns hits without
        // tripping on the index path.
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        seed_via_mcp(&server, "Alpha");
        seed_via_mcp(&server, "Beta");

        let result = server.memstead_search(Parameters(SearchParams {
            query: None,
            mem: None,
            entity_type: None,
            expand_via: None,
            expand_depth: None,
            related_to: None,
            depth: None,
            edge_type: None,
            limit: None,
            offset: None,
            filters: None,
            range_filters: None,
            stub: Some(false),
            token_budget: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let text = result.content.first().unwrap().as_text().unwrap().text.clone();
        // Both seeded entities appear when there is no text filter.
        assert!(text.contains("demo--alpha") || text.contains("Alpha"));
        assert!(text.contains("demo--beta") || text.contains("Beta"));
    }

    #[test]
    fn overview_returns_schema_and_mem_sections() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        seed_via_mcp(&server, "Alpha");
        seed_via_mcp(&server, "Beta");

        let result = server.memstead_overview(Parameters(OverviewParams {
            rebuild: None,
            chunk: None,
            mem: None,
            include: None,
            token_budget: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let text = result.content.first().unwrap().as_text().unwrap().text.clone();
        // Frontmatter carries the workspace's pinned schema and an
        // overview_mode field; the markdown body has the standard
        // section headings.
        assert!(text.contains("_mem_schema: default@1.0.0"));
        assert!(text.contains("_overview_mode:"));
        assert!(text.contains("## Schemas"));
        assert!(text.contains("## Mems"));
        assert!(text.contains("### demo"));
        assert!(text.contains("- **Entities:** 2"));
        assert!(text.contains("## Communities"));
    }

    #[test]
    fn overview_rejects_unknown_mem_filter() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let result = server.memstead_overview(Parameters(OverviewParams {
            rebuild: None,
            chunk: None,
            mem: Some("not-the-mem".into()),
            include: None,
            token_budget: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "INVALID_INPUT");
    }

    #[test]
    fn overview_include_community_members_renders_member_ids() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (a, _) = seed_via_mcp(&server, "Alpha");
        let (b, _) = seed_via_mcp(&server, "Beta");
        // Edge so the cluster has structure to discuss.
        server.memstead_relate(Parameters(RelateParams {
            from: a.clone(),
            to: b.clone(),
            r#type: "USES".into(),
            remove: None,
            note: None,
            description: None,
        }));

        let result = server.memstead_overview(Parameters(OverviewParams {
            rebuild: None,
            chunk: None,
            mem: None,
            include: Some(vec!["community_members".into()]),
            token_budget: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let text = result.content.first().unwrap().as_text().unwrap().text.clone();
        // With community_members forced, the rendered cluster lists
        // each member entity id as a bullet — both Alpha and Beta
        // should appear under Communities.
        assert!(text.contains(&format!("- {a}")));
        assert!(text.contains(&format!("- {b}")));
    }

    /// `memstead_overview include=["dangling_links"]` lists every non-stub
    /// entity whose section body wiki-links resolve to a stub or
    /// missing target. Pre-fix the lean overview surface hardcoded
    /// `[]` here and the `## Dangling Links` block never rendered,
    /// even when the health surface populated the same
    /// view. The test fails against that state.
    #[test]
    fn overview_dangling_links_surfaces_stub_targets() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (id, hash) = seed_via_mcp(&server, "Anchor");

        // Rewrite Identity to carry a body wiki-link to a slug with
        // no on-disk file, backed by an atomic REFERENCES declaration
        // (forward-reference auto-stub). The stub is the dangling
        // signal — the dangling-links surface flags wiki-links whose
        // target resolves to a stub entity.
        let mut sections = indexmap::IndexMap::new();
        sections.insert(
            "identity".to_string(),
            "Refers to [[gone]] in prose.".to_string(),
        );
        let upd = server.memstead_update(Parameters(UpdateParams {
            relations_unset: None,
            id: id.clone(),
            expected_hash: hash,
            sections: Some(sections),
            append_sections: None,
            patch_sections: None,
            metadata: None,
            metadata_unset: None,
            dry_run: None,
            note: Some("seed dangling link".into()),
            // Body wiki-link `[[gone]]` is auto-emitted as REFERENCES
            // via the alias-synthesis pass — explicit author refused
            // under the schema's `manual_authoring: forbidden` posture.
            declare_relations: None,
        }));
        assert!(!upd.is_error.unwrap_or(false), "{upd:?}");

        let result = server.memstead_overview(Parameters(OverviewParams {
            rebuild: None,
            chunk: None,
            mem: None,
            include: Some(vec!["dangling_links".into()]),
            token_budget: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let text = result
            .content
            .first()
            .unwrap()
            .as_text()
            .unwrap()
            .text
            .clone();
        assert!(
            text.contains("## Dangling Links"),
            "overview must render the Dangling Links section when an opt-in caller finds one: {text}"
        );
        assert!(
            text.contains(&id),
            "Dangling Links must name the linking entity ({id}): {text}"
        );
        assert!(
            text.contains("demo--gone"),
            "Dangling Links must name the dangling target: {text}"
        );
    }

    #[test]
    fn overview_unknown_include_key_emits_warning() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        seed_via_mcp(&server, "Alpha");

        let result = server.memstead_overview(Parameters(OverviewParams {
            rebuild: None,
            chunk: None,
            mem: None,
            include: Some(vec!["totally-bogus".into()]),
            token_budget: None,
        }));
        assert!(!result.is_error.unwrap_or(false));
        let text = result.content.first().unwrap().as_text().unwrap().text.clone();
        assert!(text.contains("## Warnings"));
        assert!(text.contains("UNKNOWN_INCLUDE_KEY"));
    }

    #[test]
    fn overview_rejects_legacy_schema_types_include() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();

        let result = server.memstead_overview(Parameters(OverviewParams {
            rebuild: None,
            chunk: None,
            mem: None,
            include: Some(vec!["schema_types".into()]),
            token_budget: None,
        }));
        assert!(result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["code"], "INVALID_INPUT");
    }

    #[test]
    fn rename_changes_id_and_persists_through_disk() {
        let tmp = TempDir::new().unwrap();
        write_workspace(&tmp, "demo");
        let server = FilesystemMcpServer::from_workspace_root(tmp.path()).unwrap();
        let (id, hash) = seed_via_mcp(&server, "Old Title");

        let result = server.memstead_rename(Parameters(RenameParams {
            id,
            new_title: "New Title".into(),
            expected_hash: hash,
            note: Some("renamed".into()),
        }));
        assert!(!result.is_error.unwrap_or(false));
        let body = result.structured_content.unwrap();
        assert_eq!(body["new_id"], "demo--new-title");
        assert_eq!(body["new_file_path"], "new-title.md");
        assert!(tmp.path().join("new-title.md").is_file());
        assert!(!tmp.path().join("old-title.md").exists());
    }
}
