//! `memstead verify-anchors --mem <name>` — the standalone drift
//! statement: verify every anchor in a mem against its declared source,
//! with no binding required. Read-only on mem CONTENT — a sidecar read
//! plus filesystem observation, no entity touched. It is not a pure
//! read: like any verify, a completed run records its findings store,
//! and (like the binding-backed verify) backfills first-observed hashes
//! onto hash-less anchors in the sidecar, so a manual re-pin drains out
//! of the recheck queue instead of queueing forever.
//!
//! `--observations <file>` supplies what the engine cannot observe itself:
//! a `url` anchor's artifact, retrieved by the caller. Each row adjudicates
//! through the same funnel a file anchor does, and the resulting state is
//! recorded on the sidecar row (`last_observed`, sidecar version 2) so the
//! row ages visibly from then on. The engine never fetches.

use clap::Parser;
use serde_json::json;

use crate::CliError;
use crate::output::{print_json, print_markdown};
use crate::setup::CliContext;

/// Verify every anchor in a mem against its declared source. Per
/// anchor: `resolves` (source present, hash matches), `drifted`
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
/// read as health. A sidecar the engine cannot read is a typed refusal
/// (`ANCHORS_SIDECAR_UNREADABLE`, the mem and the parse reason named,
/// `fully_adjudicated: false` in its details), never zero rows: nothing
/// is recorded for a pass that measured nothing.
#[derive(Parser, Debug)]
pub struct Args {
    /// Which mem to verify (by name).
    #[arg(long = "mem", value_name = "NAME")]
    pub mem_name: String,

    /// JSON file of observer-supplied observations for the anchors the
    /// engine cannot observe itself (`url` grain; the engine never fetches).
    /// Either a bare array or `{"observations": [...]}`; each row is
    /// `{"artifact": "<url>", "hash": "<prepared-content hash>" | "content":
    /// "<retrieved text>" | "absent": true, "observed_at": "<ISO-8601>"?}`
    /// — exactly one of `hash` / `content` / `absent`, `observed_at`
    /// defaulting to now. `content` is hashed under the same rule the write
    /// path applies to an anchor's `content`. A url row with a supplied
    /// observation adjudicates like a file anchor (equal hash `resolves`,
    /// differing hash `drifted` under `stable` and `recheck` under
    /// `unstable`, `absent` → `recheck`); a url row without one stays
    /// `unobserved`. Matched observations are recorded on the sidecar rows
    /// as `last_observed`, so later runs and every anchor surface show how
    /// long each row has gone unobserved. Rows naming no url anchor of the
    /// mem are reported as unmatched and change nothing. A malformed row
    /// refuses the whole run with `INVALID_OBSERVATION` before any state
    /// changes.
    #[arg(long = "observations", value_name = "FILE")]
    pub observations: Option<std::path::PathBuf>,
}

/// Read and validate the `--observations` file: a bare array or an object
/// with an `observations` array. Refuses typed before the engine boots.
fn load_observations(
    path: &std::path::Path,
    now: &str,
) -> anyhow::Result<memstead_base::engine::query::SuppliedObservations> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        CliError::new(
            crate::output::ExitKind::Generic,
            "INVALID_OBSERVATION",
            format!("reading {}: {e}", path.display()),
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        CliError::new(
            crate::output::ExitKind::Validation,
            "INVALID_OBSERVATION",
            format!("{} is not valid JSON: {e}", path.display()),
        )
    })?;
    let rows_value = match &value {
        serde_json::Value::Array(_) => value.clone(),
        serde_json::Value::Object(o) if o.get("observations").is_some_and(|v| v.is_array()) => {
            o["observations"].clone()
        }
        _ => {
            return Err(CliError::new(
                crate::output::ExitKind::Validation,
                "INVALID_OBSERVATION",
                format!(
                    "{} must be a JSON array of observation rows or an object with an \
                     `observations` array",
                    path.display()
                ),
            )
            .into());
        }
    };
    let rows: Vec<memstead_base::anchor::SuppliedObservationInput> =
        serde_json::from_value(rows_value).map_err(|e| {
            CliError::new(
                crate::output::ExitKind::Validation,
                "INVALID_OBSERVATION",
                format!("{}: observation rows do not parse: {e}", path.display()),
            )
        })?;
    memstead_base::anchor::validate_supplied_observations(&rows, now).map_err(|e| {
        CliError::new(crate::output::ExitKind::Validation, e.code(), e.to_string())
            .with_details(serde_json::Value::Object(e.detail().into_iter().collect()))
            .into()
    })
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    // Validate the supplied observations before any engine state is touched:
    // a malformed row refuses the run, and nothing has changed yet.
    let now = memstead_base::engine::mutation::iso_now();
    let supplied = match args.observations.as_deref() {
        Some(path) => load_observations(path, &now)?,
        None => memstead_base::engine::query::SuppliedObservations::new(),
    };

    let mut engine = ctx.cli_engine()?.into_base();

    let report = engine
        .verify_mem_anchors_with(&args.mem_name, &supplied)
        .map_err(|e| anyhow::Error::from(CliError::from_engine_op(e)))?;

    // Nothing was measured: refuse before any observation, backfill or
    // finding is recorded — a findings store fed from an unread sidecar
    // would close every prior finding as "verified clean".
    if let Some(why) = &report.sidecar_error {
        return Err(CliError::new(
            crate::output::ExitKind::Validation,
            "ANCHORS_SIDECAR_UNREADABLE",
            format!(
                "mem `{}`: the anchors sidecar could not be read ({why}); nothing was \
                 measured, so no state is reported and nothing was recorded. Fix or remove \
                 the sidecar, then run again.",
                report.mem
            ),
        )
        .with_details(json!({
            "mem": report.mem,
            "reason": why,
            "fully_adjudicated": report.fully_adjudicated(),
            "population": report.population_statement(),
            "verdict_coverage": crate::coverage::VERIFY_ANCHORS
                .axis_coverage()
                .expect("verify-anchors is a verdict surface")
                .wire_line(),
        }))
        .into());
    }

    // Record the supplied observations on their url rows (`last_observed`),
    // so the rows carry a dated state from here on and age visibly. An
    // identical re-run stages nothing.
    let observations_recorded = engine
        .record_anchor_observations(
            &args.mem_name,
            &report.recordable_observations,
            Some("verify-anchors: supplied observations recorded"),
        )
        .map_err(|e| anyhow::Error::from(CliError::from_engine_op(e)))?;

    // Backfill observed hashes onto hash-less anchors, exactly as the
    // binding-backed verify does after its pass — the engine writer skips
    // anchors that already carry a hash, so a re-run stages nothing. Until
    // 2026-08-31 only the binding path backfilled, so every manually
    // re-pinned anchor on a binding-less mem read `recheck` forever and its
    // repair waited on a verify surface the mem did not have (backlog, live
    // melt: every manual re-pin read `hash_source: backfill`).
    let backfill: Vec<memstead_base::anchor::ObservedArtifactHash> = report
        .anchors
        .iter()
        .filter(|a| {
            a.state == "recheck"
                && a.observed_hash.is_some()
                && matches!(a.class.as_str(), "anchored" | "derived")
        })
        .map(|a| memstead_base::anchor::ObservedArtifactHash {
            entity: a.entity_id.clone(),
            artifact: a.artifact.clone(),
            hash: a.observed_hash.clone().expect("filtered on Some"),
        })
        .collect();
    let backfilled = engine
        .record_anchor_observed_hashes(
            &args.mem_name,
            &backfill,
            Some("verify-anchors: first-observation hash backfill"),
        )
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
            "resolves": report.resolves,
            "drifted": report.drifted,
            "recheck": report.recheck,
            "unresolvable": report.unresolvable,
            "unobserved": report.unobserved,
            "dangling": report.dangling,
            "population": report.population_statement(),
            "fully_adjudicated": report.fully_adjudicated(),
            "entity_end_unreconciled": report.unreconciled,
            "anchors": report.anchors,
            "hash_backfilled": backfilled,
            "observations": {
                "supplied": supplied.len(),
                "matched": supplied.len() - report.unmatched_observations.len(),
                "unmatched": report.unmatched_observations,
                "recorded": observations_recorded,
            },
            "findings": findings,
        }))?;
    } else {
        // The figures and the population they were computed over render as ONE
        // unit (consistency-sweep 03/05, criteria 1 and 3): a count shown
        // without what it could not adjudicate is read as health.
        let mut out = format!(
            "# Anchor verification — `{}`\n\n- Resolves: {}\n- Drifted: {}\n- Recheck: {}\n\
             - Unresolvable (artifact gone): {}\n- Unobserved (not measured this pass): {}\n\
             - Dangling (entity gone): {}\n- Population: {}\n",
            report.mem,
            report.resolves,
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
            // Rows that do not resolve are the actionable set; resolving
            // rows stay off the detail list so a healthy mem reads as four
            // counts, not a table.
            let flagged: Vec<_> = report
                .anchors
                .iter()
                .filter(|a| a.state != memstead_base::anchor::AnchorState::Resolves.as_wire())
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
            // Rows whose state rests on a recorded observation, by age: a
            // url row is adjudicated only when someone observes it, so how
            // long ago that was is part of what the state means.
            let aging: Vec<_> = report
                .anchors
                .iter()
                .filter(|a| a.observed_at.is_some())
                .collect();
            if !aging.is_empty() {
                out.push_str("\n## Observed rows (url)\n\n");
                for a in aging {
                    let days = a.unobserved_for_days.unwrap_or(0);
                    let age = if a.observation_supplied {
                        "observed this run".to_string()
                    } else {
                        format!("unobserved for {days} day(s)")
                    };
                    out.push_str(&format!(
                        "- **{}**: `{}` → `{}` — observed {} ({age})\n",
                        a.state,
                        a.entity_id,
                        a.artifact,
                        a.observed_at.as_deref().unwrap_or("?"),
                    ));
                }
            }
        }
        if !supplied.is_empty() {
            out.push_str(&format!(
                "\nObservations supplied: {}, matched {}, recorded on {} row(s).\n",
                supplied.len(),
                supplied.len() - report.unmatched_observations.len(),
                observations_recorded,
            ));
            if !report.unmatched_observations.is_empty() {
                out.push_str("Unmatched (no url anchor of this mem names the artifact):\n");
                for u in &report.unmatched_observations {
                    out.push_str(&format!("- `{u}`\n"));
                }
            }
        }
        if backfilled > 0 {
            out.push_str(&format!(
                "\nBackfilled {backfilled} observed hash(es) onto hash-less anchors — the \
                 recheck queue drains on the next pass.\n"
            ));
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
