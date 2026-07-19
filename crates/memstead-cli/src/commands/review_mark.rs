//! `memstead review-mark` — read and move the per-mem review mark.
//!
//! The mark is the engine's one pointer per mem to the last
//! human-approved state (mem-repo state: every consumer of the mem
//! sees the same mark). `list` answers "what has un-reviewed
//! changes"; `set` moves a mem's mark to an explicitly named state
//! (never implicitly "now" — writers may have advanced the mem
//! mid-review); `clear` returns a mem to the ordinary markless
//! state; `diff` reports the accumulated per-entity delta from the
//! mark to the current head, in the same change-envelope shape
//! `memstead changes` uses. Marks never gate writes — ignoring them
//! entirely is a first-class state.

use clap::{Args as ClapArgs, Parser, Subcommand};

use crate::CliError;
use crate::output::{print_json, print_markdown};
use crate::setup::CliContext;

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub action: ReviewMarkAction,
}

#[derive(Subcommand, Debug)]
pub enum ReviewMarkAction {
    /// Every mem's mark (or its absence) alongside the current head.
    /// A mem with no mark is an ordinary state, not a warning.
    List,
    /// Set a mem's mark to an explicitly named state — the state the
    /// review actually covered. The value is a backend cursor: a
    /// commit SHA for git-branch mems, an RFC 3339 timestamp for
    /// folder mems (the same cursor `memstead changes --since`
    /// consumes; `list` shows each mem's current head in that
    /// vocabulary). An invalid cursor refuses with `INVALID_CURSOR`
    /// and leaves the mark untouched.
    Set(SetArgs),
    /// Clear a mem's mark, returning it to the markless state.
    Clear(ClearArgs),
    /// The accumulated per-entity delta from the mem's mark to its
    /// current head. Refuses with `REVIEW_MARK_NOT_SET` on a markless
    /// mem — marklessness is visible in `list`, never silently
    /// equated with "no changes".
    Diff(DiffArgs),
}

#[derive(ClapArgs, Debug)]
pub struct SetArgs {
    /// Writable mem name.
    pub mem: String,
    /// The reviewed state (backend cursor — see `set --help`).
    pub state: String,
    /// Provenance note (≤280 chars). Under `[mutations].require_notes`
    /// a missing note adds a `NOTE_MISSING` warning; the write still
    /// commits (warn-and-commit, like every note-gated mutation).
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(ClapArgs, Debug)]
pub struct ClearArgs {
    /// Writable mem name.
    pub mem: String,
    /// Provenance note (≤280 chars).
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(ClapArgs, Debug)]
pub struct DiffArgs {
    /// Mem name.
    pub mem: String,
    /// Rename detection threshold in [0.1, 1.0]; mirrors
    /// `memstead changes --rename-similarity`. Git-branch mems only —
    /// folder mems have no rename detection (renames surface as
    /// updates).
    #[arg(long)]
    pub rename_similarity: Option<f32>,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    let mut engine = ctx.cli_engine()?.into_base();
    match args.action {
        ReviewMarkAction::List => {
            let marks = engine.review_marks();
            if ctx.json {
                print_json(&serde_json::json!({ "marks": marks }))?;
                return Ok(());
            }
            let mut lines = vec!["# Review marks".to_string(), String::new()];
            if marks.is_empty() {
                lines.push("_no mems loaded_".to_string());
            }
            for s in &marks {
                let head = s.head.as_deref().unwrap_or("(no head)");
                let line = match s.mark.as_deref() {
                    None => format!("- `{}` — no mark (head `{head}`)", s.mem),
                    Some(mark) if Some(mark) == s.head.as_deref() => {
                        format!("- `{}` — mark `{mark}` at head (nothing unreviewed)", s.mem)
                    }
                    Some(mark) => format!(
                        "- `{}` — mark `{mark}`, head `{head}` (unreviewed changes — `memstead review-mark diff {}`)",
                        s.mem, s.mem
                    ),
                };
                lines.push(line);
            }
            print_markdown(&lines.join("\n"));
            Ok(())
        }
        ReviewMarkAction::Set(a) => {
            let outcome = engine
                .set_review_mark(&a.mem, Some(&a.state), a.note.as_deref())
                .map_err(CliError::from_engine_op)?;
            report_outcome(ctx, &outcome, "set")
        }
        ReviewMarkAction::Clear(a) => {
            let outcome = engine
                .set_review_mark(&a.mem, None, a.note.as_deref())
                .map_err(CliError::from_engine_op)?;
            report_outcome(ctx, &outcome, "cleared")
        }
        ReviewMarkAction::Diff(a) => {
            let report = engine
                .review_mark_diff(&a.mem, a.rename_similarity)
                .map_err(CliError::from_engine_op)?;
            if ctx.json {
                print_json(&report)?;
                return Ok(());
            }
            let mut lines = vec![
                format!(
                    "# Unreviewed changes in `{}` since mark `{}`",
                    report.mem, report.since
                ),
                String::new(),
                format!("- HEAD: `{}`", report.head),
                format!("- Changes: {}", report.changes.len()),
                String::new(),
            ];
            if report.changes.is_empty() {
                lines.push("_head is at the mark — nothing unreviewed_".to_string());
            } else {
                for change in &report.changes {
                    use memstead_base::ChangeEnvelope::*;
                    let type_suffix = |t: &Option<String>| {
                        t.as_ref().map(|s| format!(" [{s}]")).unwrap_or_default()
                    };
                    let title_suffix = |t: &Option<String>| {
                        t.as_ref().map(|s| format!(" — {s}")).unwrap_or_default()
                    };
                    lines.push(match change {
                        Added {
                            id,
                            title,
                            entity_type,
                        } => format!(
                            "- **added** `{id}`{}{}",
                            type_suffix(entity_type),
                            title_suffix(title)
                        ),
                        Updated {
                            id,
                            title,
                            entity_type,
                        } => format!(
                            "- **updated** `{id}`{}{}",
                            type_suffix(entity_type),
                            title_suffix(title)
                        ),
                        Removed {
                            id,
                            title,
                            entity_type,
                        } => format!(
                            "- **removed** `{id}`{}{}",
                            type_suffix(entity_type),
                            title_suffix(title)
                        ),
                        Renamed {
                            from_id,
                            to_id,
                            title,
                            entity_type,
                        } => format!(
                            "- **renamed** `{from_id}` → `{to_id}`{}{}",
                            type_suffix(entity_type),
                            title_suffix(title)
                        ),
                    });
                }
            }
            print_markdown(&lines.join("\n"));
            Ok(())
        }
    }
}

fn report_outcome(
    ctx: &CliContext,
    outcome: &memstead_base::SetReviewMarkOutcome,
    verb: &str,
) -> anyhow::Result<()> {
    if ctx.json {
        print_json(outcome)?;
        return Ok(());
    }
    let mut lines = vec![match outcome.mark.as_deref() {
        Some(mark) => format!("Review mark {verb} on `{}`: `{mark}`", outcome.mem),
        None => format!("Review mark {verb} on `{}`", outcome.mem),
    }];
    if let Some(prev) = outcome.previous.as_deref() {
        lines.push(format!("- previous: `{prev}`"));
    }
    for w in &outcome.warnings {
        lines.push(format!("- warning: {w}"));
    }
    print_markdown(&lines.join("\n"));
    Ok(())
}
