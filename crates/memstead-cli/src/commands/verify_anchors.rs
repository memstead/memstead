//! `memstead verify-anchors --mem <name>` — the standalone drift
//! statement: verify every anchor in a mem against its declared source,
//! with no binding required. Read-only on mem CONTENT — a sidecar read
//! plus filesystem observation, no entity touched. It is not a pure
//! read: like any verify, a completed run records its findings store.

use clap::Parser;
use serde_json::json;

use crate::CliError;
use crate::output::{print_json, print_markdown};
use crate::setup::CliContext;

/// Verify every anchor in a mem against its declared source. Per
/// anchor: `resolved` (source present, hash matches), `drifted`
/// (present, hash differs, stability says drifted), `recheck` (hash
/// differs under `unstable`, or a hash is missing on either side),
/// `unresolvable` (the source artifact is GONE: a measured failure), or
/// `unobserved` (the pass could not observe the anchor at all, so
/// nothing about it was measured) — honestly, never fabricating a
/// state. The last two shared one bucket until consistency-sweep 03/05,
/// which is why this surface, the one you reach without a binding,
/// could not tell a measured failure from an absent measurement. Works
/// on a hand-authored mem with no binding at all; on a binding-backed
/// mem it reports the same states the binding verify sees (one shared
/// resolution mechanism). No entity changes; findings are recorded for
/// the measured conditions only, since a finding asserts something that
/// was measured.
///
/// A row whose ENTITY the mem no longer holds is reported as `dangling`,
/// its own class beside the states above: those describe the artifact
/// end, and a vanished entity says nothing about the source. Nothing
/// repairs it. Where the entity end could not be reconciled at all, the
/// output says so rather than showing clean counts over state it never
/// examined.
///
/// The counts never travel alone: every rendering states the
/// `population` they were computed over and whether the axis was
/// `fully_adjudicated`, because a resolution figure read on its own is
/// read as health.
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
            memstead_base::ingest::findings::record_standalone_findings(root, &report).map_err(
                |e| {
                    anyhow::Error::from(CliError::new(
                        crate::output::ExitKind::Generic,
                        "FINDINGS_STORE_ERROR",
                        e.to_string(),
                    ))
                },
            )
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
            // The coverage rule (memstead_base::ops::coverage): this
            // surface answers for anchors alone and says so.
            "verdict_coverage": crate::coverage::VERIFY_ANCHORS
                .axis_coverage()
                .expect("verify-anchors is a verdict surface")
                .wire_line(),
            "mem": report.mem,
            "resolved": report.resolved,
            "drifted": report.drifted,
            "recheck": report.recheck,
            "unresolvable": report.unresolvable,
            "unobserved": report.unobserved,
            "dangling": report.dangling,
            "population": report.population_statement(),
            "fully_adjudicated": report.fully_adjudicated(),
            "entity_end_unreconciled": report.unreconciled,
            "anchors": report.anchors,
            "findings": findings,
        }))?;
    } else {
        // The figures and the population they were computed over render as ONE
        // unit (consistency-sweep 03/05, criteria 1 and 3): a count shown
        // without what it could not adjudicate is read as health.
        let mut out = format!(
            "# Anchor verification — `{}`\n\n- Resolved: {}\n- Drifted: {}\n- Recheck: {}\n\
             - Unresolvable (artifact gone): {}\n- Unobserved (not measured this pass): {}\n\
             - Dangling (entity gone): {}\n- Population: {}\n",
            report.mem,
            report.resolved,
            report.drifted,
            report.recheck,
            report.unresolvable,
            report.unobserved,
            report.dangling,
            report.population_statement(),
        );
        // The coverage rule: the one axis this verdict answers for,
        // in the output itself (memstead_base::ops::coverage).
        if let Some(cov) = crate::coverage::VERIFY_ANCHORS.axis_coverage() {
            out.push_str(&format!("- Verdict coverage: {}\n", cov.wire_line()));
        }
        // Stated both ways (consistency-sweep 03/02): a dangling count of
        // zero means "reconciled, none found" only when the reconciliation
        // ran, and four clean counts over a mem whose entity end was never
        // examined are the silent-clean this campaign exists to remove.
        if let Some(why) = &report.unreconciled {
            out.push_str(&format!(
                "\n> **Entity end not reconciled** — {why}. Dangling rows would not have been \
                 detected, so the counts above describe the artifact end only.\n"
            ));
        }
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
            None => out.push_str("\n_Findings not persisted — engine has no workspace root._\n"),
        }
        print_markdown(&out);
    }
    Ok(())
}
