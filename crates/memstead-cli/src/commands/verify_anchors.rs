//! `memstead verify-anchors --mem <name>` — the standalone drift
//! statement: verify every anchor in a mem against its declared source,
//! with no binding required. Read-only on mem content — pure sidecar
//! read plus filesystem observation, no commit on any backend.

use clap::Parser;
use serde_json::json;

use crate::CliError;
use crate::output::{print_json, print_markdown};
use crate::setup::CliContext;

/// Verify every anchor in a mem against its declared source. Per
/// anchor: `resolved` (source present, hash matches), `drifted`
/// (present, hash differs, stability says drifted), `recheck` (hash
/// differs under `unstable`, or a hash is missing on either side), or
/// `unresolvable` (source absent, or a grain the mechanism does not
/// reach) — honestly, never fabricating a state. Works on a
/// hand-authored mem with no binding at all; on a binding-backed mem it
/// reports the same states the binding verify sees (one shared
/// resolution mechanism). Read-only: no entity changes, no commit.
#[derive(Parser, Debug)]
pub struct Args {
    /// Which mem to verify (by name).
    #[arg(long = "mem", value_name = "NAME")]
    pub mem_name: String,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let cli_engine = ctx.cli_engine()?;
    let engine = cli_engine.base();

    let report = engine
        .verify_mem_anchors(&args.mem_name)
        .map_err(|e| anyhow::Error::from(CliError::from_engine_op(e)))?;

    // Persist the flagged findings under the mem-scoped standalone
    // key (agent-trust plan 14): a binding-less mem's verification no
    // longer observes-and-forgets — the next pass re-serves what the
    // previous one recorded as `already_seen`. Binding-backed stores
    // (keyed by hash(D), own files) are untouched. An engine without
    // a workspace root has no durable store; stated, never silent.
    let persisted = engine
        .workspace_root()
        .map(|root| {
            memstead_base::ingest::findings::record_standalone_findings(root, &report)
                .map_err(|e| {
                    anyhow::Error::from(CliError::new(
                        crate::output::ExitKind::Generic,
                        "FINDINGS_STORE_ERROR",
                        e.to_string(),
                    ))
                })
        })
        .transpose()?;

    if ctx.json {
        let findings = persisted.as_ref().map(|fs| {
            json!({
                "new": fs.iter().filter(|f| !f.already_seen).count(),
                "already_seen": fs.iter().filter(|f| f.already_seen).count(),
                "items": fs,
            })
        });
        print_json(&json!({
            "mem": report.mem,
            "resolved": report.resolved,
            "drifted": report.drifted,
            "recheck": report.recheck,
            "unresolvable": report.unresolvable,
            "anchors": report.anchors,
            "findings": findings,
        }))?;
    } else {
        let mut out = format!(
            "# Anchor verification — `{}`\n\n- Resolved: {}\n- Drifted: {}\n- Recheck: {}\n- Unresolvable: {}\n",
            report.mem, report.resolved, report.drifted, report.recheck, report.unresolvable,
        );
        if report.anchors.is_empty() {
            out.push_str("\n_(no anchors in this mem)_\n");
        } else {
            // Non-resolved rows are the actionable set; resolved rows
            // stay off the detail list so a healthy mem reads as four
            // counts, not a table.
            let flagged: Vec<_> = report
                .anchors
                .iter()
                .filter(|a| a.state != "resolved")
                .collect();
            if !flagged.is_empty() {
                out.push_str("\n## Flagged anchors\n\n");
                for a in flagged {
                    out.push_str(&format!(
                        "- **{}**: `{}` → `{}` ({} {})\n",
                        a.state, a.entity_id, a.artifact, a.class, a.grain,
                    ));
                }
            }
        }
        match &persisted {
            Some(fs) => {
                let new = fs.iter().filter(|f| !f.already_seen).count();
                let seen = fs.len() - new;
                out.push_str(&format!(
                    "\nFindings persisted (standalone store): {new} new, {seen} already seen.\n"
                ));
            }
            None => out.push_str(
                "\n_Findings not persisted — engine has no workspace root._\n",
            ),
        }
        print_markdown(&out);
    }
    Ok(())
}
