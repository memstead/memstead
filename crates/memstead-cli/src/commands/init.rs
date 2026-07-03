//! `memstead init` — bootstrap a filesystem mem in the current (or named) folder.
//!
//! filesystem-mem is the single-mem, history-free, filesystem-backed product
//! surface. After `memstead init` the folder contains:
//!
//! - `.memstead/config.json` — workspace shape (mem name, schema pin,
//!   empty deps list). Pinned via [`memstead_base::filesystem::config`].
//! - `.memstead/cache/` — empty placeholder for any engine-managed cache
//!   data the workspace acquires later (e.g. resolved schema bytes).
//! - `.memstead/memstead-io/` — empty placeholder for cross-mem dependency
//!   archives populated by `memstead link`.
//!
//! No `.gitignore` is written — filesystem-mem does not assume a surrounding
//! git repo, and writing one would surprise users who *do* track the
//! workspace under git themselves.
//!
//! Strict mode in non-empty folders: see plan trade-off "Adopt vs.
//! strict for `memstead init`". A non-empty target errors out cleanly so
//! the user explicitly clears or moves files before initialising —
//! never silently ingests unrelated `.md` files.

use std::path::{Path, PathBuf};

use clap::Args;
use memstead_base::filesystem::config::{
    FILESYSTEM_WORKSPACE_FORMAT, config_path, init_filesystem_mem, validate_mem_name,
};
use memstead_schema::SchemaRef;
use serde_json::json;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

/// Recovery hint for the nested-workspace refusal. Every printed
/// alternative must exist and be able to succeed in the binary that
/// prints it: `memstead mem init` is the full (mem-repo) verb; the
/// lean binary has no `mem` subcommand group, so it points outside
/// the existing workspace instead.
#[cfg(feature = "mem-repo")]
const NESTED_WORKSPACE_HINT: &str = "If you meant to add a mem inside the existing \
     workspace, run `memstead mem init` instead; for a separate graph, initialise in a \
     folder outside the existing workspace.";
#[cfg(not(feature = "mem-repo"))]
const NESTED_WORKSPACE_HINT: &str = "Initialise in a folder outside the existing \
     workspace instead.";

/// `memstead init` arguments.
#[derive(Args, Debug)]
pub struct InitArgs {
    /// Target folder. Defaults to the current working directory.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Mem name. Slug-shaped: `^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$`.
    #[arg(long)]
    pub name: String,

    /// Schema pin in exact `<name>@<version>` form (e.g.
    /// `default@1.0.0`). Bare-name pins are rejected. filesystem-mem v1
    /// resolves against the engine's builtin schema set;
    /// registry-resolved schemas land in a follow-up.
    #[arg(long)]
    pub schema: String,
}

pub fn run(ctx: &CliContext, args: InitArgs) -> anyhow::Result<()> {
    let target = args
        .path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let schema_pin: SchemaRef = args.schema.parse().map_err(|e: String| CliError {
        code: "INVALID_INPUT",
        message: format!("invalid --schema {value:?}: {e}", value = args.schema),
        kind: ExitKind::Validation,
        details: None,
    })?;

    // The mem name is path-derived and no longer round-trips through
    // `config.json`, so validate the slug shape here at the boundary.
    validate_mem_name(&args.name).map_err(|e| CliError {
        code: "INVALID_INPUT",
        message: format!("invalid --name: {e}"),
        kind: ExitKind::Validation,
        details: None,
    })?;

    if target.exists() {
        if !target.is_dir() {
            return Err(CliError {
                code: "INVALID_INPUT",
                message: format!("target {} exists but is not a directory", target.display()),
                kind: ExitKind::Validation,
                details: None,
            }
            .into());
        }
        ensure_empty(&target)?;
    } else {
        std::fs::create_dir_all(&target).map_err(|e| CliError {
            code: crate::INTERNAL_CODE,
            message: format!(
                "failed to create target directory {}: {e}",
                target.display()
            ),
            kind: ExitKind::Generic,
            details: None,
        })?;
    }

    // Refuse when an ancestor directory already has a
    // `.memstead/workspace.toml` — never nest a fresh filesystem-mem
    // workspace inside an existing one (the outer's `mem list` would
    // miss the inner, the inner would miss the outer). The walk starts
    // at the target's parent (target itself is what we're initialising)
    // and stops at the filesystem root.
    if let Some(found_at) = find_ancestor_workspace(&target)? {
        return Err(CliError {
            code: crate::WORKSPACE_ALREADY_EXISTS_ABOVE_CODE,
            kind: ExitKind::Validation,
            message: format!(
                "an existing memstead workspace lives above {} at {}; \
                 `memstead init` refuses to nest workspaces. {}",
                target.display(),
                found_at.display(),
                NESTED_WORKSPACE_HINT,
            ),
            details: Some(serde_json::json!({
                "found_at": found_at.display().to_string(),
                "hint": NESTED_WORKSPACE_HINT,
            })),
        }
        .into());
    }

    // Write the seed structure (config + `.memstead/` subdirs + adapter
    // marker + one-folder-mount roster) through the engine's shared
    // initialiser, so the CLI and the in-process embedders (the macOS app's
    // bootstrap) produce a byte-identical filesystem mem from one place.
    init_filesystem_mem(&target, &args.name, &schema_pin).map_err(|e| CliError {
        code: crate::INTERNAL_CODE,
        message: format!("initialise filesystem mem: {e}"),
        kind: ExitKind::Generic,
        details: None,
    })?;

    if ctx.json {
        let payload = json!({
            "workspace_root": target.display().to_string(),
            "config_path": config_path(&target).display().to_string(),
            "name": args.name,
            "schema": schema_pin.as_display(),
            "format": FILESYSTEM_WORKSPACE_FORMAT,
        });
        return print_json(&payload);
    }

    let lines = vec![
        format!("# Initialised filesystem mem `{}`", args.name),
        String::new(),
        format!("- Workspace root: `{}`", target.display()),
        format!("- Config:         `{}`", config_path(&target).display()),
        format!("- Schema pin:     `{}`", schema_pin.as_display()),
        String::new(),
        "Next steps:".to_string(),
        "- Drop `.md` entities into the workspace root.".to_string(),
        "- `memstead link <scope/name>` to add a cross-mem dependency.".to_string(),
        "- `memstead publish` to push the mem to the registry.".to_string(),
    ];
    print_markdown(&lines.join("\n"));
    Ok(())
}

/// Walk parent directories looking for `.memstead/workspace.toml`.
/// Returns the absolute path of the first match, or `None` if no
/// ancestor carries the marker. Stops at the filesystem root. Symlinks are
/// not dereferenced — `ancestors()` operates on the resolved
/// `canonicalize`d path, which traverses symlinks once at the
/// boundary and then stays on the resolved filesystem.
/// Shared with `memstead quickstart`, which enforces the same
/// no-nested-workspaces rule.
pub(crate) fn find_ancestor_workspace(target: &Path) -> anyhow::Result<Option<PathBuf>> {
    let abs = std::fs::canonicalize(target).map_err(|e| CliError {
        code: crate::INTERNAL_CODE,
        kind: ExitKind::Generic,
        message: format!("canonicalize {}: {e}", target.display()),
        details: None,
    })?;
    // Skip `abs` itself — the target is what we're initialising; we
    // only care about ancestors. `ancestors()` yields `abs` first,
    // then each parent.
    for ancestor in abs.ancestors().skip(1) {
        if memstead_base::is_workspace_root(ancestor) {
            return Ok(Some(
                ancestor
                    .join(memstead_base::WORKSPACE_STORE_DIR)
                    .join("workspace.toml"),
            ));
        }
    }
    Ok(None)
}

/// Strict-mode emptiness check. The folder is "empty" when it contains
/// no entries at all — a `.git/` from a parent repo (the user's outer
/// project) is fine because that lives outside `target`. A pre-existing
/// `.memstead/`, any `.md` file, or any other content forces the user to
/// resolve the conflict before init proceeds.
fn ensure_empty(target: &Path) -> anyhow::Result<()> {
    let mut iter = std::fs::read_dir(target).map_err(|e| CliError {
        code: crate::INTERNAL_CODE,
        message: format!("read target {}: {e}", target.display()),
        kind: ExitKind::Generic,
        details: None,
    })?;
    if let Some(entry) = iter.next().transpose().map_err(|e| CliError {
        code: crate::INTERNAL_CODE,
        message: format!("read target {}: {e}", target.display()),
        kind: ExitKind::Generic,
        details: None,
    })? {
        let found = entry.file_name().to_string_lossy().to_string();
        return Err(CliError {
            code: crate::TARGET_NOT_EMPTY_CODE,
            message: format!(
                "target {} is not empty (found `{}`); \
                 memstead init refuses to ingest existing content — clear or move files first, \
                 or pick a fresh folder",
                target.display(),
                found,
            ),
            kind: ExitKind::Validation,
            details: Some(serde_json::json!({
                "path": target.display().to_string(),
                "found": [found],
            })),
        }
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memstead_base::filesystem::config::read_workspace_config;
    use tempfile::TempDir;

    fn run_init(target: &Path, name: &str, schema: &str) -> anyhow::Result<()> {
        let ctx = CliContext {
            json: false,
            quiet: false,
        };
        run(
            &ctx,
            InitArgs {
                path: Some(target.to_path_buf()),
                name: name.to_string(),
                schema: schema.to_string(),
            },
        )
    }

    #[test]
    fn init_creates_config_and_subdirs_in_empty_folder() {
        // Identity is path-derived: the mem lives in a folder named after it.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("demo");
        run_init(&root, "demo", "default@1.0.0").unwrap();

        let cfg = read_workspace_config(&root).unwrap();
        assert_eq!(cfg.name, "demo"); // filled from the basename, not config.json
        assert_eq!(cfg.schema.as_display(), "default@1.0.0");
        assert!(cfg.deps.is_empty());

        // The persisted config carries no `name` (path-derived; the schema
        // validator tombstones a stray one).
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config_path(&root)).unwrap()).unwrap();
        assert!(
            raw.get("name").is_none(),
            "config.json must not persist `name`"
        );

        assert!(root.join(".memstead").join("cache").is_dir());
        assert!(root.join(".memstead").join("memstead-io").is_dir());
        // No .gitignore is written.
        assert!(!root.join(".gitignore").exists());
    }

    #[test]
    fn init_creates_target_when_missing() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nested-fresh");
        run_init(&target, "demo", "default@1.0.0").unwrap();
        assert!(target.join(".memstead").join("config.json").is_file());
    }

    #[test]
    fn init_rejects_non_empty_folder() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("preexisting.md"), b"# pre").unwrap();
        let err = run_init(tmp.path(), "demo", "default@1.0.0").unwrap_err();
        assert!(
            err.to_string().contains("not empty"),
            "expected 'not empty' rejection, got: {err}"
        );
    }

    #[test]
    fn init_rejects_invalid_schema_pin() {
        let tmp = TempDir::new().unwrap();
        // Range syntax is rejected upstream by SchemaRef's FromStr.
        let err = run_init(tmp.path(), "demo", "default@^1.0.0").unwrap_err();
        assert!(
            err.to_string().contains("invalid --schema"),
            "expected schema rejection, got: {err}"
        );
    }

    #[test]
    fn init_rejects_invalid_name() {
        // The name is path-derived and no longer round-trips through
        // `config.json`, so the slug shape is enforced at the CLI boundary:
        // an invalid `--name` is rejected up front rather than on a later read.
        let tmp = TempDir::new().unwrap();
        let err = run_init(tmp.path(), "Demo Bad", "default@1.0.0").unwrap_err();
        assert!(
            err.to_string().contains("invalid --name"),
            "expected --name rejection, got: {err}"
        );
    }

    #[test]
    fn init_rejects_bare_name_schema_pin() {
        let tmp = TempDir::new().unwrap();
        let err = run_init(tmp.path(), "demo", "default").unwrap_err();
        assert!(
            err.to_string().contains("invalid --schema"),
            "expected bare-name pin rejection, got: {err}"
        );
    }

    /// A fresh `memstead init` in a subdirectory of an existing workspace
    /// refuses with the typed `WORKSPACE_ALREADY_EXISTS_ABOVE`
    /// envelope rather than silently nesting a new workspace inside
    /// the existing one.
    #[test]
    fn init_refuses_nested_workspace_under_existing_one() {
        let tmp = TempDir::new().unwrap();
        // Seed an outer workspace at tmp.
        std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
        std::fs::write(
            tmp.path().join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n",
        )
        .unwrap();

        // Attempt a nested init under a sibling subdir.
        let inner = tmp.path().join("inner-mem");
        std::fs::create_dir_all(&inner).unwrap();
        let err = run_init(&inner, "inner", "default@1.0.0").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("nest workspaces") || msg.contains("memstead mem init"),
            "expected nested-workspace refusal hint, got: {msg}"
        );
    }

    /// A fresh init in a clean directory (no ancestor workspace)
    /// still succeeds.
    #[test]
    fn init_succeeds_when_no_ancestor_workspace() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("clean");
        std::fs::create_dir_all(&target).unwrap();
        run_init(&target, "demo", "default@1.0.0").unwrap();
        assert!(target.join(".memstead").join("workspace.toml").is_file());
    }
}
