//! Output rendering and exit codes.
//!
//! Exit-code table — process status only; the JSON body carries the
//! stable `UPPER_SNAKE_CASE` code under `code`:
//!
//! | Code | Meaning |
//! |------|---------|
//! | 0 | Success |
//! | 1 | Generic error (IO, engine init, parse) |
//! | 2 | Usage / invalid arguments (emitted by clap) |
//! | 3 | Not found (entity / mem / resource missing) |
//! | 4 | Hash mismatch (optimistic-lock violation) |
//! | 5 | Validation error (schema, relation type, etc.) |
//!
//! In `--json` mode, errors emit the documented `{code, message,
//! details}` envelope — same shape agents consume over MCP — to
//! **stdout**, so the documented `memstead <sub> … --json | jq -r .code`
//! recipe retrieves the typed code on the error path without a `2>&1`
//! redirect (success responses already go to stdout; the structured
//! error joins them there). The human-facing markdown error path
//! (no `--json`) stays on stderr and begins `memstead: ERROR [<CODE>]:
//! <message>` so consumers reading only the text channel recover the
//! typed code with one regex.

use serde::Serialize;

/// Exit-code kind for CLI errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitKind {
    Generic = 1,
    NotFound = 3,
    HashMismatch = 4,
    Validation = 5,
}

/// Which standard stream a rendered CLI error is written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorStream {
    Stdout,
    Stderr,
}

/// Decide the target stream and serialized line for a CLI error. Pure —
/// the routing decision is unit-testable without capturing process
/// streams. Under `--json` the `{code, message, details}` envelope goes
/// to **stdout** so the documented `memstead <sub> … --json | jq -r .code`
/// recipe works on the error path (a stdout-only pipe); the human
/// markdown form goes to stderr. `code` is the stable
/// `UPPER_SNAKE_CASE` token (matching the wire envelope agents see over
/// MCP); `details` carries the structured recovery payload under the
/// `details` key.
pub fn render_cli_error(
    code: &str,
    message: &str,
    json_mode: bool,
    details: Option<&serde_json::Value>,
) -> (ErrorStream, String) {
    if json_mode {
        let envelope = match details {
            Some(d) => serde_json::json!({
                "code": code,
                "message": message,
                "details": d,
            }),
            None => serde_json::json!({
                "code": code,
                "message": message,
            }),
        };
        (
            ErrorStream::Stdout,
            serde_json::to_string(&envelope).unwrap_or_default(),
        )
    } else {
        (ErrorStream::Stderr, format!("memstead: ERROR [{code}]: {message}"))
    }
}

/// Print a typed CLI error in the documented surface shape, routed per
/// [`render_cli_error`]: the structured `--json` envelope to stdout, the
/// human markdown form to stderr. The exit code (set by the caller from
/// [`ExitKind`]) signals failure independently of the stream.
pub fn print_cli_error(
    code: &str,
    message: &str,
    _kind: ExitKind,
    json_mode: bool,
    details: Option<&serde_json::Value>,
) {
    let (stream, line) = render_cli_error(code, message, json_mode, details);
    match stream {
        ErrorStream::Stdout => println!("{line}"),
        ErrorStream::Stderr => eprintln!("{line}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Under `--json` the error envelope routes to stdout (so
    /// `… --json | jq -r .code` captures it) and carries the typed `code`.
    #[test]
    fn json_error_routes_to_stdout_with_code() {
        let details = serde_json::json!({ "id": "test--missing" });
        let (stream, line) =
            render_cli_error("ENTITY_NOT_FOUND", "not found", true, Some(&details));
        assert_eq!(stream, ErrorStream::Stdout);
        let parsed: serde_json::Value = serde_json::from_str(&line).expect("valid JSON line");
        assert_eq!(parsed["code"], "ENTITY_NOT_FOUND");
        assert_eq!(parsed["details"]["id"], "test--missing");
    }

    /// The human markdown error form stays on stderr (no `--json`).
    #[test]
    fn markdown_error_routes_to_stderr() {
        let (stream, line) = render_cli_error("ENTITY_NOT_FOUND", "not found", false, None);
        assert_eq!(stream, ErrorStream::Stderr);
        assert!(line.starts_with("memstead: ERROR [ENTITY_NOT_FOUND]: "));
    }
}

/// Print markdown (or JSON when requested) to stdout.
pub fn print_markdown(markdown: &str) {
    println!("{markdown}");
}

/// Print a serializable value as pretty JSON to stdout.
pub fn print_json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    let s = serde_json::to_string_pretty(value)?;
    println!("{s}");
    Ok(())
}
