//! `memstead batch-relate --from <file.json>` — apply many edge changes
//! in one call: one workspace load, one commit per touched mem,
//! all-or-nothing with report-all refusals.
//!
//! One list carries both additions and removals, **applied in order**
//! (a later entry sees the effect of an earlier one — an add followed
//! by a remove of the same edge nets to no edge). Each entry mirrors
//! what `memstead relate` accepts, plus its own provenance `note` —
//! per-entry, like the rest of the batch family; there is no
//! batch-level note flag.
//!
//! ```json
//! { "relates": [
//!     { "from": "specs--alpha", "type": "USES", "to": "specs--beta" },
//!     { "from": "specs--alpha", "type": "USES", "to": "specs--gamma",
//!       "remove": true, "note": "rehang: gamma superseded" },
//!     { "from": "specs--beta", "type": "PART_OF", "to": "specs--suite",
//!       "description": "core member" }
//! ] }
//! ```

use std::path::PathBuf;

use clap::Parser;
use serde::Deserialize;

use memstead_base::vcs::Actor;
use memstead_base::{EntityId, RelateEntityArgs};

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

#[derive(Parser, Debug)]
pub struct Args {
    /// JSON file with a top-level `relates: [...]` array.
    #[arg(long = "from", value_name = "FILE")]
    pub from: PathBuf,
    /// Rehearse the whole batch: run the full in-order validation
    /// (identical refusals, report-all) and report the would-be
    /// receipt, committing nothing — no edge, no stub. `commit_sha`
    /// stays empty (the rehearsal marker).
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

/// Per-entry payload — the `memstead relate` argument set, per entry:
/// `from` / `type` / `to`, optional `remove` (default add), optional
/// per-edge `description` (add path only), optional per-entry `note`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryPayload {
    /// Source entity id.
    from: String,
    /// Rel-type (UPPER_SNAKE_CASE; engine canonicalises).
    #[serde(rename = "type")]
    rel_type: String,
    /// Target entity id.
    to: String,
    /// `false` (default) adds the edge; `true` removes it.
    #[serde(default)]
    remove: bool,
    /// Per-edge description applied on add — validated against the
    /// rel-type's `per_edge_description` posture, like single relate.
    #[serde(default)]
    description: Option<String>,
    /// Agent-authored provenance note for THIS entry's commit record —
    /// mirrors the family's per-entry note handling exactly.
    #[serde(default)]
    note: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let entry_values = super::batch::parse_batch_envelope(&args.from, "relates")?;

    let mut relates: Vec<(RelateEntityArgs, Option<String>)> =
        Vec::with_capacity(entry_values.len());
    for (idx, entry_value) in entry_values.into_iter().enumerate() {
        let entry = match serde_json::from_value::<EntryPayload>(entry_value) {
            Ok(entry) => entry,
            Err(e) => {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    format!("entry {idx}: invalid shape — {e}"),
                )
                .with_details(serde_json::json!({
                    "entry_index": idx,
                    "parser_error": e.to_string(),
                }))
                .into());
            }
        };
        relates.push((
            RelateEntityArgs {
                source: EntityId::canonical(&entry.from),
                expected_hash: None,
                rel_type: entry.rel_type,
                target: EntityId::canonical(&entry.to),
                remove: entry.remove,
                description: entry.description,
                dry_run: false,
            },
            entry.note,
        ));
    }

    let mut engine = crate::setup::full_engine(ctx)?;
    let result = engine
        .batch_relate(relates, Actor::Cli, None, args.dry_run)
        .map_err(CliError::from_engine_op)?;
    // Reload-before-op runs inside `batch_relate` for every mem the
    // batch touches; drain any `mem_changed` notice it stashed.
    let mem_changed = engine.take_mem_changed_notices();

    if result.applied {
        if ctx.json {
            let mut body = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
            crate::commands::merge_mem_changed_json(&mut body, &mem_changed);
            print_json(&body)?;
        } else {
            let mut md = super::batch::render_batch_markdown("relate", &result);
            md.push_str(&crate::commands::render_mem_changed_block(&mem_changed));
            print_markdown(&md);
        }
        return Ok(());
    }

    // Refused batch: standard error envelope with the full result on
    // `details` — same contract as `batch-update` (CLI F12).
    if !ctx.json {
        print_markdown(&super::batch::render_batch_markdown("relate", &result));
    }
    Err(super::batch::batch_refused_error("relate", &result).into())
}
