//! Shared by the test files that run an operation against a second
//! writer: a backend wrapper that reports, and can fail, every storage
//! call, and a probe of a mem's write lock.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use memstead_base::Provenance;
use memstead_base::backend::{BackendError, CompletedCommit, EntityRead, MemBackend};
use memstead_base::storage::CommitId;
use memstead_base::vcs::CommitContext;

// ---------------------------------------------------------------------
// The tap: a backend wrapper that reports every call before it runs
// ---------------------------------------------------------------------

/// Called with the mem and the storage method's name before the method
/// runs, on the thread that runs it. `true` fails the call in place of
/// running it: a storage error at that step.
pub type Tap = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

pub struct Tapped {
    pub inner: Box<dyn MemBackend>,
    pub mem: &'static str,
    pub tap: Tap,
}

impl Tapped {
    fn on(&self, method: &str) -> Result<(), BackendError> {
        if (self.tap)(self.mem, method) {
            Err(BackendError::Other(format!(
                "injected failure of {method} on {}",
                self.mem
            )))
        } else {
            Ok(())
        }
    }
}

/// The storage methods that land a write.
pub fn lands(method: &str) -> bool {
    matches!(
        method,
        "commit"
            | "commit_with_expected_parent"
            | "append_provenance"
            | "write_mem_config"
            | "write_mem_config_cas"
            | "record_pipeline_edit"
            | "delete_artifacts"
    )
}

impl MemBackend for Tapped {
    fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
        self.on("list_entities")?;
        self.inner.list_entities()
    }
    fn read_entity(&self, rel_path: &Path) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_entity")?;
        self.inner.read_entity(rel_path)
    }
    fn entity_exists(&self, rel_path: &Path) -> Result<bool, BackendError> {
        self.on("entity_exists")?;
        self.inner.entity_exists(rel_path)
    }
    fn read_all_entities(&self) -> Result<Vec<EntityRead>, BackendError> {
        self.on("read_all_entities")?;
        self.inner.read_all_entities()
    }
    fn storage_present(&self) -> Result<bool, BackendError> {
        self.on("storage_present")?;
        self.inner.storage_present()
    }
    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), BackendError> {
        self.on("write_entity")?;
        self.inner.write_entity(rel_path, content)
    }
    fn delete_entity(&self, rel_path: &Path) -> Result<(), BackendError> {
        self.on("delete_entity")?;
        self.inner.delete_entity(rel_path)
    }
    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), BackendError> {
        self.on("move_entity")?;
        self.inner.move_entity(from, to)
    }
    fn discard_pending(&self) -> Result<(), BackendError> {
        self.on("discard_pending")?;
        self.inner.discard_pending()
    }
    fn commit(&self, message: &str, ctx: &CommitContext<'_>) -> Result<CommitId, BackendError> {
        self.on("commit")?;
        self.inner.commit(message, ctx)
    }
    fn commit_with_expected_parent(
        &self,
        message: &str,
        ctx: &CommitContext<'_>,
        expected_parent: Option<&str>,
    ) -> Result<CommitId, BackendError> {
        self.on("commit_with_expected_parent")?;
        self.inner
            .commit_with_expected_parent(message, ctx, expected_parent)
    }
    fn stage_provenance(&self, record: &Provenance) -> Result<(), BackendError> {
        self.on("stage_provenance")?;
        self.inner.stage_provenance(record)
    }
    fn append_provenance(&self, record: &Provenance) -> Result<(), BackendError> {
        self.on("append_provenance")?;
        self.inner.append_provenance(record)
    }
    fn complete_interrupted_commit(&self) -> Result<Option<CompletedCommit>, BackendError> {
        self.on("complete_interrupted_commit")?;
        self.inner.complete_interrupted_commit()
    }
    fn read_provenance(&self, cursor: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
        self.on("read_provenance")?;
        self.inner.read_provenance(cursor)
    }
    fn current_head(&self) -> Result<Option<String>, BackendError> {
        self.on("current_head")?;
        self.inner.current_head()
    }
    fn write_lock_path(&self) -> Option<PathBuf> {
        let _ = self.on("write_lock_path");
        self.inner.write_lock_path()
    }
    fn read_mem_config(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_mem_config")?;
        self.inner.read_mem_config()
    }
    fn read_archive_provenance(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_archive_provenance")?;
        self.inner.read_archive_provenance()
    }
    fn read_archive_checks(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_archive_checks")?;
        self.inner.read_archive_checks()
    }
    fn read_proposal_record(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_proposal_record")?;
        self.inner.read_proposal_record()
    }
    fn write_mem_config(&self, bytes: &[u8], ctx: &CommitContext<'_>) -> Result<(), BackendError> {
        self.on("write_mem_config")?;
        self.inner.write_mem_config(bytes, ctx)
    }
    fn write_mem_config_cas(
        &self,
        expected: Option<&[u8]>,
        bytes: &[u8],
        ctx: &CommitContext<'_>,
    ) -> Result<bool, BackendError> {
        self.on("write_mem_config_cas")?;
        self.inner.write_mem_config_cas(expected, bytes, ctx)
    }
    fn record_pipeline_edit(
        &self,
        kind: &str,
        edits: &[(String, Option<Vec<u8>>)],
        ctx: &CommitContext<'_>,
        verb: &str,
    ) -> Result<(), BackendError> {
        self.on("record_pipeline_edit")?;
        self.inner.record_pipeline_edit(kind, edits, ctx, verb)
    }
    fn read_anchors_sidecar(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_anchors_sidecar")?;
        self.inner.read_anchors_sidecar()
    }
    fn write_anchors_sidecar(&self, bytes: &[u8]) -> Result<(), BackendError> {
        self.on("write_anchors_sidecar")?;
        self.inner.write_anchors_sidecar(bytes)
    }
    fn delete_artifacts(&self, ctx: &CommitContext<'_>) -> Result<(), BackendError> {
        self.on("delete_artifacts")?;
        self.inner.delete_artifacts(ctx)
    }
}

/// Whether nobody holds the lock at `path`, asked through an open file
/// of this probe's own: it conflicts with every holder, a holder on the
/// calling thread included.
pub fn lock_is_free(path: &Path) -> bool {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    else {
        // No lock file yet: nobody ever took the lock.
        return true;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            true
        }
        Err(_) => false,
    }
}
