//! Mounting a mem that already exists as a branch.
//!
//! `create_mem` lays storage; a mount lays none. It writes the roster
//! row a workspace needs to serve a branch that is already there: in
//! the workspace's own mem-repo, in another git directory, or on a
//! remote from which it is fetched first. The row is then attached by
//! the roster reconciliation, the same path by which a running engine
//! follows a roster another process changed, so the two can never
//! disagree about what a row means.
//!
//! What a mount writes, and where:
//!
//! - the roster row, always, in the mounting workspace;
//! - for a branch in the workspace's own mem-repo, the mem's entry in
//!   that mem-repo's registry when there is none yet (from the fetched
//!   copy of a remote's registry when that lists the mem, else from the
//!   schema the caller names);
//! - nothing in a git directory other than the workspace's own
//!   mem-repo: such a mount's schema pin, when that directory's
//!   registry has no entry for the mem, rides the roster row.
//!
//! Removal is the router-only unregister of [`super::delete_mem`].

use std::path::{Path, PathBuf};

use super::{NOTE_MAX_LEN, admit_by_create_rules, classify_invalid_mem_name};
use crate::FullEngineError;
use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};
use crate::workspace_store::WorkspaceStoreAdapter;

/// Where the branch a mount serves comes from.
#[derive(Debug, Clone)]
pub enum MountSource {
    /// A branch that exists locally: in `gitdir` when given, else in the
    /// workspace's own mem-repo. `branch` is a short name or a full
    /// `refs/heads/...` ref.
    Branch {
        branch: String,
        gitdir: Option<PathBuf>,
    },
    /// The mem's branch on a configured remote of the workspace's own
    /// mem-repo: fetched, and mounted from the local branch created at
    /// its tip.
    Remote { remote: String },
}

/// Input of [`mount_mem`].
#[derive(Debug, Clone)]
pub struct MemMountParams {
    pub name: String,
    pub source: MountSource,
    pub capability: MountCapability,
    /// The schema pin, for a mem no registry the verb reads has an
    /// entry for. Refused when it contradicts an entry that exists.
    pub schema: Option<memstead_schema::SchemaRef>,
    pub note: Option<String>,
    pub operator_mode: bool,
    pub actor: crate::vcs::Actor,
    pub client: Option<crate::vcs::ClientId>,
}

/// What [`mount_mem`] did.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemMountResponse {
    pub name: String,
    /// The git directory that holds the branch.
    pub gitdir: String,
    /// The full ref of the mounted branch.
    pub branch: String,
    /// `write` or `read-only`.
    pub capability: &'static str,
    /// The schema the mem serves under.
    pub schema_ref: String,
    /// The branch tip the mount serves.
    pub head: String,
    /// Whether the branch was fetched from a remote for this mount.
    pub fetched: bool,
    /// Whether the mem's registry entry was written into the
    /// workspace's own mem-repo by this mount.
    pub config_written: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<crate::ops::WarningHint>,
}

const REGISTRY_REF: &str = "refs/heads/__MEMSTEAD";

/// Mount a mem that already exists as a branch. Everything that can
/// refuse the mount on its inputs (the name, the branch, the pin, the
/// create rules, a schema this workspace cannot resolve) is judged
/// before anything is written, so such a refusal leaves the roster,
/// the branches and the registry as they were; a fetch has placed
/// remote-tracking refs by then and nothing else. A failure after that
/// point takes back what this call wrote and nothing it found: a
/// registry entry that stood before the call is never removed.
pub fn mount_mem(
    engine: &mut crate::Engine,
    params: MemMountParams,
) -> Result<MemMountResponse, FullEngineError> {
    // ---- input ----
    if let Some(note) = params.note.as_deref()
        && note.chars().count() > NOTE_MAX_LEN
    {
        return Err(crate::EngineError::InvalidInput(format!(
            "note exceeds {NOTE_MAX_LEN} characters"
        ))
        .into());
    }
    if let Some(reason) = classify_invalid_mem_name(&params.name) {
        return Err(FullEngineError::InvalidMemName {
            name: params.name.clone(),
            reason,
        });
    }
    if crate::entity::id::validate_mem_name_grammar(&params.name).is_err() {
        return Err(FullEngineError::InvalidMemName {
            name: params.name.clone(),
            reason: "invalid_char",
        });
    }
    let root = engine
        .workspace_root()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            crate::EngineError::InvalidInput(
                "mem mount requires a workspace root to write its roster".to_string(),
            )
        })?;
    let ops = engine.git_branch_ops().ok_or_else(|| {
        crate::EngineError::InvalidInput(
            "mem mount requires the git-branch ops bundle (full boot only)".to_string(),
        )
    })?;

    // The provenance of whatever this mount commits or prunes.
    let ctx = engine.commit_context(
        Some("memstead_mem_mount"),
        params.actor,
        params.client.clone(),
        params.note.clone(),
    );

    // The roster as other processes left it: a name another process
    // mounted meanwhile is a collision too.
    engine
        .reconcile_roster_forced()
        .map_err(FullEngineError::from)?;
    if let Some(existing) = engine.mem_router().origin_for_mem(&params.name) {
        return Err(crate::EngineError::MemNameCollision {
            name: params.name,
            source_origin: existing.render_source(),
        }
        .into());
    }
    if engine
        .mem_router()
        .archive_path_for_mem(&params.name)
        .is_some()
        || engine.mount(&params.name).is_some()
    {
        return Err(crate::EngineError::MemNameCollision {
            name: params.name,
            source_origin: "attached read mem".to_string(),
        }
        .into());
    }
    if let Some(q) = engine.quarantine_reason(&params.name) {
        return Err(crate::EngineError::MemNameCollision {
            name: params.name.clone(),
            source_origin: format!("a quarantined mount ({})", q.reason_code),
        }
        .into());
    }

    // ---- the git directory and the branch ----
    let own_gitdir = root.join("mem-repo").join(".git");
    let own_gitdir = own_gitdir.canonicalize().unwrap_or(own_gitdir);
    let (gitdir, branch_ref, fetched) = match &params.source {
        MountSource::Branch { branch, gitdir } => {
            let gitdir = match gitdir {
                Some(path) => {
                    let path = if path.is_absolute() {
                        path.clone()
                    } else {
                        root.join(path)
                    };
                    path.canonicalize().unwrap_or(path)
                }
                None => own_gitdir.clone(),
            };
            require_mem_repo(&ops, &gitdir)?;
            let branch_ref = crate::workspace::branch_full_ref(branch);
            (gitdir, branch_ref, false)
        }
        MountSource::Remote { remote } => {
            require_mem_repo_dir(&own_gitdir)?;
            let branch_ref = format!("refs/heads/{}", params.name);
            fetch_mem_branch(&ops, &own_gitdir, remote, &params.name)?;
            (own_gitdir.clone(), branch_ref, true)
        }
    };
    let in_own_mem_repo = gitdir == own_gitdir;
    let tracking_ref = match &params.source {
        MountSource::Remote { remote } => Some(format!("refs/remotes/{remote}/{}", params.name)),
        MountSource::Branch { .. } => None,
    };

    // The branch tip the mount will serve.
    let existing_tip =
        (ops.resolve_ref)(&gitdir, &branch_ref).map_err(crate::EngineError::Backend)?;
    // `adopt` is set when the local branch does not exist yet and is to
    // be created at the fetched tip, which happens once nothing can
    // refuse the mount any more.
    let (head, adopt) = match (&tracking_ref, existing_tip) {
        (None, Some(tip)) => (tip, false),
        (None, None) => {
            return Err(crate::EngineError::UnknownRef(format!(
                "{branch_ref} does not exist in {}: a mount serves a branch that is already \
                 there (`memstead mem init` creates a mem, `--remote` fetches one first)",
                gitdir.display()
            ))
            .into());
        }
        (Some(tracking), existing) => {
            let fetched_tip = (ops.resolve_ref)(&gitdir, tracking)
                .map_err(crate::EngineError::Backend)?
                .ok_or_else(|| crate::EngineError::UnknownRef(tracking.clone()))?;
            match existing {
                // A local branch of that name already stands at the
                // fetched tip (an earlier mount that was unregistered):
                // it is the branch to mount.
                Some(tip) if tip == fetched_tip => (tip, false),
                Some(tip) => {
                    return Err(crate::EngineError::InvalidInput(format!(
                        "{branch_ref} already exists locally at {tip} and the remote's branch \
                         is at {fetched_tip}: mount the local branch with `--branch {}` and \
                         bring it up to date with `memstead pull`",
                        params.name
                    ))
                    .into());
                }
                None => (fetched_tip, true),
            }
        }
    };

    // ---- the registry entry and the pin ----
    // A registry keys a mem's entry by its branch, which is where the
    // mounted backend reads it from, whatever name the mount carries.
    let entry_name = crate::workspace::branch_short_name(&branch_ref).to_string();
    let own_entry = (ops.read_config_at_ref)(&gitdir, REGISTRY_REF, &entry_name)
        .map_err(crate::EngineError::Backend)?;
    let remote_entry = match (&params.source, &own_entry) {
        (MountSource::Remote { remote }, None) => {
            let tracking_registry = format!("refs/remotes/{remote}/__MEMSTEAD");
            match (ops.resolve_ref)(&gitdir, &tracking_registry) {
                Ok(Some(_)) => (ops.read_config_at_ref)(&gitdir, &tracking_registry, &entry_name)
                    .map_err(crate::EngineError::Backend)?,
                _ => None,
            }
        }
        _ => None,
    };
    let entry_pin = |bytes: &[u8]| -> Option<memstead_schema::SchemaRef> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        memstead_schema::config::parse_mem_config(&value)
            .ok()?
            .schema
    };
    let recorded_pin = own_entry
        .as_deref()
        .and_then(entry_pin)
        .or_else(|| remote_entry.as_deref().and_then(entry_pin));
    let pin = match (recorded_pin, params.schema.clone()) {
        (Some(recorded), Some(asked)) if recorded != asked => {
            return Err(crate::EngineError::InvalidInput(format!(
                "mem {:?} is pinned to {recorded} in its registry entry and the mount names \
                 {asked}: a mount never re-pins (omit `--schema`, or change the pin with \
                 `memstead mem set-schema` once the mem is mounted writable)",
                params.name
            ))
            .into());
        }
        (Some(recorded), _) => recorded,
        (None, Some(asked)) => asked,
        (None, None) => {
            return Err(crate::EngineError::InvalidInput(format!(
                "mem {:?} has no schema pin: no registry this mount reads has an entry for \
                 it, so the pin must be named. Pass `--schema <name>@<version>`, the schema \
                 the mem was written under; a mount never guesses one",
                params.name
            ))
            .into());
        }
    };

    // ---- the create rules, as for any mem a workspace gains ----
    if !params.operator_mode {
        let mut catalogue: Vec<std::sync::Arc<memstead_schema::Schema>> =
            engine.workspace_schemas().to_vec();
        catalogue.extend_from_slice(engine.builtin_schemas());
        admit_by_create_rules(
            engine,
            &params.name,
            &root.join(&params.name),
            &pin,
            &catalogue,
        )?;
    }

    // ---- the mount ----
    let mount = Mount {
        mem: params.name.clone(),
        schema: Some(pin.clone()),
        storage: MountStorage::GitBranch {
            gitdir: gitdir.clone(),
            branch: branch_ref.clone(),
        },
        capability: params.capability,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };

    // The pin must resolve here before anything is written: against
    // the built-in schemas, the ones this workspace has installed, and
    // the ones installed in the git directory that holds the branch.
    let mut schema_report = crate::ops::FullRefreshReport::default();
    engine.refresh_schema_sources_for(&mut schema_report, std::slice::from_ref(&mount));
    let resolves = engine
        .workspace_schemas()
        .iter()
        .chain(engine.builtin_schemas())
        .any(|schema| {
            let (name, version) = schema.id();
            memstead_schema::SchemaRef::new(name, version) == pin
        });
    if !resolves {
        return Err(crate::EngineError::InvalidInput(format!(
            "mem {:?} is pinned to {pin}, a schema this workspace cannot resolve \
             (SCHEMA_NOT_FOUND): it is neither built in nor installed here or in the git \
             directory that holds the branch. Install it (`memstead schema install`, or \
             `memstead pull --all` where the remote's registry carries it) and mount again; \
             nothing was mounted",
            params.name
        ))
        .into());
    }

    // ---- writes begin ----
    // The local branch of an adopted mem, at the fetched tip.
    let created_branch = adopt;
    if adopt {
        (ops.create_branch_at)(&gitdir, &params.name, &head)
            .map_err(crate::EngineError::Backend)?;
    }
    // What a failure from here on takes back: the registry entry when
    // this call wrote it, and the branch when this call created it and
    // no entry stood before (the branch and the entry go together). A
    // branch created beside an entry that already stood stays: it is
    // the state an unregister leaves, and the entry is not this call's
    // to remove.
    let entry_stood = own_entry.is_some();
    let undo_storage = |engine: &crate::Engine, config_written: bool| {
        if let Some(ops) = engine.git_branch_ops() {
            if created_branch && !entry_stood {
                let _ = (ops.prune_residue)(&gitdir, &params.name, &ctx);
            } else if config_written {
                let _ = (ops.prune_config_blob)(&gitdir, &entry_name, &ctx);
            }
        }
    };

    // The registry entry, in the workspace's own mem-repo only: a
    // branch there with no entry gets one, taken from the remote's
    // registry when that lists the mem, else carrying the named pin.
    let mut config_written = false;
    if in_own_mem_repo && own_entry.is_none() {
        let bytes = match remote_entry {
            Some(bytes) => bytes,
            None => {
                let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
                    "version": "0.1.0",
                    "schema": pin.to_string(),
                }))
                .map_err(|e| {
                    crate::EngineError::InvalidInput(format!("could not serialize mem config: {e}"))
                })?;
                bytes.push(b'\n');
                bytes
            }
        };
        let written = engine
            .instantiate_backend(&mount)
            .map_err(|e| crate::EngineError::Mem(e.to_string()))
            .and_then(|backend| {
                backend
                    .write_mem_config(&bytes, &ctx)
                    .map_err(crate::EngineError::Backend)
            });
        if let Err(e) = written {
            undo_storage(engine, false);
            return Err(e.into());
        }
        config_written = true;
    }

    // ---- the roster row, attached by the reconciliation ----
    if let Err(e) = write_roster_row(&root, Some(&mount), &params.name) {
        undo_storage(engine, config_written);
        return Err(e.into());
    }
    let attached = engine.reconcile_roster_forced();
    let serving = engine.mount(&params.name).is_some();
    if !serving {
        // The row did not attach: take it back, and say why.
        let reason = engine
            .quarantine_reason(&params.name)
            .map(|q| (q.reason_code.clone(), q.reason_message.clone()));
        let _ = write_roster_row(&root, None, &params.name);
        let _ = engine.reconcile_roster_forced();
        undo_storage(engine, config_written);
        return Err(match (attached, reason) {
            (Err(e), _) => e.into(),
            (Ok(_), Some((code, message))) => crate::EngineError::InvalidInput(format!(
                "mem {:?} did not mount ({code}): {message}",
                params.name
            ))
            .into(),
            (Ok(_), None) => crate::EngineError::InvalidInput(format!(
                "mem {:?} did not mount: the roster row was not attached",
                params.name
            ))
            .into(),
        });
    }

    let mut warnings: Vec<crate::ops::WarningHint> = Vec::new();
    if engine.settings().mutations.require_notes.unwrap_or(false)
        && params.note.as_deref().is_none_or(|n| n.trim().is_empty())
    {
        warnings.push(crate::ops::WarningHint::NoteMissing {
            tool: "memstead_mem_mount".to_string(),
        });
    }

    Ok(MemMountResponse {
        name: params.name,
        gitdir: gitdir.display().to_string(),
        branch: branch_ref,
        capability: match params.capability {
            MountCapability::Write => "write",
            MountCapability::ReadOnly => "read-only",
        },
        schema_ref: pin.to_string(),
        head,
        fetched,
        config_written,
        warnings,
    })
}

/// A git directory that is a mem-repo: a directory that carries the
/// registry ref.
fn require_mem_repo(
    ops: &crate::engine::GitBranchOps,
    gitdir: &Path,
) -> Result<(), FullEngineError> {
    require_mem_repo_dir(gitdir)?;
    let registry = (ops.resolve_ref)(gitdir, REGISTRY_REF).unwrap_or(None);
    if registry.is_none() {
        return Err(crate::EngineError::InvalidInput(format!(
            "{} is not a mem-repo: it carries no {REGISTRY_REF} (a mount serves a mem of a \
             mem-repo; name the mem-repo's git directory, usually `<workspace>/mem-repo/.git`)",
            gitdir.display()
        ))
        .into());
    }
    Ok(())
}

fn require_mem_repo_dir(gitdir: &Path) -> Result<(), FullEngineError> {
    if !gitdir.is_dir() {
        return Err(crate::EngineError::InvalidInput(format!(
            "{} is not a git directory: a mount serves a branch of a mem-repo (pass \
             `--gitdir <path>` for a mem that lives in another workspace's mem-repo, or run \
             `memstead mem-repo init` to give this workspace its own)",
            gitdir.display()
        ))
        .into());
    }
    Ok(())
}

/// Fetch a mem's branch, and the remote's registry when it has one,
/// into the remote-tracking refs of `gitdir`.
fn fetch_mem_branch(
    ops: &crate::engine::GitBranchOps,
    gitdir: &Path,
    remote: &str,
    mem: &str,
) -> Result<(), FullEngineError> {
    let lift = |e: crate::backend::BackendError| -> FullEngineError {
        if crate::EngineError::is_remote_marker(&e) {
            crate::EngineError::from_transport(e).into()
        } else {
            crate::EngineError::Backend(e).into()
        }
    };
    let heads = (ops.ls_remote)(gitdir, remote).map_err(lift)?;
    let branch_ref = format!("refs/heads/{mem}");
    if !heads.iter().any(|(name, _)| name == &branch_ref) {
        return Err(crate::EngineError::UnknownRef(format!(
            "remote {remote:?} has no branch {branch_ref}: the mem {mem:?} is not on that remote"
        ))
        .into());
    }
    let mut refspecs = vec![format!("+{branch_ref}:refs/remotes/{remote}/{mem}")];
    if heads.iter().any(|(name, _)| name == REGISTRY_REF) {
        refspecs.push(format!("+{REGISTRY_REF}:refs/remotes/{remote}/__MEMSTEAD"));
    }
    (ops.fetch)(gitdir, remote, &refspecs).map_err(lift)?;
    Ok(())
}

/// Write the roster with the row of `name` set to `row` (or removed),
/// every other row as the file holds it. The write is compare-and-swap
/// against the bytes read, so a row another process wrote meanwhile is
/// never lost.
fn write_roster_row(
    root: &Path,
    row: Option<&Mount>,
    name: &str,
) -> Result<(), crate::EngineError> {
    let store = crate::FileWorkspaceStore::new();
    let failed = |e: crate::workspace_store::StoreError| {
        crate::EngineError::Mem(format!("mount roster: {e}"))
    };
    for _ in 0..8 {
        let before = store.read_state_bytes(root).map_err(failed)?;
        let mut workspace = store.load(root).map_err(failed)?;
        workspace.mounts.retain(|m| m.mem != name);
        if let Some(row) = row {
            workspace.mounts.push(row.clone());
        }
        if store
            .save_state_cas(root, &workspace, before.as_deref())
            .map_err(failed)?
        {
            return Ok(());
        }
    }
    Err(crate::EngineError::Mem(
        "mount roster: the file kept changing under the write; nothing was mounted".to_string(),
    ))
}
