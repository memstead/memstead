//! `memstead conflicts` — the sanctioned door for git merge conflicts
//! in folder mems (backlog-sweep plan 07, decision 20).
//!
//! A hand-committed folder mem lives in the user's own git repository,
//! so an ordinary merge can write conflict markers into entity files —
//! at which point the file refuses to load and every other repair
//! route (git verbs, raw edits) is correctly blocked by the guards.
//! `conflicts list` shows what is conflicted; `conflicts resolve`
//! picks a side per entity through the engine — validated before it
//! lands, committed as an attributed, note-carrying mutation.

use clap::{Parser, Subcommand};

use memstead_base::EntityId;
use memstead_base::engine::conflicts::ConflictSide;
use memstead_base::vcs::Actor;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, CliEngine};

/// List and resolve git merge conflicts in folder-backed mems.
#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// List every entity file carrying git merge-conflict markers.
    /// Scope with `--mem`; unscoped sweeps every writable folder mem.
    /// Naming a non-folder mem refuses `CONFLICT_RESOLVE_UNSUPPORTED_BACKEND`
    /// — only folder mems live in a user git repo where merges can
    /// conflict entity files.
    List {
        /// Restrict the sweep to one folder mem.
        #[arg(long)]
        mem: Option<String>,
    },
    /// Resolve one conflicted entity to the chosen side. The chosen
    /// side is validated as an entity BEFORE anything is written — an
    /// invalid side refuses with the validation error. Sides are the
    /// standard two; there is no merged-content resolution: to merge,
    /// resolve to the better base side, then edit the entity through
    /// the normal mutation surface (`memstead update`), and say so in
    /// the note (e.g. "base for a manual merge; discarded side: theirs").
    /// A non-conflicted target refuses `NOT_CONFLICTED`.
    Resolve {
        /// Entity id (e.g. `specs--torn-entity`), as listed by
        /// `conflicts list`.
        id: String,
        /// Which side to keep: `ours` or `theirs`.
        #[arg(long, value_name = "ours|theirs")]
        side: String,
        /// Agent-authored provenance note (≤280 chars) — lands in the
        /// resolution's commit and provenance record.
        #[arg(long)]
        note: Option<String>,
    },
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    match ctx.cli_engine()? {
        #[cfg(feature = "mem-repo")]
        CliEngine::MemRepo(engine) => dispatch(ctx, engine, args),
        CliEngine::Filesystem(engine) => dispatch(ctx, engine, args),
    }
}

fn dispatch(ctx: &CliContext, mut engine: memstead_base::Engine, args: Args) -> anyhow::Result<()> {
    match args.command {
        Command::List { mem } => {
            let conflicts = engine
                .list_merge_conflicts(mem.as_deref())
                .map_err(CliError::from_engine_op)?;
            if ctx.json {
                print_json(&serde_json::json!({
                    "count": conflicts.len(),
                    "conflicts": conflicts,
                }))?;
            } else if conflicts.is_empty() {
                print_markdown("# Merge conflicts\n\nNo conflicted entities.");
            } else {
                let mut body = format!("# Merge conflicts ({})\n", conflicts.len());
                for c in &conflicts {
                    body.push_str(&format!("\n- `{}` — {} ({})", c.id, c.file_path, c.mem));
                }
                body.push_str(
                    "\n\nResolve each with: `memstead conflicts resolve <id> --side ours|theirs`",
                );
                print_markdown(&body);
            }
            Ok(())
        }
        Command::Resolve { id, side, note } => {
            let side = ConflictSide::from_wire(&side).ok_or_else(|| {
                CliError::new(
                    ExitKind::Validation,
                    "INVALID_INPUT",
                    format!("unknown side: {side:?} — expected \"ours\" or \"theirs\""),
                )
            })?;
            let id = EntityId::canonical(&id);
            let outcome = engine
                .resolve_merge_conflict(&id, side, Actor::Cli, None, note.as_deref())
                .map_err(CliError::from_engine_op)?;
            if ctx.json {
                print_json(&serde_json::json!({
                    "id": outcome.id.as_ref(),
                    "side": outcome.side,
                    "commit_sha": outcome.commit_sha,
                }))?;
            } else {
                print_markdown(&format!(
                    "# Resolved `{}`\n\n- Kept side: {}\n- The discarded side is gone from the \
                     file; to fold parts of it back in, edit through `memstead update` and note \
                     the manual merge.",
                    outcome.id, outcome.side,
                ));
            }
            Ok(())
        }
    }
}
