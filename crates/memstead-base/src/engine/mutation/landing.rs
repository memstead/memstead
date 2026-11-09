//! The commit point of a mutation and the steps that follow it.
//!
//! A mutation of one mem either commits or does not, and the caller is
//! told which. Three outcomes of the commit call exist, and each has
//! one report:
//!
//! - **Not committed.** The staged writes are dropped and the error
//!   goes to the caller. Storage and the backend are as they were, so
//!   the same call sent again is a first call.
//! - **Committed.** The write is in storage. A step that fails after
//!   this point (the backend's own follow-up, the engine's store
//!   update) is never reported as a failed write: the caller would
//!   resend a write that already landed. The engine brings its view
//!   level with storage by reloading the mem and reports success with
//!   a `WRITE_FOLLOW_UP_FAILED` warning that names the step.
//! - **Decided, not applied.** A folder commit whose record is durable
//!   and whose files are not all in place yet. Reported as
//!   `COMMIT_NOT_APPLIED`; the next load of the mem completes it.
//!
//! Every mutation verb commits through [`Engine::commit_staged`], so
//! the three reports are made in one place.

use crate::backend::BackendError;
use crate::entity::EntityId;
use crate::ops::WarningHint;
use crate::provenance::{Provenance, ProvenanceKind};
use crate::vcs::{Actor, ClientId, CommitContext};

use crate::engine::{Engine, EngineError};

/// A commit that is in storage: its id, and the warning for a
/// follow-up step of the backend that failed after it landed.
pub(in crate::engine) struct Landed {
    pub(in crate::engine) write_id: String,
    pub(in crate::engine) warnings: Vec<WarningHint>,
}

impl Engine {
    /// The provenance record of one mutation, stamped with the
    /// session's declared role and identity.
    pub(in crate::engine) fn provenance_record(
        &self,
        kind: ProvenanceKind,
        entity: &EntityId,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Provenance {
        Provenance::new(
            std::time::SystemTime::now(),
            kind,
            Some(entity.to_string()),
            actor,
            client.cloned(),
            note.map(String::from),
        )
        .with_role(self.current_role)
        .with_identity(self.current_identity.clone())
    }

    /// Commit what is staged on `mount_idx`, with `records` as the
    /// commit's provenance. The records are staged first so they land
    /// with the commit and never without it.
    pub(in crate::engine) fn commit_staged(
        &self,
        mount_idx: usize,
        subject: &str,
        ctx: &CommitContext<'_>,
        records: &[Provenance],
    ) -> Result<Landed, EngineError> {
        self.commit_staged_pinned(mount_idx, subject, ctx, records, None)
    }

    /// [`Self::commit_staged`] with the backend's parent pin
    /// ([`crate::backend::MemBackend::commit_with_expected_parent`]).
    pub(in crate::engine) fn commit_staged_pinned(
        &self,
        mount_idx: usize,
        subject: &str,
        ctx: &CommitContext<'_>,
        records: &[Provenance],
        expected_parent: Option<&str>,
    ) -> Result<Landed, EngineError> {
        let state = &self.mounts[mount_idx];
        let backend = state.backend.as_ref();
        let staged = records
            .iter()
            .try_for_each(|record| backend.stage_provenance(record));
        let committed = staged
            .and_then(|()| backend.commit_with_expected_parent(subject, ctx, expected_parent));
        match committed {
            Ok(write_id) => Ok(Landed {
                write_id,
                warnings: Vec::new(),
            }),
            Err(BackendError::FollowUpFailed {
                commit,
                step,
                detail,
            }) => Ok(Landed {
                write_id: commit,
                warnings: vec![WarningHint::WriteFollowUpFailed {
                    mem: state.mount.mem.clone(),
                    step: step.to_string(),
                    detail,
                }],
            }),
            Err(BackendError::CommitNotApplied { detail }) => Err(EngineError::CommitNotApplied {
                mem: state.mount.mem.clone(),
                detail,
            }),
            Err(e) => {
                // Nothing landed. The backends end their own
                // transaction on a failed commit; a failure that struck
                // before the backend's commit ran (a refused record, a
                // wrapping backend) left the set staged, so drop it
                // here as well.
                let _ = backend.discard_pending();
                Err(e.into())
            }
        }
    }

    /// Run `step`, a part of a mutation that follows its landed commit.
    /// `None` means the step failed: the write is in storage all the
    /// same, so the mem is reloaded from storage (the engine's view is
    /// then level with what was written) and the failure is recorded in
    /// `warnings` under `name`. The caller reads what it reports from
    /// the reloaded store.
    pub(in crate::engine) fn after_commit<T>(
        &mut self,
        mount_idx: usize,
        name: &'static str,
        warnings: &mut Vec<WarningHint>,
        step: impl FnOnce(&mut Self) -> Result<T, EngineError>,
    ) -> Option<T> {
        self.after_commits(&[mount_idx], name, warnings, step)
    }

    /// [`Self::after_commit`] for a step that follows the commits of
    /// several mems (a batch): every one of them is reloaded when the
    /// step fails.
    pub(in crate::engine) fn after_commits<T>(
        &mut self,
        mounts: &[usize],
        name: &'static str,
        warnings: &mut Vec<WarningHint>,
        step: impl FnOnce(&mut Self) -> Result<T, EngineError>,
    ) -> Option<T> {
        #[cfg(test)]
        let step = |engine: &mut Self| {
            if test_fault::take() {
                // The step is skipped whole, the hardest case for the
                // recovery: the store has not seen the write at all.
                return Err(EngineError::ParseAfterWrite(
                    "injected follow-up failure".to_string(),
                ));
            }
            step(engine)
        };
        match step(self) {
            Ok(value) => Some(value),
            Err(e) => {
                let mems: Vec<String> = mounts
                    .iter()
                    .map(|&m| self.mounts[m].mount.mem.clone())
                    .collect();
                for mem in mems {
                    let mut detail = e.to_string();
                    if let Err(reload) = self.reload_one_mem(&mem) {
                        detail.push_str(&format!(
                            "; reloading the mem from storage failed as well: {reload}"
                        ));
                    }
                    warnings.push(WarningHint::WriteFollowUpFailed {
                        mem,
                        step: name.to_string(),
                        detail,
                    });
                }
                None
            }
        }
    }

    /// Bring the engine's view level with storage for mems whose commit
    /// landed inside an operation that then failed on another mem: the
    /// operation's store rollback put the view behind what those
    /// commits wrote. Each commit is recorded as the engine's own, then
    /// its mem is reloaded.
    pub(in crate::engine) fn level_committed_mounts(&mut self, commits: &[(usize, String)]) {
        for (mount_idx, write_id) in commits {
            self.record_self_write(*mount_idx, write_id);
            let mem = self.mounts[*mount_idx].mount.mem.clone();
            if let Err(e) = self.reload_one_mem(&mem) {
                tracing::warn!(
                    mem = %mem,
                    error = %e,
                    "reload after a partly failed batch did not complete; \
                     the next operation retries"
                );
            }
        }
    }
}

/// Test seam: fail the next step that follows a landed commit, on this
/// thread. A store update cannot be made to fail from outside (it
/// re-parses bytes the engine composed and validated itself), so the
/// tests that prove the recovery ask for the failure here.
#[cfg(test)]
pub(in crate::engine) mod test_fault {
    use std::cell::Cell;

    thread_local! {
        static ARMED: Cell<bool> = const { Cell::new(false) };
    }

    pub(in crate::engine) fn arm() {
        ARMED.with(|a| a.set(true));
    }

    pub(super) fn take() -> bool {
        ARMED.with(|a| a.replace(false))
    }
}

#[cfg(test)]
mod tests;
