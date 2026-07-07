//! `memstead ingest` — the engine-side ingest-orchestration surface.
//!
//! `memstead ingest brief <name>` renders the **run-brief** for an ingest —
//! the Markdown prompt an ingest agent consumes — on stdout. All assembly
//! lives in the shared engine entry point
//! [`memstead_base::ingest::render_ingest_brief`], which the macOS app's
//! UniFFI surface calls too, so the CLI and app briefs are byte-identical by
//! construction.
//!
//! Rendered today: **discovery** mode (with the changed-slice preface from
//! live source state). Refinement / one-shot modes and selection/backoff
//! follow.

use clap::{Args as ClapArgs, Subcommand};
use serde_json::json;

use memstead_base::ingest::{RenderBriefError, ResolveError, render_ingest_brief, select_next_due};
use memstead_base::pipeline_store::load_pipeline_configs;

use crate::CliError;
use crate::output::{ExitKind, print_json};
use crate::setup::{CliContext, CliEngine};

#[derive(ClapArgs, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: IngestCommand,
}

#[derive(Subcommand, Debug)]
pub enum IngestCommand {
    /// Render the run-brief for an ingest — the Markdown prompt an agent
    /// consumes — on stdout. Reads the four-primitive config and the
    /// destination mem's schema / writing guidance.
    Brief(BriefArgs),
}

#[derive(ClapArgs, Debug)]
pub struct BriefArgs {
    /// The ingest name (its `.memstead/ingests/<name>.json` file stem).
    /// Omit (or pass `--all`) to select the next due ingest by round-robin +
    /// backoff.
    pub name: Option<String>,
    /// Select the next due ingest across all ingests (round-robin + backoff)
    /// and render its brief, instead of naming one.
    #[arg(long)]
    pub all: bool,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    match args.command {
        IngestCommand::Brief(a) => brief(ctx, a),
    }
}

fn brief(ctx: &CliContext, args: BriefArgs) -> anyhow::Result<()> {
    let (_, root) = ctx.workspace_shape().ok_or_else(|| {
        CliError::new(
            ExitKind::Generic,
            "NO_WORKSPACE",
            "not inside a Memstead workspace (no `.memstead/workspace.toml` in any ancestor)"
                .to_string(),
        )
    })?;

    let engine = match ctx.cli_engine_at(&root)? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(engine) => engine,
        CliEngine::Filesystem(engine) => engine,
    };

    // Resolve which ingest to render: a named one, or the next due ingest in a
    // round-robin `--all` rotation (which advances the cursor + backoff state).
    let selected = match args.name {
        Some(name) if !args.all => Some(name),
        _ => {
            let configs = load_pipeline_configs(&root).map_err(|e| {
                CliError::new(
                    ExitKind::Generic,
                    "PIPELINE_LOAD_FAILED",
                    format!("could not load pipeline config: {e}"),
                )
            })?;
            select_next_due(&engine, &root, &configs)
        }
    };

    let Some(name) = selected else {
        // Every eligible ingest is backing off this pass — a valid outcome.
        if ctx.json {
            print_json(&json!({ "skipped": true }))?;
        } else {
            println!("> **[ingest] Skipped — every eligible ingest is backing off this pass.**");
        }
        return Ok(());
    };

    let brief = render_ingest_brief(&engine, &root, &name).map_err(map_brief_err)?;

    if ctx.json {
        print_json(&json!({ "brief": brief }))?;
    } else {
        // The brief *is* the stdout content (the skill pipes it as the agent
        // prompt) — write it verbatim, no added trailing newline.
        print!("{brief}");
    }
    Ok(())
}

fn map_brief_err(err: RenderBriefError) -> anyhow::Error {
    let (code, message) = match &err {
        RenderBriefError::ConfigLoad(_) => ("PIPELINE_LOAD_FAILED", err.to_string()),
        RenderBriefError::ModeUnsupported { .. } => ("INGEST_MODE_UNSUPPORTED", err.to_string()),
        RenderBriefError::Resolve(inner) => (resolve_err_code(inner), err.to_string()),
    };
    CliError::new(ExitKind::Generic, code, message).into()
}

fn resolve_err_code(err: &ResolveError) -> &'static str {
    match err {
        ResolveError::IngestNotFound { .. } => "INGEST_NOT_FOUND",
        ResolveError::MalformedProjectionRef { .. } => "INGEST_PROJECTION_MALFORMED",
        ResolveError::ProjectionNotFound { .. } => "INGEST_PROJECTION_NOT_FOUND",
        ResolveError::FacetNotFound { .. } => "INGEST_FACET_NOT_FOUND",
        ResolveError::MediumNotFound { .. } => "INGEST_MEDIUM_NOT_FOUND",
    }
}
