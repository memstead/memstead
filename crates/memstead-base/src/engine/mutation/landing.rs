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
//!   and whose files are not all in place yet. The engine completes it
//!   at once and reports it as committed with a failed follow-up; if
//!   the completion fails as well, the call returns
//!   `COMMIT_NOT_APPLIED` and the next load of the mem completes it.
//!
//! Every mutation verb commits through [`Engine::commit_staged`], so
//! the three reports are made in one place. The same place refuses a
//! write whose mem could not be reloaded before the operation: a view
//! that is known to be behind storage serves reads, never writes.

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
        // The reload before this operation failed, so everything staged
        // here was computed from a view that is behind storage, and its
        // hash compare ran against content that may have moved on.
        // Landing it would overwrite what the view has not seen. The
        // next operation retries the reload.
        if let Some(reason) = &state.reload_failure {
            let _ = backend.discard_pending();
            return Err(EngineError::StaleView {
                mem: state.mount.mem.clone(),
                detail: reason.clone(),
            });
        }
        let staged = records
            .iter()
            .try_for_each(|record| backend.stage_provenance(record));
        let committed = staged
            .and_then(|()| backend.commit_with_expected_parent(subject, ctx, expected_parent));
        self.settle(mount_idx, committed)
    }

    /// Append `record` on its own, for a mutation that writes no file
    /// (the delete of a stub). The record is the whole write, and it
    /// gets the same three reports a commit gets.
    pub(in crate::engine) fn append_record(
        &self,
        mount_idx: usize,
        record: &Provenance,
    ) -> Result<Landed, EngineError> {
        let state = &self.mounts[mount_idx];
        if let Some(reason) = &state.reload_failure {
            return Err(EngineError::StaleView {
                mem: state.mount.mem.clone(),
                detail: reason.clone(),
            });
        }
        let appended = state
            .backend
            .append_provenance(record)
            .map(|()| String::new());
        self.settle(mount_idx, appended)
    }

    /// For an operation over several mems: one more attempt at a
    /// commit that came back decided and not applied. A verb on one mem
    /// can return `COMMIT_NOT_APPLIED` and has told its caller
    /// everything; an operation over several mems would stop there
    /// with the other mems' parts unwritten, and report a commit that
    /// is in storage. So the completion is tried once more here: when
    /// it goes through, the commit is landed and the operation goes
    /// on, with the failed step as a warning.
    pub(in crate::engine) fn commit_or_complete(
        &self,
        mount_idx: usize,
        committed: Result<Landed, EngineError>,
    ) -> Result<Landed, EngineError> {
        let (mem, detail) = match committed {
            Err(EngineError::CommitNotApplied { mem, detail }) => (mem, detail),
            other => return other,
        };
        match self.mounts[mount_idx].backend.complete_interrupted_commit() {
            Ok(_) => Ok(Landed {
                write_id: crate::storage::filesystem::make_commit_id(),
                warnings: vec![WarningHint::WriteFollowUpFailed {
                    mem,
                    step: "commit_apply".to_string(),
                    detail,
                }],
            }),
            Err(again) => Err(EngineError::CommitNotApplied {
                mem,
                detail: format!("{detail}; and once more: {again}"),
            }),
        }
    }

    /// The report for what a commit call came to.
    fn settle(
        &self,
        mount_idx: usize,
        committed: Result<String, BackendError>,
    ) -> Result<Landed, EngineError> {
        let state = &self.mounts[mount_idx];
        let backend = state.backend.as_ref();
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
            // Decided and cut off. The write is the commit's from here on,
            // so the engine finishes it at once rather than leave its
            // view behind storage until the next operation: the same
            // completion a load runs, under the write lock this
            // mutation holds. Only when that fails too (the fault
            // persists) does the caller get the not-applied condition.
            Err(BackendError::CommitNotApplied { detail }) => {
                match backend.complete_interrupted_commit() {
                    Ok(_) => Ok(Landed {
                        write_id: crate::storage::filesystem::make_commit_id(),
                        warnings: vec![WarningHint::WriteFollowUpFailed {
                            mem: state.mount.mem.clone(),
                            step: "commit_apply".to_string(),
                            detail,
                        }],
                    }),
                    Err(again) => Err(EngineError::CommitNotApplied {
                        mem: state.mount.mem.clone(),
                        detail: format!("{detail}; completing it at once failed as well: {again}"),
                    }),
                }
            }
            Err(e) => {
                // Nothing landed. The backends end their own
                // transaction on a failed commit; a failure that struck
                // before the backend's commit ran (a refused record, a
                // wrapping backend) left the set staged, so drop it
                // here as well. The write lock tries once more when it
                // is released and remembers a backend it could not
                // clear.
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
                        // The view stays behind the write that just
                        // landed, and the write is already recorded as
                        // the engine's own, so no drift would ever
                        // reload it: mark the mem, so it takes no write
                        // and is reloaded before the next operation.
                        self.note_reload_failure(&mem, &reload);
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

    /// The report for a batch whose commit into `failed_mount` failed.
    /// With nothing in storage the batch failed as a whole and `cause`
    /// is the report, and so it is for a batch that writes one mem.
    /// With a commit in storage the report is the partial-failure
    /// envelope: for every mem the batch writes, in commit order,
    /// whether its commit is in storage, and for every entity the
    /// batch writes there, whether it landed and its hash now. Sending
    /// the items it names as not landed completes the batch. The
    /// caller has rolled the store back; the mems with a commit in
    /// storage are reloaded here, so the hashes are read from a view
    /// that is level with it.
    pub(in crate::engine) fn batch_commit_failed(
        &mut self,
        mounts: &[usize],
        items: &[(usize, EntityId)],
        committed: Vec<(usize, String)>,
        failed_mount: usize,
        cause: EngineError,
    ) -> EngineError {
        use crate::engine::error::{PartialItem, PartialMem};
        let in_storage = with_a_decided_commit(committed, failed_mount, &cause);
        if in_storage.is_empty() || mounts.len() < 2 {
            return cause;
        }
        self.level_committed_mounts(&in_storage);
        let name = |mount_idx: usize| self.mounts[mount_idx].mount.mem.clone();
        let mems = mounts
            .iter()
            .map(|&mount_idx| {
                let landed = in_storage.iter().any(|(idx, _)| *idx == mount_idx);
                let mut ids: Vec<&EntityId> = items
                    .iter()
                    .filter(|(idx, _)| *idx == mount_idx)
                    .map(|(_, id)| id)
                    .collect();
                ids.dedup();
                PartialMem {
                    mem: name(mount_idx),
                    landed,
                    error: (mount_idx == failed_mount).then(|| cause.to_string()),
                    items: ids
                        .into_iter()
                        .map(|id| PartialItem {
                            id: id.to_string(),
                            landed,
                            current_hash: self.hash_now(id),
                        })
                        .collect(),
                }
            })
            .collect();
        EngineError::RenamePartialFailure {
            committed_mems: in_storage.iter().map(|(idx, _)| name(*idx)).collect(),
            failed_mem: name(failed_mount),
            failure_cause: failure_cause_of(&cause).to_string(),
            mems,
        }
    }

    /// The content hash the entity has now, for a partial-failure
    /// report. `None` when it does not exist, and when its mem's view
    /// could not be brought level with storage: a hash from a view
    /// that is known to be behind is not the entity's hash now.
    pub(in crate::engine) fn hash_now(&self, id: &EntityId) -> Option<String> {
        let stale = self
            .mounts
            .iter()
            .any(|m| m.mount.mem == id.mem() && m.reload_failure.is_some());
        if stale {
            return None;
        }
        self.store
            .get(id)
            .filter(|e| !e.stub)
            .map(|e| e.content_hash.clone())
    }

    /// Bring the engine's view level with storage for mems whose commit
    /// is in storage inside an operation that then failed on another
    /// mem: the operation's store rollback, or the store update it
    /// never reached, left the view behind what those commits wrote. A
    /// commit with an id is recorded as the engine's own; then the mem
    /// is reloaded, which also completes a commit that was decided and
    /// not applied. A reload that fails is tried once more, because a
    /// fault that passes should not leave a report without its hashes;
    /// one that fails again marks the mem, whose view then takes no
    /// write, and the next operation retries.
    pub(in crate::engine) fn level_committed_mounts(&mut self, commits: &[(usize, String)]) {
        for (mount_idx, write_id) in commits {
            if !write_id.is_empty() {
                self.record_self_write(*mount_idx, write_id);
            }
            let mem = self.mounts[*mount_idx].mount.mem.clone();
            let reloaded = self
                .reload_one_mem(&mem)
                .or_else(|_| self.reload_one_mem(&mem));
            if let Err(e) = reloaded {
                self.note_reload_failure(&mem, &e);
                tracing::warn!(
                    mem = %mem,
                    error = %e,
                    "reload after a partly failed operation did not complete; \
                     the next operation retries"
                );
            }
        }
    }
}

/// The mems of an operation whose commit is in storage: the ones that
/// committed, and the failed one too when its failure is
/// `COMMIT_NOT_APPLIED`. That commit is decided, its record is durable,
/// and the next load of the mem applies it; reporting it as not landed
/// would have the caller send again what storage already holds.
pub(in crate::engine) fn with_a_decided_commit(
    mut committed: Vec<(usize, String)>,
    failed_mount: usize,
    cause: &EngineError,
) -> Vec<(usize, String)> {
    if matches!(cause, EngineError::CommitNotApplied { .. }) {
        committed.push((failed_mount, String::new()));
    }
    committed
}

/// The partial-failure report's word for why a mem's part failed.
pub(in crate::engine) fn failure_cause_of(cause: &EngineError) -> &'static str {
    match cause {
        EngineError::Backend(BackendError::ParentMismatch { .. }) => "drift",
        _ => "storage",
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

#[cfg(test)]
mod decided_commit_tests;
