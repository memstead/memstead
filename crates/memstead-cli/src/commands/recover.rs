//! `memstead recover` — bulk-fix accumulated parse-time drift.
//!
//! Walks `engine.load_warnings()` for `PARSED_RELATION_INVALID`
//! warnings, dispatches the `remove_explicit_relation` recovery on
//! every writable-origin entry (one re-render per source entity),
//! reports the read-only-origin entries as skipped. Per-entry
//! failures keep the rest of the batch alive — the operator inspects
//! the result for `outcome: "failed"` rows.
//!
//! Output:
//! - JSON (default with `--json` global): the raw
//!   `ParseRecoveryReport` shape — `{ entries: [...], commit_sha }`.
//! - Markdown: counts header + one bullet per entry with
//!   outcome / reason / id.

use clap::Parser;

use memstead_base::vcs::Actor;

use crate::CliError;
use crate::output::{print_json, print_markdown};
use crate::setup::CliContext;

/// Apply parse-time-drift recovery actions across every writable
/// mem. Read-only-origin warnings remain (out of scope) and are
/// reported as skipped.
#[derive(Parser, Debug)]
pub struct Args {
    /// Optional commit-body note recorded on every per-source
    /// re-render commit the recovery produces.
    #[arg(long)]
    pub note: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let mut engine = crate::setup::pro_engine(ctx)?;
    let note_ref = args.note.as_deref();
    let report = engine
        .apply_parse_recovery(Actor::Cli, None, note_ref)
        .map_err(CliError::from_engine_op)?;

    let (removed, skipped, failed) = recovery_counts(&report);

    if ctx.json {
        print_json(&recovery_json_envelope(&report))?;
        return Ok(());
    }

    let mut lines = vec![
        format!(
            "# Parse-recovery — {} removed, {} skipped, {} failed",
            removed, skipped, failed
        ),
        String::new(),
    ];
    if report.entries.is_empty() {
        lines.push("_(workspace already clean — no parse-time drops)_".into());
    } else {
        for entry in &report.entries {
            let marker = match entry.outcome.as_str() {
                "removed" => "✓",
                "skipped" => "·",
                _ => "✗",
            };
            let reason = entry
                .reason
                .as_ref()
                .map(|r| format!(" — {r}"))
                .unwrap_or_default();
            lines.push(format!(
                "- {marker} `{}` `{}` → `{}` ({}){}",
                entry.entity_id, entry.rel_type, entry.target, entry.outcome, reason,
            ));
        }
        if !report.commit_sha.is_empty() {
            lines.push(String::new());
            lines.push(format!("Last commit: `{}`", report.commit_sha));
        }
    }
    print_markdown(&lines.join("\n"));

    // Parse-recovery is a best-effort sweep (unlike the atomic
    // `batch-update`): per-entry failures land on the response and the
    // surviving entries are still recovered. Scripts that want a
    // non-zero exit on a partial failure inspect the JSON.
    Ok(())
}

/// `(removed, skipped, failed)` outcome counts over the report's
/// entries — the same three numbers the markdown header and the JSON
/// envelope both surface.
fn recovery_counts(report: &memstead_base::ops::ParseRecoveryReport) -> (usize, usize, usize) {
    let removed = report.entries.iter().filter(|e| e.outcome == "removed").count();
    let skipped = report.entries.iter().filter(|e| e.outcome == "skipped").count();
    let failed = report.entries.iter().filter(|e| e.outcome == "failed").count();
    (removed, skipped, failed)
}

/// Build the `memstead recover --json` envelope. Carries the three counters
/// unconditionally plus the always-present `entries` array, so a clean
/// workspace returns `{removed:0, skipped:0, failed:0, entries:[]}` —
/// distinguishable from a serialization failure or an unrecognised
/// command (the raw report serialises to `{}` when clean because both
/// its fields skip-serialise when empty). Mirrors the markdown channel's
/// counter summary. `commit_sha` stays omitted when no recovery wrote.
fn recovery_json_envelope(
    report: &memstead_base::ops::ParseRecoveryReport,
) -> serde_json::Value {
    let (removed, skipped, failed) = recovery_counts(report);
    let mut obj = serde_json::json!({
        "removed": removed,
        "skipped": skipped,
        "failed": failed,
        "entries": report.entries,
    });
    if !report.commit_sha.is_empty() {
        obj["commit_sha"] = serde_json::json!(report.commit_sha);
    }
    obj
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_json_envelope_clean_workspace_carries_zero_counters_and_empty_entries() {
        // A clean workspace yields a default report (no entries, empty
        // commit_sha). The envelope must be the unambiguous zero-counter
        // object, not `{}`.
        let report = memstead_base::ops::ParseRecoveryReport::default();
        let json = recovery_json_envelope(&report);
        assert_eq!(json["removed"], 0);
        assert_eq!(json["skipped"], 0);
        assert_eq!(json["failed"], 0);
        assert_eq!(json["entries"], serde_json::json!([]));
        // `commit_sha` omitted when nothing wrote — the clean shape is
        // exactly the four documented keys.
        assert!(json.get("commit_sha").is_none());
        assert_ne!(json, serde_json::json!({}), "must not be the empty object");
    }
}
