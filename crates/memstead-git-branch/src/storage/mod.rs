//! Re-export shim over `memstead_base::storage` (the [`MemWriter`] trait
//! and [`MemWriterError`]) plus the git-tree adapter that stays in
//! this crate.
//!
//! The git-tree adapter ([`git_tree::GitTreeMemWriter`]) buffers
//! mutations and applies them via `gix::object::tree::Editor` against a
//! multi-root `mem-repo-git` repository, one branch per mem.

pub mod git_tree;

use std::path::PathBuf;

pub use memstead_base::storage::{CommitId, MemWriter, MemWriterError};

/// Construct a `Box<dyn MemWriter>` for the git-object-backed path.
/// `gitdir` points at the multi-root `mem-repo-git` repo; `ref_name`
/// is the per-mem branch (fully-qualified, e.g.
/// `refs/heads/<mem>`). The first commit creates the ref if it does
/// not yet exist.
#[cfg(feature = "git-object-storage")]
pub fn git_tree_mem_writer(gitdir: PathBuf, ref_name: String) -> Box<dyn MemWriter> {
    Box::new(git_tree::GitTreeMemWriter::new(gitdir, ref_name))
}

/// Full counterpart of [`memstead_base::instantiate_lean_backend`]: turns
/// any [`memstead_base::Mount`] into a `Box<dyn MemBackend>`, including
/// the git-branch variant that the lean flavour cannot construct.
///
/// Folder and Archive variants delegate to the lean function so the
/// instantiation paths share one implementation. The git-branch
/// variant constructs a [`git_tree::GitTreeMemWriter`] using the
/// mount's `gitdir` + `branch`, fully-qualifying the ref-name as
/// `refs/heads/<branch>` so the per-branch mutex inside the writer
/// keys consistently with what `agent_notes_since` and
/// `read_branch_blobs` expect.
pub fn instantiate_full_backend(
    mount: &memstead_base::Mount,
) -> Result<Box<dyn memstead_base::MemBackend>, memstead_base::InstantiateError> {
    use memstead_base::MountStorage;
    match &mount.storage {
        MountStorage::Folder { .. }
        | MountStorage::Archive { .. }
        | MountStorage::InMemory => memstead_base::instantiate_lean_backend(mount),
        MountStorage::GitBranch { gitdir, branch } => {
            let ref_name = if branch.starts_with("refs/") {
                branch.clone()
            } else {
                format!("refs/heads/{branch}")
            };
            Ok(Box::new(git_tree::GitTreeMemWriter::new(
                gitdir.clone(),
                ref_name,
            )))
        }
    }
}

/// The git-branch ops bundle installed on `memstead_base::Engine` by full
/// boot. Wraps `crate::ops::changes::changes_since` and
/// `crate::ops::export::export_mem_from_branch` so the engine can
/// dispatch from a [`MountStorage::GitBranch`] mount without an extra
/// trait or downcast.
pub const FULL_GIT_BRANCH_OPS: memstead_base::GitBranchOps = memstead_base::GitBranchOps {
    changes_since: changes_since_dispatch,
    diff: diff_dispatch,
    branch_reset: branch_reset_dispatch,
    fetch: fetch_dispatch,
    pull: pull_dispatch,
    push: push_dispatch,
    remote_add: remote_add_dispatch,
    read_tree: read_tree_dispatch,
    export: export_dispatch,
    export_to_bytes: export_to_bytes_dispatch,
    prune_residue: prune_residue_dispatch,
    write_schema: write_schema_dispatch,
};

/// Dispatcher for `Engine::install_schema` on git-branch workspaces.
/// Writes the schema package onto the unified `__MEMSTEAD:schemas/` ref
/// and returns the resulting commit sha.
fn write_schema_dispatch(
    gitdir: &std::path::Path,
    name: &str,
    version: &str,
    files: &[(String, Vec<u8>)],
) -> Result<String, memstead_base::backend::BackendError> {
    crate::storage_memstead::write_schema_to_memstead_ref(gitdir, name, version, files)
        .map(|outcome| outcome.commit_sha)
        .map_err(|e| {
            memstead_base::backend::BackendError::Other(format!(
                "schema install onto __MEMSTEAD ref at {}: {e}",
                gitdir.display(),
            ))
        })
}

/// Dispatcher for
/// `RecoveryAction::ForceOverwrite` in `create_mem`. Drops the
/// per-mem branch + `__MEMSTEAD` config blob in one ref-edit
/// transaction by delegating to `delete_mem_artifacts_at_gitdir`
/// (the same helper `MemBackend::delete_artifacts` already wraps
/// for delete-files flows). Operates on an unmounted gitdir —
/// callers don't need an instantiated backend, which is why the
/// orchestrator reaches for this through `Engine::git_branch_ops()`
/// rather than constructing a backend just to call `delete_artifacts`.
fn prune_residue_dispatch(
    gitdir: &std::path::Path,
    branch_full_path: &str,
) -> Result<(), memstead_base::backend::BackendError> {
    let ctx = memstead_base::vcs::CommitContext {
        actor: memstead_base::vcs::Actor::Agent,
        client: None,
        tool: Some("memstead_mem_create (force_overwrite)"),
        note: None,
        logical_operation_id: None,
        entity_ids: None,
    };
    crate::storage_memstead::delete_mem_artifacts_at_gitdir(
        gitdir,
        branch_full_path,
        &ctx,
    )
    .map_err(|e| {
        memstead_base::backend::BackendError::Other(format!(
            "force_overwrite prune at {}: {e}",
            branch_full_path,
        ))
    })
}

fn changes_since_dispatch(
    gitdir: &std::path::Path,
    branch: &str,
    mem: &str,
    since: &str,
    rename_similarity: f32,
) -> Result<memstead_base::ops::BackendChanges, memstead_base::backend::BackendError> {
    let ref_name = if branch.starts_with("refs/") {
        branch.to_string()
    } else {
        format!("refs/heads/{branch}")
    };
    let empty_store = memstead_base::Store::new();
    let report = crate::ops::changes::changes_since(
        &empty_store,
        mem,
        gitdir,
        since,
        rename_similarity,
        Some(&ref_name),
    )
    .map_err(|e| {
        // A bad `since`
        // SHA (malformed or absent) is a recoverable caller-argument
        // fault, not a backend fault. Encode it as a typed prefix the
        // engine lifts to `COMMIT_NOT_FOUND` (carrying the untruncated
        // SHA), reserving the `MEM_ERROR` catch-all for genuine faults.
        match e {
            crate::vcs::VcsError::ObjectNotFound(_) => {
                memstead_base::backend::BackendError::Other(format!("COMMIT_NOT_FOUND:{since}"))
            }
            other => memstead_base::backend::BackendError::Other(format!(
                "git-branch changes_since: {other}"
            )),
        }
    })?;
    Ok(memstead_base::ops::BackendChanges {
        since: report.since,
        head: report.head,
        changes: report.changes,
        notes: report.notes.unwrap_or_default(),
        memstead_ref: report.memstead_ref,
    })
}

fn export_dispatch(
    gitdir: &std::path::Path,
    branch: &str,
    mem: &str,
    config: &memstead_schema::MemConfig,
    output_path: &std::path::Path,
    workspace_root: Option<&std::path::Path>,
    workspace_schemas_dir: Option<&std::path::Path>,
    provenance_bytes: Option<&[u8]>,
) -> Result<memstead_base::ops::MemExportResult, memstead_base::backend::BackendError> {
    let _ = branch;
    crate::ops::export::export_mem_from_branch(
        gitdir,
        mem,
        config,
        output_path,
        workspace_root,
        workspace_schemas_dir,
        provenance_bytes,
    )
    .map_err(|e| {
        memstead_base::backend::BackendError::Other(format!("export_mem_from_branch: {e}"))
    })
}

fn read_tree_dispatch(
    gitdir: &std::path::Path,
    ref_name: &str,
) -> Result<Vec<(String, String)>, memstead_base::backend::BackendError> {
    #[cfg(feature = "git-object-storage")]
    {
        crate::ops::transport::read_md_blobs_at_ref(gitdir, ref_name)
    }
    #[cfg(not(feature = "git-object-storage"))]
    {
        let _ = (gitdir, ref_name);
        Err(memstead_base::backend::BackendError::Other(
            "read_tree: git-object-storage feature not enabled".to_string(),
        ))
    }
}

fn fetch_dispatch(
    gitdir: &std::path::Path,
    remote: &str,
    refspecs: &[String],
) -> Result<memstead_base::ops::FetchOutcome, memstead_base::backend::BackendError> {
    #[cfg(feature = "git-object-storage")]
    {
        crate::ops::transport::fetch_in_gitdir(gitdir, remote, refspecs)
    }
    #[cfg(not(feature = "git-object-storage"))]
    {
        let _ = (gitdir, remote, refspecs);
        Err(memstead_base::backend::BackendError::Other(
            "fetch: git-object-storage feature not enabled".to_string(),
        ))
    }
}

fn pull_dispatch(
    gitdir: &std::path::Path,
    remote: &str,
    mem: &str,
) -> Result<memstead_base::ops::PullOutcome, memstead_base::backend::BackendError> {
    #[cfg(feature = "git-object-storage")]
    {
        crate::ops::transport::pull_in_gitdir(gitdir, remote, mem)
    }
    #[cfg(not(feature = "git-object-storage"))]
    {
        let _ = (gitdir, remote, mem);
        Err(memstead_base::backend::BackendError::Other(
            "pull: git-object-storage feature not enabled".to_string(),
        ))
    }
}

fn push_dispatch(
    gitdir: &std::path::Path,
    remote: &str,
    mem: &str,
    force: bool,
) -> Result<memstead_base::ops::PushOutcome, memstead_base::backend::BackendError> {
    #[cfg(feature = "git-object-storage")]
    {
        crate::ops::transport::push_in_gitdir(gitdir, remote, mem, force)
    }
    #[cfg(not(feature = "git-object-storage"))]
    {
        let _ = (gitdir, remote, mem, force);
        Err(memstead_base::backend::BackendError::Other(
            "push: git-object-storage feature not enabled".to_string(),
        ))
    }
}

fn remote_add_dispatch(
    gitdir: &std::path::Path,
    name: &str,
    url: &str,
) -> Result<memstead_base::ops::RemoteAddOutcome, memstead_base::backend::BackendError> {
    #[cfg(feature = "git-object-storage")]
    {
        crate::ops::transport::remote_add_in_gitdir(gitdir, name, url)
    }
    #[cfg(not(feature = "git-object-storage"))]
    {
        let _ = (gitdir, name, url);
        Err(memstead_base::backend::BackendError::Other(
            "remote_add: git-object-storage feature not enabled".to_string(),
        ))
    }
}

fn branch_reset_dispatch(
    gitdir: &std::path::Path,
    branch: &str,
    target_sha: &str,
) -> Result<memstead_base::ops::BranchResetOutcome, memstead_base::backend::BackendError> {
    #[cfg(feature = "git-object-storage")]
    {
        crate::ops::branch_reset::branch_reset_in_gitdir(gitdir, branch, target_sha)
    }
    #[cfg(not(feature = "git-object-storage"))]
    {
        let _ = (gitdir, branch, target_sha);
        Err(memstead_base::backend::BackendError::Other(
            "branch_reset: git-object-storage feature not enabled".to_string(),
        ))
    }
}

fn diff_dispatch(
    gitdir: &std::path::Path,
    mem: &str,
    ref_a: &str,
    ref_b: &str,
    config: &memstead_base::ops::DiffConfig,
) -> Result<memstead_base::ops::Diff, memstead_base::backend::BackendError> {
    #[cfg(feature = "git-object-storage")]
    {
        crate::ops::diff::diff_two_refs(gitdir, mem, ref_a, ref_b, config)
    }
    #[cfg(not(feature = "git-object-storage"))]
    {
        let _ = (gitdir, mem, ref_a, ref_b, config);
        Err(memstead_base::backend::BackendError::Other(
            "diff_two_refs: git-object-storage feature not enabled".to_string(),
        ))
    }
}

fn export_to_bytes_dispatch(
    gitdir: &std::path::Path,
    branch: &str,
    mem: &str,
    config: &memstead_schema::MemConfig,
    workspace_root: Option<&std::path::Path>,
    workspace_schemas_dir: Option<&std::path::Path>,
    provenance_bytes: Option<&[u8]>,
) -> Result<memstead_base::ops::MemExportBytes, memstead_base::backend::BackendError> {
    let _ = branch;
    crate::ops::export::export_mem_from_branch_to_bytes(
        gitdir,
        mem,
        config,
        workspace_root,
        workspace_schemas_dir,
        provenance_bytes,
    )
    .map_err(|e| {
        memstead_base::backend::BackendError::Other(format!(
            "export_mem_from_branch_to_bytes: {e}"
        ))
    })
}
