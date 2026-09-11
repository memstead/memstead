//! Git-tree-backed [`MemBackend`](memstead_base::backend::MemBackend) — the second
//! storage adapter. Buffers mutations
//! in memory and applies them to a tree built via
//! `gix::object::tree::Editor`, then advances the target ref via
//! [`gix::Repository::commit_as`].
//!
//! No working tree is written: the mem's content lives only in the
//! multi-root `mem-repo-git` object store, one branch per mem. Each
//! commit rebuilds the tree from the buffered op log against the
//! snapshotted parent tree.
//!
//! ## Snapshot + CAS
//!
//! On the first mutation of a "session" (the period between two
//! successful commits, or between construction and the first commit),
//! the writer snapshots the current ref tip's [`gix::ObjectId`]. That
//! snapshot is the `parents` argument to
//! [`gix::Repository::commit_as`]; gix's underlying ref-edit transaction
//! enforces `PreviousValue::ExistingMustMatch(previous)` for non-`HEAD`
//! refs, which is the exact CAS guard we want. If a concurrent writer
//! advanced the ref between snapshot and commit, the gix call returns
//! [`gix::commit::Error::ReferenceEdit`]; we re-resolve the live tip and
//! surface [`super::BackendError::HashMismatch`] with the new tip's
//! hex OID. That maps into
//! [`crate::EngineError::HashMismatch`] so MCP agents see a stable
//! `_hash` to retry with.
//!
//! No internal retry loop: every CAS conflict bubbles up. Cross-process
//! contention in Phase 1 is intentionally simple — concurrency hardening
//! comes later (D7 in the design doc).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use gix::objs::tree::EntryKind;

use super::CommitId;
use crate::vcs::{CommitContext, acquire_branch_mutex, author_identity, format_commit_message};
use memstead_base::backend::{BackendError, MemBackend};

/// Per-path final state for the buffered op log. Move operations
/// resolve at call time into a `Delete(from)` + `Upsert(to, bytes)`
/// pair so commit-time replay only ever sees these two terminal states.
#[derive(Clone)]
enum PendingState {
    Upsert(Vec<u8>),
    Delete,
}

/// In-flight mutation buffer. Snapshotted parent SHA + the per-path
/// final-state map. Both reset to `(None, empty)` after a successful
/// `commit()`.
struct Pending {
    /// Parent ref tip captured on the first mutation of this session.
    /// `None` either when the ref does not yet exist (commit creates
    /// it) or before the first mutation. The same `parent` value is
    /// passed verbatim to `commit_as`'s `parents` argument; gix uses
    /// it as the CAS guard.
    parent: Option<gix::ObjectId>,
    /// Per-path final state. Values are stored mem-relative as
    /// forward-slash strings — git tree entries are slash-separated
    /// regardless of host OS, and the editor APIs take string keys.
    ops: HashMap<String, PendingState>,
}

impl Pending {
    fn new() -> Self {
        Self {
            parent: None,
            ops: HashMap::new(),
        }
    }

    fn clear(&mut self) {
        self.parent = None;
        self.ops.clear();
    }
}

/// Git-tree-backed implementation of [`MemBackend`]. Holds the
/// gitdir path and target ref name; opens the [`gix::Repository`]
/// per call (matches the [`crate::vcs::GixVcs`] pattern, since
/// `gix::Repository` is `Send` but not `Sync` — its object-database
/// cache uses interior mutability via `RefCell`).
///
/// Mutations buffer in memory until [`Self::commit`].
pub struct GitTreeBackend {
    gitdir: PathBuf,
    ref_name: String,
    pending: Mutex<Pending>,
}

impl GitTreeBackend {
    /// Build a writer against the repository at `gitdir` targeting
    /// `ref_name`. The ref need not exist yet — the first commit
    /// creates it. `ref_name` is the per-branch mutex key; pass the
    /// fully-qualified form (e.g. `refs/heads/main`) so writers
    /// targeting the same branch under one gitdir share the same key.
    pub fn new(gitdir: PathBuf, ref_name: String) -> Self {
        Self {
            gitdir,
            ref_name,
            pending: Mutex::new(Pending::new()),
        }
    }

    fn open_repo(&self) -> Result<gix::Repository, BackendError> {
        gix::open(&self.gitdir).map_err(|e| {
            BackendError::Path(format!(
                "git-tree writer: open repo at {}: {e}",
                self.gitdir.display()
            ))
        })
    }

    /// Capture the current tip of `ref_name` if no snapshot has been
    /// taken in this session. Idempotent: subsequent mutations reuse
    /// the same snapshot. A missing ref leaves `parent = None`.
    fn ensure_snapshot(&self, pending: &mut Pending) -> Result<(), BackendError> {
        if pending.parent.is_some() || !pending.ops.is_empty() {
            return Ok(());
        }
        let repo = self.open_repo()?;
        let mut reference = match repo.try_find_reference(&self.ref_name).map_err(|e| {
            BackendError::Path(format!(
                "git-tree writer: resolve ref {}: {e}",
                self.ref_name
            ))
        })? {
            Some(r) => r,
            None => return Ok(()),
        };
        let id = reference.peel_to_id().map_err(|e| {
            BackendError::Path(format!(
                "git-tree writer: peel ref {} to id: {e}",
                self.ref_name
            ))
        })?;
        pending.parent = Some(id.detach());
        Ok(())
    }

    /// Peel the live `ref_name` tip to its commit id, or `None` when the
    /// ref does not exist yet. Unlike [`Self::ensure_snapshot`] this
    /// does *not* pin anything onto `pending` — it is the fresh-read
    /// path used between write transactions, so a sibling engine's
    /// commit is visible on the next read rather than frozen at the
    /// snapshot captured by the first read of the session.
    fn live_tip(&self) -> Result<Option<gix::ObjectId>, BackendError> {
        let repo = self.open_repo()?;
        let mut reference = match repo.try_find_reference(&self.ref_name).map_err(|e| {
            BackendError::Path(format!(
                "git-tree writer: resolve ref {}: {e}",
                self.ref_name
            ))
        })? {
            Some(r) => r,
            None => return Ok(None),
        };
        let id = reference.peel_to_id().map_err(|e| {
            BackendError::Path(format!(
                "git-tree writer: peel ref {} to id: {e}",
                self.ref_name
            ))
        })?;
        Ok(Some(id.detach()))
    }

    /// Read a blob at `path` from the snapshotted parent tree. Used by
    /// `move_entity` to fetch the source bytes when the path has no
    /// pending upsert.
    fn read_blob_from_parent(
        &self,
        parent: gix::ObjectId,
        path: &str,
    ) -> Result<Option<Vec<u8>>, BackendError> {
        let repo = self.open_repo()?;
        let commit = repo
            .find_object(parent)
            .map_err(|e| BackendError::Path(format!("git-tree writer: open parent commit: {e}")))?
            .into_commit();
        let tree = commit.tree().map_err(|e| {
            BackendError::Path(format!("git-tree writer: peel commit to tree: {e}"))
        })?;
        let entry = match tree.lookup_entry_by_path(path).map_err(|e| {
            BackendError::Path(format!(
                "git-tree writer: lookup {path} in parent tree: {e}"
            ))
        })? {
            Some(e) => e,
            None => return Ok(None),
        };
        if !entry.mode().is_blob() {
            return Ok(None);
        }
        let object = repo
            .find_object(entry.id())
            .map_err(|e| BackendError::Path(format!("git-tree writer: read blob {path}: {e}")))?;
        Ok(Some(object.data.clone()))
    }

    /// Tree-lookup-class existence probe: ref → commit → tree →
    /// `lookup_entry_by_path`, stopping at the entry — the blob's
    /// bytes are never read (flywheel W7/02: the write-time cross-mem
    /// target check's primitive; the listing walk reads every blob and
    /// is the wrong tool for one path's existence).
    fn path_exists_from_parent(
        &self,
        parent: gix::ObjectId,
        path: &str,
    ) -> Result<bool, BackendError> {
        let repo = self.open_repo()?;
        let commit = repo
            .find_object(parent)
            .map_err(|e| BackendError::Path(format!("git-tree writer: open parent commit: {e}")))?
            .into_commit();
        let tree = commit.tree().map_err(|e| {
            BackendError::Path(format!("git-tree writer: peel commit to tree: {e}"))
        })?;
        let entry = tree.lookup_entry_by_path(path).map_err(|e| {
            BackendError::Path(format!(
                "git-tree writer: lookup {path} in parent tree: {e}"
            ))
        })?;
        Ok(entry.is_some_and(|e| e.mode().is_blob()))
    }
}

/// Normalise a mem-relative path to forward-slash form. Rejects
/// empty paths and any path that contains `..` segments — git tree
/// entries cannot escape upward and this guards the caller against
/// accidentally writing past the mem root via a relative-path bug.
fn normalise_rel_path(rel_path: &Path) -> Result<String, BackendError> {
    if rel_path.as_os_str().is_empty() {
        return Err(BackendError::Path("mem-relative path is empty".to_string()));
    }
    let mut parts: Vec<String> = Vec::new();
    for component in rel_path.components() {
        use std::path::Component;
        match component {
            Component::Normal(s) => match s.to_str() {
                Some(p) if !p.is_empty() => parts.push(p.to_string()),
                _ => {
                    return Err(BackendError::Path(format!(
                        "non-utf-8 or empty path component in {}",
                        rel_path.display()
                    )));
                }
            },
            Component::CurDir => continue,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(BackendError::Path(format!(
                    "path traversal or absolute component in {}",
                    rel_path.display()
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(BackendError::Path(
            "mem-relative path is empty after normalisation".to_string(),
        ));
    }
    Ok(parts.join("/"))
}

impl memstead_base::backend::MemBackend for GitTreeBackend {
    /// The per-mem branch ref exists. `list_entities` folds a missing
    /// branch into an empty list (a fresh mem has no commits yet), so
    /// this is the only way boot can tell "never created" from "empty"
    /// and say `missing_ref` instead of `empty`.
    fn storage_present(&self) -> Result<bool, memstead_base::backend::BackendError> {
        let repo = gix::open(&self.gitdir).map_err(|e| {
            memstead_base::backend::BackendError::Other(format!(
                "git-tree backend storage_present: open {}: {e}",
                self.gitdir.display()
            ))
        })?;
        repo.try_find_reference(&self.ref_name)
            .map(|r| r.is_some())
            .map_err(|e| {
                memstead_base::backend::BackendError::Other(format!(
                    "git-tree backend storage_present: resolve {}: {e}",
                    self.ref_name
                ))
            })
    }

    fn list_entities(&self) -> Result<Vec<PathBuf>, memstead_base::backend::BackendError> {
        // Walk the per-mem branch tree, return only `.md` paths
        // outside the `.memstead/` umbrella (config / schemas / changelog
        // live there and don't surface as entities at this layer).
        // Branch-missing → empty list (a fresh mem has no commits yet).
        let blobs = match read_branch_blobs(&self.gitdir, &self.ref_name) {
            Ok(b) => b,
            Err(BranchReadError::BranchMissing { .. }) => return Ok(Vec::new()),
            Err(e) => {
                return Err(memstead_base::backend::BackendError::Other(format!(
                    "git-tree backend list_entities: {e}"
                )));
            }
        };
        Ok(blobs
            .into_iter()
            .filter_map(|b| {
                if b.path.ends_with(".md") && !b.path.starts_with(".memstead/") {
                    Some(PathBuf::from(b.path))
                } else {
                    None
                }
            })
            .collect())
    }

    fn read_entity(
        &self,
        rel_path: &Path,
    ) -> Result<Option<Vec<u8>>, memstead_base::backend::BackendError> {
        let key = normalise_rel_path(rel_path)?;
        // Pending ops win over the branch tip — same precedence as the
        // folder backend.
        let pending = self.pending.lock().map_err(|_| {
            memstead_base::backend::BackendError::Other(
                "git-tree backend pending state poisoned".to_string(),
            )
        })?;
        if let Some(state) = pending.ops.get(&key) {
            return Ok(match state {
                PendingState::Upsert(b) => Some(b.clone()),
                PendingState::Delete => None,
            });
        }
        // Mid-transaction (one or more writes already staged): reads
        // must see the same snapshotted parent the buffered ops will be
        // composed onto, for a consistent commit. Between transactions
        // (no pending ops — boot loads, `reload_one_mem` re-reads, any
        // read before the first write of an op), read the *live* ref tip
        // so a sibling engine's commit is visible. The previous code
        // pinned the parent on the first read of the session and froze
        // every later read at that snapshot, which defeated
        // reload-before-operation for entities that already existed at
        // boot (a sibling's modification came back stale).
        let snapshot_parent = if pending.ops.is_empty() {
            None
        } else {
            pending.parent
        };
        drop(pending);

        let source = match snapshot_parent {
            Some(p) => Some(p),
            None => self.live_tip()?,
        };
        match source {
            Some(p) => self.read_blob_from_parent(p, &key),
            None => Ok(None),
        }
    }

    /// The whole mem in one walk: open the repository, peel the ref
    /// and inflate each tree exactly once, then read every blob. The
    /// per-path route (`list_entities`, which already reads every
    /// blob and drops the bytes, then `read_entity` per path, which
    /// re-opens the repository and re-inflates the root tree each
    /// time) made a boot quadratic in the mem size. Source selection is
    /// `read_entity`'s: the snapshotted parent while writes are staged,
    /// the live tip between transactions; the pending buffer is then
    /// composed over the rows (a staged delete hides its row, a staged
    /// upsert replaces or adds one), so the answer is what per-path
    /// reads would have given, in one walk.
    fn read_all_entities(
        &self,
    ) -> Result<Vec<memstead_base::backend::EntityRead>, memstead_base::backend::BackendError> {
        let is_entity = |path: &str| path.ends_with(".md") && !path.starts_with(".memstead/");
        let (snapshot_parent, ops) = {
            let pending = self.pending.lock().map_err(|_| {
                memstead_base::backend::BackendError::Other(
                    "git-tree backend pending state poisoned".to_string(),
                )
            })?;
            let parent = if pending.ops.is_empty() {
                None
            } else {
                pending.parent
            };
            (parent, pending.ops.clone())
        };
        let source = match snapshot_parent {
            Some(p) => Some(p),
            None => self.live_tip()?,
        };
        let mut rows: Vec<memstead_base::backend::EntityRead> = match source {
            None => Vec::new(),
            Some(id) => read_commit_blobs(&self.gitdir, id)
                .map_err(|e| {
                    memstead_base::backend::BackendError::Other(format!(
                        "git-tree backend read_all_entities: {e}"
                    ))
                })?
                .into_iter()
                .filter(|b| is_entity(&b.path) && !ops.contains_key(&b.path))
                .map(|b| (PathBuf::from(b.path), Ok(b.bytes)))
                .collect(),
        };
        for (path, state) in ops {
            if let PendingState::Upsert(bytes) = state
                && is_entity(path.as_str())
            {
                rows.push((PathBuf::from(path), Ok(bytes)));
            }
        }
        Ok(rows)
    }

    /// Existence probe with `read_entity`'s exact source-selection
    /// semantics (pending buffer first, snapshotted parent
    /// mid-transaction, live tip between transactions) — but stopping
    /// at the tree entry, never reading the blob.
    fn entity_exists(&self, rel_path: &Path) -> Result<bool, memstead_base::backend::BackendError> {
        let key = normalise_rel_path(rel_path)?;
        let pending = self.pending.lock().map_err(|_| {
            memstead_base::backend::BackendError::Other(
                "git-tree backend pending state poisoned".to_string(),
            )
        })?;
        if let Some(state) = pending.ops.get(&key) {
            return Ok(matches!(state, PendingState::Upsert(_)));
        }
        let snapshot_parent = if pending.ops.is_empty() {
            None
        } else {
            pending.parent
        };
        drop(pending);

        let source = match snapshot_parent {
            Some(p) => Some(p),
            None => self.live_tip()?,
        };
        match source {
            Some(p) => self.path_exists_from_parent(p, &key),
            None => Ok(false),
        }
    }

    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), BackendError> {
        let key = normalise_rel_path(rel_path)?;
        let mut pending = self.pending.lock().map_err(|_| {
            BackendError::Path("git-tree writer pending state poisoned".to_string())
        })?;
        self.ensure_snapshot(&mut pending)?;
        pending
            .ops
            .insert(key, PendingState::Upsert(content.to_vec()));
        Ok(())
    }

    fn delete_entity(&self, rel_path: &Path) -> Result<(), BackendError> {
        let key = normalise_rel_path(rel_path)?;
        let mut pending = self.pending.lock().map_err(|_| {
            BackendError::Path("git-tree writer pending state poisoned".to_string())
        })?;
        self.ensure_snapshot(&mut pending)?;
        pending.ops.insert(key, PendingState::Delete);
        Ok(())
    }

    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), BackendError> {
        let from_key = normalise_rel_path(from)?;
        let to_key = normalise_rel_path(to)?;
        let mut pending = self.pending.lock().map_err(|_| {
            BackendError::Path("git-tree writer pending state poisoned".to_string())
        })?;
        self.ensure_snapshot(&mut pending)?;

        // Resolve the from-content. If a pending upsert exists, take
        // its bytes; otherwise look up the blob in the snapshotted
        // parent tree. Absent from both: nothing to move.
        let bytes = match pending.ops.remove(&from_key) {
            Some(PendingState::Upsert(b)) => b,
            Some(PendingState::Delete) => {
                pending.ops.insert(from_key, PendingState::Delete);
                return Err(BackendError::Path(format!(
                    "move source {} is already pending deletion",
                    from.display()
                )));
            }
            None => {
                let parent = pending.parent;
                let blob = match parent {
                    Some(p) => self.read_blob_from_parent(p, &from_key)?,
                    None => None,
                };
                match blob {
                    Some(b) => b,
                    None => {
                        return Err(BackendError::Path(format!(
                            "move source {} does not exist",
                            from.display()
                        )));
                    }
                }
            }
        };

        if matches!(pending.ops.get(&to_key), Some(PendingState::Upsert(_))) {
            // A move refuses when the target already has a pending write.
            return Err(BackendError::Path(format!(
                "move target {} already has a pending write",
                to.display()
            )));
        }
        pending.ops.insert(from_key, PendingState::Delete);
        pending.ops.insert(to_key, PendingState::Upsert(bytes));
        Ok(())
    }

    fn discard_pending(&self) -> Result<(), memstead_base::backend::BackendError> {
        // Drop the staged tree edits and the captured parent snapshot
        // without committing — symmetric with the `pending.clear()`
        // that `commit` runs on success. The atomic batch path calls
        // this to roll back staged writes when a later item refuses
        // the whole batch.
        let mut pending = self.pending.lock().map_err(|_| {
            memstead_base::backend::BackendError::Other(
                "git-tree writer pending state poisoned".to_string(),
            )
        })?;
        pending.clear();
        Ok(())
    }

    fn commit(&self, message: &str, ctx: &CommitContext<'_>) -> Result<CommitId, BackendError> {
        // Serialise commits against the same target ref at process
        // scope. Different refs under the same gitdir proceed in
        // parallel — that is the whole point of the per-branch key.
        let mutex = acquire_branch_mutex(&self.ref_name);
        let _guard = mutex.lock().map_err(|_| {
            BackendError::Path(format!(
                "git-tree writer mutex poisoned for ref {} (gitdir {})",
                self.ref_name,
                self.gitdir.display()
            ))
        })?;
        let repo = self.open_repo()?;

        let mut pending = self.pending.lock().map_err(|_| {
            BackendError::Path("git-tree writer pending state poisoned".to_string())
        })?;

        // Make sure we have a parent snapshot even if the caller went
        // straight to commit() without any mutations — exercises the
        // `no-op commit` edge case sensibly.
        self.ensure_snapshot(&mut pending)?;
        let parent_snapshot = pending.parent;

        // Build the editor on top of the snapshotted tree.
        let mut editor = match parent_snapshot {
            Some(parent_id) => {
                let commit = repo
                    .find_object(parent_id)
                    .map_err(|e| {
                        BackendError::Path(format!("git-tree writer: open parent {parent_id}: {e}"))
                    })?
                    .into_commit();
                let tree = commit.tree().map_err(|e| {
                    BackendError::Path(format!("git-tree writer: peel parent tree: {e}"))
                })?;
                tree.edit()
                    .map_err(|e| BackendError::Path(format!("git-tree writer: editor init: {e}")))?
            }
            None => repo.empty_tree().edit().map_err(|e| {
                BackendError::Path(format!("git-tree writer: empty editor init: {e}"))
            })?,
        };

        // Replay ops. Order is irrelevant since map keys are unique
        // and final-state semantics already collapsed any duplicates.
        for (path, state) in pending.ops.iter() {
            match state {
                PendingState::Upsert(bytes) => {
                    let blob_id = repo
                        .write_blob(bytes.as_slice())
                        .map_err(|e| {
                            BackendError::Path(format!(
                                "git-tree writer: write blob for {path}: {e}"
                            ))
                        })?
                        .detach();
                    editor
                        .upsert(path.as_str(), EntryKind::Blob, blob_id)
                        .map_err(|e| {
                            BackendError::Path(format!("git-tree writer: tree upsert {path}: {e}"))
                        })?;
                }
                PendingState::Delete => {
                    editor.remove(path.as_str()).map_err(|e| {
                        BackendError::Path(format!("git-tree writer: tree remove {path}: {e}"))
                    })?;
                }
            }
        }

        let tree_id = editor
            .write()
            .map_err(|e| BackendError::Path(format!("git-tree writer: tree write: {e}")))?
            .detach();

        // Build signatures via the same convention the disk adapter
        // uses (see vcs::format_commit_message + author_identity).
        let time = gix::date::Time::now_local_or_utc();
        let committer_sig = gix::actor::Signature {
            name: COMMITTER_NAME.into(),
            email: COMMITTER_EMAIL.into(),
            time,
        };
        let author_sig = match author_identity(ctx) {
            Some((name, email)) => gix::actor::Signature {
                name: name.into(),
                email: email.into(),
                time,
            },
            None => committer_sig.clone(),
        };
        let mut author_buf = gix::date::parse::TimeBuf::default();
        let mut committer_buf = gix::date::parse::TimeBuf::default();
        let author_ref = author_sig.to_ref(&mut author_buf);
        let committer_ref = committer_sig.to_ref(&mut committer_buf);

        let full_message = format_commit_message(message, ctx);

        let parents: Vec<gix::ObjectId> = match parent_snapshot {
            Some(p) => vec![p],
            None => Vec::new(),
        };
        let commit_result = repo.commit_as(
            committer_ref,
            author_ref,
            self.ref_name.as_str(),
            full_message,
            tree_id,
            parents,
        );
        // The staged ops were fully replayed into `tree_id` above, so
        // `pending` is no longer needed regardless of the commit outcome.
        // Clear it here so EVERY exit ends the transaction — success, CAS
        // conflict, or any other commit failure. Leaving it populated on a
        // failed commit is a coherence bug: `read_entity` prefers pending
        // over the committed tip, so an orphaned op would be served as
        // phantom truth (and pulled into the in-memory store by a later
        // `reload_one_mem`) until the process restarts.
        pending.clear();
        let commit_id = match commit_result {
            Ok(id) => id,
            Err(gix::commit::Error::ReferenceEdit(_)) => {
                // CAS conflict. Re-resolve the live tip and surface
                // the new sha so the caller can retry with a fresh
                // `_hash`.
                let mut reference = repo
                    .try_find_reference(&self.ref_name)
                    .map_err(|e| {
                        BackendError::Path(format!(
                            "git-tree writer: re-resolve ref after CAS: {e}"
                        ))
                    })?
                    .ok_or_else(|| {
                        BackendError::Path(format!(
                            "git-tree writer: ref {} vanished during CAS recovery",
                            self.ref_name
                        ))
                    })?;
                let live_id = reference.peel_to_id().map_err(|e| {
                    BackendError::Path(format!("git-tree writer: peel live tip after CAS: {e}"))
                })?;
                return Err(BackendError::HashMismatch {
                    current: live_id.to_hex().to_string(),
                });
            }
            Err(e) => {
                return Err(BackendError::Path(format!(
                    "git-tree writer: commit_as failed: {e}"
                )));
            }
        };

        let sha_hex = commit_id.to_hex().to_string();

        // Refresh index + working tree if the just-written ref is what
        // HEAD currently points at. Keeps `git status` clean for human
        // visualizers (GitHub Desktop and friends) which would
        // otherwise misread the engine's tree-editor commits as a
        // pending "delete" diff. No-op for bare repos and for writes
        // to a ref that is not the checked-out branch.
        sync_index_and_worktree(&repo, &self.ref_name)?;

        Ok(sha_hex)
    }

    fn commit_with_expected_parent(
        &self,
        message: &str,
        ctx: &CommitContext<'_>,
        expected_parent: Option<&str>,
    ) -> Result<CommitId, memstead_base::backend::BackendError> {
        // No pin requested → identical to commit().
        let Some(expected) = expected_parent else {
            return <Self as MemBackend>::commit(self, message, ctx);
        };

        // Acquire the same per-ref mutex `commit` uses so the parent
        // check and the subsequent commit are sequenced w.r.t. other
        // in-process writers on this ref. The mutex must be released
        // before delegating to `commit` (std `Mutex` is not reentrant);
        // any in-process writer that slips in between the drop and
        // `commit`'s re-acquire would advance the ref past the
        // already-captured `pending.parent`, and gix's CAS inside
        // `commit_as` would surface that as `HashMismatch` — semantically
        // equivalent to `ParentMismatch` for the engine layer above.
        let mutex = acquire_branch_mutex(&self.ref_name);
        let guard = mutex.lock().map_err(|_| {
            memstead_base::backend::BackendError::Other(format!(
                "git-tree writer mutex poisoned for ref {} (gitdir {})",
                self.ref_name,
                self.gitdir.display()
            ))
        })?;

        let actual = match gix::open(&self.gitdir) {
            Ok(repo) => match repo.try_find_reference(&self.ref_name) {
                Ok(Some(mut r)) => r
                    .peel_to_id()
                    .ok()
                    .map(|id| id.detach().to_hex().to_string()),
                Ok(None) => None,
                Err(e) => {
                    return Err(memstead_base::backend::BackendError::Other(format!(
                        "git-tree writer: resolve ref {} for parent check: {e}",
                        self.ref_name
                    )));
                }
            },
            Err(e) => {
                return Err(memstead_base::backend::BackendError::Other(format!(
                    "git-tree writer: open repo at {} for parent check: {e}",
                    self.gitdir.display()
                )));
            }
        };
        let actual_str = actual.unwrap_or_default();
        if actual_str != expected {
            return Err(memstead_base::backend::BackendError::ParentMismatch {
                expected: expected.to_string(),
                actual: actual_str,
            });
        }

        drop(guard);
        <Self as MemBackend>::commit(self, message, ctx)
    }

    fn append_provenance(
        &self,
        _record: &memstead_base::Provenance,
    ) -> Result<(), memstead_base::backend::BackendError> {
        // No-op. The git-branch backend encodes provenance directly in
        // the commit object: subject (`memstead: <verb> <entity>`) carries
        // the kind + entity, the trailer block carries actor / client /
        // tool, and the body paragraph carries the agent note. The next
        // `commit()` call writes all of it via `format_commit_message`.
        // `read_provenance` reconstructs `Provenance` records by walking
        // commits and re-parsing the bodies — symmetric round-trip
        // without a side-channel log. Folder backend writes a separate
        // JSONL line because it has no commit object to carry the data.
        Ok(())
    }

    fn read_provenance(
        &self,
        cursor: Option<&str>,
    ) -> Result<Vec<memstead_base::Provenance>, memstead_base::backend::BackendError> {
        let since = cursor.unwrap_or(crate::ops::changes::EMPTY_TREE_SHA);
        let report = match crate::ops::agent_notes::agent_notes_since(
            "",
            &self.gitdir,
            since,
            Some(&self.ref_name),
        ) {
            Ok(r) => r,
            Err(e) => {
                return Err(memstead_base::backend::BackendError::Other(format!(
                    "git-tree backend read_provenance: {e}"
                )));
            }
        };
        // `agent_notes_since` returns newest-first (`git log` default).
        // The folder backend's `read_provenance` returns oldest-first
        // (insertion order in the JSONL). Reverse here so consumers
        // observe a single ordering convention regardless of backend.
        let mut out: Vec<memstead_base::Provenance> = report
            .notes
            .into_iter()
            .map(commit_note_to_provenance)
            .collect();
        out.reverse();
        Ok(out)
    }

    fn current_head(&self) -> Result<Option<String>, memstead_base::backend::BackendError> {
        // Open the gitdir and peel the per-mem branch ref to its
        // commit object id. Missing ref / missing repo / peel failure
        // collapse to `Ok(None)` — the engine treats them as "no
        // drift signal", same as folder/archive. Surfaced log lines
        // give operators a breadcrumb when a branch genuinely
        // disappears between probes.
        let repo = match gix::open(&self.gitdir) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(
                    gitdir = %self.gitdir.display(),
                    error = %e,
                    "current_head: open gitdir failed; treating as no baseline"
                );
                return Ok(None);
            }
        };
        let mut reference = match repo.try_find_reference(&self.ref_name) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(None),
            Err(e) => {
                tracing::debug!(
                    ref_name = %self.ref_name,
                    error = %e,
                    "current_head: ref lookup failed; treating as no baseline"
                );
                return Ok(None);
            }
        };
        Ok(reference
            .peel_to_id()
            .ok()
            .map(|id| id.detach().to_hex().to_string()))
    }

    fn read_mem_config(&self) -> Result<Option<Vec<u8>>, memstead_base::backend::BackendError> {
        // Resolve the mem leaf from `self.ref_name`. V1 unified
        // mounts are flat (`refs/heads/<leaf>`); hierarchical
        // layouts are not yet supported on the unified path.
        let leaf = self
            .ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(&self.ref_name);

        // `__MEMSTEAD:mems/<leaf>/config.json` is the only read path.
        // Every workspace the engine touches has `__MEMSTEAD` populated
        // by boot — the legacy registry-class refs are no longer
        // read at runtime.
        read_blob_from_ref(
            &self.gitdir,
            "refs/heads/__MEMSTEAD",
            &format!("mems/{leaf}/config.json"),
        )
    }

    fn read_anchors_sidecar(
        &self,
    ) -> Result<Option<Vec<u8>>, memstead_base::backend::BackendError> {
        // Read via the MemBackend entity path so pending-buffer
        // precedence applies (a staged sidecar write is visible before
        // its commit) and a sibling engine's committed sidecar is seen on
        // a between-transaction read — identical semantics to entity reads.
        <Self as memstead_base::backend::MemBackend>::read_entity(
            self,
            Path::new(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
        )
    }

    fn write_anchors_sidecar(
        &self,
        bytes: &[u8],
    ) -> Result<(), memstead_base::backend::BackendError> {
        // Stage into the same pending op buffer entity writes use, under
        // the `.memstead/anchors.json` path, so the next commit() carries
        // entity + sidecar atomically. `list_entities` filters `.memstead/`,
        // so the sidecar never surfaces as an entity.
        <Self as MemBackend>::write_entity(
            self,
            Path::new(memstead_base::anchor::ANCHOR_SIDECAR_PATH),
            bytes,
        )
    }

    fn delete_artifacts(
        &self,
        ctx: &CommitContext<'_>,
    ) -> Result<(), memstead_base::backend::BackendError> {
        // The branch leaf is the per-mem ref minus the
        // `refs/heads/` prefix — symmetric with the resolution done
        // by `read_mem_config` / `write_mem_config` above.
        // Hierarchical layouts (e.g. `refs/heads/planning/plan-q4`)
        // strip to `planning/plan-q4`; flat layouts to the bare leaf.
        let branch_leaf = self
            .ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(&self.ref_name);
        crate::storage_memstead::delete_mem_artifacts_at_gitdir(&self.gitdir, branch_leaf, ctx)
            .map_err(|e| memstead_base::backend::BackendError::Other(e.to_string()))
    }

    fn write_mem_config(
        &self,
        bytes: &[u8],
        ctx: &CommitContext<'_>,
    ) -> Result<(), memstead_base::backend::BackendError> {
        // Write `__MEMSTEAD:mems/<mem>/config.json` only. The legacy
        // `mem_repo_config::read_config` consumer chain reads
        // through `__MEMSTEAD`, so a dual-write to any retired ref would
        // be wasted work.
        //
        // Mem leaf comes from `self.ref_name` (the per-mem
        // branch); for hierarchical mounts (refs/heads/<path>/<leaf>)
        // the helper's `resolve_full_path_at_gitdir` walks the
        // branch list to find the matching full path. For a fresh
        // mem not yet present in the branch list, the helper
        // falls back to the flat `<leaf>/config.json` shape —
        // unified `create_mem` writes the per-mem branch commit
        // AFTER this call, so during the very first
        // write_mem_config the branch isn't yet present.
        // Hierarchical-path semantics for fresh mems need a
        // small lift in a follow-up (pass full path explicitly).
        //
        // The caller's context rides the commit, so a version bump or
        // a sync-state stamp carries the same provenance every other
        // commit-producing operation does.
        let leaf = self
            .ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(&self.ref_name);
        crate::storage_memstead::commit_config_to_memstead_at_gitdir(
            &self.gitdir,
            leaf,
            bytes,
            ctx,
            &format!("memstead: commit __MEMSTEAD:mems/{leaf}/config.json"),
        )
        .map_err(|e| memstead_base::backend::BackendError::Other(e.to_string()))
    }

    fn record_pipeline_edit(
        &self,
        kind: &str,
        edits: &[(String, Option<Vec<u8>>)],
        ctx: &CommitContext<'_>,
        verb: &str,
    ) -> Result<(), memstead_base::backend::BackendError> {
        // Mirror the pipeline-config edit under
        // `__MEMSTEAD:pipeline/<kind>/<leaf>/<name>.json` — the commit
        // (subject + Note trailer) is the provenance record for a disk
        // write that has no commit of its own.
        let leaf = self
            .ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(&self.ref_name);
        let tree_edits: Vec<(String, Option<Vec<u8>>)> = edits
            .iter()
            .map(|(name, bytes)| (format!("pipeline/{kind}/{leaf}/{name}.json"), bytes.clone()))
            .collect();
        let names: Vec<&str> = edits.iter().map(|(n, _)| n.as_str()).collect();
        crate::storage_memstead::commit_paths_to_memstead_at_gitdir(
            &self.gitdir,
            &tree_edits,
            ctx,
            &format!("memstead: {verb} {kind} {leaf}/{}", names.join(", ")),
        )
        .map_err(|e| memstead_base::backend::BackendError::Other(e.to_string()))
    }
}

/// Read a blob from `ref_name:path` in the gitdir. Returns
/// `Ok(None)` when the ref is missing or the path doesn't exist
/// in the tree. Errors propagate as `BackendError::Other`.
///
/// Used by `read_mem_config` to read per-mem configs from
/// `__MEMSTEAD` without needing a full full `MemConfig` parser path —
/// the engine parses bytes uniformly across backends.
fn read_blob_from_ref(
    gitdir: &Path,
    ref_name: &str,
    path: &str,
) -> Result<Option<Vec<u8>>, memstead_base::backend::BackendError> {
    let repo = match gix::open(gitdir) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    let reference = match repo.try_find_reference(ref_name) {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(None),
        Err(e) => {
            return Err(memstead_base::backend::BackendError::Other(format!(
                "find ref {ref_name}: {e}"
            )));
        }
    };
    let id = reference.into_fully_peeled_id().map_err(|e| {
        memstead_base::backend::BackendError::Other(format!("peel {ref_name}: {e}"))
    })?;
    let object = id.object().map_err(|e| {
        memstead_base::backend::BackendError::Other(format!("read obj {ref_name}: {e}"))
    })?;
    let commit = match object.try_into_commit() {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let tree = commit.tree().map_err(|e| {
        memstead_base::backend::BackendError::Other(format!("read tree {ref_name}: {e}"))
    })?;
    let entry = match tree.lookup_entry_by_path(path) {
        Ok(Some(e)) => e,
        Ok(None) => return Ok(None),
        Err(e) => {
            return Err(memstead_base::backend::BackendError::Other(format!(
                "lookup {ref_name}:{path}: {e}"
            )));
        }
    };
    let blob = entry.object().map_err(|e| {
        memstead_base::backend::BackendError::Other(format!("read blob {ref_name}:{path}: {e}"))
    })?;
    Ok(Some(blob.data.clone()))
}

/// Build a [`memstead_base::Provenance`] from a parsed commit note. Best-
/// effort: unrecognised verbs map to `Update`, missing actors to
/// `Unknown`, malformed client trailers drop the field. Matches the
/// folder backend's tolerant-reader stance.
fn commit_note_to_provenance(n: crate::ops::agent_notes::CommitNote) -> memstead_base::Provenance {
    let kind = n
        .tool_verb
        .as_deref()
        .and_then(memstead_base::ProvenanceKind::parse)
        .unwrap_or(memstead_base::ProvenanceKind::Update);
    let actor = n
        .actor
        .as_deref()
        .and_then(memstead_base::vcs::Actor::from_trailer)
        .unwrap_or(memstead_base::vcs::Actor::Unknown);
    let client = n
        .client
        .as_deref()
        .and_then(memstead_base::vcs::parse_client_id);
    let timestamp = if n.timestamp >= 0 {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(n.timestamp as u64)
    } else {
        std::time::UNIX_EPOCH
    };
    let mut record =
        memstead_base::Provenance::new(timestamp, kind, n.entity_id, actor, client, n.note);
    if let Some(id) = n.logical_operation_id {
        record = record.with_logical_operation_id(id);
    }
    if let Some(role) = n
        .role
        .as_deref()
        .and_then(memstead_base::vcs::Role::from_wire)
    {
        record = record.with_role(role);
    }
    record = record.with_identity(n.identity);
    record
}

/// Refresh the working tree and index from `HEAD` when the just-
/// written `ref_name` matches the symbolic ref `HEAD` resolves to.
///
/// Engine writes go through `gix::Repository::commit_as`, which
/// advances the target ref in the object store but never touches the
/// index or the working tree. On a non-bare repo (the shape humans
/// open in GitHub Desktop) that drift surfaces as a spurious "deleted"
/// diff against every file the engine just wrote — and clicking
/// "commit" on that diff silently undoes the engine's work. Running
/// `git read-tree --reset -u HEAD` after each on-checked-out-branch
/// commit closes the drift. The `--reset -u` combination updates
/// tracked-file state to match HEAD and removes tracked files HEAD
/// no longer knows about; truly-untracked files in the working tree
/// are left alone.
///
/// Short-circuits as `Ok(())` when:
/// - the repo is bare (no working tree to sync);
/// - HEAD is detached, absent, or unreadable (no symbolic ref to
///   compare — a corrupted `.git/HEAD` is its own problem and should
///   not conflate with a write failure when the commit landed
///   durably);
/// - HEAD's full ref name does not match `ref_name` (we wrote to a
///   branch other than the checked-out one — syncing would clobber
///   the user's checked-out tree with content from a different
///   branch).
///
/// Spawn failure or non-zero exit maps to
/// [`BackendError::Io`] with the workdir and the captured stderr
/// in the message, plus an actionable hint pointing the caller at
/// `git -C <workdir> reset --hard HEAD` for manual recovery (the
/// commit itself already landed successfully — a sync failure leaves
/// the object store correct but the working tree stale).
///
/// Cross-process coordination is out of scope: when two engine
/// processes write the same ref concurrently, the worktree converges
/// to whoever's `read-tree` ran last; intermediate readers may see a
/// mix. Single-process engines today; the open seam is documented in
/// `mem-repo-write-cutover`'s "Open seams" section.
fn sync_index_and_worktree(repo: &gix::Repository, ref_name: &str) -> Result<(), BackendError> {
    let Some(workdir) = repo.workdir() else {
        return Ok(());
    };
    // `BStr: PartialEq<str>` is byte-exact, which matches the
    // engine's `refs/heads/<branch>` naming convention — non-ASCII
    // drift here is a real bug worth catching, not something to
    // normalise away. A `head_name()` failure (corrupted `.git/HEAD`,
    // permissions error) short-circuits the sync rather than
    // surfacing as a write failure: the commit already landed
    // durably, and a malformed HEAD will be diagnosed via the next
    // `git status` the operator runs.
    let head_matches = matches!(repo.head_name(), Ok(Some(name)) if name.as_bstr() == ref_name);
    if !head_matches {
        return Ok(());
    }

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["read-tree", "--reset", "-u", "HEAD"])
        // Never prompt: a misconfigured `core.askpass` or
        // `credential.helper` could otherwise stall the sync forever
        // on a misrouted code path.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            std::io::Error::other(format!(
                "worktree sync: spawn `git -C {} read-tree --reset -u HEAD`: {e}; \
                 commit already landed in the object store, recover with \
                 `git -C {} reset --hard HEAD`",
                workdir.display(),
                workdir.display()
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BackendError::Io(std::io::Error::other(format!(
            "worktree sync: `git -C {} read-tree --reset -u HEAD` failed (status {}): {}; \
             commit already landed in the object store, recover with \
             `git -C {} reset --hard HEAD` (or remove a stale \
             `<workdir>/.git/index.lock` if one exists)",
            workdir.display(),
            output.status,
            stderr.trim(),
            workdir.display()
        ))));
    }
    Ok(())
}

/// Deterministic committer identity — must stay byte-for-byte aligned
/// with [`crate::vcs::COMMITTER_NAME`] / `COMMITTER_EMAIL`. Re-declared
/// here as private constants to keep the `vcs` module's public surface
/// minimal; the alignment is locked by the
/// `git_tree_writer_blob_oid_matches_disk_oid` test below, which only
/// passes when both adapters produce byte-identical commit objects.
const COMMITTER_NAME: &str = "engine";
const COMMITTER_EMAIL: &str = "noreply@memstead.io";

/// One blob entry returned by [`read_branch_blobs`] — the
/// mem-relative forward-slash path and the blob bytes. The list is
/// sorted by `path` so callers (e.g. archive emitters) get
/// deterministic ordering without a re-sort.
#[derive(Debug, Clone)]
pub struct BranchBlob {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// Errors surfaced by [`read_branch_blobs`]. Wraps the gix repo /
/// reference / object operations so callers can map a missing branch
/// or a malformed object to a structured error without re-importing
/// gix's error types.
#[derive(Debug, thiserror::Error)]
pub enum BranchReadError {
    #[error("git-tree reader: open repo at {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: gix::open::Error,
    },
    #[error("git-tree reader: branch {ref_name} not found")]
    BranchMissing { ref_name: String },
    #[error("git-tree reader: resolve branch {ref_name}: {message}")]
    Resolve { ref_name: String, message: String },
    #[error("git-tree reader: read object: {message}")]
    Read { message: String },
}

/// Walk every blob in the tree pointed at by `<gitdir>:<ref_name>`'s
/// commit and return their (path, bytes) pairs sorted by path. The
/// branch tip's commit is peeled to a tree, then the tree is recursed
/// breadth-first. Subtrees are descended; symlinks and non-blob entries
/// are skipped (mem content is regular files only).
///
/// `ref_name` is the fully-qualified ref form, e.g.
/// `refs/heads/<mem>` for mem-content reads or `refs/heads/main`
/// for schema/config reads against the `mem-repo-git` repo.
///
/// A missing ref returns [`BranchReadError::BranchMissing`] so callers
/// can distinguish "branch never created" from "branch exists but is
/// empty" — the latter returns `Ok(vec![])`.
pub fn read_branch_blobs(
    gitdir: &Path,
    ref_name: &str,
) -> Result<Vec<BranchBlob>, BranchReadError> {
    let repo = gix::open(gitdir).map_err(|e| BranchReadError::Open {
        path: gitdir.display().to_string(),
        source: e,
    })?;
    let mut reference =
        match repo
            .try_find_reference(ref_name)
            .map_err(|e| BranchReadError::Resolve {
                ref_name: ref_name.to_string(),
                message: e.to_string(),
            })? {
            Some(r) => r,
            None => {
                return Err(BranchReadError::BranchMissing {
                    ref_name: ref_name.to_string(),
                });
            }
        };
    let id = reference
        .peel_to_id()
        .map_err(|e| BranchReadError::Resolve {
            ref_name: ref_name.to_string(),
            message: e.to_string(),
        })?;
    read_commit_blobs_in(&repo, id.detach())
}

/// Every blob under one commit's tree, by commit id rather than by
/// ref: the form a reader holding a snapshotted parent uses so that
/// its rows come from the same commit its staged writes compose onto.
pub fn read_commit_blobs(
    gitdir: &Path,
    commit: gix::ObjectId,
) -> Result<Vec<BranchBlob>, BranchReadError> {
    let repo = gix::open(gitdir).map_err(|e| BranchReadError::Open {
        path: gitdir.display().to_string(),
        source: e,
    })?;
    read_commit_blobs_in(&repo, commit)
}

fn read_commit_blobs_in(
    repo: &gix::Repository,
    id: gix::ObjectId,
) -> Result<Vec<BranchBlob>, BranchReadError> {
    let commit = repo
        .find_object(id)
        .map_err(|e| BranchReadError::Read {
            message: format!("open commit {id}: {e}"),
        })?
        .into_commit();
    let tree = commit.tree().map_err(|e| BranchReadError::Read {
        message: format!("peel commit to tree: {e}"),
    })?;

    let mut out: Vec<BranchBlob> = Vec::new();
    walk_tree(repo, &tree, "", &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn walk_tree(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &str,
    out: &mut Vec<BranchBlob>,
) -> Result<(), BranchReadError> {
    use gix::objs::tree::EntryKind;
    let iter = tree.iter();
    for entry_res in iter {
        let entry = entry_res.map_err(|e| BranchReadError::Read {
            message: format!("decode tree entry: {e}"),
        })?;
        let name = entry.filename().to_string();
        let full = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        match entry.mode().kind() {
            EntryKind::Blob | EntryKind::BlobExecutable => {
                let object = repo
                    .find_object(entry.oid())
                    .map_err(|e| BranchReadError::Read {
                        message: format!("read blob {full}: {e}"),
                    })?;
                out.push(BranchBlob {
                    path: full,
                    bytes: object.data.clone(),
                });
            }
            EntryKind::Tree => {
                let subtree = repo
                    .find_object(entry.oid())
                    .map_err(|e| BranchReadError::Read {
                        message: format!("read subtree {full}: {e}"),
                    })?
                    .into_tree();
                walk_tree(repo, &subtree, &full, out)?;
            }
            // Symlinks and commits (submodules) — mem content is
            // regular files only; ignore.
            EntryKind::Link | EntryKind::Commit => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
