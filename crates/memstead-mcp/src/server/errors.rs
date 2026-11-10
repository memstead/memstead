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
/// An engine error, encoded: code, payload and message are the error's own.
pub(super) fn engine_err(e: memstead_base::EngineError) -> CallToolResult {
    let message = e.prose_render();
    let code = e.code();
    tool_error_with_payload(code, &message, envelope(code, message.clone(), e.details()))
}

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
