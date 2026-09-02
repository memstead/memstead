//! `memstead anchors` — read provenance anchors (E3a).
//!
//! Two read modes, no mutation:
//!
//! * **By entity.** `memstead anchors <id>` lists the entity's stored
//!   anchors plus their class/grain composition.
//! * **By artifact (reverse lookup).** `memstead anchors --artifact <path>`
//!   lists every `(entity, anchor)` across all mems whose anchor
//!   references that path. This is the query the rebuilt
//!   check-realization plugin hook consumes: given the file an agent just
//!   edited, which entities anchored to it. `tree`-grain anchors match the
//!   path and anything beneath the tree.

use clap::Parser;

use memstead_base::EntityId;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// Read provenance anchors by entity or by referenced artifact path.
#[derive(Parser, Debug)]
pub struct Args {
    /// Entity ID (e.g. `specs--my-entity`). Required unless `--artifact`
    /// is given.
    pub id: Option<String>,

    /// Reverse lookup: list every entity whose anchor references this
    /// artifact path. Mutually exclusive with a positional entity id.
    #[arg(long = "artifact", value_name = "PATH", conflicts_with = "id")]
    pub artifact: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    if args.id.is_none() && args.artifact.is_none() {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            "pass an entity id or `--artifact <path>`",
        )
        .into());
    }

    // Collect the anchor rows off whichever engine backs the workspace.
    // Both variants expose the same read surface. `state` carries the live
    // resolution (present only for a by-entity lookup on a path-medium mem;
    // `None` for the reverse `--artifact` lookup, which spans mems).
    let rows: Vec<AnchorRow> = match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(engine) => collect(&engine, &args),
        CliEngine::Filesystem(engine) => collect(&engine, &args),
    };

    let now = memstead_base::engine::mutation::iso_now();
    if ctx.json {
        let anchors_json: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, a, state, observed_at)| {
                let mut v = serde_json::to_value(a).unwrap_or(serde_json::Value::Null);
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("entity_id".into(), serde_json::json!(id));
                    if let Some(s) = state {
                        obj.insert("state".into(), serde_json::json!(s.as_wire()));
                    }
                    if let Some(at) = observed_at {
                        obj.insert("observed_at".into(), serde_json::json!(at));
                        if let Some(days) = memstead_base::anchor::days_between(at, &now) {
                            obj.insert("unobserved_for_days".into(), serde_json::json!(days));
                        }
                    }
                }
                v
            })
            .collect();
        let anchors_only: Vec<memstead_base::anchor::Anchor> =
            rows.iter().map(|(_, a, _, _)| a.clone()).collect();
        let composition = memstead_base::anchor::compose_entity_anchors(&anchors_only);
        print_json(&serde_json::json!({
            "count": rows.len(),
            "anchors": anchors_json,
            "composition": composition,
        }))?;
    } else if rows.is_empty() {
        let subject = args
            .artifact
            .as_deref()
            .map(|p| format!("artifact `{p}`"))
            .or_else(|| args.id.as_deref().map(|i| format!("entity `{i}`")))
            .unwrap_or_default();
        print_markdown(&format!("# Anchors\n\nNo anchors for {subject}."));
    } else {
        let mut body = format!("# Anchors ({})\n", rows.len());
        for (id, a, state, observed_at) in &rows {
            let hash = a.hash.as_deref().unwrap_or("-");
            let state_str = state
                .map(|s| format!(", state: {}", s.as_wire()))
                .unwrap_or_default();
            // A recorded observation's age travels with its state: a url
            // row's state is exactly as current as the observation it rests on.
            let age_str = observed_at
                .as_deref()
                .map(|at| {
                    let days = memstead_base::anchor::days_between(at, &now).unwrap_or(0);
                    format!(", observed {at}, unobserved for {days} day(s)")
                })
                .unwrap_or_default();
            body.push_str(&format!(
                "\n- `{id}` — {} {} `{}` (hash: {hash}{state_str}{age_str})",
                a.class.as_wire(),
                a.grain.as_wire(),
                a.artifact,
            ));
        }
        print_markdown(&body);
    }
    Ok(())
}

/// One anchor row: `(entity_id, anchor, live_state, observed_at)` — the
/// last element is present for a row whose state rests on a recorded
/// observation (a `url` row) rather than a live one.
type AnchorRow = (
    String,
    memstead_base::anchor::Anchor,
    Option<memstead_base::anchor::AnchorState>,
    Option<String>,
);

/// Gather anchor rows from an engine per the requested mode. The by-entity
/// lookup carries the live resolution state; the reverse `--artifact` lookup
/// spans mems and carries none.
fn collect(engine: &memstead_base::Engine, args: &Args) -> Vec<AnchorRow> {
    if let Some(path) = args.artifact.as_deref() {
        engine
            .anchors_referencing_artifact(path)
            .into_iter()
            .map(|(id, a)| (id.to_string(), a, None, None))
            .collect()
    } else if let Some(id) = args.id.as_deref() {
        let eid = EntityId::canonical(id);
        engine
            .entity_anchors_resolved(&eid)
            .into_iter()
            .map(|r| (eid.to_string(), r.anchor, r.state, r.observed_at))
            .collect()
    } else {
        Vec::new()
    }
}
