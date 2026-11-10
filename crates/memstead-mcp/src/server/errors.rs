//! Error translation: the exhaustive `EngineError` to typed-envelope mapping both handler flavours share, with the not-found and did-you-mean helpers that dress it.

use super::*;

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
pub(super) fn engine_err_unified(
    e: memstead_base::EngineError,
    _engine: &memstead_base::Engine,
) -> CallToolResult {
    // One engine-side value, encoded: the code, the recovery payload and
    // the message all come from the error itself, so this surface and the
    // CLI cannot serve two payloads for one state. The text channel uses
    // the rich-prose renderer so an agent reading only
    // `result.content[0].text` sees the full recovery payload inline.
    let message = e.prose_render();
    let code = e.code();
    tool_error_with_payload(code, &message, envelope(code, message.clone(), e.details()))
}

/// Typed-envelope translator for `FullEngineError`. Delegates wrapped
/// base-engine errors to [`engine_err_unified`]; constructs the lifecycle-
/// specific envelopes (`MEM_PATH_NOT_ALLOWED`,
/// `MEM_REFERENCED_BY_POLICY`, `MEM_SCHEMA_NOT_ALLOWED`,
/// `CONFIG_ERROR`) here. The wire shape is
/// bit-identical to what `engine_err_unified` produced for the same
/// variants before the lifecycle variants moved off
/// `memstead_base::EngineError`; the move is pure plumbing.
/// An engine error with one sentence of this surface's own appended to
/// the message: the recovery step named in MCP terms (a tool or a
/// parameter). The code and the payload stay the engine's.
pub(super) fn engine_err_with_surface_hint(
    e: memstead_base::EngineError,
    hint: &str,
) -> CallToolResult {
    let message = format!("{} {hint}", e.prose_render());
    let code = e.code();
    tool_error_with_payload(code, &message, envelope(code, message.clone(), e.details()))
}

pub(super) fn full_engine_err_unified(
    e: memstead_base::FullEngineError,
    engine: &memstead_base::Engine,
) -> CallToolResult {
    match e {
        memstead_base::FullEngineError::Engine(inner) => engine_err_unified(inner, engine),
        lifecycle => {
            let message = lifecycle.prose_render();
            let code = lifecycle.code();
            tool_error_with_payload(
                code,
                &message,
                envelope(code, message.clone(), lifecycle.details()),
            )
        }
    }
}

/// Find entity IDs that end with the given suffix (slug or medium--slug).
/// Returns up to `max` suggestions for "did you mean?" messages.
///
/// Takes `&Store` directly so the helper composes against any
/// engine — `memstead_base::Engine::store()` exposes a `memstead_base::Store`.
pub(super) fn suggest_similar(store: &memstead_base::Store, input: &str) -> Vec<String> {
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
pub(super) fn not_found_error(store: &memstead_base::Store, id: &EntityId) -> CallToolResult {
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
