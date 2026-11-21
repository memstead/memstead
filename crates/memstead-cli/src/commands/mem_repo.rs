//! `memstead mem-repo ...` — mem-repo-git lifecycle commands.
//!
//! Today this module hosts `init [<path>]` — bootstrap a fresh
//! mem-repo-git workspace at `<path>/mem-repo/.git/` with a working
//! tree on `main`, an initial commit carrying the README template, and
//! (optionally) an outer-repo `.gitignore` append so a surrounding git
//! repo does not see the new `mem-repo/` as a gitlink.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

use crate::CliError;
use crate::outer_gitignore::{OuterRepoOutcome, apply_outer_gitignore};
use crate::output::ExitKind;
use crate::setup::CliContext;

/// Subcommands under `memstead mem-repo`.
#[derive(Subcommand, Debug)]
pub enum MemRepoAction {
    /// Bootstrap a fresh mem-repo-git workspace.
    Init(InitArgs),

    /// Configure (or re-point) a named git remote on the mem-repo, so
    /// `memstead fetch` / `pull` / `push` have somewhere to go. Upsert:
    /// re-running with a new URL re-points the remote.
    #[command(name = "remote-add")]
    RemoteAdd(RemoteAddArgs),
}

/// `memstead mem-repo remote-add <name> <url>` arguments.
#[derive(Args, Debug)]
pub struct RemoteAddArgs {
    /// Remote name (e.g. `origin`).
    pub name: String,
    /// Remote URL (e.g. `git@github.com:you/mem-backup.git` or a local
    /// bare-repo path).
    pub url: String,
}

/// `memstead mem-repo init [<path>]` arguments.
#[derive(Args, Debug)]
pub struct InitArgs {
    /// Workspace directory to bootstrap. Created if missing. Defaults to
    /// the current directory.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Skip outer-repo `.gitignore` auto-append. Useful when the user
    /// intends to track `mem-repo/` as a git submodule, or when the
    /// detection heuristic would pick the wrong outer repo.
    #[arg(long)]
    pub no_gitignore: bool,

    /// Configure this URL as the mem-repo's `origin` remote right after
    /// the bootstrap, so a workspace whose tracked engine state was
    /// cloned without its mem-repo is restored by one more command,
    /// `memstead pull --all`. A relative local path is resolved against
    /// the current directory. On a workspace whose mem-repo already
    /// exists, nothing is re-initialised: the remote is re-pointed, so a
    /// restore can be retried with a corrected URL.
    #[arg(long, value_name = "URL")]
    pub remote: Option<String>,
}

pub fn run(ctx: &CliContext, action: MemRepoAction) -> anyhow::Result<()> {
    match action {
        MemRepoAction::Init(args) => init(ctx, args),
        MemRepoAction::RemoteAdd(args) => remote_add(ctx, args),
    }
}

/// A relative local path given as a remote URL is resolved against the
/// current directory. Git itself would resolve it against the mem-repo
/// gitdir it runs in, so `../backup.git` typed next to the workspace
/// would name a path two levels off. URLs (`scheme://`, `user@host:`)
/// and absolute paths pass through unchanged.
fn absolutize_local_url(url: &str) -> String {
    let looks_remote = url.contains("://") || (url.contains(':') && !url.starts_with('.'));
    let path = std::path::Path::new(url);
    if looks_remote || path.is_absolute() {
        return url.to_string();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path).display().to_string(),
        Err(_) => url.to_string(),
    }
}

fn remote_add(ctx: &CliContext, args: RemoteAddArgs) -> anyhow::Result<()> {
    let outcome = match ctx.cli_engine()? {
        crate::setup::CliEngine::MemRepo(engine) => engine
            .remote_add(&args.name, &absolutize_local_url(&args.url))
            .map_err(CliError::from_engine_op)?,
        crate::setup::CliEngine::Filesystem(_) => {
            return Err(
                CliError::from_engine_op(memstead_base::EngineError::NotGitBacked {
                    mem: None,
                    operation: "memstead mem-repo remote-add".to_string(),
                })
                .into(),
            );
        }
    };
    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        let verb = if outcome.updated {
            "Re-pointed"
        } else {
            "Added"
        };
        crate::output::print_markdown(&format!(
            "# {verb} remote `{}`\n\n- URL: `{}`",
            outcome.remote, outcome.url,
        ));
    }
    Ok(())
}

fn init(ctx: &CliContext, args: InitArgs) -> anyhow::Result<()> {
    // With `--remote` on a workspace whose mem-repo already exists (a
    // restore retried with a corrected URL), the bootstrap is not
    // repeated and nothing is overwritten: the remote is re-pointed.
    if let Some(url) = &args.remote {
        // The same shape rule `remote-add` applies: a value that would
        // parse as a flag is refused before anything is written.
        if url.is_empty() || url.starts_with('-') {
            return Err(CliError {
                code: "INVALID_INPUT",
                kind: ExitKind::Validation,
                message: format!(
                    "remote url must be non-empty and must not start with '-' (got '{url}')"
                ),
                details: None,
            }
            .into());
        }
        let gitdir = args.path.join("mem-repo").join(".git");
        if gitdir.is_dir() {
            let r = memstead_git_branch::ops::transport::remote_add_in_gitdir(
                &gitdir,
                "origin",
                &absolutize_local_url(url),
            )
            .map_err(|e| CliError::from_engine_op(memstead_base::EngineError::Backend(e)))?;
            if ctx.json {
                crate::output::print_json(&serde_json::json!({
                    "mem_repo_dir": args.path.join("mem-repo").display().to_string(),
                    "already_initialised": true,
                    "remote": r,
                }))?;
            } else {
                println!(
                    "mem-repo already exists at {}; remote `{}` {} -> {} \
                     (restore with `memstead pull --all`)",
                    args.path.join("mem-repo").display(),
                    r.remote,
                    if r.updated { "re-pointed" } else { "added" },
                    r.url
                );
            }
            return Ok(());
        }
    }
    let outcome = run_init(&args.path, args.no_gitignore)?;
    let remote = match &args.remote {
        Some(url) => Some(
            memstead_git_branch::ops::transport::remote_add_in_gitdir(
                &outcome.mem_repo_dir.join(".git"),
                "origin",
                &absolutize_local_url(url),
            )
            // The engine's error, as `remote-add` raises it: a remote that
            // cannot be configured is the same refusal on both verbs.
            .map_err(|e| CliError::from_engine_op(memstead_base::EngineError::Backend(e)))?,
        ),
        None => None,
    };

    // `--json` stdout is machine-only: exactly one JSON document, the
    // contract `--help` advertises and steers callers to pipe through
    // `jq`. The primary result becomes a structured envelope; the
    // human progress block is suppressed.
    if ctx.json {
        crate::output::print_json(&serde_json::json!({
            "mem_repo_dir": outcome.mem_repo_dir.display().to_string(),
            "workspace_toml": outcome.workspace_toml.display().to_string(),
            "workspace_shape": crate::setup::WorkspaceShape::MemRepo.label(),
            "remote": remote,
            "workspace_shape_disclosure":
                crate::setup::shape_disclosure(crate::setup::WorkspaceShape::MemRepo).to_json(),
        }))?;
    } else {
        println!(
            "Initialised mem-repo-git at {}",
            outcome.mem_repo_dir.display(),
        );
        println!("  main: README.md (initial commit)");
        println!(
            "  __MEMSTEAD: empty (unified registry ref for workspace schemas + per-mem configs)"
        );
        println!("  config: {}", outcome.workspace_toml.display());
        if let Some(r) = &remote {
            println!(
                "  remote: `{}` -> {} (restore with `memstead pull --all`)",
                r.remote, r.url
            );
        }
        // Symmetric disclosure: whichever verb opened the workspace
        // says which of the two shapes the user now has, what it costs,
        // and the command for the other one. A shape statement attached
        // only to the filesystem branch would read as a warning about
        // that branch rather than as the fork it actually is.
        println!();
        println!(
            "{}",
            crate::setup::shape_disclosure_lines(crate::setup::WorkspaceShape::MemRepo).join("\n"),
        );
    }

    // Outer-repo provenance is human-facing context, not part of the
    // structured result — it always goes to stderr (never stdout) so a
    // `--json` caller's stdout stays exactly one JSON document, and is
    // suppressed under `--quiet`. A human still sees it on the terminal.
    match outcome.gitignore {
        OuterRepoOutcome::Appended { outer_root, rel } => {
            if !ctx.quiet {
                eprintln!(
                    "  outer:    {} — added `{}` to .gitignore \
                     (`.memstead/` is intentionally trackable — don't ignore it)",
                    outer_root.display(),
                    rel,
                );
                eprint_source_layout_hint();
            }
        }
        OuterRepoOutcome::AlreadyIgnored { outer_root, rel } => {
            if !ctx.quiet {
                eprintln!(
                    "  outer:    {} — `{}` already in .gitignore, no change \
                     (`.memstead/` is intentionally trackable — don't ignore it)",
                    outer_root.display(),
                    rel,
                );
                eprint_source_layout_hint();
            }
        }
        OuterRepoOutcome::NoOuter | OuterRepoOutcome::Skipped => {}
    }
    Ok(())
}

/// The source-layout hint, emitted when `mem-repo init` runs inside (or
/// at the root of) a git repository — the moment the layout decision is
/// made, and the layout where the classic multi-repo mistake happens:
/// workspace inside one source repo, bindings pointing at siblings.
/// Out-of-root sources are a supported shape — enumeration, change
/// detection, and anchor resolution all work — but artifact ids render
/// as workspace-relative `../…` chains and the workspace-to-source
/// relative layout must stay fixed. Context, not a warning: nothing is
/// wrong yet, so it rides stderr with the other provenance lines.
fn eprint_source_layout_hint() {
    eprintln!(
        "  layout:   sources you later bind (`projection init`) resolve against this \
         workspace root. A source outside it is supported — enumeration, change \
         detection, and anchor resolution all work — but its artifact ids render as \
         `../…` chains and the workspace-to-source layout must stay fixed. To model \
         several sibling repos, root the workspace at their common parent directory."
    );
}

/// Library-form entry point for `memstead mem-repo init`. Creates
/// `<workspace>/mem-repo/.git/` with a working tree on `main`, makes
/// the initial commit, seeds the unified `__MEMSTEAD` registry ref with an
/// empty tree, and (unless suppressed) appends `mem-repo/` to an
/// enclosing outer-repo's `.gitignore`. Idempotent on a fresh target;
/// refuses to overwrite an existing `mem-repo/` directory.
pub(crate) fn run_init(
    workspace_path: &Path,
    skip_outer_gitignore: bool,
) -> anyhow::Result<InitOutcome> {
    // The laying itself is the library's; a program that links the
    // engine calls the same function with no binary involved.
    let laid = memstead_git_branch::init_mem_repo(workspace_path).map_err(|e| match e {
        memstead_git_branch::MemRepoInitError::Exists(exists) => {
            anyhow::Error::from(CliError::from_workspace_create(
                exists,
                "Delete or move the existing mem-repo before re-running `mem-repo init`.",
            ))
        }
        memstead_git_branch::MemRepoInitError::Failed(msg) => generic_error(msg),
    })?;

    // Outer-repo gitignore append: walk up from the workspace root
    // itself looking for an enclosing `.git/`, append `mem-repo/` to
    // its `.gitignore`. Starting AT the workspace (not its parent)
    // covers the workspace-IS-the-repo-root layout — the case where the
    // append matters most, since a nested `mem-repo/.git` can never be
    // tracked normally. The new mem-repo's own gitdir sits a level
    // below (`<workspace>/mem-repo/.git`), never on the walk path, so
    // it cannot be rediscovered as the outer. Idempotent on re-run.
    let gitignore = if skip_outer_gitignore {
        OuterRepoOutcome::Skipped
    } else {
        apply_outer_gitignore(&laid.workspace_root, &laid.mem_repo_dir)?
    };

    Ok(InitOutcome {
        mem_repo_dir: laid.mem_repo_dir,
        workspace_toml: laid.workspace_toml,
        gitignore,
    })
}

/// Result of a successful `mem-repo init`. Useful in tests for
/// asserting on the produced shape without re-walking the repo.
#[derive(Debug)]
pub(crate) struct InitOutcome {
    pub mem_repo_dir: PathBuf,
    pub workspace_toml: PathBuf,
    pub gitignore: OuterRepoOutcome,
}

fn generic_error(msg: String) -> anyhow::Error {
    CliError {
        code: "MEM_REPO_INIT_FAILED",
        kind: ExitKind::Generic,
        message: msg,
        details: None,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn memstead_mem_repo_init_creates_layout() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("ws");
        let outcome = run_init(&workspace, true).unwrap();

        assert!(outcome.mem_repo_dir.exists(), "mem-repo/ must exist");
        assert!(
            outcome.mem_repo_dir.join(".git").exists(),
            "mem-repo/.git/ must exist"
        );
        assert!(
            outcome.mem_repo_dir.join("README.md").is_file(),
            "mem-repo/README.md must be checked out",
        );
        assert!(
            !outcome.mem_repo_dir.join("schemas").exists(),
            "mem-repo/schemas/ must NOT be materialised (schemas live on __MEMSTEAD)",
        );

        let repo = gix::open(outcome.mem_repo_dir.join(".git")).unwrap();
        let id = repo
            .find_reference("refs/heads/main")
            .unwrap()
            .into_fully_peeled_id()
            .unwrap();
        let tree = repo.find_object(id).unwrap().into_commit().tree().unwrap();
        assert!(
            tree.lookup_entry_by_path("README.md").unwrap().is_some(),
            "main:README.md must be present"
        );
        assert!(
            tree.lookup_entry_by_path("schemas").unwrap().is_none(),
            "main must NOT carry schemas/"
        );
        assert!(
            tree.lookup_entry_by_path("configs").unwrap().is_none(),
            "main must NOT carry configs/"
        );

        assert!(
            repo.find_reference("refs/heads/__MEMSTEAD").is_ok(),
            "refs/heads/__MEMSTEAD must exist after init"
        );
        assert!(
            repo.try_find_reference("refs/heads/__SYSTEM")
                .unwrap()
                .is_none(),
            "refs/heads/__SYSTEM must NOT be written by init"
        );
        assert!(
            repo.try_find_reference("refs/heads/__SCHEMAS")
                .unwrap()
                .is_none(),
            "refs/heads/__SCHEMAS must NOT be written by init"
        );

        // `memstead mem-repo init` must leave the workspace in a state
        // every subsequent command can boot from. Without
        // `.memstead/workspace.toml` the engine's loader bails with
        // `StoreError::NotInitialised` and `memstead status` fails.
        let workspace_toml = workspace
            .canonicalize()
            .unwrap()
            .join(".memstead")
            .join("workspace.toml");
        assert_eq!(outcome.workspace_toml, workspace_toml);
        assert!(
            workspace_toml.is_file(),
            ".memstead/workspace.toml must be materialised by init",
        );
        let body = fs::read_to_string(&workspace_toml).unwrap();
        assert!(
            body.contains("format = \"memstead-git-branch-2\""),
            "workspace.toml must declare the engine format, got:\n{body}",
        );
        assert!(
            body.contains("name = \"file-two-layer\""),
            "workspace.toml must declare the file-two-layer adapter, got:\n{body}",
        );
    }

    #[test]
    fn memstead_mem_repo_init_preserves_existing_workspace_toml() {
        // Operator-authored `.memstead/workspace.toml` survives a re-init
        // under the same workspace path: the init must not clobber
        // hand-edited allowlist / cross-link / mutation policy.
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("ws");
        fs::create_dir_all(workspace.join(".memstead")).unwrap();
        let toml_path = workspace.join(".memstead").join("workspace.toml");
        let authored = "# operator-authored\n\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n\
\n\
[[mem_management.create]]\n\
pattern = \"exec-*\"\n\
schemas = [\"default@1.0.0\"]\n";
        fs::write(&toml_path, authored).unwrap();

        run_init(&workspace, true).unwrap();
        let actual = fs::read_to_string(&toml_path).unwrap();
        assert_eq!(
            actual, authored,
            "init must not overwrite hand-edited workspace.toml"
        );
    }

    #[test]
    fn memstead_mem_repo_init_handles_outer_repo_gitignore() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        fs::create_dir_all(&outer).unwrap();
        gix::init(&outer).unwrap();
        let workspace = outer.join("ws");

        let outcome = run_init(&workspace, false).unwrap();
        match outcome.gitignore {
            OuterRepoOutcome::Appended { ref outer_root, .. } => {
                assert_eq!(
                    outer_root.canonicalize().unwrap(),
                    outer.canonicalize().unwrap()
                );
            }
            other => panic!("expected Appended, got {other:?}"),
        }

        let gitignore = fs::read_to_string(outer.join(".gitignore")).unwrap();
        assert!(
            gitignore.contains("ws/mem-repo/"),
            "expected ws/mem-repo/ in outer .gitignore, got:\n{gitignore}",
        );

        let workspace2 = outer.join("ws2");
        fs::remove_dir_all(workspace.join("mem-repo")).unwrap();
        let outcome2 = run_init(&workspace, false).unwrap();
        match outcome2.gitignore {
            OuterRepoOutcome::AlreadyIgnored { .. } => {}
            _ => panic!("re-init under same workspace must be idempotent"),
        }
        let gitignore2 = fs::read_to_string(outer.join(".gitignore")).unwrap();
        let count = gitignore2.matches("ws/mem-repo/").count();
        assert_eq!(
            count, 1,
            "outer .gitignore must carry exactly one `ws/mem-repo/` line, got {count}\n{gitignore2}",
        );
        let _ = workspace2;
    }

    /// Workspace AT the git-repo root — the layout the source-layout
    /// recipe recommends — appends `mem-repo/` to that repo's own
    /// `.gitignore`. The old parent-first walk skipped exactly this
    /// case, the one where the append matters most (a nested
    /// `mem-repo/.git` can never be tracked normally).
    #[test]
    fn memstead_mem_repo_init_workspace_at_repo_root_appends() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("repo");
        fs::create_dir_all(&workspace).unwrap();
        gix::init(&workspace).unwrap();

        let outcome = run_init(&workspace, false).unwrap();
        match outcome.gitignore {
            OuterRepoOutcome::Appended {
                ref outer_root,
                ref rel,
            } => {
                assert_eq!(
                    outer_root.canonicalize().unwrap(),
                    workspace.canonicalize().unwrap(),
                    "the workspace itself is the outer repo"
                );
                assert_eq!(rel, "mem-repo/");
            }
            other => panic!("expected Appended, got {other:?}"),
        }
        let gitignore = fs::read_to_string(workspace.join(".gitignore")).unwrap();
        assert!(
            gitignore.contains("mem-repo/"),
            "expected mem-repo/ in the repo's own .gitignore, got:\n{gitignore}",
        );
    }

    /// Complement: a workspace under no git repo at all appends nowhere
    /// and reports `NoOuter` — no `.gitignore` is invented.
    #[test]
    fn memstead_mem_repo_init_without_any_outer_repo_appends_nowhere() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("free").join("ws");

        let outcome = run_init(&workspace, false).unwrap();
        assert!(
            matches!(outcome.gitignore, OuterRepoOutcome::NoOuter),
            "expected NoOuter, got {:?}",
            outcome.gitignore
        );
        assert!(
            !tmp.path().join(".gitignore").exists()
                && !workspace.join(".gitignore").exists()
                && !tmp.path().join("free").join(".gitignore").exists(),
            "no .gitignore may be invented anywhere on the walk path"
        );
    }

    #[test]
    fn memstead_mem_repo_init_no_gitignore_flag() {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        fs::create_dir_all(&outer).unwrap();
        gix::init(&outer).unwrap();
        let workspace = outer.join("ws");

        run_init(&workspace, true).unwrap();
        let gitignore_path = outer.join(".gitignore");
        if gitignore_path.exists() {
            let body = fs::read_to_string(&gitignore_path).unwrap();
            assert!(
                !body.contains("mem-repo"),
                "with --no-gitignore the outer repo's .gitignore must be untouched, got:\n{body}",
            );
        }
    }

    #[test]
    fn memstead_mem_repo_init_existing_fails() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("ws");
        run_init(&workspace, true).unwrap();
        let err = run_init(&workspace, true).unwrap_err();
        let cli_err = err.downcast_ref::<CliError>().expect("CliError expected");
        assert_eq!(cli_err.kind, ExitKind::Validation);
        // The typed code is a first-class field on `CliError` rather than a
        // `details.code` breadcrumb.
        assert_eq!(cli_err.code, "MEM_DB_ALREADY_EXISTS");
    }
}
