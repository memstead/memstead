//! In-memory [`MemBackend`](crate::backend::MemBackend) — the
//! writable, filesystem- and git-free companion to the folder backend.
//!
//! The mem lives entirely in RAM: committed entity bytes, the staged
//! mutation buffer, the provenance log, and the per-mem config all
//! sit on one [`Mutex`]-guarded [`State`]. Creating a backend
//! provisions nothing on disk; dropping it releases the mem with no
//! residue to clean up. Built to serve ephemeral per-session
//! playground mems the session server spins up and tears down on
//! demand.
//!
//! ## Why model on the folder backend
//!
//! The folder backend ([`super::filesystem::FilesystemMemWriter`]) is
//! the closest sibling: it buffers mutations until commit, mints a
//! synthetic history-free commit-id, and keeps a sidecar provenance
//! log. This backend mirrors that contract one-for-one — same
//! buffer-then-commit semantics, same `read`-sees-pending ordering,
//! same path-rejection rules (it reuses
//! [`super::filesystem::normalise_rel_path`]), same synthetic commit-id
//! ([`super::filesystem::make_commit_id`]) — so the engine produces
//! identical engine-level outcomes regardless of which of the two
//! serves a mount. The only difference is the substrate: a `HashMap`
//! in RAM instead of a directory tree on disk.
//!
//! ## No durable history
//!
//! Like the folder backend, this one has no commit history:
//! [`MemBackend::current_head`](crate::backend::MemBackend::current_head)
//! inherits the trait default returning `Ok(None)` rather than
//! fabricating a git-like head, and
//! `commit_with_expected_parent` inherits the default that ignores the
//! pin and delegates to [`commit`](crate::backend::MemBackend::commit).
//! Optimistic locking is unaffected — the engine enforces the
//! `expected_hash` CAS above the backend, so a stale hash trips the
//! same `HASH_MISMATCH` here as on any other backend.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::CommitId;
use super::filesystem::{make_commit_id, normalise_rel_path};
use crate::backend::{BackendError, MemBackend};
use crate::filesystem::changelog::format_rfc3339_utc;
use crate::provenance::Provenance;
use crate::vcs::CommitContext;

/// Per-path terminal state for the staged op buffer. Move resolves at
/// call time into a `Delete(from)` + `Upsert(to, bytes)` pair, so
/// commit-time replay only ever sees these two states — mirroring the
/// folder backend's [`super::filesystem`] buffer shape.
enum PendingState {
    Upsert(Vec<u8>),
    Delete,
}

/// Every byte the mem holds, behind one lock. Committed bytes are
/// the in-RAM analogue of the folder backend's on-disk directory; the
/// pending buffer folds into them on [`commit`](MemBackend::commit).
#[derive(Default)]
struct State {
    /// Committed entity bytes keyed by normalised mem-relative path
    /// (forward-slash). The in-RAM stand-in for the folder backend's
    /// directory tree.
    committed: HashMap<String, Vec<u8>>,
    /// Staged mutations not yet committed. Cleared on commit or
    /// [`discard_pending`](MemBackend::discard_pending).
    pending: HashMap<String, PendingState>,
    /// Append-only provenance log. The folder backend persists this as
    /// JSONL under `.memstead/changes.jsonl`; here it is a `Vec` in RAM.
    provenance: Vec<Provenance>,
    /// Per-mem `.memstead/config.json` bytes once written. `None`
    /// until the create path writes the config.
    config: Option<Vec<u8>>,
}

/// Writable mem backend whose entire state lives in memory. See the
/// module docs for the contract it shares with the folder backend.
pub struct InMemoryBackend {
    state: Mutex<State>,
}

impl InMemoryBackend {
    /// Build an empty in-memory mem. Nothing is provisioned on disk;
    /// the mem exists only for as long as this backend is held.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, BackendError> {
        self.state
            .lock()
            .map_err(|_| BackendError::Other("in-memory backend state poisoned".to_string()))
    }
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MemBackend for InMemoryBackend {
    fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
        // Committed state only — pending writes are not listable until
        // they commit, matching the folder backend (which walks the
        // on-disk tree and ignores its in-memory buffer).
        let state = self.lock()?;
        Ok(state.committed.keys().map(PathBuf::from).collect())
    }

    fn read_entity(&self, rel_path: &Path) -> Result<Option<Vec<u8>>, BackendError> {
        let key = normalise_rel_path(rel_path)?;
        let state = self.lock()?;
        // Pending buffer wins over committed — a read after an
        // uncommitted write sees the staged bytes, and a read after a
        // staged delete sees `None`. Same ordering as the folder
        // backend's "pending first, then disk".
        if let Some(staged) = state.pending.get(&key) {
            return Ok(match staged {
                PendingState::Upsert(bytes) => Some(bytes.clone()),
                PendingState::Delete => None,
            });
        }
        Ok(state.committed.get(&key).cloned())
    }

    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), BackendError> {
        let key = normalise_rel_path(rel_path)?;
        let mut state = self.lock()?;
        state
            .pending
            .insert(key, PendingState::Upsert(content.to_vec()));
        Ok(())
    }

    fn delete_entity(&self, rel_path: &Path) -> Result<(), BackendError> {
        let key = normalise_rel_path(rel_path)?;
        let mut state = self.lock()?;
        state.pending.insert(key, PendingState::Delete);
        Ok(())
    }

    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), BackendError> {
        let from_key = normalise_rel_path(from)?;
        let to_key = normalise_rel_path(to)?;
        let mut state = self.lock()?;

        // Resolve the source bytes from the pending buffer if staged,
        // otherwise from committed state — mirroring the folder
        // backend's `read_source`.
        let bytes = match state.pending.remove(&from_key) {
            Some(PendingState::Upsert(b)) => b,
            Some(PendingState::Delete) => {
                state.pending.insert(from_key, PendingState::Delete);
                return Err(BackendError::Other(format!(
                    "move source {} is already pending deletion",
                    from.display()
                )));
            }
            None => match state.committed.get(&from_key) {
                Some(b) => b.clone(),
                None => {
                    return Err(BackendError::Other(format!(
                        "move source {} does not exist",
                        from.display()
                    )));
                }
            },
        };

        if matches!(state.pending.get(&to_key), Some(PendingState::Upsert(_))) {
            return Err(BackendError::Other(format!(
                "move target {} already has a pending write",
                to.display()
            )));
        }
        state.pending.insert(from_key, PendingState::Delete);
        state.pending.insert(to_key, PendingState::Upsert(bytes));
        Ok(())
    }

    fn discard_pending(&self) -> Result<(), BackendError> {
        let mut state = self.lock()?;
        state.pending.clear();
        Ok(())
    }

    fn commit(&self, _message: &str, _ctx: &CommitContext<'_>) -> Result<CommitId, BackendError> {
        let mut state = self.lock()?;
        let ops: Vec<(String, PendingState)> = state.pending.drain().collect();
        for (key, op) in ops {
            match op {
                PendingState::Upsert(bytes) => {
                    state.committed.insert(key, bytes);
                }
                PendingState::Delete => {
                    state.committed.remove(&key);
                }
            }
        }
        Ok(make_commit_id())
    }

    fn append_provenance(&self, record: &Provenance) -> Result<(), BackendError> {
        let mut state = self.lock()?;
        state.provenance.push(record.clone());
        Ok(())
    }

    fn read_provenance(&self, cursor: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
        let state = self.lock()?;
        // Cursor is an opaque RFC-3339 timestamp string, same as the
        // folder backend's `ts` field. String compare matches its
        // `ts_str <= c` filter — lexical order of RFC-3339 UTC strings
        // is chronological.
        let out = state
            .provenance
            .iter()
            .filter(|r| match cursor {
                Some(c) => format_rfc3339_utc(r.timestamp).as_str() > c,
                None => true,
            })
            .cloned()
            .collect();
        Ok(out)
    }

    fn read_mem_config(&self) -> Result<Option<Vec<u8>>, BackendError> {
        let state = self.lock()?;
        Ok(state.config.clone())
    }

    fn write_mem_config(&self, bytes: &[u8]) -> Result<(), BackendError> {
        // Writable backend — unlike the sealed archive (whose default
        // impl returns `Sealed`), the in-memory mem stores the config
        // so the create path can write it and boot can read it back.
        let mut state = self.lock()?;
        state.config = Some(bytes.to_vec());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::ProvenanceKind;
    use crate::vcs::{Actor, ClientId, CommitContext};
    use std::time::{Duration, UNIX_EPOCH};

    fn ctx<'a>() -> CommitContext<'a> {
        CommitContext {
            actor: Actor::Cli,
            client: Some(ClientId {
                name: "claude-code".to_string(),
                version: "0.1.0".to_string(),
            }),
            tool: Some("test"),
            note: None,
            logical_operation_id: None,
            entity_ids: None,
        }
    }

    #[test]
    fn write_then_commit_round_trips_in_ram() {
        let b = InMemoryBackend::new();
        b.write_entity(Path::new("notes/hello.md"), b"# hi\n")
            .unwrap();
        let id = b.commit("c1", &ctx()).unwrap();
        assert!(!id.is_empty());
        assert_eq!(
            b.read_entity(Path::new("notes/hello.md")).unwrap(),
            Some(b"# hi\n".to_vec())
        );
        assert_eq!(
            b.list_entities().unwrap(),
            vec![PathBuf::from("notes/hello.md")]
        );
    }

    #[test]
    fn read_sees_pending_write_before_commit() {
        let b = InMemoryBackend::new();
        b.write_entity(Path::new("a.md"), b"staged").unwrap();
        // Visible to read, not yet to list (uncommitted).
        assert_eq!(
            b.read_entity(Path::new("a.md")).unwrap(),
            Some(b"staged".to_vec())
        );
        assert!(b.list_entities().unwrap().is_empty());
    }

    #[test]
    fn delete_removes_committed_path() {
        let b = InMemoryBackend::new();
        b.write_entity(Path::new("a.md"), b"a").unwrap();
        b.write_entity(Path::new("b.md"), b"b").unwrap();
        b.commit("seed", &ctx()).unwrap();
        b.delete_entity(Path::new("a.md")).unwrap();
        b.commit("drop", &ctx()).unwrap();
        assert_eq!(b.read_entity(Path::new("a.md")).unwrap(), None);
        assert_eq!(
            b.read_entity(Path::new("b.md")).unwrap(),
            Some(b"b".to_vec())
        );
    }

    #[test]
    fn delete_of_missing_path_is_idempotent() {
        let b = InMemoryBackend::new();
        b.delete_entity(Path::new("ghost.md")).unwrap();
        b.commit("noop", &ctx()).unwrap();
        assert_eq!(b.read_entity(Path::new("ghost.md")).unwrap(), None);
    }

    #[test]
    fn move_renames_committed_path() {
        let b = InMemoryBackend::new();
        b.write_entity(Path::new("from.md"), b"payload").unwrap();
        b.commit("seed", &ctx()).unwrap();
        b.move_entity(Path::new("from.md"), Path::new("nested/to.md"))
            .unwrap();
        b.commit("rename", &ctx()).unwrap();
        assert_eq!(b.read_entity(Path::new("from.md")).unwrap(), None);
        assert_eq!(
            b.read_entity(Path::new("nested/to.md")).unwrap(),
            Some(b"payload".to_vec())
        );
    }

    #[test]
    fn move_carries_pending_upsert_bytes() {
        let b = InMemoryBackend::new();
        b.write_entity(Path::new("a.md"), b"alpha").unwrap();
        b.move_entity(Path::new("a.md"), Path::new("b.md")).unwrap();
        b.commit("write+move", &ctx()).unwrap();
        assert_eq!(b.read_entity(Path::new("a.md")).unwrap(), None);
        assert_eq!(
            b.read_entity(Path::new("b.md")).unwrap(),
            Some(b"alpha".to_vec())
        );
    }

    #[test]
    fn move_missing_source_errors() {
        let b = InMemoryBackend::new();
        let err = b
            .move_entity(Path::new("ghost.md"), Path::new("here.md"))
            .unwrap_err();
        assert!(matches!(err, BackendError::Other(_)));
    }

    #[test]
    fn rejects_path_traversal_absolute_and_empty() {
        let b = InMemoryBackend::new();
        // Same rejection rules as the folder backend — reused via
        // `normalise_rel_path`, surfaced as MemWriter path errors.
        assert!(b.write_entity(Path::new("../escape.md"), b"x").is_err());
        assert!(b.write_entity(Path::new("/etc/passwd"), b"x").is_err());
        assert!(b.write_entity(Path::new(""), b"x").is_err());
    }

    #[test]
    fn discard_pending_drops_staged_writes() {
        let b = InMemoryBackend::new();
        b.write_entity(Path::new("a.md"), b"first").unwrap();
        b.commit("seed", &ctx()).unwrap();
        b.write_entity(Path::new("a.md"), b"second-uncommitted")
            .unwrap();
        b.discard_pending().unwrap();
        b.commit("after-discard", &ctx()).unwrap();
        // The discarded write never landed; committed bytes unchanged.
        assert_eq!(
            b.read_entity(Path::new("a.md")).unwrap(),
            Some(b"first".to_vec())
        );
    }

    #[test]
    fn reports_no_durable_history() {
        // Matches the folder backend: no fabricated git-like head.
        let b = InMemoryBackend::new();
        assert_eq!(b.current_head().unwrap(), None);
    }

    #[test]
    fn mem_config_round_trips() {
        let b = InMemoryBackend::new();
        assert_eq!(b.read_mem_config().unwrap(), None);
        b.write_mem_config(b"{\"schema\":\"default@1.0.0\"}")
            .unwrap();
        assert_eq!(
            b.read_mem_config().unwrap(),
            Some(b"{\"schema\":\"default@1.0.0\"}".to_vec())
        );
    }

    #[test]
    fn provenance_appends_and_reads_with_cursor() {
        let b = InMemoryBackend::new();
        let t0 = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let t1 = UNIX_EPOCH + Duration::from_secs(1_700_000_001);
        b.append_provenance(&Provenance::new(
            t0,
            ProvenanceKind::Create,
            Some("specs--a".to_string()),
            Actor::Cli,
            None,
            None,
        ))
        .unwrap();
        b.append_provenance(&Provenance::new(
            t1,
            ProvenanceKind::Update,
            Some("specs--a".to_string()),
            Actor::Cli,
            None,
            None,
        ))
        .unwrap();

        // No cursor → both records.
        assert_eq!(b.read_provenance(None).unwrap().len(), 2);
        // Cursor at t0 → only the strictly-later t1 record.
        let cursor = format_rfc3339_utc(t0);
        let after = b.read_provenance(Some(&cursor)).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].kind, ProvenanceKind::Update);
    }
}
