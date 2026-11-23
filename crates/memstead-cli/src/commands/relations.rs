use clap::Parser;

use memstead_base::EntityId;
use memstead_base::render;

use crate::CliError;
use crate::output::{print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// List typed edges for an entity.
#[derive(Parser, Debug)]
pub struct Args {
    /// Entity ID.
    pub id: String,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let id = EntityId::canonical(&args.id);
    let (outgoing, incoming) = match ctx.cli_engine()? {
        CliEngine::MemRepo(engine) => relations_from_engine(&id, &args.id, &engine)?,
        CliEngine::Filesystem(engine) => relations_from_engine(&id, &args.id, &engine)?,
    };
    let payload = render::render_relations_json(id.as_ref(), &outgoing, &incoming);

    if ctx.json {
        print_json(&payload)?;
        return Ok(());
    }

    let mut lines = Vec::new();
    lines.push(format!("# Relations — {}", args.id));
    lines.push(String::new());

    lines.push("## Outgoing".to_string());
    if outgoing.is_empty() {
        lines.push("_none_".to_string());
    } else {
        for e in &outgoing {
            lines.push(format!("- **{}** → [[{}]]", e.rel_type, e.target));
        }
    }
    lines.push(String::new());

    lines.push("## Incoming".to_string());
    if incoming.is_empty() {
        lines.push("_none_".to_string());
    } else {
        for e in &incoming {
            lines.push(format!("- **{}** ← [[{}]]", e.rel_type, e.from));
        }
    }

    print_markdown(&lines.join("\n"));
    Ok(())
}

/// Resolve outgoing/incoming edge lists for `id` from a `&Store`.
/// Engine-agnostic: both workspace shapes expose the same store accessor.
/// Returns `Err(NotFound)` when the entity is not in the store; the
/// not-found check is here so the CLI exit code is uniform across
/// workspace shapes.
fn relations_from_engine(
    id: &EntityId,
    id_for_err: &str,
    engine: &memstead_base::Engine,
) -> anyhow::Result<(Vec<memstead_base::Edge>, Vec<memstead_base::InEdge>)> {
    let store = engine.store();
    if store.get(id).is_none() {
        return Err(CliError::from_engine_op(engine.not_found(id_for_err)).into());
    }
    Ok((store.outgoing(id).to_vec(), store.incoming(id).to_vec()))
}
