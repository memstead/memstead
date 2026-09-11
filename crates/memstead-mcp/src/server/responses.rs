//! Response construction: the JSON and markdown envelopes, the mem-changed and drift attachments, the schema anchor injection, and the input validators that short-circuit into an error result.

use super::*;

/// Build a `_meta` map carrying `anthropic/alwaysLoad: true` so Claude
/// Code excludes the tagged tool from its `ToolSearch`-deferred set.
/// Applied to `memstead_overview` — the cold-start entry point that the
/// server `instructions` direct agents to call first. Without this,
/// agents pay an extra `ToolSearch` round-trip before they can reach
/// overview.
pub(super) fn always_load_meta() -> rmcp::model::MetaObject {
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
pub(super) fn validate_entity_id(id: &str) -> Option<CallToolResult> {
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
pub(super) fn validate_note(note: Option<&str>) -> Option<CallToolResult> {
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
pub(super) fn json_response<T: serde::Serialize>(data: &T) -> CallToolResult {
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
pub(super) fn finalize_health_text(
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
pub(super) fn append_warning_hint(
    mut res: CallToolResult,
    warning: &WarningHint,
) -> CallToolResult {
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
pub(super) fn md_response(markdown: String) -> CallToolResult {
    CallToolResult::success(vec![rmcp::model::ContentBlock::text(markdown)])
}

/// Tool response that pairs rendered markdown on the text channel with
/// a structured envelope on `structured_content`. Tools whose response
/// has a
/// canonical human-readable form (entity, search) ship the markdown to
/// terminal/inline consumers and the typed JSON to branching agents in
/// one call, with no extra round-trip. The agent contract: branch on
/// `structured_content`; read the text channel for prose.
pub(super) fn md_with_structured(
    markdown: String,
    structured: serde_json::Value,
) -> CallToolResult {
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
pub(super) fn attach_mem_changed(body: &mut serde_json::Value, notices: Vec<MemChangedNotice>) {
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
pub(super) fn attach_durability(
    body: &mut serde_json::Value,
    engine: &memstead_base::Engine,
    mem: &str,
) {
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
pub(super) fn attach_mem_changed_to_result(
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
pub(super) fn notices_as_reload_warnings(notices: &[MemChangedNotice]) -> Vec<WarningHint> {
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
pub(super) fn attach_drift_to_error(
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
pub(super) fn prepend_drift_warnings_to_result_text(
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

pub(super) fn prepend_drift_warnings_md(md: String, drift_warnings: &[WarningHint]) -> String {
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
pub(super) fn mem_schema_ref_unified(
    engine: &memstead_base::Engine,
    mem_name: &str,
) -> Option<String> {
    memstead_base::overview::mem_schema_ref(engine, mem_name)
}

pub(super) fn find_schema_unified<'a>(
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
pub(super) fn find_schema_by_name<'a>(
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
pub(super) fn inject_md_mem_schema(md: &mut String, schema_ref: &str) {
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
pub(super) fn with_mem_schema_anchor(mut res: CallToolResult, schema_ref: &str) -> CallToolResult {
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
