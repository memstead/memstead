//! `memstead fetch` / `memstead pull` / `memstead push` CLI subcommands. The
//! three engine surfaces share refusal codes and an outcome shape;
//! the CLI front-end is a thin print of each.

use clap::Args;

use crate::CliError;
use crate::output::ExitKind;
use crate::setup::{CliContext, CliEngine};

/// `memstead fetch <vault> [--remote <name>] [<refspec>...]` arguments.
#[derive(Args, Debug)]
pub struct FetchArgs {
    pub vault: String,
    #[arg(long, default_value = "origin")]
    pub remote: String,
    /// Optional refspecs forwarded to the underlying `git fetch`.
    /// Empty list uses the remote's configured defaults.
    #[arg(num_args = 0..)]
    pub refspecs: Vec<String>,
}

/// `memstead pull <vault> [--remote <name>]` arguments.
#[derive(Args, Debug)]
pub struct PullArgs {
    pub vault: String,
    #[arg(long, default_value = "origin")]
    pub remote: String,
}

/// `memstead push <vault> [--remote <name>] [--force]` arguments.
#[derive(Args, Debug)]
pub struct PushArgs {
    pub vault: String,
    #[arg(long, default_value = "origin")]
    pub remote: String,
    /// Force-push (`--force-with-lease` under the hood). Refused
    /// non-fast-forward pushes only happen here. Use with care — the
    /// remote's view of the branch is overwritten.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

pub fn run_fetch(ctx: &CliContext, args: FetchArgs) -> anyhow::Result<()> {
    let outcome = match ctx.cli_engine()? {
        CliEngine::VaultRepo(engine) => engine
            .fetch(&args.vault, &args.remote, &args.refspecs)
            .map_err(CliError::from_engine_op)?,
        CliEngine::Filesystem(_) => return Err(folder_refusal("memstead fetch", &args.vault)),
    };
    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        let updated = if outcome.updated_refs.is_empty() {
            "  (no refs changed)".to_string()
        } else {
            outcome
                .updated_refs
                .iter()
                .map(|u| {
                    let prev = if u.previous_sha.is_empty() {
                        "<new>".to_string()
                    } else {
                        u.previous_sha.clone()
                    };
                    format!("  - {} : {prev} -> {}", u.ref_name, u.new_sha)
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        crate::output::print_markdown(&format!(
            "# Fetched from `{}`\n\n- Refspecs: {}\n- Updated refs:\n{}",
            outcome.remote,
            if outcome.refspecs.is_empty() {
                "<defaults>".to_string()
            } else {
                outcome.refspecs.join(", ")
            },
            updated,
        ));
    }
    Ok(())
}

pub fn run_pull(ctx: &CliContext, args: PullArgs) -> anyhow::Result<()> {
    let outcome = match ctx.cli_engine()? {
        CliEngine::VaultRepo(mut engine) => engine
            .pull(&args.vault, &args.remote)
            .map_err(CliError::from_engine_op)?,
        CliEngine::Filesystem(_) => return Err(folder_refusal("memstead pull", &args.vault)),
    };
    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        let prev = if outcome.previous_sha.is_empty() {
            "<new branch>".to_string()
        } else {
            outcome.previous_sha.clone()
        };
        crate::output::print_markdown(&format!(
            "# Pulled `{}`\n\n- Branch ref: `{}`\n- Source ref: `{}`\n- Previous: `{prev}`\n- New: `{}`",
            outcome.vault, outcome.branch_ref, outcome.source_ref, outcome.new_sha,
        ));
    }
    Ok(())
}

pub fn run_push(ctx: &CliContext, args: PushArgs) -> anyhow::Result<()> {
    let outcome = match ctx.cli_engine()? {
        CliEngine::VaultRepo(engine) => engine
            .push(&args.vault, &args.remote, args.force)
            .map_err(CliError::from_engine_op)?,
        CliEngine::Filesystem(_) => return Err(folder_refusal("memstead push", &args.vault)),
    };
    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        let force_note = if outcome.forced { " (forced)" } else { "" };
        crate::output::print_markdown(&format!(
            "# Pushed `{}` to `{}`{force_note}\n\n- Branch ref: `{}`\n- New SHA at remote: `{}`",
            outcome.vault, outcome.remote, outcome.branch_ref, outcome.new_sha,
        ));
    }
    Ok(())
}

fn folder_refusal(op: &str, vault: &str) -> anyhow::Error {
    CliError {
        code: "INVALID_INPUT",
        kind: ExitKind::Validation,
        message: format!(
            "vault '{vault}' is not git-backed — `{op}` requires a git-branch mount",
        ),
        details: None,
    }
    .into()
}
