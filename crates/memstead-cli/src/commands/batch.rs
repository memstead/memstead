//! Shared plumbing for the batch command family (`batch-update`,
//! `batch-create`, `batch-relate`): the per-entry markdown breakdown,
//! the refused-batch error envelope, and the exit-code mapping. One
//! module so the three commands render and refuse identically — the
//! family contract is enforced by construction, not by convention.

use crate::CliError;
use crate::output::ExitKind;

/// Render the per-entry markdown breakdown for a batch result (success
/// or failure). Each entry shows a status marker, its id/action, and any
/// per-entry error code+message; an applied batch appends its commit SHA.
/// `command` is the human-facing command name (`update` / `create` /
/// `relate`).
pub(crate) fn render_batch_markdown(
    command: &str,
    result: &memstead_base::ops::BatchResult,
    dry_run: bool,
) -> String {
    // A rehearsal must never read as an applied batch: `--dry-run`
    // validates everything and writes nothing, and the human-facing
    // markdown has to say so as plainly as the JSON envelope's empty
    // `commit_sha` does (cold-start 0-8-0, F5).
    let header = if result.applied && dry_run {
        format!(
            "# Batch {command} rehearsed — {} item(s) valid, nothing written",
            result.succeeded
        )
    } else if result.applied {
        format!(
            "# Batch {command} applied — {} item(s) in one commit",
            result.succeeded
        )
    } else if dry_run {
        format!(
            "# Batch {command} rehearsal REFUSED — {} item(s) failed (nothing would have been written anyway)",
            result.failed
        )
    } else {
        format!(
            "# Batch {command} REFUSED — {} item(s) failed, nothing committed",
            result.failed
        )
    };
    let mut lines = vec![header, String::new()];
    for entry in &result.results {
        let marker = if entry.action == "error" {
            "✗"
        } else if entry.action == "not_applied" {
            "·"
        } else {
            "✓"
        };
        // On a rehearsal, engine actions arrive in the same past tense
        // as a real run ("created"); render them as conditionals so no
        // line claims a write that did not happen.
        let action: std::borrow::Cow<'_, str> = if dry_run {
            match entry.action.as_str() {
                "created" => "would create".into(),
                "updated" => "would update".into(),
                "related" => "would relate".into(),
                other => other.into(),
            }
        } else {
            entry.action.as_str().into()
        };
        let detail = entry
            .error
            .as_ref()
            .map(|e| format!(" — [{}] {}", e.code, e.message))
            .unwrap_or_default();
        lines.push(format!("- {marker} `{}` ({}){}", entry.id, action, detail));
    }
    if result.errors_suppressed > 0 {
        lines.push(String::new());
        lines.push(format!(
            "{} further failing entr(y/ies) suppressed beyond the detailed-report cap — \
             every failing entry is still marked `error` above.",
            result.errors_suppressed
        ));
    }
    if result.applied && !result.commit_sha.is_empty() {
        lines.push(String::new());
        lines.push(format!("Commit: `{}`", result.commit_sha));
    }
    lines.join("\n")
}

/// Build the error envelope for a refused (atomic) batch. The top-level
/// `code` is the stable `BATCH_REFUSED` token; the `ExitKind` mirrors the
/// dominant (first-reported) entry's failure so `$?` matches the
/// equivalent single command and the documented table (hash mismatch → 4,
/// missing entity / mem → 3, schema/policy refusal → 5). The full
/// [`BatchResult`](memstead_base::ops::BatchResult) rides on `details` —
/// per-entry codes stay available without re-running.
pub(crate) fn batch_refused_error(
    command: &str,
    result: &memstead_base::ops::BatchResult,
) -> CliError {
    let dominant = result.results.iter().find(|e| e.error.is_some());
    let (code, failing_id, message) = match dominant {
        Some(entry) => {
            let err = entry.error.as_ref().expect("dominant entry has an error");
            (err.code.as_str(), entry.id.to_string(), err.message.clone())
        }
        None => (
            "",
            String::new(),
            format!("batch-{command} refused; nothing committed"),
        ),
    };
    let kind = batch_refused_exit_kind(code);
    let summary = format!(
        "batch-{command} refused — {} item(s) failed, nothing committed; first failure [{}] on `{}`: {}",
        result.failed, code, failing_id, message,
    );
    CliError::new(kind, "BATCH_REFUSED", summary)
        .with_details(serde_json::to_value(result).unwrap_or(serde_json::Value::Null))
}

/// Map the dominant per-entry failure code to the process exit code,
/// reusing the documented `0/1/3/4/5` taxonomy so a refused batch exits
/// the same way the equivalent single command would. Unrecognised codes
/// fall to `Validation` (5) — the bucket for schema/policy refusals,
/// which is what most batch-entry failures are.
pub(crate) fn batch_refused_exit_kind(code: &str) -> ExitKind {
    match code {
        "HASH_MISMATCH" => ExitKind::HashMismatch,
        "ENTITY_NOT_FOUND" | "UNKNOWN_MEM" => ExitKind::NotFound,
        _ => ExitKind::Validation,
    }
}

/// Parse a batch `--from` file's envelope: exactly one top-level key
/// (`array_key`, e.g. `updates` / `creates` / `relates`) holding a
/// non-empty JSON array. Unknown top-level keys refuse with a
/// `suggested` hint; a missing or non-array value refuses with the
/// expected shape named.
pub(crate) fn parse_batch_envelope(
    path: &std::path::Path,
    array_key: &'static str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let bytes = std::fs::read(path).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "INVALID_INPUT",
            format!("failed to read {}: {e}", path.display()),
        )
    })?;
    let envelope: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!("invalid JSON in {}: {e}", path.display()),
        )
        .with_details(serde_json::json!({
            "path": path.display().to_string(),
            "parser_error": e.to_string(),
        }))
    })?;
    let entries_value = envelope
        .get(array_key)
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    let entries = match entries_value {
        serde_json::Value::Array(a) => a,
        _ => {
            return Err(CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!("`{array_key}` must be a JSON array"),
            )
            .into());
        }
    };
    // Surface top-level unknown keys too (e.g. a singular typo for the
    // expected plural key).
    if let serde_json::Value::Object(map) = &envelope {
        let unknown: Vec<String> = map
            .keys()
            .filter(|k| k.as_str() != array_key)
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!(
                    "unknown top-level key(s) {unknown:?} — only `{array_key}: [...]` is recognised"
                ),
            )
            .with_details(serde_json::json!({
                "unknown_keys": unknown,
                "suggested": array_key,
            }))
            .into());
        }
    }
    if entries.is_empty() {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!("{array_key}[] is empty"),
        )
        .into());
    }
    Ok(entries)
}
