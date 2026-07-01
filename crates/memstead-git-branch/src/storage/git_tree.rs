//! Git-tree-backed [`VaultWriter`](super::VaultWriter) — the second
//! storage adapter. Buffers mutations
//! in memory and applies them to a tree built via
//! `gix::object::tree::Editor`, then advances the target ref via
//! [`gix::Repository::commit_as`].
//!
//! No working tree is written: the vault's content lives only in the
//! multi-root `vault-repo-git` object store, one branch per vault. Each
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
//! surface [`super::VaultWriterError::HashMismatch`] with the new tip's
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

use super::{CommitId, VaultWriter, VaultWriterError};
use crate::vcs::{acquire_branch_mutex, author_identity, format_commit_message, CommitContext};

/// Per-path final state for the buffered op log. Move operations
/// resolve at call time into a `Delete(from)` + `Upsert(to, bytes)`
/// pair so commit-time replay only ever sees these two terminal states.
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
    /// Per-path final state. Values are stored vault-relative as
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

/// Git-tree-backed implementation of [`VaultWriter`]. Holds the
/// gitdir path and target ref name; opens the [`gix::Repository`]
/// per call (matches the [`crate::vcs::GixVcs`] pattern, since
/// `gix::Repository` is `Send` but not `Sync` — its object-database
/// cache uses interior mutability via `RefCell`).
///
/// Mutations buffer in memory until [`Self::commit`].
pub struct GitTreeVaultWriter {
    gitdir: PathBuf,
    ref_name: String,
    pending: Mutex<Pending>,
}

impl GitTreeVaultWriter {
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

    fn open_repo(&self) -> Result<gix::Repository, VaultWriterError> {
        gix::open(&self.gitdir).map_err(|e| {
            VaultWriterError::Path(format!(
                "git-tree writer: open repo at {}: {e}",
                self.gitdir.display()
            ))
        })
    }

    /// Capture the current tip of `ref_name` if no snapshot has been
    /// taken in this session. Idempotent: subsequent mutations reuse
    /// the same snapshot. A missing ref leaves `parent = None`.
    fn ensure_snapshot(&self, pending: &mut Pending) -> Result<(), VaultWriterError> {
        if pending.parent.is_some() || !pending.ops.is_empty() {
            return Ok(());
        }
        let repo = self.open_repo()?;
        let mut reference = match repo
            .try_find_reference(&self.ref_name)
            .map_err(|e| {
                VaultWriterError::Path(format!(
                    "git-tree writer: resolve ref {}: {e}",
                    self.ref_name
                ))
            })? {
            Some(r) => r,
            None => return Ok(()),
        };
        let id = reference.peel_to_id().map_err(|e| {
            VaultWriterError::Path(format!(
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
    fn live_tip(&self) -> Result<Option<gix::ObjectId>, VaultWriterError> {
        let repo = self.open_repo()?;
        let mut reference = match repo.try_find_reference(&self.ref_name).map_err(|e| {
            VaultWriterError::Path(format!(
                "git-tree writer: resolve ref {}: {e}",
                self.ref_name
            ))
        })? {
            Some(r) => r,
            None => return Ok(None),
        };
        let id = reference.peel_to_id().map_err(|e| {
            VaultWriterError::Path(format!(
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
    ) -> Result<Option<Vec<u8>>, VaultWriterError> {
        let repo = self.open_repo()?;
        let commit = repo
            .find_object(parent)
            .map_err(|e| {
                VaultWriterError::Path(format!("git-tree writer: open parent commit: {e}"))
            })?
            .into_commit();
        let tree = commit.tree().map_err(|e| {
            VaultWriterError::Path(format!("git-tree writer: peel commit to tree: {e}"))
        })?;
        let entry = match tree.lookup_entry_by_path(path).map_err(|e| {
            VaultWriterError::Path(format!(
                "git-tree writer: lookup {path} in parent tree: {e}"
            ))
        })? {
            Some(e) => e,
            None => return Ok(None),
        };
        if !entry.mode().is_blob() {
            return Ok(None);
        }
        let object = repo.find_object(entry.id()).map_err(|e| {
            VaultWriterError::Path(format!("git-tree writer: read blob {path}: {e}"))
        })?;
        Ok(Some(object.data.clone()))
    }
}

/// Normalise a vault-relative path to forward-slash form. Rejects
/// empty paths and any path that contains `..` segments — git tree
/// entries cannot escape upward and this guards the caller against
/// accidentally writing past the vault root via a relative-path bug.
fn normalise_rel_path(rel_path: &Path) -> Result<String, VaultWriterError> {
    if rel_path.as_os_str().is_empty() {
        return Err(VaultWriterError::Path(
            "vault-relative path is empty".to_string(),
        ));
    }
    let mut parts: Vec<String> = Vec::new();
    for component in rel_path.components() {
        use std::path::Component;
        match component {
            Component::Normal(s) => match s.to_str() {
                Some(p) if !p.is_empty() => parts.push(p.to_string()),
                _ => {
                    return Err(VaultWriterError::Path(format!(
                        "non-utf-8 or empty path component in {}",
                        rel_path.display()
                    )))
                }
            },
            Component::CurDir => continue,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(VaultWriterError::Path(format!(
                    "path traversal or absolute component in {}",
                    rel_path.display()
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(VaultWriterError::Path(
            "vault-relative path is empty after normalisation".to_string(),
        ));
    }
    Ok(parts.join("/"))
}

impl VaultWriter for GitTreeVaultWriter {
    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), VaultWriterError> {
        let key = normalise_rel_path(rel_path)?;
        let mut pending = self.pending.lock().map_err(|_| {
            VaultWriterError::Path("git-tree writer pending state poisoned".to_string())
        })?;
        self.ensure_snapshot(&mut pending)?;
        pending.ops.insert(key, PendingState::Upsert(content.to_vec()));
        Ok(())
    }

    fn delete_entity(&self, rel_path: &Path) -> Result<(), VaultWriterError> {
        let key = normalise_rel_path(rel_path)?;
        let mut pending = self.pending.lock().map_err(|_| {
            VaultWriterError::Path("git-tree writer pending state poisoned".to_string())
        })?;
        self.ensure_snapshot(&mut pending)?;
        pending.ops.insert(key, PendingState::Delete);
        Ok(())
    }

    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), VaultWriterError> {
        let from_key = normalise_rel_path(from)?;
        let to_key = normalise_rel_path(to)?;
        let mut pending = self.pending.lock().map_err(|_| {
            VaultWriterError::Path("git-tree writer pending state poisoned".to_string())
        })?;
        self.ensure_snapshot(&mut pending)?;

        // Resolve the from-content. If a pending upsert exists, take
        // its bytes; otherwise look up the blob in the snapshotted
        // parent tree. Absent from both: nothing to move.
        let bytes = match pending.ops.remove(&from_key) {
            Some(PendingState::Upsert(b)) => b,
            Some(PendingState::Delete) => {
                pending.ops.insert(from_key, PendingState::Delete);
                return Err(VaultWriterError::Path(format!(
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
                        return Err(VaultWriterError::Path(format!(
                            "move source {} does not exist",
                            from.display()
                        )));
                    }
                }
            }
        };

        if matches!(pending.ops.get(&to_key), Some(PendingState::Upsert(_))) {
            // A move refuses when the target already has a pending write.
            return Err(VaultWriterError::Path(format!(
                "move target {} already has a pending write",
                to.display()
            )));
        }
        pending.ops.insert(from_key, PendingState::Delete);
        pending.ops.insert(to_key, PendingState::Upsert(bytes));
        Ok(())
    }

    fn commit(
        &self,
        message: &str,
        ctx: &CommitContext<'_>,
    ) -> Result<CommitId, VaultWriterError> {
        // Serialise commits against the same target ref at process
        // scope. Different refs under the same gitdir proceed in
        // parallel — that is the whole point of the per-branch key.
        let mutex = acquire_branch_mutex(&self.ref_name);
        let _guard = mutex.lock().map_err(|_| {
            VaultWriterError::Path(format!(
                "git-tree writer mutex poisoned for ref {} (gitdir {})",
                self.ref_name,
                self.gitdir.display()
            ))
        })?;
        let repo = self.open_repo()?;

        let mut pending = self.pending.lock().map_err(|_| {
            VaultWriterError::Path("git-tree writer pending state poisoned".to_string())
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
                        VaultWriterError::Path(format!(
                            "git-tree writer: open parent {parent_id}: {e}"
                        ))
                    })?
                    .into_commit();
                let tree = commit.tree().map_err(|e| {
                    VaultWriterError::Path(format!("git-tree writer: peel parent tree: {e}"))
                })?;
                tree.edit().map_err(|e| {
                    VaultWriterError::Path(format!("git-tree writer: editor init: {e}"))
                })?
            }
            None => repo.empty_tree().edit().map_err(|e| {
                VaultWriterError::Path(format!("git-tree writer: empty editor init: {e}"))
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
                            VaultWriterError::Path(format!(
                                "git-tree writer: write blob for {path}: {e}"
                            ))
                        })?
                        .detach();
                    editor
                        .upsert(path.as_str(), EntryKind::Blob, blob_id)
                        .map_err(|e| {
                            VaultWriterError::Path(format!(
                                "git-tree writer: tree upsert {path}: {e}"
                            ))
                        })?;
                }
                PendingState::Delete => {
                    editor.remove(path.as_str()).map_err(|e| {
                        VaultWriterError::Path(format!(
                            "git-tree writer: tree remove {path}: {e}"
                        ))
                    })?;
                }
            }
        }

        let tree_id = editor
            .write()
            .map_err(|e| VaultWriterError::Path(format!("git-tree writer: tree write: {e}")))?
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
        // `reload_one_vault`) until the process restarts.
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
                        VaultWriterError::Path(format!(
                            "git-tree writer: re-resolve ref after CAS: {e}"
                        ))
                    })?
                    .ok_or_else(|| VaultWriterError::Path(format!(
                        "git-tree writer: ref {} vanished during CAS recovery",
                        self.ref_name
                    )))?;
                let live_id = reference.peel_to_id().map_err(|e| {
                    VaultWriterError::Path(format!(
                        "git-tree writer: peel live tip after CAS: {e}"
                    ))
                })?;
                return Err(VaultWriterError::HashMismatch {
                    current: live_id.to_hex().to_string(),
                });
            }
            Err(e) => {
                return Err(VaultWriterError::Path(format!(
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
}

impl memstead_base::backend::VaultBackend for GitTreeVaultWriter {
    fn list_entities(&self) -> Result<Vec<PathBuf>, memstead_base::backend::BackendError> {
        // Walk the per-vault branch tree, return only `.md` paths
        // outside the `.memstead/` umbrella (config / schemas / changelog
        // live there and don't surface as entities at this layer).
        // Branch-missing → empty list (a fresh vault has no commits yet).
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
        // (no pending ops — boot loads, `reload_one_vault` re-reads, any
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
            None => self
                .live_tip()
                .map_err(memstead_base::backend::BackendError::from)?,
        };
        match source {
            Some(p) => self
                .read_blob_from_parent(p, &key)
                .map_err(memstead_base::backend::BackendError::from),
            None => Ok(None),
        }
    }

    fn write_entity(
        &self,
        rel_path: &Path,
        content: &[u8],
    ) -> Result<(), memstead_base::backend::BackendError> {
        <Self as VaultWriter>::write_entity(self, rel_path, content).map_err(Into::into)
    }

    fn delete_entity(
        &self,
        rel_path: &Path,
    ) -> Result<(), memstead_base::backend::BackendError> {
        <Self as VaultWriter>::delete_entity(self, rel_path).map_err(Into::into)
    }

    fn move_entity(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<(), memstead_base::backend::BackendError> {
        <Self as VaultWriter>::move_entity(self, from, to).map_err(Into::into)
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

    fn commit(
        &self,
        message: &str,
        ctx: &CommitContext<'_>,
    ) -> Result<CommitId, memstead_base::backend::BackendError> {
        <Self as VaultWriter>::commit(self, message, ctx).map_err(Into::into)
    }

    fn commit_with_expected_parent(
        &self,
        message: &str,
        ctx: &CommitContext<'_>,
        expected_parent: Option<&str>,
    ) -> Result<CommitId, memstead_base::backend::BackendError> {
        // No pin requested → identical to commit().
        let Some(expected) = expected_parent else {
            return <Self as VaultWriter>::commit(self, message, ctx).map_err(Into::into);
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
        <Self as VaultWriter>::commit(self, message, ctx).map_err(Into::into)
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

    fn current_head(
        &self,
    ) -> Result<Option<String>, memstead_base::backend::BackendError> {
        // Open the gitdir and peel the per-vault branch ref to its
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

    fn read_vault_config(
        &self,
    ) -> Result<Option<Vec<u8>>, memstead_base::backend::BackendError> {
        // Resolve the vault leaf from `self.ref_name`. V1 unified
        // mounts are flat (`refs/heads/<leaf>`); hierarchical
        // layouts are not yet supported on the unified path.
        let leaf = self
            .ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(&self.ref_name);

        // `__MEMSTEAD:vaults/<leaf>/config.json` is the only read path.
        // Every workspace the engine touches has `__MEMSTEAD` populated
        // by boot — the legacy registry-class refs are no longer
        // read at runtime.
        Ok(read_blob_from_ref(
            &self.gitdir,
            "refs/heads/__MEMSTEAD",
            &format!("vaults/{leaf}/config.json"),
        )?)
    }

    fn delete_artifacts(
        &self,
    ) -> Result<(), memstead_base::backend::BackendError> {
        // The branch leaf is the per-vault ref minus the
        // `refs/heads/` prefix — symmetric with the resolution done
        // by `read_vault_config` / `write_vault_config` above.
        // Hierarchical layouts (e.g. `refs/heads/planning/plan-q4`)
        // strip to `planning/plan-q4`; flat layouts to the bare leaf.
        let branch_leaf = self
            .ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(&self.ref_name);
        let ctx = CommitContext {
            actor: memstead_base::vcs::Actor::Agent,
            client: None,
            tool: Some("memstead_vault_delete"),
            note: None,
            logical_operation_id: None,
            entity_ids: None,
        };
        crate::storage_memstead::delete_vault_artifacts_at_gitdir(
            &self.gitdir,
            branch_leaf,
            &ctx,
        )
        .map_err(|e| memstead_base::backend::BackendError::Other(e.to_string()))
    }

    fn write_vault_config(
        &self,
        bytes: &[u8],
    ) -> Result<(), memstead_base::backend::BackendError> {
        self.write_vault_config_with_note(bytes, None)
    }

    fn write_vault_config_with_note(
        &self,
        bytes: &[u8],
        note: Option<&str>,
    ) -> Result<(), memstead_base::backend::BackendError> {
        // Write `__MEMSTEAD:vaults/<vault>/config.json` only. The legacy
        // `vault_repo_config::read_config` consumer chain reads
        // through `__MEMSTEAD`, so a dual-write to any retired ref would
        // be wasted work.
        //
        // Vault leaf comes from `self.ref_name` (the per-vault
        // branch); for hierarchical mounts (refs/heads/<path>/<leaf>)
        // the helper's `resolve_full_path_at_gitdir` walks the
        // branch list to find the matching full path. For a fresh
        // vault not yet present in the branch list, the helper
        // falls back to the flat `<leaf>/config.json` shape —
        // unified `create_vault` writes the per-vault branch commit
        // AFTER this call, so during the very first
        // write_vault_config the branch isn't yet present.
        // Hierarchical-path semantics for fresh vaults need a
        // small lift in a follow-up (pass full path explicitly).
        //
        // `note` rides the commit body so a version bump (or any
        // config write that supplies one) carries the same provenance
        // reason the other commit-producing lifecycle operations do.
        let leaf = self
            .ref_name
            .strip_prefix("refs/heads/")
            .unwrap_or(&self.ref_name);
        let ctx = CommitContext {
            actor: memstead_base::vcs::Actor::Agent,
            client: None,
            tool: Some("memstead_vault_config_write"),
            note: note.map(str::to_string),
            logical_operation_id: None,
            entity_ids: None,
        };
        crate::storage_memstead::commit_config_to_memstead_at_gitdir(
            &self.gitdir,
            leaf,
            bytes,
            &ctx,
            &format!("memstead: commit __MEMSTEAD:vaults/{leaf}/config.json"),
        )
        .map_err(|e| memstead_base::backend::BackendError::Other(e.to_string()))
    }

}

/// Read a blob from `ref_name:path` in the gitdir. Returns
/// `Ok(None)` when the ref is missing or the path doesn't exist
/// in the tree. Errors propagate as `BackendError::Other`.
///
/// Used by `read_vault_config` to read per-vault configs from
/// `__MEMSTEAD` without needing a full pro `VaultConfig` parser path —
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
    let object = id
        .object()
        .map_err(|e| memstead_base::backend::BackendError::Other(format!("read obj {ref_name}: {e}")))?;
    let commit = match object.try_into_commit() {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let tree = commit
        .tree()
        .map_err(|e| memstead_base::backend::BackendError::Other(format!("read tree {ref_name}: {e}")))?;
    let entry = match tree.lookup_entry_by_path(path) {
        Ok(Some(e)) => e,
        Ok(None) => return Ok(None),
        Err(e) => {
            return Err(memstead_base::backend::BackendError::Other(format!(
                "lookup {ref_name}:{path}: {e}"
            )));
        }
    };
    let blob = entry
        .object()
        .map_err(|e| memstead_base::backend::BackendError::Other(format!("read blob {ref_name}:{path}: {e}")))?;
    Ok(Some(blob.data.clone()))
}

/// Build a [`memstead_base::Provenance`] from a parsed commit note. Best-
/// effort: unrecognised verbs map to `Update`, missing actors to
/// `Unknown`, malformed client trailers drop the field. Matches the
/// folder backend's tolerant-reader stance.
fn commit_note_to_provenance(
    n: crate::ops::agent_notes::CommitNote,
) -> memstead_base::Provenance {
    let kind = n
        .tool_verb
        .as_deref()
        .and_then(memstead_base::ProvenanceKind::from_str)
        .unwrap_or(memstead_base::ProvenanceKind::Update);
    let actor = n
        .actor
        .as_deref()
        .and_then(memstead_base::vcs::Actor::from_trailer)
        .unwrap_or(memstead_base::vcs::Actor::Unknown);
    let client = n.client.as_deref().and_then(memstead_base::vcs::parse_client_id);
    let timestamp = if n.timestamp >= 0 {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(n.timestamp as u64)
    } else {
        std::time::UNIX_EPOCH
    };
    let mut record = memstead_base::Provenance::new(timestamp, kind, n.entity_id, actor, client, n.note);
    if let Some(id) = n.logical_operation_id {
        record = record.with_logical_operation_id(id);
    }
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
/// [`VaultWriterError::Io`] with the workdir and the captured stderr
/// in the message, plus an actionable hint pointing the caller at
/// `git -C <workdir> reset --hard HEAD` for manual recovery (the
/// commit itself already landed successfully — a sync failure leaves
/// the object store correct but the working tree stale).
///
/// Cross-process coordination is out of scope: when two engine
/// processes write the same ref concurrently, the worktree converges
/// to whoever's `read-tree` ran last; intermediate readers may see a
/// mix. Single-process engines today; the open seam is documented in
/// `vault-repo-write-cutover`'s "Open seams" section.
fn sync_index_and_worktree(
    repo: &gix::Repository,
    ref_name: &str,
) -> Result<(), VaultWriterError> {
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
        return Err(VaultWriterError::Io(std::io::Error::other(format!(
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
/// vault-relative forward-slash path and the blob bytes. The list is
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
/// are skipped (vault content is regular files only).
///
/// `ref_name` is the fully-qualified ref form, e.g.
/// `refs/heads/<vault>` for vault-content reads or `refs/heads/main`
/// for schema/config reads against the `vault-repo-git` repo.
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
    let mut reference = match repo.try_find_reference(ref_name).map_err(|e| {
        BranchReadError::Resolve {
            ref_name: ref_name.to_string(),
            message: e.to_string(),
        }
    })? {
        Some(r) => r,
        None => {
            return Err(BranchReadError::BranchMissing {
                ref_name: ref_name.to_string(),
            });
        }
    };
    let id = reference.peel_to_id().map_err(|e| BranchReadError::Resolve {
        ref_name: ref_name.to_string(),
        message: e.to_string(),
    })?;
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
    walk_tree(&repo, &tree, "", &mut out)?;
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
                let object = repo.find_object(entry.oid()).map_err(|e| {
                    BranchReadError::Read {
                        message: format!("read blob {full}: {e}"),
                    }
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
            // Symlinks and commits (submodules) — vault content is
            // regular files only; ignore.
            EntryKind::Link | EntryKind::Commit => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::{Actor, ClientId, CommitContext};
    use std::path::Path;
    use tempfile::TempDir;

    /// Build a fresh bare repo at `<tmp>/vault-repo.git` and return the
    /// canonical gitdir path. Tests open the repo per-call via
    /// `gix::open` so the writer's `gitdir + ref_name` shape is what
    /// gets exercised.
    fn fresh_repo_dir(tmp: &Path) -> PathBuf {
        let git_dir = tmp.join("vault-repo.git");
        gix::init_bare(&git_dir).unwrap();
        std::fs::canonicalize(&git_dir).unwrap()
    }

    fn ctx_for_test<'a>() -> CommitContext<'a> {
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

    fn read_blob(gitdir: &Path, ref_name: &str, path: &str) -> Option<Vec<u8>> {
        let repo = gix::open(gitdir).unwrap();
        let mut reference = repo.try_find_reference(ref_name).unwrap()?;
        let id = reference.peel_to_id().unwrap();
        let commit = repo.find_object(id).unwrap().into_commit();
        let tree = commit.tree().unwrap();
        let entry = tree.lookup_entry_by_path(path).unwrap()?;
        let object = repo.find_object(entry.id()).unwrap();
        Some(object.data.clone())
    }

    fn tree_path_exists(gitdir: &Path, ref_name: &str, path: &str) -> bool {
        read_blob(gitdir, ref_name, path).is_some()
    }

    #[test]
    fn git_tree_writer_round_trip() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        writer
            .write_entity(Path::new("notes/hello.md"), b"# hi\n")
            .unwrap();
        let sha = writer.commit("first commit", &ctx_for_test()).unwrap();
        assert_eq!(sha.len(), 40);

        let bytes = read_blob(&gitdir, "refs/heads/test", "notes/hello.md").unwrap();
        assert_eq!(bytes, b"# hi\n");
    }

    #[test]
    fn git_tree_writer_delete_removes_path() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        writer.write_entity(Path::new("a.md"), b"a").unwrap();
        writer.write_entity(Path::new("b.md"), b"b").unwrap();
        writer.commit("seed", &ctx_for_test()).unwrap();

        writer.delete_entity(Path::new("a.md")).unwrap();
        writer.commit("drop a", &ctx_for_test()).unwrap();

        assert!(!tree_path_exists(&gitdir, "refs/heads/test", "a.md"));
        assert!(tree_path_exists(&gitdir, "refs/heads/test", "b.md"));
    }

    #[test]
    fn git_tree_writer_move_renames_path() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        writer.write_entity(Path::new("from.md"), b"payload").unwrap();
        writer.commit("seed", &ctx_for_test()).unwrap();

        writer
            .move_entity(Path::new("from.md"), Path::new("nested/to.md"))
            .unwrap();
        writer.commit("rename", &ctx_for_test()).unwrap();

        assert!(!tree_path_exists(&gitdir, "refs/heads/test", "from.md"));
        let moved = read_blob(&gitdir, "refs/heads/test", "nested/to.md").unwrap();
        assert_eq!(moved, b"payload");
    }

    #[test]
    fn git_tree_writer_multi_op_commit() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        // Seed an entry that will be deleted in the same multi-op
        // commit as two new writes.
        writer.write_entity(Path::new("doomed.md"), b"x").unwrap();
        writer.commit("seed", &ctx_for_test()).unwrap();

        writer.write_entity(Path::new("a.md"), b"alpha").unwrap();
        writer.write_entity(Path::new("nested/b.md"), b"beta").unwrap();
        writer.delete_entity(Path::new("doomed.md")).unwrap();
        writer.commit("multi-op", &ctx_for_test()).unwrap();

        assert!(!tree_path_exists(&gitdir, "refs/heads/test", "doomed.md"));
        assert_eq!(
            read_blob(&gitdir, "refs/heads/test", "a.md").unwrap(),
            b"alpha"
        );
        assert_eq!(
            read_blob(&gitdir, "refs/heads/test", "nested/b.md").unwrap(),
            b"beta"
        );
    }

    #[test]
    fn git_tree_writer_cas_conflict_surfaces_hash_mismatch() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());

        // Seed so both writers snapshot the same parent SHA.
        let seeder = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());
        seeder.write_entity(Path::new("seed.md"), b"x").unwrap();
        let seed_sha = seeder.commit("seed", &ctx_for_test()).unwrap();

        let a = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());
        let b = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        // Both writers take their snapshot at the same parent.
        a.write_entity(Path::new("a.md"), b"a").unwrap();
        b.write_entity(Path::new("b.md"), b"b").unwrap();
        assert_eq!(
            a.pending.lock().unwrap().parent.unwrap().to_hex().to_string(),
            seed_sha
        );
        assert_eq!(
            b.pending.lock().unwrap().parent.unwrap().to_hex().to_string(),
            seed_sha
        );

        // A commits, advancing the ref. B then tries to commit and
        // gets the typed CAS conflict.
        let new_tip = a.commit("a wins", &ctx_for_test()).unwrap();
        let err = b
            .commit("b loses", &ctx_for_test())
            .expect_err("B's commit must fail with HashMismatch");
        match err {
            VaultWriterError::HashMismatch { current } => {
                assert_eq!(current, new_tip);
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn cas_conflict_clears_pending_so_reads_fall_back_to_committed_truth() {
        // Regression: a commit that loses the CAS race must ABORT its
        // staged ops. Before the fix, `pending` was left populated on a
        // CAS conflict, and because `read_entity` prefers pending over the
        // committed tip, the loser's never-committed write was served as
        // phantom truth (and a later `reload_one_vault` pulled it into the
        // in-memory store) until the process restarted.
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());

        // Seed a shared entity both writers will target, so they snapshot
        // the same parent SHA.
        let seeder = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());
        seeder.write_entity(Path::new("shared.md"), b"v1").unwrap();
        seeder.commit("seed", &ctx_for_test()).unwrap();

        let a = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());
        let b = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        // Both snapshot the same parent, then stage conflicting updates to
        // the SAME entity.
        a.write_entity(Path::new("shared.md"), b"A-committed").unwrap();
        b.write_entity(Path::new("shared.md"), b"B-phantom").unwrap();

        // A wins the race; B's commit hits the typed CAS conflict.
        a.commit("a wins", &ctx_for_test()).unwrap();
        let err = b
            .commit("b loses", &ctx_for_test())
            .expect_err("B must lose the CAS race");
        assert!(
            matches!(err, VaultWriterError::HashMismatch { .. }),
            "expected HashMismatch, got {err:?}"
        );

        // The failed transaction must be aborted: B's pending buffer empty…
        assert!(
            b.pending.lock().unwrap().ops.is_empty(),
            "pending must be cleared after a failed commit"
        );
        // …so a read falls through to the committed tip and returns A's
        // value, NOT B's orphaned "B-phantom" staged write.
        let read = <GitTreeVaultWriter as memstead_base::backend::VaultBackend>::read_entity(
            &b,
            Path::new("shared.md"),
        )
        .unwrap();
        assert_eq!(
            read.as_deref(),
            Some(&b"A-committed"[..]),
            "read must serve committed truth, not the phantom staged write"
        );
    }

    #[test]
    fn commit_with_expected_parent_succeeds_when_ref_matches_pin() {
        use memstead_base::backend::VaultBackend;

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());

        let seeder = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());
        <GitTreeVaultWriter as VaultWriter>::write_entity(&seeder, Path::new("seed.md"), b"x")
            .unwrap();
        let seed_sha = <GitTreeVaultWriter as VaultWriter>::commit(
            &seeder,
            "seed",
            &ctx_for_test(),
        )
        .unwrap();

        // Engine-style flow: snapshot head, mutate, then commit pinned.
        let writer = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());
        let expected = <GitTreeVaultWriter as VaultBackend>::current_head(&writer)
            .unwrap()
            .expect("seeded ref has a head");
        assert_eq!(expected, seed_sha);

        <GitTreeVaultWriter as VaultWriter>::write_entity(&writer, Path::new("after.md"), b"after")
            .unwrap();

        let new_tip = <GitTreeVaultWriter as VaultBackend>::commit_with_expected_parent(
            &writer,
            "pinned commit",
            &ctx_for_test(),
            Some(&expected),
        )
        .expect("parent matches pin → commit must succeed");
        assert_ne!(new_tip, seed_sha);
        assert_eq!(
            read_blob(&gitdir, "refs/heads/test", "after.md").unwrap(),
            b"after"
        );
    }

    #[test]
    fn commit_with_expected_parent_surfaces_parent_mismatch_when_sibling_advances_ref() {
        use memstead_base::backend::{BackendError, VaultBackend};

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());

        // Seed so both writers start from the same commit.
        let seeder = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());
        <GitTreeVaultWriter as VaultWriter>::write_entity(&seeder, Path::new("seed.md"), b"x")
            .unwrap();
        let seed_sha = <GitTreeVaultWriter as VaultWriter>::commit(
            &seeder,
            "seed",
            &ctx_for_test(),
        )
        .unwrap();

        // Engine A snapshots head — this is the pin it will retain
        // through any number of intermediate writes.
        let a = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());
        let pin = <GitTreeVaultWriter as VaultBackend>::current_head(&a)
            .unwrap()
            .expect("seeded ref has a head");
        assert_eq!(pin, seed_sha);

        // A sibling writer (another engine instance, manual git op,
        // out-of-band CLI invocation, …) advances the ref between A's
        // snapshot and A's commit attempt.
        let sibling = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());
        <GitTreeVaultWriter as VaultWriter>::write_entity(
            &sibling,
            Path::new("drift.md"),
            b"drift",
        )
        .unwrap();
        let new_tip = <GitTreeVaultWriter as VaultWriter>::commit(
            &sibling,
            "sibling advance",
            &ctx_for_test(),
        )
        .unwrap();
        assert_ne!(new_tip, seed_sha);

        // A now tries to land a pinned commit. The pin no longer
        // matches the live tip → typed `ParentMismatch`.
        <GitTreeVaultWriter as VaultWriter>::write_entity(&a, Path::new("a.md"), b"a").unwrap();
        let err = <GitTreeVaultWriter as VaultBackend>::commit_with_expected_parent(
            &a,
            "pinned commit",
            &ctx_for_test(),
            Some(&pin),
        )
        .expect_err("pin no longer matches live tip → commit must refuse");
        match err {
            BackendError::ParentMismatch { expected, actual } => {
                assert_eq!(expected, pin);
                assert_eq!(actual, new_tip);
            }
            other => panic!("expected ParentMismatch, got {other:?}"),
        }
    }

    #[test]
    fn commit_with_expected_parent_none_pin_is_equivalent_to_commit() {
        use memstead_base::backend::VaultBackend;

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        <GitTreeVaultWriter as VaultWriter>::write_entity(&writer, Path::new("hello.md"), b"hi")
            .unwrap();
        let sha = <GitTreeVaultWriter as VaultBackend>::commit_with_expected_parent(
            &writer,
            "unpinned",
            &ctx_for_test(),
            None,
        )
        .expect("None pin → plain commit semantics, must succeed against empty ref");
        assert_eq!(sha.len(), 40);
        assert_eq!(
            read_blob(&gitdir, "refs/heads/test", "hello.md").unwrap(),
            b"hi"
        );
    }

    #[test]
    fn git_tree_writer_blob_oid_is_content_addressed() {
        // The git-tree writer is content-addressed: writing the same
        // bytes through two independent writers must yield the same
        // blob OID, since the OID is a hash of the content.
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        let payload = b"shared content\n";

        let gitdir_a = fresh_repo_dir(tmp_a.path());
        let writer_a = GitTreeVaultWriter::new(gitdir_a.clone(), "refs/heads/a".to_string());
        writer_a
            .write_entity(Path::new("file.md"), payload)
            .unwrap();
        let sha_a = writer_a.commit("a", &ctx_for_test()).unwrap();
        let repo_a = gix::open(&gitdir_a).unwrap();
        let commit_a = repo_a
            .find_object(gix::ObjectId::from_hex(sha_a.as_bytes()).unwrap())
            .unwrap()
            .into_commit();
        let blob_id_a = commit_a
            .tree()
            .unwrap()
            .lookup_entry_by_path("file.md")
            .unwrap()
            .unwrap()
            .id()
            .detach();

        let gitdir_b = fresh_repo_dir(tmp_b.path());
        let writer_b = GitTreeVaultWriter::new(gitdir_b.clone(), "refs/heads/b".to_string());
        writer_b
            .write_entity(Path::new("file.md"), payload)
            .unwrap();
        let sha_b = writer_b.commit("b", &ctx_for_test()).unwrap();
        let repo_b = gix::open(&gitdir_b).unwrap();
        let commit_b = repo_b
            .find_object(gix::ObjectId::from_hex(sha_b.as_bytes()).unwrap())
            .unwrap()
            .into_commit();
        let blob_id_b = commit_b
            .tree()
            .unwrap()
            .lookup_entry_by_path("file.md")
            .unwrap()
            .unwrap()
            .id()
            .detach();

        assert_eq!(
            blob_id_a, blob_id_b,
            "same content must produce byte-identical blob OIDs"
        );
    }

    /// Initialise a non-bare repo at `<workdir>` with `refs/heads/main`
    /// as the symbolic HEAD. Returns `(workdir, gitdir)` — the workdir
    /// is what GitHub Desktop would open; the gitdir is what
    /// `GitTreeVaultWriter::new` consumes.
    fn fresh_non_bare_repo(tmp: &Path) -> (PathBuf, PathBuf) {
        let workdir = tmp.join("vault-repo-workdir");
        std::fs::create_dir_all(&workdir).unwrap();
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&workdir)
            .args(["init", "--initial-branch=main", "--quiet"])
            .status()
            .expect("git init must succeed");
        assert!(status.success(), "git init failed");
        let workdir = std::fs::canonicalize(&workdir).unwrap();
        let gitdir = workdir.join(".git");
        (workdir, gitdir)
    }

    #[test]
    fn sync_helper_skips_on_bare_repo() {
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let repo = gix::open(&gitdir).unwrap();

        // No working tree exists — the helper must short-circuit Ok(())
        // regardless of the ref name passed.
        sync_index_and_worktree(&repo, "refs/heads/main").unwrap();
    }

    #[test]
    fn sync_helper_updates_worktree_when_ref_matches_head() {
        let tmp = TempDir::new().unwrap();
        let (workdir, gitdir) = fresh_non_bare_repo(tmp.path());
        let writer = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/main".to_string());

        writer
            .write_entity(Path::new("configs/alpha.json"), b"{\"name\":\"alpha\"}\n")
            .unwrap();
        writer.commit("seed alpha", &ctx_for_test()).unwrap();

        // The writer's commit() invokes sync_index_and_worktree via the
        // post-commit hook — the file must now exist on disk.
        let on_disk = workdir.join("configs/alpha.json");
        assert!(
            on_disk.exists(),
            "worktree sync must materialise the new blob at {}",
            on_disk.display()
        );
        let bytes = std::fs::read(&on_disk).unwrap();
        assert_eq!(bytes, b"{\"name\":\"alpha\"}\n");

        // git status is also clean (HEAD == index == worktree).
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&workdir)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            output.stdout.is_empty(),
            "git status --porcelain must be empty post-sync, got: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn sync_helper_skips_when_ref_does_not_match_head() {
        let tmp = TempDir::new().unwrap();
        let (workdir, gitdir) = fresh_non_bare_repo(tmp.path());

        // Write to refs/heads/feature; HEAD still points at
        // refs/heads/main. The worktree must NOT receive the feature
        // branch's content.
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/feature".to_string());
        writer
            .write_entity(Path::new("only-on-feature.md"), b"feature-only\n")
            .unwrap();
        writer.commit("first commit on feature", &ctx_for_test()).unwrap();

        // Object store has the blob on the feature branch...
        assert!(tree_path_exists(
            &gitdir,
            "refs/heads/feature",
            "only-on-feature.md"
        ));
        // ...but the worktree (which reflects main) does not.
        assert!(
            !workdir.join("only-on-feature.md").exists(),
            "worktree must not be polluted by writes to a non-checked-out branch"
        );
    }

    #[test]
    fn sync_helper_preserves_untracked_files() {
        let tmp = TempDir::new().unwrap();
        let (workdir, gitdir) = fresh_non_bare_repo(tmp.path());
        let writer = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/main".to_string());

        // Drop an untracked file in the workdir before any engine
        // commit runs. `git read-tree --reset -u HEAD` only touches
        // tracked-file state; untracked content must survive.
        let untracked = workdir.join("scratch.txt");
        std::fs::write(&untracked, b"operator notes\n").unwrap();

        writer
            .write_entity(Path::new("seed.md"), b"seed\n")
            .unwrap();
        writer.commit("create seed", &ctx_for_test()).unwrap();

        assert!(
            untracked.exists(),
            "sync must leave untracked files in place"
        );
        assert_eq!(
            std::fs::read(&untracked).unwrap(),
            b"operator notes\n"
        );
        // The tracked entity is also materialised.
        assert_eq!(std::fs::read(workdir.join("seed.md")).unwrap(), b"seed\n");
    }

    #[test]
    fn sync_helper_updates_through_delete_and_overwrite() {
        let tmp = TempDir::new().unwrap();
        let (workdir, gitdir) = fresh_non_bare_repo(tmp.path());
        let writer = GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/main".to_string());

        writer.write_entity(Path::new("a.md"), b"first\n").unwrap();
        writer.commit("create a", &ctx_for_test()).unwrap();
        assert_eq!(
            std::fs::read(workdir.join("a.md")).unwrap(),
            b"first\n"
        );

        writer
            .write_entity(Path::new("a.md"), b"second\n")
            .unwrap();
        writer.commit("overwrite a", &ctx_for_test()).unwrap();
        assert_eq!(
            std::fs::read(workdir.join("a.md")).unwrap(),
            b"second\n",
            "overwrite must propagate to the worktree"
        );

        writer.delete_entity(Path::new("a.md")).unwrap();
        writer.commit("delete a", &ctx_for_test()).unwrap();
        assert!(
            !workdir.join("a.md").exists(),
            "delete must remove the file from the worktree"
        );

        // git status remains clean across all three transitions.
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&workdir)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            output.stdout.is_empty(),
            "git status --porcelain must be empty after every commit, got: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    // ----- VaultBackend impl -----------------------------------------

    /// Build a CommitContext that produces an `memstead: <verb> <id>`
    /// subject with a given verb. The agent-notes parser keys off the
    /// subject's verb to recover the mutation kind.
    fn commit_with_verb(
        writer: &GitTreeVaultWriter,
        verb: &str,
        entity_id: &str,
        ctx: &CommitContext<'_>,
    ) {
        let subject = format!("memstead: {verb} {entity_id}");
        <GitTreeVaultWriter as VaultWriter>::commit(writer, &subject, ctx).unwrap();
    }

    fn ctx_with_note<'a>(note: &'a str) -> CommitContext<'a> {
        CommitContext {
            actor: Actor::Agent,
            client: Some(ClientId {
                name: "claude-code".to_string(),
                version: "2.1.0".to_string(),
            }),
            tool: Some("memstead_create"),
            note: Some(note.to_string()),
            logical_operation_id: None,
            entity_ids: None,
        }
    }

    #[test]
    fn backend_list_entities_returns_only_md_outside_memstead_namespace() {
        use memstead_base::backend::VaultBackend;

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        // Seed via VaultWriter (fully-qualified to avoid trait
        // ambiguity once VaultBackend enters scope below).
        <GitTreeVaultWriter as VaultWriter>::write_entity(&writer, Path::new("a.md"), b"# a")
            .unwrap();
        <GitTreeVaultWriter as VaultWriter>::write_entity(
            &writer,
            Path::new("nested/b.md"),
            b"# b",
        )
        .unwrap();
        <GitTreeVaultWriter as VaultWriter>::write_entity(
            &writer,
            Path::new("notes.json"),
            b"{}",
        )
        .unwrap();
        <GitTreeVaultWriter as VaultWriter>::write_entity(
            &writer,
            Path::new(".memstead/config.json"),
            b"{}",
        )
        .unwrap();
        <GitTreeVaultWriter as VaultWriter>::write_entity(
            &writer,
            Path::new(".memstead/notes.md"),
            b"# skip me",
        )
        .unwrap();
        <GitTreeVaultWriter as VaultWriter>::write_entity(
            &writer,
            Path::new(".other/notes.md"),
            b"# no longer special, walked like any non-meta dir",
        )
        .unwrap();
        <GitTreeVaultWriter as VaultWriter>::commit(&writer, "seed", &ctx_for_test()).unwrap();

        let backend: &dyn VaultBackend = &writer;
        let mut paths: Vec<String> = backend
            .list_entities()
            .unwrap()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        paths.sort();
        // `.memstead/` stays skipped; an ordinary dot-dir is walked.
        assert_eq!(
            paths,
            vec![
                ".other/notes.md".to_string(),
                "a.md".to_string(),
                "nested/b.md".to_string(),
            ]
        );
    }

    #[test]
    fn backend_list_entities_returns_empty_for_missing_branch() {
        use memstead_base::backend::VaultBackend;

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(gitdir, "refs/heads/never".to_string());
        let backend: &dyn VaultBackend = &writer;
        // Branch never created → empty, no error.
        assert!(backend.list_entities().unwrap().is_empty());
    }

    #[test]
    fn backend_read_entity_consults_pending_then_branch_tip() {
        use memstead_base::backend::VaultBackend;

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        // Seed a committed entry.
        <GitTreeVaultWriter as VaultWriter>::write_entity(
            &writer,
            Path::new("on_branch.md"),
            b"branch",
        )
        .unwrap();
        <GitTreeVaultWriter as VaultWriter>::commit(&writer, "seed", &ctx_for_test()).unwrap();

        let backend: &dyn VaultBackend = &writer;
        // Branch path → reads from the branch tip.
        assert_eq!(
            backend.read_entity(Path::new("on_branch.md")).unwrap(),
            Some(b"branch".to_vec())
        );
        // Buffered upsert wins over the branch tip.
        backend
            .write_entity(Path::new("on_branch.md"), b"buffered")
            .unwrap();
        assert_eq!(
            backend.read_entity(Path::new("on_branch.md")).unwrap(),
            Some(b"buffered".to_vec())
        );
        // Buffered delete masks the branch.
        backend.delete_entity(Path::new("on_branch.md")).unwrap();
        assert_eq!(backend.read_entity(Path::new("on_branch.md")).unwrap(), None);
        // Unknown path → None.
        assert_eq!(backend.read_entity(Path::new("never.md")).unwrap(), None);
    }

    #[test]
    fn backend_read_provenance_reconstructs_from_commit_log() {
        use memstead_base::backend::VaultBackend;

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        // Two commits with memstead: subjects so the verb maps back to a
        // ProvenanceKind. The first carries an agent note, the second
        // does not.
        <GitTreeVaultWriter as VaultWriter>::write_entity(&writer, Path::new("a.md"), b"a")
            .unwrap();
        commit_with_verb(&writer, "create", "v:a", &ctx_with_note("first draft"));

        <GitTreeVaultWriter as VaultWriter>::write_entity(&writer, Path::new("a.md"), b"a2")
            .unwrap();
        commit_with_verb(
            &writer,
            "update",
            "v:a",
            &CommitContext {
                actor: Actor::Cli,
                client: None,
                tool: Some("memstead_update"),
                note: None,
                logical_operation_id: None,
                entity_ids: None,
            },
        );

        let backend: &dyn VaultBackend = &writer;
        // append_provenance is the no-op contract; calling it with a
        // throw-away record must not perturb the read path.
        backend
            .append_provenance(&memstead_base::Provenance::new(
                std::time::UNIX_EPOCH,
                memstead_base::ProvenanceKind::Create,
                Some("ignored".into()),
                Actor::Unknown,
                None,
                None,
            ))
            .unwrap();

        let records = backend.read_provenance(None).unwrap();
        assert_eq!(records.len(), 2, "expected two commits, got {records:?}");
        // Oldest-first ordering (matches folder backend).
        assert_eq!(records[0].kind, memstead_base::ProvenanceKind::Create);
        assert_eq!(records[0].entity.as_deref(), Some("v:a"));
        assert_eq!(records[0].actor, Actor::Agent);
        assert_eq!(records[0].note.as_deref(), Some("first draft"));
        assert_eq!(
            records[0]
                .client
                .as_ref()
                .map(|c| (c.name.as_str(), c.version.as_str())),
            Some(("claude-code", "2.1.0"))
        );
        assert_eq!(records[1].kind, memstead_base::ProvenanceKind::Update);
        assert_eq!(records[1].actor, Actor::Cli);
        assert!(records[1].note.is_none());
        assert!(records[1].client.is_none());
    }

    #[test]
    fn backend_read_provenance_filters_by_cursor_sha() {
        use memstead_base::backend::VaultBackend;

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        // Seed three commits; the cursor will be the SHA of the first.
        <GitTreeVaultWriter as VaultWriter>::write_entity(&writer, Path::new("a.md"), b"a")
            .unwrap();
        let first_sha =
            <GitTreeVaultWriter as VaultWriter>::commit(&writer, "memstead: create v:a", &ctx_for_test())
                .unwrap();
        <GitTreeVaultWriter as VaultWriter>::write_entity(&writer, Path::new("a.md"), b"a2")
            .unwrap();
        <GitTreeVaultWriter as VaultWriter>::commit(&writer, "memstead: update v:a", &ctx_for_test())
            .unwrap();
        <GitTreeVaultWriter as VaultWriter>::write_entity(&writer, Path::new("a.md"), b"a3")
            .unwrap();
        <GitTreeVaultWriter as VaultWriter>::commit(&writer, "memstead: update v:a", &ctx_for_test())
            .unwrap();

        let backend: &dyn VaultBackend = &writer;
        // Cursor at the first SHA → returns only the two newer commits.
        let after = backend.read_provenance(Some(&first_sha)).unwrap();
        assert_eq!(after.len(), 2, "expected commits after cursor, got {after:?}");
        for r in &after {
            assert_eq!(r.kind, memstead_base::ProvenanceKind::Update);
        }
    }

    #[test]
    fn backend_read_provenance_empty_for_missing_branch() {
        use memstead_base::backend::VaultBackend;

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(gitdir, "refs/heads/never".to_string());
        let backend: &dyn VaultBackend = &writer;
        // No commits yet → empty record list, no error.
        assert!(backend.read_provenance(None).unwrap().is_empty());
    }

    #[test]
    fn backend_unknown_verb_falls_back_to_update_kind() {
        use memstead_base::backend::VaultBackend;

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer =
            GitTreeVaultWriter::new(gitdir.clone(), "refs/heads/test".to_string());

        <GitTreeVaultWriter as VaultWriter>::write_entity(&writer, Path::new("a.md"), b"a")
            .unwrap();
        // Verb that isn't in the ProvenanceKind enum (e.g. lifecycle
        // verbs like `vault_create`) — round-trips as Update under the
        // tolerant-reader convention shared with the folder backend.
        commit_with_verb(&writer, "vault_create", "v:a", &ctx_for_test());

        let backend: &dyn VaultBackend = &writer;
        let records = backend.read_provenance(None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, memstead_base::ProvenanceKind::Update);
    }

    #[test]
    fn instantiate_pro_backend_constructs_git_branch_writer() {
        // Smoke test: instantiate_pro_backend on a GitBranch mount
        // produces a backend that can list against an empty branch
        // without erroring (proves the writer is wired with the
        // right gitdir + ref shape).
        use memstead_base::{
            Mount, MountCapability, MountLifecycle, MountStorage, VaultBackend,
        };

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let mount = Mount {
            vault: "engine".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::GitBranch {
                gitdir,
                branch: "engine".to_string(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let backend: Box<dyn VaultBackend> =
            crate::storage::instantiate_pro_backend(&mount).unwrap();
        // Empty branch → empty list, no error.
        assert!(backend.list_entities().unwrap().is_empty());
        // Provenance log on a fresh branch → empty.
        assert!(backend.read_provenance(None).unwrap().is_empty());
    }

    #[test]
    fn instantiate_pro_backend_accepts_branch_with_or_without_refs_prefix() {
        // The pro instantiator normalises a bare branch name
        // ("engine") to its fully-qualified ref ("refs/heads/engine").
        // Mounts may carry either shape; the writer must end up keyed
        // on the same per-branch mutex regardless.
        use memstead_base::{
            Mount, MountCapability, MountLifecycle, MountStorage, VaultBackend,
        };

        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        for branch in ["engine", "refs/heads/engine"] {
            let mount = Mount {
                vault: "engine".to_string(),
                schema: Some("default@1.0.0".parse().unwrap()),
                storage: MountStorage::GitBranch {
                    gitdir: gitdir.clone(),
                    branch: branch.to_string(),
                },
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
            migration_target: None,
        };
            let backend: Box<dyn VaultBackend> =
                crate::storage::instantiate_pro_backend(&mount).unwrap();
            // Both shapes resolve cleanly (no panic, no error).
            assert!(backend.list_entities().unwrap().is_empty());
        }
    }

    // ---- VaultBackend::current_head ----------------------------------

    #[test]
    fn current_head_returns_none_for_empty_branch() {
        // A fresh bare repo has no commits and no branches; the
        // writer's `try_find_reference` returns Ok(None) and
        // current_head collapses to Ok(None) — drift detection on
        // an unborn vault is a clean no-op.
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer =
            GitTreeVaultWriter::new(gitdir, "refs/heads/specs".to_string());
        let head = <GitTreeVaultWriter as memstead_base::backend::VaultBackend>::current_head(&writer).unwrap();
        assert!(head.is_none());
    }

    #[test]
    fn current_head_returns_hex_sha_after_commit() {
        // After the first commit, current_head returns the 40-char
        // hex SHA matching what `commit` returned. The two values
        // are read through different paths (commit returns the value
        // straight from the writer; current_head re-opens the gitdir
        // and peels the ref) so equality proves end-to-end consistency.
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(
            gitdir.clone(),
            "refs/heads/specs".to_string(),
        );

        writer.write_entity(Path::new("a.md"), b"a").unwrap();
        let sha = writer.commit("first", &ctx_for_test()).unwrap();
        assert_eq!(sha.len(), 40);

        let head =
            <GitTreeVaultWriter as memstead_base::backend::VaultBackend>::current_head(&writer)
                .unwrap()
                .expect("head present after commit");
        assert_eq!(head, sha);
    }

    #[test]
    fn current_head_advances_on_subsequent_commits() {
        // Two back-to-back commits produce two distinct SHAs;
        // current_head reflects the latest after each. This is the
        // signal Engine::reload_if_stale compares against the
        // cached last_known_head to detect a sibling writer.
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(
            gitdir.clone(),
            "refs/heads/specs".to_string(),
        );

        writer.write_entity(Path::new("a.md"), b"a").unwrap();
        let first = writer.commit("first", &ctx_for_test()).unwrap();
        let head_after_first =
            <GitTreeVaultWriter as memstead_base::backend::VaultBackend>::current_head(&writer)
                .unwrap()
                .unwrap();
        assert_eq!(head_after_first, first);

        writer.write_entity(Path::new("b.md"), b"b").unwrap();
        let second = writer.commit("second", &ctx_for_test()).unwrap();
        assert_ne!(first, second);
        let head_after_second =
            <GitTreeVaultWriter as memstead_base::backend::VaultBackend>::current_head(&writer)
                .unwrap()
                .unwrap();
        assert_eq!(head_after_second, second);
    }

    #[test]
    fn current_head_returns_none_for_missing_gitdir() {
        // A writer pointed at a non-existent gitdir collapses to
        // Ok(None) (with a debug log) rather than surfacing the
        // open failure as an Err. Drift detection is best-effort —
        // a transient broken mount doesn't poison the read it
        // accompanies.
        let tmp = TempDir::new().unwrap();
        let writer = GitTreeVaultWriter::new(
            tmp.path().join("does-not-exist.git"),
            "refs/heads/specs".to_string(),
        );
        let head = <GitTreeVaultWriter as memstead_base::backend::VaultBackend>::current_head(&writer).unwrap();
        assert!(head.is_none());
    }

    // ---- git-branch changes_since dispatch --------------------------
    //
    // Tests the `PRO_GIT_BRANCH_OPS.changes_since` dispatcher that
    // pro boot installs on `memstead_base::Engine`. The dispatcher wraps
    // `crate::ops::changes::changes_since` and presents it through the
    // `memstead_base::GitBranchChangesSinceFn` signature.

    fn dispatch_changes(
        gitdir: &Path,
        branch: &str,
        vault: &str,
        since: &str,
    ) -> Result<memstead_base::ops::BackendChanges, memstead_base::backend::BackendError> {
        (crate::storage::PRO_GIT_BRANCH_OPS.changes_since)(
            gitdir,
            branch,
            vault,
            since,
            memstead_base::ops::RENAME_SIMILARITY_DEFAULT,
        )
    }

    #[test]
    fn changes_since_empty_repo_with_sentinel_returns_empty_changes() {
        // Fresh bare repo: no commits, no branches. With the empty-tree
        // sentinel as `since`, the dispatcher short-circuits to "no
        // diff, head echoes sentinel".
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let result = dispatch_changes(
            &gitdir,
            "specs",
            "specs",
            memstead_base::ops::EMPTY_TREE_SHA,
        )
        .unwrap();
        assert_eq!(result.since, memstead_base::ops::EMPTY_TREE_SHA);
        assert_eq!(result.head, memstead_base::ops::EMPTY_TREE_SHA);
        assert!(result.changes.is_empty());
    }

    #[test]
    fn changes_since_after_commit_returns_added_envelopes_id_only() {
        // Commit two new entities, poll from the empty-tree sentinel,
        // and expect both as Added envelopes. Dispatch returns id-only
        // envelopes — the engine wrapper enriches.
        use memstead_base::ops::ChangeEnvelope;
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(
            gitdir.clone(),
            "refs/heads/specs".to_string(),
        );

        writer
            .write_entity(Path::new("alpha.md"), b"# Alpha")
            .unwrap();
        writer
            .write_entity(Path::new("beta.md"), b"# Beta")
            .unwrap();
        let head_sha = writer.commit("seed", &ctx_for_test()).unwrap();

        let result = dispatch_changes(
            &gitdir,
            "specs",
            "specs",
            memstead_base::ops::EMPTY_TREE_SHA,
        )
        .unwrap();
        assert_eq!(result.since, memstead_base::ops::EMPTY_TREE_SHA);
        assert_eq!(result.head, head_sha);
        assert_eq!(result.changes.len(), 2);
        for env in &result.changes {
            match env {
                ChangeEnvelope::Added {
                    id,
                    title,
                    entity_type,
                } => {
                    assert!(
                        id.0.starts_with("specs--"),
                        "expected vault-prefixed id, got {}",
                        id.0
                    );
                    assert!(title.is_none(), "dispatch must not enrich title");
                    assert!(
                        entity_type.is_none(),
                        "dispatch must not enrich entity_type"
                    );
                }
                other => panic!("expected Added envelope, got {other:?}"),
            }
        }
    }

    #[test]
    fn changes_since_between_two_commits_yields_updated_envelope() {
        use memstead_base::ops::ChangeEnvelope;
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(
            gitdir.clone(),
            "refs/heads/specs".to_string(),
        );

        writer
            .write_entity(Path::new("alpha.md"), b"# Alpha v1")
            .unwrap();
        let sha_v1 = writer.commit("v1", &ctx_for_test()).unwrap();

        writer
            .write_entity(Path::new("alpha.md"), b"# Alpha v2")
            .unwrap();
        let sha_v2 = writer.commit("v2", &ctx_for_test()).unwrap();
        assert_ne!(sha_v1, sha_v2);

        let result = dispatch_changes(&gitdir, "specs", "specs", &sha_v1).unwrap();
        assert_eq!(result.since, sha_v1);
        assert_eq!(result.head, sha_v2);
        assert_eq!(result.changes.len(), 1);
        match &result.changes[0] {
            ChangeEnvelope::Updated { id, title, entity_type } => {
                assert!(id.0.starts_with("specs--"));
                assert!(title.is_none());
                assert!(entity_type.is_none());
            }
            other => panic!("expected Updated envelope, got {other:?}"),
        }
    }

    #[test]
    fn changes_since_unknown_cursor_returns_typed_commit_not_found_marker() {
        // A `since` that
        // doesn't resolve is a recoverable caller-argument fault, not a
        // backend fault. The dispatch encodes it as the typed prefix
        // `COMMIT_NOT_FOUND:<sha>` (untruncated) that `Engine::changes_since`
        // lifts to `EngineError::InvalidChangesCursor` (code INVALID_CURSOR)
        // — distinct from the generic `git-branch changes_since: …`
        // wrapper used for real backend faults.
        let tmp = TempDir::new().unwrap();
        let gitdir = fresh_repo_dir(tmp.path());
        let writer = GitTreeVaultWriter::new(
            gitdir.clone(),
            "refs/heads/specs".to_string(),
        );
        // Seed one commit so the gitdir is not empty.
        writer.write_entity(Path::new("a.md"), b"a").unwrap();
        writer.commit("seed", &ctx_for_test()).unwrap();

        let bad_sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let err = dispatch_changes(&gitdir, "specs", "specs", bad_sha).unwrap_err();
        match err {
            memstead_base::backend::BackendError::Other(msg) => {
                assert_eq!(
                    msg,
                    format!("COMMIT_NOT_FOUND:{bad_sha}"),
                    "bad-since must carry the typed marker with the untruncated sha: {msg}",
                );
            }
            other => panic!("expected BackendError::Other, got {other:?}"),
        }
    }
}
