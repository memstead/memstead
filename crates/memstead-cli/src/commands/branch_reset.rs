//! `memstead branch-reset <mem> <target_sha>` — the engine's history-
//! rewrite primitive exposed as a CLI subcommand.
//!
//! Replay workflows that need to rewind a mem's branch pointer to
//! retry against a different upstream state consume this. Safety
//! contract lives on `Engine::branch_reset`: no pushed commit can be
//! discarded by the reset (the engine's definition of "pushed" is
//! reachability from any `refs/remotes/*` ref). Refusal surfaces as
//! `PUSHED_COMMITS_PROTECTED` with the offending SHAs on stderr /
//! `details.pushed_shas`.

use clap::Args;

use crate::CliError;
use crate::setup::{CliContext, CliEngine};

/// `memstead branch-reset <mem> <target_sha>` arguments.
#[derive(Args, Debug)]
pub struct BranchResetArgs {
    /// Mem whose branch pointer to reset. Must be git-branch-backed.
    pub mem: String,
    /// Target ref or SHA. Accepts anything `git rev-parse` admits —
    /// branch names, abbreviated SHAs, full SHAs, tags.
    pub target_sha: String,
}

pub fn run(ctx: &CliContext, args: BranchResetArgs) -> anyhow::Result<()> {
    let outcome = match ctx.cli_engine()? {
        CliEngine::MemRepo(mut engine) => engine
            .branch_reset(&args.mem, &args.target_sha)
            .map_err(CliError::from_engine_op)?,
        CliEngine::Filesystem(_) => {
            // Folder mounts have no git refs — same refusal the
            // engine surface emits, but the CLI catches it before
            // wiring an irrelevant call.
            return Err(CliError {
                code: "INVALID_INPUT",
                kind: crate::output::ExitKind::Validation,
                message: format!(
                    "mem '{}' is not git-backed — `memstead branch-reset` requires a git-branch mount",
                    args.mem,
                ),
                details: None,
            }
            .into());
        }
    };

    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        let discarded = if outcome.discarded_commits.is_empty() {
            "  (no commits discarded — target equalled the current head)".to_string()
        } else {
            outcome
                .discarded_commits
                .iter()
                .map(|s| format!("  - {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        crate::output::print_markdown(&format!(
            "# Branch reset: `{}`\n\n- Branch ref: `{}`\n- Previous: `{}`\n- New: `{}`\n- Discarded commits:\n{}",
            outcome.mem, outcome.branch_ref, outcome.previous_sha, outcome.new_sha, discarded,
        ));
    }
    Ok(())
}
