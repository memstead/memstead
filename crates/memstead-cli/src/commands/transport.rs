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
    /// The mem whose remote to fetch. The mem's check records travel
    /// with it: the remote's check-ledger ref is fetched too and the
    /// mem's rows are added to this workspace's ledger (rows already
    /// present are skipped; nothing is rewritten). Only a mounted mem's
    /// own rows are ever imported.
    pub mem: String,
    #[arg(long, default_value = "origin")]
    pub remote: String,
    /// Optional refspecs forwarded to the underlying `git fetch`.
    /// Empty list uses the remote's configured defaults.
    #[arg(num_args = 0..)]
    pub refspecs: Vec<String>,
}

/// `memstead pull <mem> [--remote <name>]` and
/// `memstead pull --all [--remote <name>]` arguments.
#[derive(Args, Debug)]
pub struct PullArgs {
    /// The mem whose branch to fast-forward. Its check records come
    /// with it: the remote's check-ledger ref is fetched and the mem's
    /// rows are added to this workspace's ledger, so an export made
    /// here seals the checks recorded elsewhere. Omitted with `--all`.
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    pub mem: Option<String>,
    #[arg(long, default_value = "origin")]
    pub remote: String,
    /// Bring the whole mem-repo to the remote's state, the inverse of
    /// `push --all`: the workspace's schema-and-config ref first, then
    /// every mounted git-branch mem's branch (validated against the
    /// schema it resolves to after the first step), then the mounted
    /// mems' check records. Fast-forward only; a ref missing locally is
    /// created. A mem quarantined only because its schema pin lives on
    /// the remote serves again afterwards. A ref with local commits the
    /// remote lacks is refused by name (`LOCAL_DIVERGENCE`) while the
    /// other refs still move, and the run exits non-zero at the end.
    #[arg(long, default_value_t = false)]
    pub all: bool,
}

/// `memstead push <mem> [--remote <name>] [--force]` and
/// `memstead push --all [--remote <name>]` arguments.
#[derive(Args, Debug)]
pub struct PushArgs {
    /// Mem whose branch to push. Omitted with `--all`. The mem's check
    /// records go with the branch: its rows of this workspace's ledger
    /// are published on the mem-repo's check-ledger ref, unioned with
    /// what the remote already holds, and the ref is pushed beside the
    /// branch.
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
    /// schema-and-config ref and its check-ledger ref, fast-forward only. Refs already at the
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
            "# Fetched from `{}`\n\n- Refspecs: {}\n- Updated refs:\n{}\n- Check records imported: {}",
            outcome.remote,
            if outcome.refspecs.is_empty() {
                "<defaults>".to_string()
            } else {
                outcome.refspecs.join(", ")
            },
            updated,
            outcome.checks_imported,
        ));
    }
    Ok(())
}

pub fn run_pull(ctx: &CliContext, args: PullArgs) -> anyhow::Result<()> {
    if args.all {
        return run_pull_all(ctx, &args.remote);
    }
    // clap guarantees `mem` when `--all` is absent.
    let mem = args.mem.as_deref().unwrap_or_default();
    let outcome = match ctx.cli_engine()? {
        CliEngine::MemRepo(mut engine) => engine
            .pull(mem, &args.remote)
            .map_err(CliError::from_engine_op)?,
        CliEngine::Filesystem(_) => return Err(folder_refusal("memstead pull", mem)),
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
            "# Pulled `{}`\n\n- Branch ref: `{}`\n- Source ref: `{}`\n- Previous: `{prev}`\n- New: `{}`\n- Check records imported: {}",
            outcome.mem,
            outcome.branch_ref,
            outcome.source_ref,
            outcome.new_sha,
            outcome.checks_imported,
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
        let checks = match &outcome.checks_published {
            Some(sha) => format!("published at `{sha}`"),
            None => "already on the remote".to_string(),
        };
        crate::output::print_markdown(&format!(
            "# Pushed `{}` to `{}`{force_note}\n\n- Branch ref: `{}`\n- New SHA at remote: `{}`\n- Check records: {checks}",
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

/// `memstead pull --all`: one line per local ref moved, then the mems
/// that returned to service, the mems still quarantined (with their
/// reason) and the notices. `--json` prints the whole outcome. Any
/// refused ref turns the exit into a typed refusal carrying the first
/// refusal's code, with the whole outcome under `details`.
fn run_pull_all(ctx: &CliContext, remote: &str) -> anyhow::Result<()> {
    let outcome = match ctx.cli_engine()? {
        CliEngine::MemRepo(mut engine) => {
            engine.pull_all(remote).map_err(CliError::from_engine_op)?
        }
        CliEngine::Filesystem(_) => {
            return Err(CliError {
                code: "INVALID_INPUT",
                kind: ExitKind::Validation,
                message: "this workspace has no git-branch mems — `memstead pull --all` \
                          requires a mem-repo workspace"
                    .to_string(),
                details: None,
            }
            .into());
        }
    };
    if ctx.json {
        if outcome.refused.is_empty() {
            crate::output::print_json(&outcome)?;
        }
    } else {
        for p in &outcome.pulled {
            let prev = if p.previous_sha.is_empty() {
                "<new>".to_string()
            } else {
                p.previous_sha.clone()
            };
            println!("{} {prev} -> {}", p.ref_name, p.new_sha);
        }
        for m in &outcome.returned_to_service {
            println!("mem `{m}` serves again: its schema arrived with the pull");
        }
        for q in &outcome.quarantined {
            println!(
                "mem `{}` is still quarantined [{}]: {}",
                q.mem, q.code, q.message
            );
        }
        for r in &outcome.local_ahead {
            println!(
                "{r} has local commits the remote lacks: left as it is \
                 (`memstead push --all` publishes them)"
            );
        }
        for u in &outcome.unmounted_on_remote {
            let schema = u.schema.as_deref().unwrap_or("<its pin>");
            if u.branch.contains('/') {
                // A namespaced branch is not itself a mem name; the
                // mount names the mem, so no command is guessed here.
                println!(
                    "branch `{}` on the remote holds a mem (schema {schema}) that is not \
                     mounted here: mount it under its mem name, then run `memstead pull --all` \
                     again",
                    u.branch
                );
            } else {
                println!(
                    "mem `{}` is on the remote but not mounted here: mount it with \
                     `memstead mem init {} --schema {schema} --reattach`, then run \
                     `memstead pull --all` again",
                    u.branch, u.branch
                );
            }
        }
        for n in &outcome.notices {
            println!("notice: {n}");
        }
    }
    // The exit code names a refused ref first: a mem-repo that could not
    // be fetched at all is reported too, but a ref-level refusal in the
    // same run is the more specific answer.
    let primary = outcome
        .refused
        .iter()
        .find(|r| r.ref_name.starts_with("refs/"))
        .or_else(|| outcome.refused.first());
    if let Some(first) = primary {
        let code: &'static str = match first.code.as_str() {
            "LOCAL_DIVERGENCE" => "LOCAL_DIVERGENCE",
            "SCHEMA_VIOLATION_IN_FETCH" => "SCHEMA_VIOLATION_IN_FETCH",
            "SCHEMA_NOT_FOUND" => "SCHEMA_NOT_FOUND",
            "UNKNOWN_REF" => "UNKNOWN_REF",
            "UNKNOWN_REMOTE" => "UNKNOWN_REMOTE",
            "MEM_ERROR" => "MEM_ERROR",
            _ => "INTERNAL",
        };
        let divergence_hint = if outcome.refused.iter().any(|r| r.code == "LOCAL_DIVERGENCE") {
            " A LOCAL_DIVERGENCE ref has local commits the remote lacks and the remote has \
             commits it lacks: push from the other clone first, or reconcile with \
             `memstead branch-reset` after inspecting both sides."
        } else {
            ""
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
                "memstead pull --all: {} refused, {} pulled, {} already in sync — refused: \
                 {listed}.{divergence_hint}",
                outcome.refused.len(),
                outcome.pulled.len(),
                outcome.in_sync.len(),
            ),
            details: Some(serde_json::to_value(&outcome)?),
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
