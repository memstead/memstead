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

    if ctx.json {
        print_json(&json!({
            "mem": report.mem,
            "resolved": report.resolved,
            "drifted": report.drifted,
            "recheck": report.recheck,
            "unresolvable": report.unresolvable,
            "anchors": report.anchors,
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
        print_markdown(&out);
    }
    Ok(())
}
