//! `memstead proposal`: the review side of a fork. Three verbs:
//! `brief`, the read that renders a fork's changes against its source
//! three ways and hands the owner the disposition file to fill;
//! `merge`, the owner's act that applies the filled file onto the
//! source under the proposer's and the merger's identities; `list`,
//! the read of the record a merge keeps on the target. The merge and
//! the list are CLI-only (the parity registry carries the rationale).

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
    /// Apply a filled disposition file onto the mem the fork was forked
    /// from: every `adopt` entity lands as the fork has it (created,
    /// updated or deleted, anchors and self-links under the target's
    /// ids) in one commit on the target branch per proposer identity,
    /// parent-pinned to the tip the file recorded, all landing or none;
    /// `adopt_with_changes` lands the fork's version there and your
    /// final body in a second commit under your identity; `reject`
    /// lands nothing. The merge commit carries the proposer's identity
    /// (read from the fork's own commits, never from you) with
    /// `Merged-By:` and `Proposal:` beside it, writes the proposal
    /// record (`.memstead/proposals.json`) on the target branch, records
    /// a verification check per entity it created or updated under your
    /// identity, and validates the whole target store afterwards.
    /// `--identity` is required. Every refusal lands nothing. A human's
    /// act on the owner's branch: CLI-only.
    Merge(MergeArgs),
    /// List the proposals merged into a mem: the record the merge keeps
    /// on the target branch (`.memstead/proposals.json`), one block per
    /// proposal with its id, proposer, merger, shas, time and every
    /// disposition with its reason. Reads a sealed archive's record
    /// too. A read: it writes nothing.
    List(ListArgs),
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

/// `memstead proposal merge <FORK> --dispositions <FILE> --identity <MERGER>` arguments.
#[derive(Args, Debug)]
pub struct MergeArgs {
    /// The fork mem the file was rendered for (`memstead proposal brief
    /// <fork> --out <file>`). The merge re-renders the brief and
    /// refuses `PROPOSAL_STALE` when the target tip (or the fork tip)
    /// moved since the file was rendered, naming both shas: re-render
    /// and fill again.
    pub fork: String,

    /// The filled disposition file: the brief's JSON with every
    /// `dispositions.<slug>.disposition` set to `adopt`,
    /// `adopt_with_changes` or `reject`. A slot missing or one the
    /// brief did not list refuses `PROPOSAL_DISPOSITIONS_INCOMPLETE`;
    /// a value outside the vocabulary, a `reject` or
    /// `adopt_with_changes` without a `reason`, or an
    /// `adopt_with_changes` without a `body` refuses `INVALID_INPUT`;
    /// `adopt` on a conflict entity refuses `PROPOSAL_CONFLICT`; an
    /// adopted entity whose last fork commit carries no identity
    /// refuses `PROPOSAL_UNATTRIBUTED`; a body the target's write gate
    /// refuses lands nothing and returns the gate's own code.
    #[arg(long, value_name = "FILE")]
    pub dispositions: PathBuf,

    /// Agent-authored provenance note (one sentence, at most 280
    /// characters) for the merge commits' note record.
    #[arg(long)]
    pub note: Option<String>,
}

/// `memstead proposal list <TARGET>` arguments.
#[derive(Args, Debug)]
pub struct ListArgs {
    /// The mem proposals were merged into. A mem no proposal was
    /// merged into lists an empty record; an unknown mem refuses
    /// `UNKNOWN_MEM`.
    pub target: String,
}

/// Dispatch `memstead proposal <action>`.
pub fn run(ctx: &CliContext, action: ProposalAction) -> anyhow::Result<()> {
    match action {
        ProposalAction::Brief(args) => run_brief(ctx, args),
        ProposalAction::Merge(args) => run_merge(ctx, args),
        ProposalAction::List(args) => run_list(ctx, args),
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

fn run_merge(ctx: &CliContext, args: MergeArgs) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(&args.dispositions).map_err(|e| {
        CliError::new(
            crate::output::ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "read the disposition file {}: {e}",
                args.dispositions.display()
            ),
        )
    })?;
    let file: memstead_base::ops::ProposalBrief = serde_json::from_str(&text).map_err(|e| {
        CliError::new(
            crate::output::ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "the disposition file {} is not the brief's JSON (`memstead proposal brief \
                 <fork> --out <file>` writes it): {e}",
                args.dispositions.display()
            ),
        )
    })?;
    let mut engine = ctx.cli_engine()?;
    let outcome = engine
        .base_mut()
        .proposal_merge(
            &args.fork,
            &file,
            memstead_base::vcs::Actor::Cli,
            Some(&crate::setup::cli_client_id()),
            args.note.as_deref(),
        )
        .map_err(CliError::from_engine_op)?;
    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        print_markdown(&memstead_base::ops::render_proposal_merge(&outcome));
    }
    Ok(())
}

fn run_list(ctx: &CliContext, args: ListArgs) -> anyhow::Result<()> {
    let mut engine = ctx.cli_engine()?;
    let record = engine
        .base_mut()
        .proposal_list(&args.target)
        .map_err(CliError::from_engine_op)?;
    if ctx.json {
        crate::output::print_json(&record)?;
    } else {
        print_markdown(&memstead_base::ops::render_proposal_record(
            &args.target,
            &record,
        ));
    }
    Ok(())
}
