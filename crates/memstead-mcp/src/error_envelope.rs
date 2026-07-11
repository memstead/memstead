//! Tool-error envelope helpers used by the full `ServerHandler` to
//! construct `CallToolResult` responses with the engine's typed wire
//! shape: text channel prefixed `ERROR [<CODE>]: <message>`,
//! `structured_content` carrying the full payload (or absent on the
//! prose-only path), `is_error` set so MCP clients render the response
//! as a failure. Both helpers live in `memstead-mcp` because the
//! full server is their only consumer; the lean filesystem server
//! builds its envelopes through the parallel helpers inline in
//! `memstead-mcp::filesystem_server`.
//!
//! Wire-shape byte-identity for these envelopes is pinned by
//! `memstead-mcp/tests/wire_shape.rs`; any change here that alters the
//! text-channel prefix or the structured payload would trip those
//! tests.

use rmcp::model::{CallToolResult, ContentBlock};

/// Build a typed-code tool-error response without a structured details
/// payload. Text channel reads `"ERROR [<CODE>]: <msg>"` and
/// `structured_content` carries `{code, message}` (no `details`); the
/// `code` field matches the wire envelope agents see from
/// `tool_error_with_payload`, so consumers that branch on the
/// `UPPER_SNAKE_CASE` token get the same shape on either form. Use
/// this helper when the failure has no structured recovery payload
/// beyond the message body; reach for [`tool_error_with_payload`]
/// otherwise.
///
/// Pre-fix the text channel emitted `"ERROR: <msg>"` without the
/// typed code, leaving the prefix-form contract a half-delivered
/// promise on the simple form.
pub fn tool_error(code: &str, msg: &str) -> CallToolResult {
    let payload = serde_json::json!({ "code": code, "message": msg });
    let mut r = CallToolResult::success(vec![ContentBlock::text(format!("ERROR [{code}]: {msg}"))]);
    r.is_error = Some(true);
    r.structured_content = Some(payload);
    r
}

/// Build a structured tool-error response with the typed envelope on
/// `structured_content`. The text channel mirrors the same `code`
/// inline as `"ERROR [<CODE>]: <msg>"` so a consumer that only reads
/// `result.content[0].text` (Claude Code's default rendering, log
/// scrapes, terminal dumps) still recovers the UPPER_SNAKE_CASE code
/// with a one-line regex (`^ERROR \[([A-Z_]+)\]: `).
///
/// The `code` argument is explicit (and non-optional). An earlier
/// shape extracted the code from `payload["code"]` and defaulted to
/// `"INTERNAL"` when the key was absent — the explicit parameter makes
/// a missing-code regression compile-impossible instead of
/// review-impossible. Callsites that
/// build a payload via `memstead_base::ops::envelope(code, message, details)`
/// spell the same `code` twice — once in the outer call, once inside
/// `envelope(...)` — by design: the outer code is what the text
/// channel emits; the inner code is what `structured_content` carries.
pub fn tool_error_with_payload(
    code: &str,
    msg: &str,
    payload: serde_json::Value,
) -> CallToolResult {
    let mut r = CallToolResult::success(vec![ContentBlock::text(format!("ERROR [{code}]: {msg}"))]);
    r.is_error = Some(true);
    r.structured_content = Some(payload);
    r
}
