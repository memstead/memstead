//! `memstead pipeline migrate` — convert the legacy `scopes|projections|
//! ingests/` JSON folders at the workspace root into the four-primitive
//! workspace-store shape under `.memstead/` (Medium / Facet / Projection /
//! Ingest). The conversion core lives in `memstead_base::pipeline_migrate`;
//! this is the operator-facing surface.

use clap::{Args as ClapArgs, Subcommand};
use serde_json::json;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

#[derive(ClapArgs, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: PipelineCommand,
}

#[derive(Subcommand, Debug)]
pub enum PipelineCommand {
    /// Migrate the legacy `scopes|projections|ingests/` JSON folders at the
    /// workspace root into the four-primitive workspace-store shape under
    /// `.memstead/`. A legacy scope splits into a Medium (territory) and a
    /// Facet (engagement). Idempotent — re-running reproduces identical
    /// files. The legacy folders are left in place; remove them when ready.
    Migrate,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    match args.command {
        PipelineCommand::Migrate => migrate(ctx),
    }
}

fn migrate(ctx: &CliContext) -> anyhow::Result<()> {
    let (_shape, root) = ctx.workspace_shape().ok_or_else(|| {
        CliError::new(
            ExitKind::Generic,
            "NO_WORKSPACE",
            "not inside a Memstead workspace (no `.memstead/workspace.toml` in any ancestor)"
                .to_string(),
        )
    })?;
    let configs = memstead_base::migrate_legacy_pipeline(&root).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "PIPELINE_MIGRATE_FAILED",
            format!("pipeline migration failed: {e}"),
        )
        .with_details(json!({ "error": e.to_string() }))
    })?;
    if ctx.json {
        print_json(&json!({
            "ok": true,
            "mediums": configs.mediums.len(),
            "facets": configs.facets.len(),
            "projections": configs.projections.len(),
            "ingests": configs.ingests.len(),
        }))?;
    } else {
        print_markdown(&format!(
            "# Pipeline migrated\n\nWrote to `.memstead/`: {} medium(s), {} facet(s), \
             {} projection(s), {} ingest(s).\n\nThe legacy `scopes|projections|ingests/` \
             folders were left in place — remove them when ready.\n",
            configs.mediums.len(),
            configs.facets.len(),
            configs.projections.len(),
            configs.ingests.len(),
        ));
    }
    Ok(())
}
