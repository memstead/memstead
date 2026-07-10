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
    // Both variants expose the same read surface.
    let rows: Vec<(String, memstead_base::anchor::Anchor)> = match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(engine) => collect(&engine, &args),
        CliEngine::Filesystem(engine) => collect(&engine, &args),
    };

    if ctx.json {
        let anchors_json: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, a)| {
                let mut v = serde_json::to_value(a).unwrap_or(serde_json::Value::Null);
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("entity_id".into(), serde_json::json!(id));
                }
                v
            })
            .collect();
        let anchors_only: Vec<memstead_base::anchor::Anchor> =
            rows.iter().map(|(_, a)| a.clone()).collect();
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
        for (id, a) in &rows {
            let hash = a.hash.as_deref().unwrap_or("-");
            body.push_str(&format!(
                "\n- `{id}` — {} {} `{}` (hash: {hash})",
                a.class.as_wire(),
                a.grain.as_wire(),
                a.artifact,
            ));
        }
        print_markdown(&body);
    }
    Ok(())
}

/// Gather anchor rows from an engine per the requested mode.
fn collect(
    engine: &memstead_base::Engine,
    args: &Args,
) -> Vec<(String, memstead_base::anchor::Anchor)> {
    if let Some(path) = args.artifact.as_deref() {
        engine
            .anchors_referencing_artifact(path)
            .into_iter()
            .map(|(id, a)| (id.to_string(), a))
            .collect()
    } else if let Some(id) = args.id.as_deref() {
        let eid = EntityId::canonical(id);
        engine
            .entity_anchors(&eid)
            .into_iter()
            .map(|a| (eid.to_string(), a))
            .collect()
    } else {
        Vec::new()
    }
}
