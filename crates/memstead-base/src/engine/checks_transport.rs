//! The check ledger's transport between machines.
//!
//! The ledger is one append-only file per workspace
//! (`.memstead/state/checks/checks.jsonl`), and `memstead check` writes
//! no commit, which is what keeps staleness derivable by hash. So the
//! ledger does not ride a mem's branch, and a clone that pulled the
//! branch had no check records to export: its sealed archives carried
//! none. This module gives the ledger a transport, not a new home.
//!
//! Every mem-repo carries one more engine-owned branch beside
//! `__MEMSTEAD`: [`crate::MEMSTEAD_CHECKS_REF_BRANCH`], a tree with
//! `mems/<mem>/checks.jsonl` per mem, the mem's ledger rows in their
//! transport form ([`CheckLedger::transport_lines`]). A push publishes
//! the mem's rows there and pushes the ref beside the branch; a fetch
//! or pull fetches the ref and unions the mem's rows into the local
//! ledger ([`CheckLedger::merge_transport_lines`]). A row is a fact
//! whose identity is its bytes: two machines checking the same mem
//! merge by set union, nothing is rewritten, and a round trip adds
//! nothing. The local branch of that name is a vehicle: before a
//! publish it is pointed at the remote's tip, so the commit lands on
//! top of what the remote holds and pushes fast-forward; a race with
//! another publisher is a non-fast-forward the publish retries once
//! after importing what landed.
//!
//! The trust rule: rows arrive only through `fetch`, `pull` and `push`
//! of a mem this workspace mounts, and only that mem's rows. A fork's
//! ref is never imported: `mem fork --remote` in either form fetches
//! the branch it names and nothing else, so a contributor cannot push
//! checker-role rows that close the owner's gates. Imported rows keep
//! their identity and role, and read exactly as local rows do
//! (independence from the branch's provenance, origin class from the
//! declared owner identities).

use std::path::Path;

use crate::backend::BackendError;
use crate::check::CheckLedger;
use crate::engine::{Engine, EngineError, GitBranchOps};
use crate::workspace::{MountCapability, MountLifecycle, MountStorage, branch_full_ref};

/// The transport ref's tracking ref for `remote`.
fn tracking_ref(remote: &str) -> String {
    format!(
        "refs/remotes/{remote}/{}",
        crate::MEMSTEAD_CHECKS_REF_BRANCH
    )
}

fn lift_remote(e: BackendError) -> EngineError {
    match e {
        BackendError::Other(msg) if msg.starts_with("UNKNOWN_REMOTE:") => {
            EngineError::UnknownRemote(msg.trim_start_matches("UNKNOWN_REMOTE:").trim().to_string())
        }
        other => EngineError::Backend(other),
    }
}

impl Engine {
    /// A backend over `ref_name` in `gitdir`, for reading and writing
    /// the transport tree. Never mounted, never registered.
    fn checks_backend(
        &self,
        gitdir: &Path,
        ref_name: &str,
    ) -> Result<Box<dyn crate::backend::MemBackend>, EngineError> {
        let mount = crate::workspace::Mount {
            mem: crate::MEMSTEAD_CHECKS_REF_BRANCH.to_string(),
            schema: None,
            storage: MountStorage::GitBranch {
                gitdir: gitdir.to_path_buf(),
                branch: ref_name.to_string(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        (self.backend_factory())(&mount)
            .map_err(|e| EngineError::Mem(format!("checks transport backend: {e}")))
    }

    /// The remote's `__MEMSTEAD_CHECKS` tip, when it carries one.
    fn remote_checks_tip(
        &self,
        hook: &GitBranchOps,
        gitdir: &Path,
        remote: &str,
    ) -> Result<Option<String>, EngineError> {
        let wanted = branch_full_ref(crate::MEMSTEAD_CHECKS_REF_BRANCH);
        Ok((hook.ls_remote)(gitdir, remote)
            .map_err(lift_remote)?
            .into_iter()
            .find(|(name, _)| *name == wanted)
            .map(|(_, sha)| sha))
    }

    /// Fetch the remote's transport ref (when it has one) and union the
    /// rows of `mems` into the workspace ledger. Returns the number of
    /// rows appended. A workspace without a root has no ledger and
    /// imports nothing.
    pub(crate) fn import_checks(
        &self,
        hook: &GitBranchOps,
        gitdir: &Path,
        remote: &str,
        mems: &[&str],
    ) -> Result<usize, EngineError> {
        let Some(root) = self.workspace_root() else {
            return Ok(0);
        };
        if self.remote_checks_tip(hook, gitdir, remote)?.is_none() {
            return Ok(0);
        }
        let tracking = tracking_ref(remote);
        (hook.fetch)(
            gitdir,
            remote,
            &[format!(
                "+{}:{tracking}",
                branch_full_ref(crate::MEMSTEAD_CHECKS_REF_BRANCH)
            )],
        )
        .map_err(lift_remote)?;
        let backend = self.checks_backend(gitdir, &tracking)?;
        let ledger = CheckLedger::for_workspace(root);
        let mut appended = 0;
        for mem in mems {
            let path = crate::checks_ref_member_path(mem);
            if let Some(bytes) = backend.read_entity(Path::new(&path))? {
                appended += ledger.merge_transport_lines(&bytes).map_err(|e| {
                    EngineError::Mem(format!("check ledger: appending imported rows: {e}"))
                })?;
            }
        }
        Ok(appended)
    }

    /// Publish the ledger rows of `mems` on the transport ref and push
    /// it to `remote`. Imports the remote's rows first, so nothing
    /// another machine published is ever dropped, then writes each
    /// mem's rows on top of the remote's tip and pushes fast-forward;
    /// a race is retried once. Returns the ref's sha when the push
    /// moved it, `None` when the remote already held the same rows (or
    /// the workspace has no ledger).
    pub(crate) fn publish_checks(
        &self,
        hook: &GitBranchOps,
        gitdir: &Path,
        remote: &str,
        mems: &[&str],
    ) -> Result<Option<String>, EngineError> {
        let Some(root) = self.workspace_root() else {
            return Ok(None);
        };
        let ledger = CheckLedger::for_workspace(root);
        let local_ref = branch_full_ref(crate::MEMSTEAD_CHECKS_REF_BRANCH);
        let mut last_err: Option<EngineError> = None;
        for _attempt in 0..2 {
            let imported = self.import_checks(hook, gitdir, remote, mems)?;
            let _ = imported;
            // The vehicle: on top of the remote's tip when it has one.
            let remote_tip = self.remote_checks_tip(hook, gitdir, remote)?;
            if let Some(tip) = &remote_tip {
                (hook.update_ref)(gitdir, &local_ref, tip).map_err(EngineError::Backend)?;
            }
            let backend = self.checks_backend(gitdir, &local_ref)?;
            let mut changed = false;
            for mem in mems {
                let path = crate::checks_ref_member_path(mem);
                let bytes = CheckLedger::transport_lines(&ledger.records_for_mem(mem));
                let current = backend.read_entity(Path::new(&path))?;
                if bytes.is_empty() && current.is_none() {
                    continue;
                }
                if current.as_deref() == Some(bytes.as_slice()) {
                    continue;
                }
                backend.write_entity(Path::new(&path), &bytes)?;
                changed = true;
            }
            let local_tip = (hook.resolve_ref)(gitdir, &local_ref).map_err(EngineError::Backend)?;
            if !changed && (local_tip.is_none() || local_tip == remote_tip) {
                return Ok(None);
            }
            if changed {
                let ctx = crate::vcs::CommitContext::new(
                    Some("memstead_push"),
                    crate::vcs::Actor::Cli,
                    None,
                    None,
                    self.current_role(),
                    self.current_identity().map(str::to_string),
                );
                let subject = format!("memstead: checks {}", mems.join(", "));
                backend.commit(&subject, &ctx)?;
            }
            match (hook.push)(
                gitdir,
                remote,
                crate::MEMSTEAD_CHECKS_REF_BRANCH,
                crate::MEMSTEAD_CHECKS_REF_BRANCH,
                false,
            ) {
                Ok(outcome) => return Ok(Some(outcome.new_sha)),
                Err(BackendError::Other(msg)) if msg.starts_with("NON_FAST_FORWARD:") => {
                    // Another publisher landed first: import what it
                    // wrote and write ours on top of it.
                    last_err = Some(EngineError::NonFastForward {
                        mem: crate::MEMSTEAD_CHECKS_REF_BRANCH.to_string(),
                        remote: remote.to_string(),
                    });
                    continue;
                }
                Err(e) => return Err(lift_remote(e)),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            EngineError::Mem("check ledger publish: no attempt ran".to_string())
        }))
    }
}
