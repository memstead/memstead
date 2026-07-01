//! Read mem configs from `mem-repo-git:__MEMSTEAD:mems/<path>/<leaf>/config.json`.
//!
//! Mem-repo-backed mems have no working directory — the canonical
//! config lives at `mem-repo-git:__MEMSTEAD:mems/<path>/<leaf>/config.json`
//! on the unified `__MEMSTEAD` ref, where `<path>/<leaf>` mirrors the
//! per-mem content branch `refs/heads/<path>/<leaf>` (the branch
//! ref name and the `__MEMSTEAD` tree path under `mems/` are kept
//! byte-identical so enumeration and config IO can share one source
//! of truth). `main` is reserved for operator-facing docs (README,
//! etc.); the engine never reads `main` for mem data.
//!
//! Flat (single-segment) layouts — `refs/heads/<leaf>` ↔
//! `__MEMSTEAD:mems/<leaf>/config.json` — are still supported as the
//! degenerate case where the organizational path is empty.
//!
//! The parser pipeline (`check_config` + `parse_mem_config`) is
//! identical to the disk path; only the byte source differs (blob in
//! the gix object database vs. file on disk).
use std::path::Path;

use memstead_schema::{
    ConfigError, SchemaRef, MemConfig,
};

use crate::vcs::CommitContext;
use crate::MemInit;

/// Errors raised while reading a mem config from `mem-repo-git:__MEMSTEAD`.
#[derive(Debug, thiserror::Error)]
pub enum MemRepoConfigError {
    /// The workspace is not a real mem-repo workspace — `mem-repo/.git/`
    /// is missing. Caller decides whether to treat as fatal (post-cutover
    /// invariant) or fall back to legacy disk shape.
    #[error("mem-repo gitdir not found at {0}")]
    GitdirNotFound(String),
    /// `mem-repo/.git/` exists but cannot be opened (corrupt repo, IO
    /// failure under the object database). The wrapped message names the
    /// underlying gix error.
    #[error("could not open mem-repo gitdir: {0}")]
    GixOpen(String),
    /// `__MEMSTEAD` registry-class branch ref is missing — empty-bare-repo
    /// stub state, or a workspace that was never initialised. Variant
    /// name retained as `NoMainBranch` for backward-compat with existing
    /// matchers; semantically it now means "no `__MEMSTEAD` ref".
    #[error("mem-repo has no `__MEMSTEAD` branch")]
    NoMainBranch,
    /// `__MEMSTEAD:mems/<mem>/config.json` does not exist in the tree.
    /// Either the mem was never registered or the config blob was
    /// deleted.
    #[error("config not found in mem-repo: __MEMSTEAD:mems/{0}/config.json")]
    ConfigNotFound(String),
    /// Generic gix-tree read failure (object missing, corrupt tree, IO
    /// underneath the object database).
    #[error("git tree read error: {0}")]
    GitTree(String),
    /// Blob bytes are not valid UTF-8.
    #[error("config blob is not valid UTF-8: {0}")]
    NotUtf8(String),
    /// Parse / validation failure surfaced from the shared schema
    /// pipeline. Wrapped so callers can branch on the inner kind if
    /// needed.
    #[error("{0}")]
    Schema(#[from] ConfigError),
}

/// Resolve a leaf mem name to its full branch path inside the
/// mem-repo by scanning local branches and matching the leaf against
/// the last `/`-separated segment of every branch shortname.
///
/// Returns `Ok(Some(full_path))` on a unique match (e.g. leaf `engine`
/// → `demo/engine`, or flat `engine` → `engine`), `Ok(None)` when
/// no branch ends in `leaf`, and `Ok(Some(_))` on the FIRST match if
/// multiple branches share a leaf — leaf collision is a mem-create
/// invariant violation that should not occur in practice; the caller
/// surfacing `Some` here lets the read path proceed and the create
/// path's collision check is the line of defense.
///
/// `main` and `__*`-prefix branches are filtered out; only writable
/// per-mem content branches are considered.
///
/// Gitdir-rooted: caller supplies the mount's gitdir directly. The
/// workspace-rooted callers in this module compose
/// `<workspace_root>/mem-repo/.git/` via [`default_gitdir`] and
/// delegate here.
pub fn resolve_full_path_at_gitdir(
    gitdir: &Path,
    leaf: &str,
) -> Result<Option<String>, MemRepoConfigError> {
    if !gitdir.is_dir() {
        return Err(MemRepoConfigError::GitdirNotFound(
            gitdir.display().to_string(),
        ));
    }
    let repo = gix::open(gitdir).map_err(|e| MemRepoConfigError::GixOpen(e.to_string()))?;
    let refs = repo
        .references()
        .map_err(|e| MemRepoConfigError::GitTree(e.to_string()))?;
    let iter = refs
        .local_branches()
        .map_err(|e| MemRepoConfigError::GitTree(e.to_string()))?;
    for r in iter {
        let reference = match r {
            Ok(reference) => reference,
            Err(_) => continue,
        };
        let short = reference.name().shorten();
        let name = match std::str::from_utf8(short) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if name == "main" {
            continue;
        }
        // Filter `__*` only on the leading segment — a real mem under
        // `foo/__weird-but-legal-leaf` would be filtered by the
        // create-path validator before it lands; here we skip
        // registry-class refs whose top-level segment starts with `__`
        // (e.g. `__MEMSTEAD`).
        if name.starts_with("__") {
            continue;
        }
        let last = name.rsplit('/').next().unwrap_or(name);
        if last == leaf {
            return Ok(Some(name.to_string()));
        }
    }
    Ok(None)
}

/// Walk the mem-repo's local branches and return every full path
/// whose final `/`-separated segment equals `leaf`.
///
/// The mem-create orchestrator pre-flights this to surface
/// tree-walk leaf collisions even when discovery's first-wins drop
/// has hidden them from the engine snapshot. Returns `Ok(vec![])` for a
/// mem-repo without any matching branches, `Ok(_)` of length 1 for
/// the typical "leaf already exists at one path" case, and the rare
/// `Ok(_)` of length ≥2 for a corrupt repo that has the same leaf
/// sealed at two distinct organizational paths (e.g. manual git ref
/// surgery, or a mid-create crash that landed both `demo/engine`
/// and `planning/engine`).
///
/// Filters identical to [`resolve_full_path_at_gitdir`]: skips `main`
/// and any branch whose leading segment starts with `__` (registry-
/// class refs like `__MEMSTEAD`).
pub fn find_branches_by_leaf_at_gitdir(
    gitdir: &Path,
    leaf: &str,
) -> Result<Vec<String>, MemRepoConfigError> {
    if !gitdir.is_dir() {
        return Err(MemRepoConfigError::GitdirNotFound(
            gitdir.display().to_string(),
        ));
    }
    let repo = gix::open(gitdir).map_err(|e| MemRepoConfigError::GixOpen(e.to_string()))?;
    let refs = repo
        .references()
        .map_err(|e| MemRepoConfigError::GitTree(e.to_string()))?;
    let iter = refs
        .local_branches()
        .map_err(|e| MemRepoConfigError::GitTree(e.to_string()))?;
    let mut matches: Vec<String> = Vec::new();
    for r in iter {
        let reference = match r {
            Ok(reference) => reference,
            Err(_) => continue,
        };
        let short = reference.name().shorten();
        let name = match std::str::from_utf8(short) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if name == "main" {
            continue;
        }
        if name.starts_with("__") {
            continue;
        }
        let last = name.rsplit('/').next().unwrap_or(name);
        if last == leaf {
            matches.push(name.to_string());
        }
    }
    matches.sort();
    Ok(matches)
}

/// Compute the fully-qualified branch ref name for `mem_name` by
/// resolving its leaf to the matching hierarchical full path on the
/// mem-repo (e.g. `refs/heads/demo/engine` for leaf `engine`,
/// `refs/heads/alpha` for flat `alpha`). Falls back to
/// `refs/heads/<mem_name>` (flat) when the leaf does not yet
/// resolve to any branch — used by the create path before the branch
/// is sealed, and by callers operating on a workspace whose mem-repo
/// is not yet present.
///
/// Errors only on hard gix failures (gitdir missing, ref iteration
/// failure); a clean "no match" returns the flat fallback so reads
/// against a stub mem-repo do not surface a different error code
/// just because the resolver runs first.
///
/// Workspace-rooted convenience wrapper around
/// [`branch_ref_for_mem_at_gitdir`].
pub fn branch_ref_for_mem(workspace_root: &Path, mem_name: &str) -> String {
    branch_ref_for_mem_at_gitdir(&gitdir_for_leaf(workspace_root, mem_name), mem_name)
}

/// Gitdir-rooted variant of [`branch_ref_for_mem`].
pub fn branch_ref_for_mem_at_gitdir(gitdir: &Path, mem_name: &str) -> String {
    match resolve_full_path_at_gitdir(gitdir, mem_name) {
        Ok(Some(full_path)) => format!("refs/heads/{full_path}"),
        _ => format!("refs/heads/{mem_name}"),
    }
}

/// Compose the workspace's gitdir path
/// (`<workspace_root>/mem-repo/.git/`). The single canonical mount
/// for git-branch-backed mems.
fn default_gitdir(workspace_root: &Path) -> std::path::PathBuf {
    workspace_root.join("mem-repo").join(".git")
}

/// Resolve the gitdir to use for a leaf's read/write operations. The
/// post-rebuild architecture has exactly one git-branch mount per
/// workspace, rooted at `<workspace_root>/mem-repo/.git/`.
fn gitdir_for_leaf(workspace_root: &Path, _mem_name: &str) -> std::path::PathBuf {
    default_gitdir(workspace_root)
}

/// Read and validate a mem config from `mem-repo-git:__MEMSTEAD:mems/<full_path>/config.json`.
///
/// `workspace_root` is the directory holding `mem-repo/.git/` (i.e. the
/// directory `memstead` lives in). Returns `Ok(MemConfig)` on a clean
/// read; the typed error variants discriminate the failure modes a
/// caller may want to branch on (missing gitdir vs. missing `__MEMSTEAD`
/// vs. missing config blob vs. parse error).
///
/// Workspace-rooted convenience wrapper around [`read_config_at_gitdir`].
pub fn read_config(
    workspace_root: &Path,
    mem_name: &str,
) -> Result<MemConfig, MemRepoConfigError> {
    read_config_at_gitdir(&gitdir_for_leaf(workspace_root, mem_name), mem_name)
}

/// Gitdir-rooted variant of [`read_config`]. Multi-mount callers pass
/// the mount's gitdir directly so the resolver and the per-mem
/// config lookup target the same mem-repo.
///
/// Reads from the unified `__MEMSTEAD` ref. `MemsteadRefError` is mapped to
/// the legacy `MemRepoConfigError` envelope so callers' branch
/// shapes stay stable.
pub fn read_config_at_gitdir(
    gitdir: &Path,
    mem_name: &str,
) -> Result<MemConfig, MemRepoConfigError> {
    if !gitdir.is_dir() {
        return Err(MemRepoConfigError::GitdirNotFound(
            gitdir.display().to_string(),
        ));
    }
    crate::storage_memstead::read_mem_config_from_memstead_ref(gitdir, mem_name).map_err(|e| {
        match e {
            crate::storage_memstead::MemsteadRefError::GixOpen(msg) => {
                MemRepoConfigError::GixOpen(msg)
            }
            crate::storage_memstead::MemsteadRefError::GitTree(msg) => {
                MemRepoConfigError::GitTree(msg)
            }
            crate::storage_memstead::MemsteadRefError::Config { path, message } => {
                // Distinguish "ref not found" (workspace never had
                // __MEMSTEAD — pre-bootstrap stub) from "config blob
                // absent for this mem" (mem not registered).
                // Mirrors the legacy reader's NoMainBranch /
                // ConfigNotFound split that callers branch on.
                if path == "refs/heads/__MEMSTEAD" {
                    MemRepoConfigError::NoMainBranch
                } else if message.contains("config not found") {
                    MemRepoConfigError::ConfigNotFound(mem_name.to_string())
                } else if message.contains("not utf-8") {
                    MemRepoConfigError::NotUtf8(message)
                } else {
                    MemRepoConfigError::Schema(ConfigError::InvalidJson(message))
                }
            }
            crate::storage_memstead::MemsteadRefError::GitCommit(msg) => {
                MemRepoConfigError::GitTree(msg)
            }
            crate::storage_memstead::MemsteadRefError::NotUtf8(_, msg) => {
                MemRepoConfigError::NotUtf8(msg)
            }
            crate::storage_memstead::MemsteadRefError::Schema { source, .. } => {
                MemRepoConfigError::Schema(ConfigError::Other(source.to_string()))
            }
        }
    })
}

/// Build a `MemInit { dir: None, .. }` for a mem-repo-backed branch.
///
/// Reads the per-mem config from `mem-repo-git:__MEMSTEAD:mems/<name>/config.json`
/// and resolves its `schema` pin into a `SchemaRef`. The placeholder
/// pin used when the config is missing or carries no `schema` field
/// is `default@1.0.0`, mirroring `memstead-git-branch::discover`'s legacy
/// fallback so disk-shaped and mem-repo-backed paths produce the same
/// MemInit shape.
///
/// Used by `memstead-swift`'s `discover_mems` to build the macOS app's
/// mem list from `enumerate_mem_repo_branches` output without
/// re-implementing the schema-pin → SchemaRef plumbing.
///
/// Workspace-rooted convenience wrapper around
/// [`mem_init_from_branch_at_gitdir`].
pub fn mem_init_from_branch(
    workspace_root: &Path,
    mem_name: &str,
) -> Result<MemInit, MemRepoConfigError> {
    mem_init_from_branch_at_gitdir(&gitdir_for_leaf(workspace_root, mem_name), mem_name)
}

/// Gitdir-rooted variant of [`mem_init_from_branch`].
pub fn mem_init_from_branch_at_gitdir(
    gitdir: &Path,
    mem_name: &str,
) -> Result<MemInit, MemRepoConfigError> {
    let config = read_config_at_gitdir(gitdir, mem_name)?;
    let schema_ref = config
        .schema
        .clone()
        .unwrap_or_else(|| SchemaRef::new("default", semver::Version::new(1, 0, 0)));
    Ok(MemInit {
        name: mem_name.to_string(),
        dir: None,
        schema_ref,
    })
}

/// "Real mem-repo" gate: returns `true` if `<workspace_root>/mem-repo/.git/`
/// carries `refs/heads/__MEMSTEAD` (the unified registry ref). Empty bare
/// repos (the `init_mem_repo_stub` shape) return `false`.
pub fn has_real_mem_repo_main(workspace_root: &Path) -> bool {
    has_real_mem_repo_main_at_gitdir(&default_gitdir(workspace_root))
}

/// Gitdir-rooted variant of [`has_real_mem_repo_main`]. Multi-mount
/// callers ask the question per mount.
pub fn has_real_mem_repo_main_at_gitdir(gitdir: &Path) -> bool {
    let Ok(repo) = gix::open(gitdir) else {
        return false;
    };
    matches!(repo.try_find_reference("refs/heads/__MEMSTEAD"), Ok(Some(_)))
}

/// Errors raised while writing a mem config to `mem-repo-git:__MEMSTEAD`.
#[derive(Debug, thiserror::Error)]
pub enum MemRepoWriteError {
    /// Could not open `<workspace_root>/mem-repo/.git/`. Wraps the gix
    /// error message; pre-flight via `has_real_mem_repo_main` to avoid
    /// surfacing this from a workspace that is not mem-repo-backed.
    #[error("could not open mem-repo gitdir at {path}: {message}")]
    GixOpen { path: String, message: String },
    /// `refs/heads/__MEMSTEAD` is missing — workspace is not initialised.
    /// Variant name retained for backward compatibility.
    #[error("mem-repo has no refs/heads/__MEMSTEAD: {0}")]
    NoMainBranch(String),
    /// Generic git-tree write failure (object database error, ref edit
    /// rejected, etc.). The wrapped string surfaces the underlying
    /// gix error for operator log lines.
    #[error("git tree write error: {0}")]
    GitTree(String),
    /// A `commit_refs` batch was rejected by gix-ref's transaction
    /// `prepare` phase — typically a `MustExistAndMatch` precondition
    /// mismatch (the observed `main` tip moved between snapshot and
    /// commit) or a `MustNotExist` violation on a branch the caller
    /// thought it was creating fresh. The wrapped string surfaces the
    /// underlying gix message for operator log lines.
    #[error("ref transaction rejected: {0}")]
    RefTransaction(String),
}

/// One ref edit inside a [`commit_refs`] batch. The caller has already
/// written the new commit object via `repo.write_object`; `RefSpec`
/// names the ref to point at it and the precondition that gates the
/// update.
///
/// The fields are deliberately the minimum needed to build a
/// `gix_ref::transaction::RefEdit` without leaking the raw type into
/// the call sites — call sites stay in plain `String`/`ObjectId`
/// territory.
pub struct RefSpec {
    /// Fully qualified ref name (e.g. `"refs/heads/main"`,
    /// `"refs/heads/<mem>"`).
    pub ref_name: String,
    /// Object id the ref will point at after the batch lands.
    pub new_oid: gix::ObjectId,
    /// Precondition. `MustNotExist` for a brand-new branch;
    /// `MustExistAndMatch(observed_tip)` for a read-modify-write update
    /// against a known previous tip.
    pub expected: gix::refs::transaction::PreviousValue,
    /// Reflog message for this ref edit. Carried into the per-ref
    /// reflog when reflogs are enabled.
    pub log_message: String,
}

/// Run a batch of ref edits as a single `edit_references` transaction
/// against `<workspace_root>/mem-repo/.git/`.
///
/// **Atomicity scope (closes [D8] in-process registry-corruption from
/// concurrent writers; commit-phase IO failure remains best-effort).**
/// gix-ref's transaction `prepare` validates every spec's precondition
/// and acquires per-ref locks; if any spec fails the precondition the
/// entire batch is rejected — no partial state. Once `prepare`
/// succeeds, `commit` writes ref values under lock; a commit-phase IO
/// failure mid-batch can leave the ref store inconsistent (per
/// `gix-ref-0.61.0/src/transaction/mod.rs:11-14`), which surfaces as a
/// `RefTransaction` error and is the operator-recoverable case
/// described in the plan's What does NOT land section.
///
/// **Cross-thread RMW caveat.** gix-ref reads `existing_ref` *before*
/// acquiring the per-ref file lock (`gix-ref-0.61.0/src/store/file/transaction/prepare.rs:31`,
/// lock at `:120`); the precondition is then checked against the
/// pre-lock snapshot at `:142`. Two parallel `commit_refs` calls on
/// separate `Repository` instances against the same observed tip can
/// both pass `MustExistAndMatch(T0)` and serialise under the lock —
/// the second silently overwrites the first. The engine is
/// `&mut self`-disciplined today (`lib.rs:259-263`) so no in-process
/// parallelism can occur; the gap is documented for the future
/// multi-thread/multi-process plan.
///
/// Used by the two existing single-ref writers (`commit_config` calls
/// from `memstead_install` and `mem_cache::register_read_mem_in_mem_repo`)
/// and by `mem_management/create.rs` for the two-ref atomic
/// mem-create batch.
///
/// Workspace-rooted convenience wrapper around [`commit_refs_at_gitdir`].
pub fn commit_refs(
    workspace_root: &Path,
    specs: &[RefSpec],
) -> Result<(), MemRepoWriteError> {
    commit_refs_at_gitdir(&default_gitdir(workspace_root), specs)
}

/// Gitdir-rooted variant of [`commit_refs`]. Multi-mount callers route
/// each batch to the target mount's gitdir directly.
pub fn commit_refs_at_gitdir(
    gitdir: &Path,
    specs: &[RefSpec],
) -> Result<(), MemRepoWriteError> {
    use gix::refs::transaction::{Change, LogChange, RefEdit, RefLog};
    use gix::refs::{FullName, Target};

    let repo = gix::open(gitdir).map_err(|e| MemRepoWriteError::GixOpen {
        path: gitdir.display().to_string(),
        message: e.to_string(),
    })?;

    let mut edits: Vec<RefEdit> = Vec::with_capacity(specs.len());
    for spec in specs {
        let name: FullName = spec.ref_name.as_str().try_into().map_err(|e| {
            MemRepoWriteError::RefTransaction(format!(
                "invalid ref name {:?}: {e}",
                spec.ref_name
            ))
        })?;
        edits.push(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: spec.log_message.as_str().into(),
                },
                expected: spec.expected.clone(),
                new: Target::Object(spec.new_oid),
            },
            name,
            deref: false,
        });
    }

    repo.edit_references(edits).map_err(|e| {
        MemRepoWriteError::RefTransaction(e.to_string())
    })?;

    Ok(())
}

/// Commit `<mem_name>/config.json` to `mem-repo-git:refs/heads/__MEMSTEAD`.
///
/// Read-modify-write: snapshot the current `__SYSTEM` tip, build a tree
/// that upserts the per-mem config blob, write a commit object that
/// chains onto the snapshot, then atomically advance `__SYSTEM` via
/// [`commit_refs`] with `MustExistAndMatch(observed_system_tip)`.
///
/// **Closes the in-process RMW race for the single-ref path** (D8 in
/// the plan): if a sequential second writer observes `main = T0` but
/// `main` has advanced to `T1` since the snapshot, the precondition
/// rejects the batch with [`MemRepoWriteError::RefTransaction`] rather
/// than silently advancing past the first writer's commit. The
/// cross-thread RMW caveat documented on [`commit_refs`] applies here
/// unchanged — the engine's `&mut self` discipline keeps the gap
/// academic in current single-process usage.
///
/// Used by:
/// - `memstead_install` to update an existing mem's `readMems` field
///   (RMW the same blob).
/// - `mem_cache::register_read_mem_in_mem_repo` (same RMW shape).
///
/// Workspace-rooted convenience wrapper around [`commit_config_at_gitdir`].
pub fn commit_config(
    workspace_root: &Path,
    mem_name: &str,
    config_bytes: &[u8],
    ctx: &CommitContext<'_>,
    message: &str,
) -> Result<(), MemRepoWriteError> {
    commit_config_at_gitdir(
        &gitdir_for_leaf(workspace_root, mem_name),
        mem_name,
        config_bytes,
        ctx,
        message,
    )
}

/// Gitdir-rooted variant of [`commit_config`]. Multi-mount callers
/// route the RMW to the target mount's gitdir directly so the leaf
/// resolver and the ref advance target the same mem-repo.
///
/// Writes only to `__MEMSTEAD` — the engine reader's sole source of truth
/// for per-mem configs post-rebuild. `commit_config_to_memstead_at_gitdir`
/// is self-creating: it advances `__MEMSTEAD` if present (MustExistAndMatch)
/// or seeds it (MustNotExist) when absent, so callers do not preflight
/// the ref.
pub fn commit_config_at_gitdir(
    gitdir: &Path,
    mem_name: &str,
    config_bytes: &[u8],
    ctx: &CommitContext<'_>,
    message: &str,
) -> Result<(), MemRepoWriteError> {
    crate::storage_memstead::commit_config_to_memstead_at_gitdir(
        gitdir,
        mem_name,
        config_bytes,
        ctx,
        message,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a minimal `mem-repo/.git/` carrying `__SYSTEM` with one
    /// config blob. Returns the workspace root.
    fn init_mem_repo_with_config(
        mem_name: &str,
        config_json: &str,
    ) -> TempDir {
        let tmp = TempDir::new().unwrap();
        let gitdir = tmp.path().join("mem-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        let repo = gix::init_bare(&gitdir).unwrap();

        let blob = repo
            .write_blob(config_json.as_bytes())
            .unwrap()
            .detach();
        let mut editor = repo.empty_tree().edit().unwrap();
        editor
            .upsert(
                format!("{mem_name}/config.json"),
                gix::objs::tree::EntryKind::Blob,
                blob,
            )
            .unwrap();
        let tree_id = editor.write().unwrap().detach();

        let actor = gix::actor::Signature {
            name: "test".into(),
            email: "test@example.com".into(),
            time: gix::date::Time {
                seconds: 0,
                offset: 0,
            },
        };
        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        repo.commit_as(
            actor_ref,
            actor_ref,
            "refs/heads/__SYSTEM",
            "seed",
            tree_id,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();

        // Project the just-written __SYSTEM content onto the unified
        // `__MEMSTEAD` ref so post-s140 reads (which target `__MEMSTEAD` only)
        // see the seeded config.
        crate::storage_memstead::migrate_to_memstead_ref(&gitdir).unwrap();

        tmp
    }

    #[test]
    fn reads_config_from_system_ref() {
        let tmp = init_mem_repo_with_config(
            "alpha",
            r#"{"schema": "default@1.0.0"}"#,
        );
        let config = read_config(tmp.path(), "alpha").unwrap();
        // Configs no longer carry an in-config `name` field; the leaf folder on
        // `__SYSTEM` is authoritative.
        assert!(config.name.is_none());
        assert_eq!(
            config.schema.as_ref().map(|s| s.name.as_str()),
            Some("default")
        );
    }

    #[test]
    fn errors_when_gitdir_missing() {
        let tmp = TempDir::new().unwrap();
        let err = read_config(tmp.path(), "alpha").unwrap_err();
        assert!(matches!(err, MemRepoConfigError::GitdirNotFound(_)));
    }

    #[test]
    fn errors_when_config_missing_in_tree() {
        let tmp = init_mem_repo_with_config(
            "alpha",
            r#"{"schema": "default@1.0.0"}"#,
        );
        let err = read_config(tmp.path(), "beta").unwrap_err();
        assert!(matches!(err, MemRepoConfigError::ConfigNotFound(name) if name == "beta"));
    }

    /// Build a `mem-repo/.git/` carrying a hierarchical branch
    /// `refs/heads/<full_path>` plus the matching `__SYSTEM` tree path.
    /// The resolver should map `<leaf>` → `<full_path>` regardless of
    /// the depth of the path prefix.
    fn init_mem_repo_with_hierarchical_branch(full_path: &str) -> TempDir {
        let tmp = TempDir::new().unwrap();
        let gitdir = tmp.path().join("mem-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        let repo = gix::init_bare(&gitdir).unwrap();
        let actor = gix::actor::Signature {
            name: "test".into(),
            email: "test@example.com".into(),
            time: gix::date::Time { seconds: 0, offset: 0 },
        };
        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);

        // Seal the per-mem content branch with an empty tree.
        let empty_tree = repo.empty_tree().id().detach();
        repo.commit_as(
            actor_ref,
            actor_ref,
            format!("refs/heads/{full_path}"),
            "seal hierarchical",
            empty_tree,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();

        // Mirror `<full_path>/config.json` on `__SYSTEM`.
        let blob = repo
            .write_blob(br#"{"schema":"default@1.0.0"}"#)
            .unwrap()
            .detach();
        let mut editor = repo.empty_tree().edit().unwrap();
        editor
            .upsert(
                format!("{full_path}/config.json"),
                gix::objs::tree::EntryKind::Blob,
                blob,
            )
            .unwrap();
        let tree_id = editor.write().unwrap().detach();
        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        repo.commit_as(
            actor_ref,
            actor_ref,
            "refs/heads/__SYSTEM",
            "seed system",
            tree_id,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();

        // Project __SYSTEM onto __MEMSTEAD so post-s140 reads land.
        crate::storage_memstead::migrate_to_memstead_ref(&gitdir).unwrap();

        tmp
    }

    #[test]
    fn resolve_full_path_returns_flat_branch_name() {
        let tmp = init_mem_repo_with_hierarchical_branch("alpha");
        let gitdir = tmp.path().join("mem-repo").join(".git");
        let resolved = super::resolve_full_path_at_gitdir(&gitdir, "alpha").unwrap();
        assert_eq!(resolved, Some("alpha".to_string()));
    }

    #[test]
    fn resolve_full_path_returns_full_branch_for_hierarchical() {
        let tmp = init_mem_repo_with_hierarchical_branch("demo/engine");
        let gitdir = tmp.path().join("mem-repo").join(".git");
        let resolved = super::resolve_full_path_at_gitdir(&gitdir, "engine").unwrap();
        assert_eq!(resolved, Some("demo/engine".to_string()));
    }

    #[test]
    fn resolve_full_path_returns_none_for_unknown_leaf() {
        let tmp = init_mem_repo_with_hierarchical_branch("demo/engine");
        let gitdir = tmp.path().join("mem-repo").join(".git");
        let resolved = super::resolve_full_path_at_gitdir(&gitdir, "ghost").unwrap();
        assert!(resolved.is_none());
    }

    /// Seal an arbitrary list of per-mem content branches at the
    /// given full paths on top of an existing fixture repo.
    fn seal_branches(gitdir: &std::path::Path, full_paths: &[&str]) {
        let repo = gix::open(gitdir).unwrap();
        let actor = gix::actor::Signature {
            name: "test".into(),
            email: "test@example.com".into(),
            time: gix::date::Time { seconds: 0, offset: 0 },
        };
        let empty_tree = repo.empty_tree().id().detach();
        for full_path in full_paths {
            let mut buf = gix::date::parse::TimeBuf::default();
            let actor_ref = actor.to_ref(&mut buf);
            repo.commit_as(
                actor_ref,
                actor_ref,
                format!("refs/heads/{full_path}"),
                "seal",
                empty_tree,
                Vec::<gix::ObjectId>::new(),
            )
            .unwrap();
        }
    }

    #[test]
    fn find_branches_by_leaf_returns_empty_for_unknown_leaf() {
        let tmp = init_mem_repo_with_hierarchical_branch("demo/engine");
        let gitdir = tmp.path().join("mem-repo").join(".git");
        let matches = super::find_branches_by_leaf_at_gitdir(&gitdir, "ghost").unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn find_branches_by_leaf_returns_single_full_path() {
        let tmp = init_mem_repo_with_hierarchical_branch("demo/engine");
        let gitdir = tmp.path().join("mem-repo").join(".git");
        let matches = super::find_branches_by_leaf_at_gitdir(&gitdir, "engine").unwrap();
        assert_eq!(matches, vec!["demo/engine".to_string()]);
    }

    #[test]
    fn find_branches_by_leaf_returns_all_colliding_paths_sorted() {
        // Two branches sharing leaf `engine` at distinct paths — the
        // exact corruption scenario Goal 11 surfaces explicitly.
        let tmp = init_mem_repo_with_hierarchical_branch("demo/engine");
        let gitdir = tmp.path().join("mem-repo").join(".git");
        seal_branches(&gitdir, &["planning/engine"]);
        let matches = super::find_branches_by_leaf_at_gitdir(&gitdir, "engine").unwrap();
        assert_eq!(
            matches,
            vec!["demo/engine".to_string(), "planning/engine".to_string()]
        );
    }

    #[test]
    fn find_branches_by_leaf_skips_main_and_registry_refs() {
        // `__SYSTEM` ref carries the same `engine` leaf if a corrupt
        // operator created `refs/heads/__weird`. The walker filters
        // both `main` and any `__*`-leading-segment ref so registry
        // refs cannot mask as content branches.
        let tmp = init_mem_repo_with_hierarchical_branch("alpha");
        let gitdir = tmp.path().join("mem-repo").join(".git");
        // No content-branch with leaf `__SYSTEM` should be reported.
        let matches = super::find_branches_by_leaf_at_gitdir(&gitdir, "__SYSTEM").unwrap();
        assert!(matches.is_empty());
        // And a flat `alpha` branch shows under leaf `alpha`.
        let alpha = super::find_branches_by_leaf_at_gitdir(&gitdir, "alpha").unwrap();
        assert_eq!(alpha, vec!["alpha".to_string()]);
    }

    #[test]
    fn find_branches_by_leaf_errors_when_gitdir_missing() {
        let tmp = TempDir::new().unwrap();
        let gitdir = tmp.path().join("nonexistent").join(".git");
        let err = super::find_branches_by_leaf_at_gitdir(&gitdir, "alpha")
            .expect_err("missing gitdir must surface as GitdirNotFound");
        assert!(matches!(err, super::MemRepoConfigError::GitdirNotFound(_)));
    }

    /// `_at_gitdir` siblings target an arbitrary gitdir, not the
    /// workspace-default `<workspace_root>/mem-repo/.git/`. This
    /// pins that the gitdir is the only thing that matters: a mem
    /// living in a non-default mount path is readable as long as its
    /// gitdir is supplied directly.
    #[test]
    fn at_gitdir_apis_target_arbitrary_gitdir() {
        // Workspace tmp has no `mem-repo/.git/` at all — only a
        // sibling mount at `external/.git/`. The workspace-rooted
        // wrappers would all surface `GitdirNotFound`; the
        // `_at_gitdir` siblings must work because we hand them the
        // explicit gitdir.
        let tmp = init_mem_repo_with_config(
            "alpha",
            r#"{"schema": "default@1.0.0"}"#,
        );
        // Move the gitdir from the workspace-default location to a
        // sibling so the workspace-rooted wrappers can no longer find
        // it. (Equivalent to a `[[mem_repos]] path = "external"`
        // declaration.)
        let default_path = tmp.path().join("mem-repo");
        let mount_path = tmp.path().join("external");
        std::fs::rename(&default_path, &mount_path).unwrap();
        let gitdir = mount_path.join(".git");

        // `_at_gitdir` siblings see the mount.
        assert!(super::has_real_mem_repo_main_at_gitdir(&gitdir));
        let cfg = super::read_config_at_gitdir(&gitdir, "alpha").unwrap();
        // Configs no longer carry an in-config `name` field.
        assert!(cfg.name.is_none());
        // The fixture seeds only `__SYSTEM:alpha/config.json` and no
        // per-mem content branch — `resolve_full_path` returns `None`
        // and `branch_ref_for_mem` falls back to the flat form. Pins
        // both behaviours against the explicit gitdir.
        assert_eq!(
            super::resolve_full_path_at_gitdir(&gitdir, "alpha").unwrap(),
            None
        );
        assert_eq!(
            super::branch_ref_for_mem_at_gitdir(&gitdir, "alpha"),
            "refs/heads/alpha"
        );

        // Workspace-rooted wrappers DON'T see the mount because the
        // synthesised default path no longer exists.
        assert!(!super::has_real_mem_repo_main(tmp.path()));
        let err = super::read_config(tmp.path(), "alpha").unwrap_err();
        assert!(matches!(err, MemRepoConfigError::GitdirNotFound(_)));
    }

    /// `commit_config_at_gitdir` routes the RMW to the supplied
    /// gitdir's `__MEMSTEAD` ref. Pins that a non-default-mount commit
    /// path advances exactly that mount's `__MEMSTEAD` and leaves any
    /// default mount untouched.
    #[test]
    fn commit_config_at_gitdir_targets_arbitrary_mount() {
        let tmp = init_mem_repo_with_config(
            "alpha",
            r#"{"schema": "default@1.0.0"}"#,
        );
        let default_path = tmp.path().join("mem-repo");
        let mount_path = tmp.path().join("external");
        std::fs::rename(&default_path, &mount_path).unwrap();
        let gitdir = mount_path.join(".git");

        let pre_tip = {
            let repo = gix::open(&gitdir).unwrap();
            repo.find_reference("refs/heads/__MEMSTEAD")
                .unwrap()
                .into_fully_peeled_id()
                .unwrap()
                .detach()
        };

        let ctx = crate::vcs::CommitContext::internal();
        super::commit_config_at_gitdir(
            &gitdir,
            "alpha",
            br#"{"schema":"default@1.0.0","note":"v1"}"#,
            &ctx,
            "external mount commit",
        )
        .expect("commit_config_at_gitdir against external mount");

        let post_tip = {
            let repo = gix::open(&gitdir).unwrap();
            repo.find_reference("refs/heads/__MEMSTEAD")
                .unwrap()
                .into_fully_peeled_id()
                .unwrap()
                .detach()
        };
        assert_ne!(pre_tip, post_tip, "external mount __MEMSTEAD must advance");

        // Workspace-rooted commit_config still surfaces GixOpen
        // because the default mount has no gitdir.
        let result = super::commit_config(
            tmp.path(),
            "alpha",
            br#"{}"#,
            &ctx,
            "should fail",
        );
        assert!(matches!(result, Err(MemRepoWriteError::GixOpen { .. })));
    }

    #[test]
    fn read_config_resolves_hierarchical_layout() {
        let tmp = init_mem_repo_with_hierarchical_branch("planning/exec-foo");
        // The lookup happens by leaf only and must walk via
        // `resolve_full_path` to reach `planning/exec-foo/config.json`.
        // Configs no longer carry an in-config `name` field — successful read of
        // the schema field proves the resolver landed on the right
        // tree path.
        let cfg = super::read_config(tmp.path(), "exec-foo").unwrap();
        assert!(cfg.name.is_none());
        assert_eq!(
            cfg.schema.as_ref().map(|s| s.name.as_str()),
            Some("default")
        );
    }

    #[test]
    fn errors_when_system_ref_missing() {
        // Empty bare repo — the stub shape used by `init_mem_repo_stub`.
        let tmp = TempDir::new().unwrap();
        let gitdir = tmp.path().join("mem-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        gix::init_bare(&gitdir).unwrap();
        let err = read_config(tmp.path(), "alpha").unwrap_err();
        assert!(matches!(err, MemRepoConfigError::NoMainBranch));
    }

    /// Sequential read-modify-write: a `commit_refs` batch whose
    /// `MustExistAndMatch(<observed_T0>)` precondition disagrees with
    /// the current `__MEMSTEAD` tip (`T1`, after a prior commit landed) is
    /// rejected with a typed `RefTransaction` error rather than
    /// silently advancing past `T1` to a new `T2`.
    ///
    /// This is the sequential-RMW shape — cross-thread parallelism
    /// (two threads racing the *same* observed `T0`) is NOT covered;
    /// gix-ref's `prepare` reads `existing_ref` before lock acquisition,
    /// so the cross-thread closure of the same race requires an outer
    /// mutex (out of scope).
    #[test]
    fn commit_config_rejects_stale_main_tip() {
        let tmp = init_mem_repo_with_config(
            "alpha",
            r#"{"schema": "default@1.0.0"}"#,
        );
        let workspace_root = tmp.path();
        let gitdir = workspace_root.join("mem-repo").join(".git");

        let observed_t0 = {
            let repo = gix::open(&gitdir).unwrap();
            repo.find_reference("refs/heads/__MEMSTEAD")
                .unwrap()
                .into_fully_peeled_id()
                .unwrap()
                .detach()
        };

        let ctx = crate::vcs::CommitContext::internal();
        commit_config(
            workspace_root,
            "alpha",
            br#"{"schema":"default@1.0.0","note":"v1"}"#,
            &ctx,
            "first commit",
        )
        .expect("first commit_config should succeed");

        let observed_t1 = {
            let repo = gix::open(&gitdir).unwrap();
            repo.find_reference("refs/heads/__MEMSTEAD")
                .unwrap()
                .into_fully_peeled_id()
                .unwrap()
                .detach()
        };
        assert_ne!(
            observed_t0, observed_t1,
            "__MEMSTEAD must have advanced after first commit_config"
        );

        let stale_result = {
            let repo = gix::open(&gitdir).unwrap();
            let memstead_tree = repo
                .find_object(observed_t1)
                .unwrap()
                .into_commit()
                .tree()
                .unwrap()
                .id()
                .detach();
            let sig = gix::actor::Signature {
                name: "test".into(),
                email: "test@example.com".into(),
                time: gix::date::Time {
                    seconds: 0,
                    offset: 0,
                },
            };
            let new_commit = gix::objs::Commit {
                message: "stale".into(),
                tree: memstead_tree,
                author: sig.clone(),
                committer: sig,
                encoding: None,
                parents: std::iter::once(observed_t1).collect(),
                extra_headers: Default::default(),
            };
            let new_oid = repo.write_object(&new_commit).unwrap().detach();

            commit_refs(
                workspace_root,
                &[RefSpec {
                    ref_name: "refs/heads/__MEMSTEAD".to_string(),
                    new_oid,
                    expected:
                        gix::refs::transaction::PreviousValue::MustExistAndMatch(
                            gix::refs::Target::Object(observed_t0),
                        ),
                    log_message: "memstead: stale RMW".to_string(),
                }],
            )
        };

        assert!(
            matches!(stale_result, Err(MemRepoWriteError::RefTransaction(_))),
            "expected RefTransaction precondition mismatch, got {:?}",
            stale_result
        );

        let observed_after = {
            let repo = gix::open(&gitdir).unwrap();
            repo.find_reference("refs/heads/__MEMSTEAD")
                .unwrap()
                .into_fully_peeled_id()
                .unwrap()
                .detach()
        };
        assert_eq!(
            observed_after, observed_t1,
            "__MEMSTEAD must remain at T1 after rejected stale RMW"
        );
    }

}
