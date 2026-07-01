use clap::Parser;

use memstead_base::Entity;
use memstead_base::EntityId;
use memstead_base::Store;
use memstead_base::chunking::apply_chunking;
use memstead_base::render;

use crate::CliError;
use crate::output::{ExitKind, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// Read one entity as markdown.
#[derive(Parser, Debug)]
pub struct Args {
    /// Entity ID (e.g. `specs--my-entity`).
    pub id: String,

    /// Restrict output to specific section keys (repeatable).
    #[arg(long = "section", value_name = "KEY")]
    pub sections: Vec<String>,

    /// Append relations as a trailing JSON code block.
    #[arg(long)]
    pub include_relations: bool,

    /// Token budget for chunking. Omit for no chunking.
    #[arg(long)]
    pub token_budget: Option<usize>,

    /// 1-based chunk index to return (requires `--token-budget`).
    #[arg(long)]
    pub chunk: Option<usize>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let id = EntityId::canonical(&args.id);
    // The engine's `get_entity` returns `Option<&Entity>`, so the
    // typed `ENTITY_NOT_FOUND` code lives on the CLI side here —
    // pin it explicitly so the wire envelope matches what the engine
    // would emit from a write-path miss.
    let not_found = || {
        CliError::new(
            ExitKind::NotFound,
            "ENTITY_NOT_FOUND",
            format!("Entity not found: {}", args.id),
        )
        .with_details(serde_json::json!({ "id": args.id }))
    };
    // Snapshot the store's outgoing edges alongside the entity so the
    // JSON envelope can resolve each relationship's `source` label after
    // the engine goes out of scope. The outgoing edges carry the
    // authoritative `EdgeSource` discriminator (`Explicit` /
    // `BodyLink` / `Hierarchy`); the entity's `relationships` vec
    // doesn't encode it.
    let (entity, output, outgoing_snapshot) = match ctx.cli_engine()? {
        #[cfg(feature = "vault-repo")]
        CliEngine::VaultRepo(engine) => {
            let entity = engine.get_entity(&id).cloned().ok_or_else(not_found)?;
            let md = render_with_optional_relations(&entity, &id, engine.store(), &args);
            let outgoing = engine.store().outgoing(&id).to_vec();
            (entity, md, outgoing)
        }
        CliEngine::Filesystem(engine) => {
            let entity = engine.get_entity(&id).cloned().ok_or_else(not_found)?;
            let md = render_with_optional_relations(&entity, &id, engine.store(), &args);
            let outgoing = engine.store().outgoing(&id).to_vec();
            (entity, md, outgoing)
        }
    };

    let chunked = match args.token_budget {
        Some(budget) => apply_chunking(&output, budget, args.chunk, &[("_hash", &entity.content_hash)])
            .map_err(|e| CliError::new(ExitKind::Generic, "CHUNK_OUT_OF_RANGE", e))?,
        None => output.to_string(),
    };

    if ctx.json {
        // The CLI `--json`
        // shape mirrors the MCP `structured_content` envelope —
        // typed fields (`_hash`, `id`, `vault`, `type`, sections,
        // relationships) rather than a
        // `{ markdown: "..." }` flat shape that would force agents to
        // string-scrape frontmatter for `_hash`. Greenfield
        // justifies the wire shape break.
        let sections_filter = if args.sections.is_empty() {
            None
        } else {
            Some(args.sections.as_slice())
        };
        let rendered_body_tokens = memstead_base::chunking::estimate_tokens(&output);
        let full_tokens = if sections_filter.is_some() {
            let full_body = render::render_entity_markdown(&entity, None);
            Some(memstead_base::chunking::estimate_tokens(&full_body))
        } else {
            None
        };
        let envelope = render::build_entity_envelope(
            &entity,
            rendered_body_tokens,
            full_tokens,
            sections_filter,
            None,
            &outgoing_snapshot,
        );
        crate::output::print_json(&envelope)?;
    } else {
        print_markdown(&chunked);
    }
    Ok(())
}

/// Render an entity's markdown body and, when `--include-relations` is
/// set, append the outgoing/incoming JSON block. Engine-agnostic: both
/// flavours expose a `&Store`.
fn render_with_optional_relations(
    entity: &Entity,
    id: &EntityId,
    store: &Store,
    args: &Args,
) -> String {
    let sections_filter = if args.sections.is_empty() {
        None
    } else {
        Some(args.sections.as_slice())
    };
    let mut md = render::render_entity_markdown(entity, sections_filter);
    if args.include_relations {
        let outgoing = store.outgoing(id).to_vec();
        let incoming = store.incoming(id).to_vec();
        let rel_json = render::render_relations_json(id.as_ref(), &outgoing, &incoming);
        md.push_str("\n## Relations (JSON)\n\n```json\n");
        md.push_str(
            &serde_json::to_string_pretty(&rel_json).unwrap_or_else(|_| "{}".to_string()),
        );
        md.push_str("\n```\n");
    }
    md
}
