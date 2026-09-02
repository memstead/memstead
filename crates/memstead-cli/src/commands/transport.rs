//! `memstead fetch` / `memstead pull` / `memstead push` CLI subcommands. The
//! three engine surfaces share refusal codes and an outcome shape;
//! the CLI front-end is a thin print of each.

use clap::Args;

use crate::CliError;
use crate::output::ExitKind;
use crate::setup::{CliContext, CliEngine};

/// `memstead fetch <mem> [--remote <name>] [<refspec>...]` arguments.
#[derive(Args, Debug)]
pub struct FetchArgs {
    pub mem: String,
    #[arg(long, default_value = "origin")]
    pub remote: String,
    /// Optional refspecs forwarded to the underlying `git fetch`.
    /// Empty list uses the remote's configured defaults.
    #[arg(num_args = 0..)]
    pub refspecs: Vec<String>,
}

/// `memstead pull <mem> [--remote <name>]` arguments.
#[derive(Args, Debug)]
pub struct PullArgs {
    pub mem: String,
    #[arg(long, default_value = "origin")]
    pub remote: String,
}

/// `memstead push <mem> [--remote <name>] [--force]` and
/// `memstead push --all [--remote <name>]` arguments.
#[derive(Args, Debug)]
pub struct PushArgs {
    /// Mem whose branch to push. Omitted with `--all`.
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    pub mem: Option<String>,
    #[arg(long, default_value = "origin")]
    pub remote: String,
    /// Force-push (`--force-with-lease` under the hood). Refused
    /// non-fast-forward pushes only happen here. Use with care — the
    /// remote's view of the branch is overwritten. Single-mem only:
    /// `--all` is fast-forward only and does not take it.
    #[arg(long, default_value_t = false, conflicts_with = "all")]
    pub force: bool,
    /// Push every mounted git-branch mem's branch plus the workspace's
    /// schema-and-config ref, fast-forward only. Refs already at the
    /// remote's SHA are skipped silently; one line per ref moved; a
    /// ref that cannot fast-forward is refused by name
    /// (`NON_FAST_FORWARD`) while the other refs still go, and the
    /// run exits non-zero at the end. Folder and archive mounts have
    /// no branch and are skipped.
    #[arg(long, default_value_t = false)]
    pub all: bool,
}

pub fn run_fetch(ctx: &CliContext, args: FetchArgs) -> anyhow::Result<()> {
    let outcome = match ctx.cli_engine()? {
        CliEngine::MemRepo(engine) => engine
            .fetch(&args.mem, &args.remote, &args.refspecs)
            .map_err(CliError::from_engine_op)?,
        CliEngine::Filesystem(_) => return Err(folder_refusal("memstead fetch", &args.mem)),
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
        CliEngine::MemRepo(mut engine) => engine
            .pull(&args.mem, &args.remote)
            .map_err(CliError::from_engine_op)?,
        CliEngine::Filesystem(_) => return Err(folder_refusal("memstead pull", &args.mem)),
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
            outcome.mem, outcome.branch_ref, outcome.source_ref, outcome.new_sha,
        ));
    }
    Ok(())
}

pub fn run_push(ctx: &CliContext, args: PushArgs) -> anyhow::Result<()> {
    if args.all {
        return run_push_all(ctx, &args.remote);
    }
    // clap guarantees `mem` when `--all` is absent.
    let mem = args.mem.as_deref().unwrap_or_default();
    let outcome = match ctx.cli_engine()? {
        CliEngine::MemRepo(engine) => engine
            .push(mem, &args.remote, args.force)
            .map_err(CliError::from_engine_op)?,
        CliEngine::Filesystem(_) => return Err(folder_refusal("memstead push", mem)),
    };
    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        let force_note = if outcome.forced { " (forced)" } else { "" };
        crate::output::print_markdown(&format!(
            "# Pushed `{}` to `{}`{force_note}\n\n- Branch ref: `{}`\n- New SHA at remote: `{}`",
            outcome.mem, outcome.remote, outcome.branch_ref, outcome.new_sha,
        ));
    }
    Ok(())
}

/// `memstead push --all`: the human surface prints exactly one line
/// per ref that moved and nothing else, so a run with nothing to
/// push is silent and a hook can echo the output verbatim. `--json`
/// prints the whole outcome. Any refused ref turns the exit into a
/// typed refusal carrying the first refusal's code, with every
/// refused and pushed ref under `details`.
fn run_push_all(ctx: &CliContext, remote: &str) -> anyhow::Result<()> {
    let outcome = match ctx.cli_engine()? {
        CliEngine::MemRepo(engine) => engine.push_all(remote).map_err(CliError::from_engine_op)?,
        CliEngine::Filesystem(_) => {
            return Err(CliError {
                code: "INVALID_INPUT",
                kind: ExitKind::Validation,
                message: "this workspace has no git-branch mems — `memstead push --all` \
                          requires a mem-repo workspace"
                    .to_string(),
                details: None,
            }
            .into());
        }
    };
    if ctx.json {
        // With a refusal the error envelope below carries the whole
        // outcome under `details`; printing it here too would put two
        // JSON documents on stdout.
        if outcome.refused.is_empty() {
            crate::output::print_json(&outcome)?;
        }
    } else {
        for p in &outcome.pushed {
            let prev = if p.previous_sha.is_empty() {
                "<new>".to_string()
            } else {
                p.previous_sha.clone()
            };
            println!("{} {prev} -> {}", p.ref_name, p.new_sha);
        }
    }
    if let Some(first) = outcome.refused.first() {
        let code: &'static str = match first.code.as_str() {
            "NON_FAST_FORWARD" => "NON_FAST_FORWARD",
            "LOCAL_INVALID_STATE" => "LOCAL_INVALID_STATE",
            "UNKNOWN_REF" => "UNKNOWN_REF",
            "UNKNOWN_REMOTE" => "UNKNOWN_REMOTE",
            _ => "INTERNAL",
        };
        let listed = outcome
            .refused
            .iter()
            .map(|r| {
                format!(
                    "{} ({}{})",
                    r.ref_name,
                    r.code,
                    r.mem
                        .as_deref()
                        .map(|m| format!(", mem `{m}`"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(CliError {
            code,
            kind: ExitKind::Validation,
            message: format!(
                "memstead push --all: {} ref(s) refused, {} pushed, {} already in sync — refused: {listed}. \
                 A NON_FAST_FORWARD ref has commits on the remote this clone lacks: \
                 `memstead fetch <mem>` then `memstead pull <mem>` for that mem, then run `memstead push --all` again.",
                outcome.refused.len(),
                outcome.pushed.len(),
                outcome.in_sync.len(),
            ),
            details: Some(serde_json::json!({
                "remote": outcome.remote,
                "refused": outcome.refused,
                "pushed": outcome.pushed,
                "in_sync": outcome.in_sync,
            })),
        }
        .into());
    }
    Ok(())
}

fn folder_refusal(op: &str, mem: &str) -> anyhow::Error {
    CliError {
        code: "INVALID_INPUT",
        kind: ExitKind::Validation,
        message: format!("mem '{mem}' is not git-backed — `{op}` requires a git-branch mount",),
        details: None,
    }
    .into()
}
