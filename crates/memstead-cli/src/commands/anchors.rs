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
//! * **By mem (the roster).** `memstead anchors --mem <name>` lists every
//!   anchor row of one mem with its live resolution — the list an observer
//!   works from when it has to supply what the engine cannot observe itself
//!   (`--grain url`: the pages to fetch for `verify-anchors --observations`).
//!
//! `--grain` narrows any of the three modes to one anchor grain.

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

    /// The roster: list every anchor row of one mem with its live
    /// resolution state — what an observer reads to learn which artifacts
    /// it must supply observations for (`--grain url` names the pages the
    /// engine never fetches). Mutually exclusive with an entity id and
    /// `--artifact`.
    #[arg(long = "mem", value_name = "NAME", conflicts_with_all = ["id", "artifact"])]
    pub mem: Option<String>,

    /// Keep only anchors of this grain: `span` / `file` / `tree` / `url` /
    /// `entity`. Applies to every mode.
    #[arg(long = "grain", value_name = "GRAIN")]
    pub grain: Option<String>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    if args.id.is_none() && args.artifact.is_none() && args.mem.is_none() {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            "pass an entity id, `--artifact <path>` or `--mem <name>`",
        )
        .into());
    }
    let grain = match args.grain.as_deref() {
        None => None,
        Some(g) => match memstead_base::anchor::AnchorGrain::from_wire(g) {
            Some(grain) => Some(grain),
            None => {
                return Err(CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    format!(
                        "`--grain {g}` is not an anchor grain; one of {}",
                        memstead_base::anchor::AnchorGrain::WIRE_VALUES.join(", ")
                    ),
                )
                .with_details(serde_json::json!({
                    "allowed": memstead_base::anchor::AnchorGrain::WIRE_VALUES
                }))
                .into());
            }
        },
    };

    // Collect the anchor rows off whichever engine backs the workspace.
    // Both variants expose the same read surface. `state` carries the live
    // resolution (present only for a by-entity lookup on a path-medium mem;
    // `None` for the reverse `--artifact` lookup, which spans mems).
    let (rows, unreadable): (Vec<AnchorRow>, Vec<SidecarCondition>) = match ctx.cli_engine()? {
        CliEngine::MemRepo(engine) => {
            if let Some(mem) = args.mem.as_deref()
                && !engine.mem_names().contains(&mem)
            {
                return Err(unknown_mem(mem, engine.mem_names()).into());
            }
            (collect(&engine, &args), unreadable_sidecars(&engine, &args))
        }
        CliEngine::Filesystem(engine) => {
            if let Some(mem) = args.mem.as_deref()
                && !engine.mem_names().contains(&mem)
            {
                return Err(unknown_mem(mem, engine.mem_names()).into());
            }
            (collect(&engine, &args), unreadable_sidecars(&engine, &args))
        }
    };
    let rows: Vec<AnchorRow> = match grain {
        Some(g) => rows
            .into_iter()
            .filter(|(_, a, _, _)| a.grain == g)
            .collect(),
        None => rows,
    };
    // By entity or by mem: the one mem's sidecar is the whole answer, and
    // an unreadable one is a refusal, not "no anchors". By artifact: the
    // lookup spans mems, so the readable ones still answer and the
    // unreadable ones ride along as conditions.
    if (args.id.is_some() || args.mem.is_some())
        && let Some(c) = unreadable.first()
    {
        return Err(CliError::new(
            ExitKind::Validation,
            "ANCHORS_SIDECAR_UNREADABLE",
            format!(
                "mem `{}`: the anchors sidecar could not be read ({}); the entity's anchors \
                 are unknown, not absent",
                c.mem, c.reason
            ),
        )
        .with_details(serde_json::json!({ "mem": c.mem, "reason": c.reason }))
        .into());
    }

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
            // Mems whose sidecar could not be read: their rows are unknown,
            // not absent, and `count` does not cover them.
            "grain": grain.map(|g| g.as_wire()),
            "mem": args.mem,
            "sidecar_unreadable": unreadable
                .iter()
                .map(|c| serde_json::json!({
                    "code": "ANCHORS_SIDECAR_UNREADABLE",
                    "mem": c.mem,
                    "reason": c.reason,
                }))
                .collect::<Vec<_>>(),
        }))?;
    } else if rows.is_empty() && unreadable.is_empty() {
        let subject = args
            .artifact
            .as_deref()
            .map(|p| format!("artifact `{p}`"))
            .or_else(|| args.id.as_deref().map(|i| format!("entity `{i}`")))
            .or_else(|| args.mem.as_deref().map(|m| format!("mem `{m}`")))
            .unwrap_or_default();
        let grain_str = grain
            .map(|g| format!(" of grain `{}`", g.as_wire()))
            .unwrap_or_default();
        print_markdown(&format!(
            "# Anchors\n\nNo anchors{grain_str} for {subject}."
        ));
    } else {
        let mut body = format!("# Anchors ({})\n", rows.len());
        for c in &unreadable {
            body.push_str(&format!(
                "\n> **ANCHORS_SIDECAR_UNREADABLE** — mem `{}`: {}. Its rows are unknown, not \
                 absent; the count above does not cover it.\n",
                c.mem, c.reason
            ));
        }
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

/// `--mem` naming a mem the workspace does not mount: a refusal with the
/// roster, never an empty roster that reads as "no anchors".
fn unknown_mem(mem: &str, known: Vec<&str>) -> CliError {
    CliError::new(
        ExitKind::NotFound,
        "MEM_NOT_FOUND",
        format!("mem `{mem}` is not mounted in this workspace"),
    )
    .with_details(serde_json::json!({ "mem": mem, "known_mems": known }))
}

/// A mem whose anchors sidecar could not be read this pass.
struct SidecarCondition {
    mem: String,
    reason: String,
}

/// The unreadable sidecars the requested lookup touches: the one mem for a
/// by-entity read, every mounted mem for the reverse lookup.
fn unreadable_sidecars(engine: &memstead_base::Engine, args: &Args) -> Vec<SidecarCondition> {
    let mems: Vec<String> = if let Some(id) = args.id.as_deref() {
        vec![EntityId::canonical(id).mem().to_string()]
    } else if let Some(mem) = args.mem.as_deref() {
        vec![mem.to_string()]
    } else {
        engine.mem_names().iter().map(|m| m.to_string()).collect()
    };
    mems.into_iter()
        .filter_map(|mem| {
            engine
                .anchors_sidecar_error(&mem)
                .map(|reason| SidecarCondition { mem, reason })
        })
        .collect()
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
/// and by-mem lookups carry the live resolution state; the reverse
/// `--artifact` lookup spans mems and carries none.
fn collect(engine: &memstead_base::Engine, args: &Args) -> Vec<AnchorRow> {
    if let Some(mem) = args.mem.as_deref() {
        engine
            .mem_anchors_resolved(mem)
            .into_iter()
            .map(|(eid, r)| (eid.to_string(), r.anchor, r.state, r.observed_at))
            .collect()
    } else if let Some(path) = args.artifact.as_deref() {
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
