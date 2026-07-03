//! `memstead mem-repo ...` — mem-repo-git lifecycle commands.
//!
//! Today this module hosts `init [<path>]` — bootstrap a fresh
//! mem-repo-git workspace at `<path>/mem-repo/.git/` with a working
//! tree on `main`, an initial commit carrying the README template, and
//! (optionally) an outer-repo `.gitignore` append so a surrounding git
//! repo does not see the new `mem-repo/` as a gitlink.

use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use gix::object::tree::EntryKind;

use crate::CliError;
use crate::output::ExitKind;
use crate::setup::CliContext;
use crate::outer_gitignore::{OuterRepoOutcome, apply_outer_gitignore};

const README_TEMPLATE: &str = include_str!("../../templates/mem-repo-readme.md");

/// Engine-required minimum for `.memstead/workspace.toml`. The two-layer
/// file adapter loader treats `format` + `[persistence_adapter]` as
/// the only mandatory keys; everything else (mem_management,
/// cross_mem_links, mutations, plugin.*) is operator-opt-in and
/// defaults to deny/empty. Matches the same baseline `memstead init`
/// writes for filesystem-mem — keeping the two flavours symmetric.
const DEFAULT_WORKSPACE_TOML: &str = "\
format = \"memstead-git-branch-2\"\n\
\n\
[persistence_adapter]\n\
name = \"file-two-layer\"\n";

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
}

pub fn run(ctx: &CliContext, action: MemRepoAction) -> anyhow::Result<()> {
    match action {
        MemRepoAction::Init(args) => init(ctx, args),
        MemRepoAction::RemoteAdd(args) => remote_add(ctx, args),
    }
}

fn remote_add(ctx: &CliContext, args: RemoteAddArgs) -> anyhow::Result<()> {
    let outcome = match ctx.cli_engine()? {
        crate::setup::CliEngine::MemRepo(engine) => engine
            .remote_add(&args.name, &args.url)
            .map_err(CliError::from_engine_op)?,
        crate::setup::CliEngine::Filesystem(_) => {
            return Err(CliError {
                code: "INVALID_INPUT",
                kind: ExitKind::Validation,
                message: "this workspace is not git-backed — `memstead mem-repo remote-add` \
                          requires a mem-repo workspace"
                    .to_string(),
                details: None,
            }
            .into());
        }
    };
    if ctx.json {
        crate::output::print_json(&outcome)?;
    } else {
        let verb = if outcome.updated { "Re-pointed" } else { "Added" };
        crate::output::print_markdown(&format!(
            "# {verb} remote `{}`\n\n- URL: `{}`",
            outcome.remote, outcome.url,
        ));
    }
    Ok(())
}

fn init(ctx: &CliContext, args: InitArgs) -> anyhow::Result<()> {
    let outcome = run_init(&args.path, args.no_gitignore)?;

    // `--json` stdout is machine-only: exactly one JSON document, the
    // contract `--help` advertises and steers callers to pipe through
    // `jq`. The primary result becomes a structured envelope; the
    // human progress block is suppressed.
    if ctx.json {
        crate::output::print_json(&serde_json::json!({
            "mem_repo_dir": outcome.mem_repo_dir.display().to_string(),
            "workspace_toml": outcome.workspace_toml.display().to_string(),
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
    }

    // Outer-repo provenance is human-facing context, not part of the
    // structured result — it always goes to stderr (never stdout) so a
    // `--json` caller's stdout stays exactly one JSON document, and is
    // suppressed under `--quiet`. A human still sees it on the terminal.
    match outcome.gitignore {
        OuterRepoOutcome::Appended { outer_root, rel } => {
            if !ctx.quiet {
                eprintln!(
                    "  outer:    {} — added `{}` to .gitignore",
                    outer_root.display(),
                    rel,
                );
            }
        }
        OuterRepoOutcome::AlreadyIgnored { outer_root, rel } => {
            if !ctx.quiet {
                eprintln!(
                    "  outer:    {} — `{}` already in .gitignore, no change",
                    outer_root.display(),
                    rel,
                );
            }
        }
        OuterRepoOutcome::NoOuter | OuterRepoOutcome::Skipped => {}
    }
    Ok(())
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
    fs::create_dir_all(workspace_path)
        .map_err(|e| generic_error(format!("create workspace directory: {e}")))?;

    let workspace = fs::canonicalize(workspace_path)
        .map_err(|e| generic_error(format!("canonicalize workspace path: {e}")))?;

    let mem_repo_root = workspace.join("mem-repo");
    if mem_repo_root.exists() {
        return Err(CliError {
            code: "MEM_DB_ALREADY_EXISTS",
            kind: ExitKind::Validation,
            message: format!(
                "{} already exists — refusing to overwrite. Delete or move \
                 the existing mem-repo before re-running `mem-repo init`.",
                mem_repo_root.display()
            ),
            details: None,
        }
        .into());
    }

    fs::create_dir_all(&mem_repo_root)
        .map_err(|e| generic_error(format!("create mem-repo directory: {e}")))?;

    gix::init(&mem_repo_root)
        .map_err(|e| generic_error(format!("init mem-repo gitdir: {e}")))?;

    let repo = gix::open(mem_repo_root.join(".git"))
        .map_err(|e| generic_error(format!("open mem-repo gitdir: {e}")))?;

    // `main` carries operator-facing docs only (README.md). Schemas and
    // per-mem configs live on the unified `__MEMSTEAD` registry ref.
    let mut editor = repo
        .empty_tree()
        .edit()
        .map_err(|e| generic_error(format!("init main tree editor: {e}")))?;

    let readme_blob = repo
        .write_blob(README_TEMPLATE.as_bytes())
        .map_err(|e| generic_error(format!("write README blob: {e}")))?
        .detach();
    editor
        .upsert("README.md", EntryKind::Blob, readme_blob)
        .map_err(|e| generic_error(format!("upsert README.md: {e}")))?;

    let main_tree = editor
        .write()
        .map_err(|e| generic_error(format!("write main tree: {e}")))?
        .detach();

    let actor = init_signature();
    let mut buf = gix::date::parse::TimeBuf::default();
    let actor_ref = actor.to_ref(&mut buf);
    repo.commit_as(
        actor_ref,
        actor_ref,
        "refs/heads/main",
        "mem-repo init: initial main commit",
        main_tree,
        Vec::<gix::ObjectId>::new(),
    )
    .map_err(|e| generic_error(format!("commit main: {e}")))?;

    // Seed the unified `__MEMSTEAD` registry ref with an empty tree. Schemas
    // (`__MEMSTEAD:schemas/<name>/...`) and per-mem configs
    // (`__MEMSTEAD:mems/<mem>/config.json`) are upserted by subsequent
    // engine writes; the empty seed lets the engine's reader resolve
    // the ref without surfacing a bootstrap error.
    let empty_tree = repo.empty_tree().id().detach();
    let mut buf = gix::date::parse::TimeBuf::default();
    let actor_ref = actor.to_ref(&mut buf);
    repo.commit_as(
        actor_ref,
        actor_ref,
        "refs/heads/__MEMSTEAD",
        "mem-repo init: seed __MEMSTEAD",
        empty_tree,
        Vec::<gix::ObjectId>::new(),
    )
    .map_err(|e| generic_error(format!("commit __MEMSTEAD: {e}")))?;

    materialise_main_worktree(&mem_repo_root)?;
    write_default_workspace_toml(&workspace)?;

    // Outer-repo gitignore append: walk up from the workspace's parent
    // (so we don't rediscover the new mem-repo/.git/ as our own outer)
    // looking for an enclosing `.git/`, append `mem-repo/` to its
    // `.gitignore`. Idempotent on re-run.
    let gitignore = if skip_outer_gitignore {
        OuterRepoOutcome::Skipped
    } else {
        let walk_start = workspace
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| workspace.clone());
        apply_outer_gitignore(&walk_start, &mem_repo_root)?
    };

    Ok(InitOutcome {
        mem_repo_dir: mem_repo_root,
        workspace_toml: workspace
            .join(memstead_base::WORKSPACE_STORE_DIR)
            .join("workspace.toml"),
        gitignore,
    })
}

/// Write `README.md` to the working tree. Mirrors the just-committed
/// `main` tree so a human inspecting `<workspace>/mem-repo/` sees
/// the file immediately. Schemas and per-mem configs live on the
/// `__MEMSTEAD` registry ref and are not surfaced via the worktree.
fn materialise_main_worktree(mem_repo_root: &Path) -> anyhow::Result<()> {
    fs::write(mem_repo_root.join("README.md"), README_TEMPLATE)
        .map_err(|e| generic_error(format!("write working-tree README.md: {e}")))?;
    Ok(())
}

/// Materialise the minimum-viable `.memstead/workspace.toml`. Required by
/// every subsequent CLI / MCP command — without the file the
/// workspace-store loader bails with `StoreError::NotInitialised` and
/// the freshly-init'd workspace is unusable. Idempotent: a
/// pre-existing file is left untouched so operator-authored content
/// survives a re-init under the same workspace path.
fn write_default_workspace_toml(workspace_root: &Path) -> anyhow::Result<()> {
    let memstead_dir = workspace_root.join(memstead_base::WORKSPACE_STORE_DIR);
    fs::create_dir_all(&memstead_dir)
        .map_err(|e| generic_error(format!("create .memstead directory: {e}")))?;
    let toml_path = memstead_dir.join("workspace.toml");
    if toml_path.exists() {
        return Ok(());
    }
    fs::write(&toml_path, DEFAULT_WORKSPACE_TOML)
        .map_err(|e| generic_error(format!("write .memstead/workspace.toml: {e}")))?;
    Ok(())
}

/// Result of a successful `mem-repo init`. Useful in tests for
/// asserting on the produced shape without re-walking the repo.
#[derive(Debug)]
pub(crate) struct InitOutcome {
    pub mem_repo_dir: PathBuf,
    pub workspace_toml: PathBuf,
    pub gitignore: OuterRepoOutcome,
}

fn init_signature() -> gix::actor::Signature {
    gix::actor::Signature {
        name: "memstead-cli mem-repo init".into(),
        email: "mem-repo-init@memstead".into(),
        time: gix::date::Time {
            seconds: 0,
            offset: 0,
        },
    }
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
            repo.try_find_reference("refs/heads/__SYSTEM").unwrap().is_none(),
            "refs/heads/__SYSTEM must NOT be written by init"
        );
        assert!(
            repo.try_find_reference("refs/heads/__SCHEMAS").unwrap().is_none(),
            "refs/heads/__SCHEMAS must NOT be written by init"
        );

        // `memstead mem-repo init` must leave the workspace in a state
        // every subsequent command can boot from. Without
        // `.memstead/workspace.toml` the engine's loader bails with
        // `StoreError::NotInitialised` and `memstead stats` fails.
        let workspace_toml = workspace.canonicalize().unwrap().join(".memstead").join("workspace.toml");
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
        assert_eq!(actual, authored, "init must not overwrite hand-edited workspace.toml");
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
