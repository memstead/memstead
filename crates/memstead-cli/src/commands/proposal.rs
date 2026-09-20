//! `memstead proposal`: the review side of a fork. One verb today,
//! `brief`, the read that renders a fork's changes against its source
//! three ways and hands the owner the disposition file to fill.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::CliError;
use crate::output::print_markdown;
use crate::setup::CliContext;

/// Subcommands under `memstead proposal`.
#[derive(Subcommand, Debug)]
pub enum ProposalAction {
    /// Render the review brief of a fork against the mem it was forked
    /// from: every entity the fork added, modified, deleted or renamed
    /// against its base, each with the sections that differ, the
    /// target's referrers, its anchor rows, a mechanics precheck
    /// through the target's write gate, a conflict mark where the
    /// target moved the same entity since the ancestor (with the three
    /// versions), and a re-proposal mark where the target's proposal
    /// record already rejected the same content or id. A read: it
    /// writes nothing.
    Brief(BriefArgs),
}

/// `memstead proposal brief <FORK> [--json] [--out <FILE>]` arguments.
#[derive(Args, Debug)]
pub struct BriefArgs {
    /// The fork mem: a mounted git-branch mem whose config records
    /// `forkedFrom` (`memstead mem fork` writes it). The fork's
    /// changes are read against its base, the fork commit
    /// (`forkedFrom.base`), or the ancestor (`forkedFrom.sha`) for a
    /// fork made before fork commits existed; the target side is the
    /// source mem's branch tip at render time, and all four shas are
    /// printed so the merge can pin them. A mem without `forkedFrom`
    /// refuses `INVALID_INPUT`; a fork whose source mem is not mounted
    /// refuses `UNKNOWN_MEM`.
    pub fork: String,

    /// Write the JSON form of the brief to this file: the fillable
    /// disposition file. It carries everything the markdown shows plus
    /// `dispositions`, keyed by entity slug, each with an empty
    /// `disposition` and `reason` and the values it accepts. The
    /// vocabulary is closed: `adopt`, `adopt_with_changes`, `reject`;
    /// `reject` and `adopt_with_changes` need a `reason`;
    /// `adopt_with_changes` carries the owner's final body as `body`
    /// in the shape a create takes (`title`, `sections`, `metadata`);
    /// a conflict entity accepts no `adopt`. Stdout still carries the
    /// brief (markdown, or the same JSON under `--json`).
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
}

/// Dispatch `memstead proposal <action>`.
pub fn run(ctx: &CliContext, action: ProposalAction) -> anyhow::Result<()> {
    match action {
        ProposalAction::Brief(args) => run_brief(ctx, args),
    }
}

fn run_brief(ctx: &CliContext, args: BriefArgs) -> anyhow::Result<()> {
    let mut engine = ctx.cli_engine()?;
    let brief = engine
        .base_mut()
        .proposal_brief(&args.fork)
        .map_err(CliError::from_engine_op)?;
    if let Some(path) = &args.out {
        let mut json = serde_json::to_string_pretty(&brief)?;
        json.push('\n');
        std::fs::write(path, json).map_err(|e| {
            CliError::new(
                crate::output::ExitKind::Generic,
                "INTERNAL_IO_ERROR",
                format!("write the brief to {}: {e}", path.display()),
            )
        })?;
    }
    if ctx.json {
        crate::output::print_json(&brief)?;
    } else {
        print_markdown(&memstead_base::ops::render_proposal_brief(&brief));
    }
    Ok(())
}
