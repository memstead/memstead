//! Filesystem-backed [`MemBackend`](crate::backend::MemBackend) — the
//! gix-free companion to `memstead_git_branch::storage::git_tree::GitTreeBackend`.
//! Used by filesystem mems, where entities live as plain files under a
//! workspace root and there is no commit history.
//!
//! ## Buffer + commit
//!
//! Mutations buffer in memory until [`FilesystemBackend::commit`].
//! Per-path final-state collapse mirrors the git-tree adapter: a chain
//! of write/delete ops on the same path collapses to a single terminal
//! state by commit time. Move resolves at call-time into a
//! `Delete(from)` + `Upsert(to, bytes)` pair — bytes come from the
//! pending buffer when present, otherwise from the live file on disk.
//!
//! ## One commit point
//!
//! A commit is all of its files and its ledger lines, or none of them.
//! It runs in two halves around one commit point:
//!
//! 1. **Before.** Every new file content is written to a temp file
//!    beside its target and synced, the directories that name the
//!    temps are synced, and the ledger lines of the staged provenance
//!    records are rendered. Then a commit record naming every file
//!    operation and every ledger line is written under `.memstead/`
//!    (temp, sync, rename). The rename that puts the record in place
//!    is the commit point; syncing `.memstead/` makes it durable.
//! 2. **After.** The temps are renamed into place, the deletes are
//!    applied, the touched directories are synced, the ledger lines
//!    are appended and synced, and the record is removed. Only then
//!    does the commit return success. The removal is not synced: a
//!    record a power loss brings back is applied again by the next
//!    load, which finds every step done.
//!
//! A failure before the commit point removes what the attempt wrote
//! and returns the IO error: storage is what it was. A failure after
//! it returns [`BackendError::CommitNotApplied`]: the commit is decided,
//! and [`MemBackend::complete_interrupted_commit`] finishes it from the
//! record at the next load. The same call removes the temps of an
//! attempt that died before its commit point. Nothing has to be undone
//! in either case, because nothing is moved into place before every
//! content is safely beside its target.
//!
//! All of it runs through the [`io::FolderIo`] seam, so tests record
//! the order of the steps and fail or cut off any one of them.
//!
//! Writers are serialised by the engine's cross-process write lock
//! ([`MemBackend::write_lock_path`]); the backend itself assumes one
//! writer at a time.
//!
//! ## CommitId
//!
//! There is no commit history. [`Self::commit`] returns a synthetic
//! opaque id (UNIX-nanos + counter, hex) to satisfy the trait surface.
//! Callers that pass this through `_hash` envelopes get a unique
//! per-commit token but no CAS guarantee — there is no parent state to
//! compare against.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use self::io::{FolderIo, RealIo};
use super::CommitId;
use super::path_lock;
use crate::backend::BackendError;
use crate::backend::CompletedCommit;
use crate::backend::MemBackend;
use crate::filesystem::changelog::{
    self, ChangeEntry, MutationKind, changelog_path, parse_rfc3339_utc,
};
use crate::provenance::{Provenance, ProvenanceKind};
use crate::vcs::{Actor, CommitContext, parse_client_id};

/// Per-path final state for the buffered op log. Move operations
/// resolve at call time into a `Delete(from)` + `Upsert(to, bytes)`
/// pair, mirroring the git-tree adapter so commit-time replay only
/// ever sees these two terminal states.
enum PendingState {
    Upsert(Vec<u8>),
    Delete,
}

/// In-flight mutation buffer: the staged file operations and the
/// provenance records that belong to the same commit. Emptied by every
/// exit of a commit, successful or not, and by a discard.
struct Pending {
    ops: HashMap<String, PendingState>,
    records: Vec<Provenance>,
}

impl Pending {
    fn new() -> Self {
        Self {
            ops: HashMap::new(),
            records: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.ops.clear();
        self.records.clear();
    }
}

/// Mem-relative path of the commit record: present from a commit's
/// commit point until the commit is fully applied, absent otherwise.
pub(crate) const COMMIT_RECORD_REL: &str = ".memstead/commit.json";

/// The drift cursor a mem reports while a commit record is on disk.
/// No ledger timestamp ever equals it, so the engine's staleness probe
/// sees a change and reloads, and the reload completes the commit.
const COMMIT_PENDING_HEAD: &str = "commit-pending";

/// One file operation of a commit, as its record names it. Paths are
/// mem-relative with forward slashes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum RecordOp {
    /// `tmp` holds the synced new content and is renamed onto `path`.
    Upsert {
        path: String,
        tmp: String,
    },
    Delete {
        path: String,
    },
}

impl RecordOp {
    fn path(&self) -> &str {
        match self {
            RecordOp::Upsert { path, .. } | RecordOp::Delete { path } => path,
        }
    }
}

/// The commit record: everything needed to finish a decided commit
/// without the process that decided it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CommitRecord {
    v: u32,
    /// File operations, in path order.
    ops: Vec<RecordOp>,
    /// The commit's ledger lines, in order, without their newline.
    ledger: Vec<String>,
}

/// What one commit attempt has put on disk before its commit point,
/// so a failed attempt can take all of it away again.
#[derive(Default)]
struct Attempt {
    /// Temp files written, the record's temp included.
    temps: Vec<PathBuf>,
    /// Directories created, outermost first.
    created_dirs: Vec<PathBuf>,
}

/// Filesystem-backed [`MemBackend`]. Mutations buffer in memory until
/// [`Self::commit`], which lands them as one commit (module docs).
pub struct FilesystemBackend {
    root: PathBuf,
    pending: Mutex<Pending>,
    io: Box<dyn FolderIo>,
}

impl FilesystemBackend {
    /// Build a writer rooted at `root`. The directory must already
    /// exist; sub-directories are created lazily as commits run.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            pending: Mutex::new(Pending::new()),
            io: Box::new(RealIo),
        }
    }

    /// [`Self::new`] over a caller-supplied IO, for tests that trace
    /// or fault the steps of a commit.
    #[cfg(test)]
    pub(crate) fn with_io(root: PathBuf, io: impl FolderIo + 'static) -> Self {
        Self {
            root,
            pending: Mutex::new(Pending::new()),
            io: Box::new(io),
        }
    }

    fn meta_dir(&self) -> PathBuf {
        self.root.join(crate::mem::MEM_META_DIR)
    }

    fn record_path(&self) -> PathBuf {
        self.root.join(COMMIT_RECORD_REL)
    }

    /// Create `dir` and every missing directory above it, noting each
    /// one created so a failed attempt can remove it again.
    fn ensure_dir(&self, dir: &Path, attempt: &mut Attempt) -> std::io::Result<()> {
        let mut missing: Vec<PathBuf> = Vec::new();
        let mut cursor = Some(dir);
        while let Some(d) = cursor {
            if d.as_os_str().is_empty() || self.io.exists(d)? {
                break;
            }
            missing.push(d.to_path_buf());
            cursor = d.parent();
        }
        for d in missing.into_iter().rev() {
            self.io.create_dir(&d)?;
            attempt.created_dirs.push(d);
        }
        Ok(())
    }

    /// Render the ledger lines of `records`, each with a timestamp
    /// strictly after the ledger's last line and after the line before
    /// it: the last line's `ts` is the drift cursor, and two lines that
    /// shared one would hide the second from every reader.
    fn render_ledger(&self, records: &[Provenance]) -> std::io::Result<Vec<String>> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let mut last = self
            .io
            .read_tail(&changelog_path(&self.root), 4096)?
            .and_then(|tail| changelog::last_ts_in_tail(&tail));
        let mut lines = Vec::with_capacity(records.len());
        for record in records {
            let ts = changelog::monotonic_ts(last, record.timestamp);
            let line = changelog::render_change_line(&change_entry(record), ts)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            lines.push(line.trim_end_matches('\n').to_string());
            last = Some(ts);
        }
        Ok(lines)
    }

    /// Everything before the commit point: contents beside their
    /// targets and synced, then the record in place. Returns the
    /// record once its rename has happened; an error means the rename
    /// has not, and `attempt` lists what to remove.
    fn prepare_commit(
        &self,
        ops: &[(String, PendingState)],
        records: &[Provenance],
        attempt: &mut Attempt,
    ) -> std::io::Result<CommitRecord> {
        self.ensure_dir(&self.meta_dir(), attempt)?;

        let mut record_ops = Vec::with_capacity(ops.len());
        for (key, state) in ops {
            match state {
                PendingState::Upsert(bytes) => {
                    let target = self.root.join(key);
                    if let Some(parent) = target.parent() {
                        self.ensure_dir(parent, attempt)?;
                    }
                    let tmp = make_tmp_path(&target);
                    attempt.temps.push(tmp.clone());
                    self.io.write(&tmp, bytes)?;
                    self.io.sync_file(&tmp)?;
                    record_ops.push(RecordOp::Upsert {
                        path: key.clone(),
                        tmp: sibling_key(key, &tmp),
                    });
                }
                PendingState::Delete => record_ops.push(RecordOp::Delete { path: key.clone() }),
            }
        }

        // The names must be as durable as the contents: a record that
        // survives a power loss while a temp's directory entry does not
        // would name a content that is gone.
        let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
        for path in attempt.temps.iter().chain(attempt.created_dirs.iter()) {
            if let Some(parent) = path.parent() {
                dirs.insert(parent.to_path_buf());
            }
        }
        for dir in &dirs {
            self.io.sync_dir(dir)?;
        }

        let record = CommitRecord {
            v: 1,
            ops: record_ops,
            ledger: self.render_ledger(records)?,
        };
        let bytes =
            serde_json::to_vec(&record).map_err(|e| std::io::Error::other(e.to_string()))?;
        let record_path = self.record_path();
        let record_tmp = make_tmp_path(&record_path);
        attempt.temps.push(record_tmp.clone());
        self.io.write(&record_tmp, &bytes)?;
        self.io.sync_file(&record_tmp)?;
        self.io.rename(&record_tmp, &record_path)?;
        Ok(record)
    }

    /// Take away what an attempt that never reached its commit point
    /// put on disk. Best effort: a temp that stays is removed by the
    /// next load, and an empty directory that stays changes nothing.
    fn abandon(&self, attempt: &Attempt) {
        for tmp in &attempt.temps {
            let _ = self.io.remove_file(tmp);
        }
        for dir in attempt.created_dirs.iter().rev() {
            let _ = self.io.remove_dir(dir);
        }
    }

    /// Everything after the commit point, written so that running it
    /// again from any cut-off point ends in the same state: a temp
    /// still present is renamed, a temp already gone means its file is
    /// in place, a ledger line already written is not written twice.
    /// Shared by the commit itself and by the completion at load.
    fn apply_record(&self, record: &CommitRecord) -> std::io::Result<()> {
        let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
        for op in &record.ops {
            let target = self.root.join(op.path());
            let changed = match op {
                RecordOp::Upsert { path, tmp } => {
                    let tmp = self.root.join(tmp);
                    if self.io.exists(&tmp)? {
                        self.io.rename(&tmp, &target)?;
                    } else if !self.io.exists(&target)? {
                        return Err(std::io::Error::other(format!(
                            "commit record names {path}, and neither its staged content \
                             nor the file is on disk"
                        )));
                    }
                    true
                }
                RecordOp::Delete { .. } => match self.io.remove_file(&target) {
                    Ok(()) => true,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                    Err(e) => return Err(e),
                },
            };
            if changed && let Some(parent) = target.parent() {
                dirs.insert(parent.to_path_buf());
            }
        }
        for dir in &dirs {
            self.io.sync_dir(dir)?;
        }

        if !record.ledger.is_empty() {
            let log = changelog_path(&self.root);
            // The commit's lines, if any are there, are the last thing
            // in the ledger, so a tail that can hold all of them is
            // enough to see which are missing.
            let span: u64 = record.ledger.iter().map(|l| l.len() as u64 + 1).sum();
            let tail = self.io.read_tail(&log, span + 8192)?.unwrap_or_default();
            let tail_text = String::from_utf8_lossy(&tail);
            let present: HashSet<&str> = tail_text.lines().collect();
            let mut out = String::new();
            // A cut-off append leaves a line without its newline. Close
            // it, so it stays one unparseable line (which every reader
            // skips) and cannot swallow the line written after it.
            if !tail.is_empty() && !tail.ends_with(b"\n") {
                out.push('\n');
            }
            let mut missing = false;
            for line in &record.ledger {
                if !present.contains(line.as_str()) {
                    out.push_str(line);
                    out.push('\n');
                    missing = true;
                }
            }
            // A ledger this commit creates gets its name synced too:
            // `.memstead/` was last synced before the file existed.
            let created = missing && !self.io.exists(&log)?;
            if missing {
                self.io.append(&log, out.as_bytes())?;
            }
            self.io.sync_file(&log)?;
            if created {
                self.io.sync_dir(&self.meta_dir())?;
            }
        }

        // The removal is not synced: if a power loss brings the record
        // back, the next load runs this again and finds every step done.
        match self.io.remove_file(&self.record_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Remove the temp files of attempts that never reached their
    /// commit point. Walks where entities and the sidecars live; the
    /// lock directory is another process's business.
    fn sweep_leftovers(&self) -> std::io::Result<()> {
        let locks = self.meta_dir().join("locks");
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in self.io.list_dir(&dir)? {
                let path = dir.join(&entry.name);
                if entry.is_dir {
                    if path != locks {
                        stack.push(path);
                    }
                } else if is_temp_name(&entry.name) {
                    match self.io.remove_file(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        Ok(())
    }

    /// Is a temp file of an abandoned attempt on disk? The read-only
    /// half of [`Self::sweep_leftovers`], so a mem with nothing to
    /// remove is never locked or written for the asking.
    fn has_leftovers(&self) -> bool {
        let locks = self.meta_dir().join("locks");
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = self.io.list_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let path = dir.join(&entry.name);
                if entry.is_dir {
                    if path != locks {
                        stack.push(path);
                    }
                } else if is_temp_name(&entry.name) {
                    return true;
                }
            }
        }
        false
    }

    /// Finish the commit whose record is on disk. The caller holds the
    /// mem's write lock. `Ok(None)` when a live writer finished it
    /// while this call waited for the lock.
    fn complete_decided_commit(&self) -> Result<Option<CompletedCommit>, BackendError> {
        let record_path = self.record_path();
        let Some(bytes) = self.io.read(&record_path).map_err(BackendError::Io)? else {
            return Ok(None);
        };
        let unreadable = |why: String| {
            BackendError::Other(format!(
                "commit record {} cannot be read ({why}); the mem holds a decided commit \
                 that cannot be completed",
                record_path.display()
            ))
        };
        let record: CommitRecord =
            serde_json::from_slice(&bytes).map_err(|e| unreadable(e.to_string()))?;
        if record.v != 1 {
            return Err(unreadable(format!("unknown record version {}", record.v)));
        }
        // The record is input from disk: hold its paths to the same
        // rules as a staged path before anything is moved or removed.
        for op in &record.ops {
            normalise_rel_path(Path::new(op.path())).map_err(|e| unreadable(e.to_string()))?;
            if let RecordOp::Upsert { tmp, .. } = op {
                normalise_rel_path(Path::new(tmp)).map_err(|e| unreadable(e.to_string()))?;
            }
        }
        self.apply_record(&record)
            .map_err(|e| BackendError::CommitNotApplied {
                detail: e.to_string(),
            })?;
        let _ = self.sweep_leftovers();
        Ok(Some(CompletedCommit {
            files: record.ops.iter().map(|op| op.path().to_string()).collect(),
        }))
    }

    /// Read the current bytes at `rel_key` from the buffered op log if
    /// present, otherwise from disk. Used by `move_entity` to resolve
    /// the source content.
    fn read_source(
        &self,
        pending: &Pending,
        rel_key: &str,
    ) -> Result<Option<Vec<u8>>, BackendError> {
        if let Some(PendingState::Upsert(bytes)) = pending.ops.get(rel_key) {
            return Ok(Some(bytes.clone()));
        }
        if let Some(PendingState::Delete) = pending.ops.get(rel_key) {
            return Ok(None);
        }
        let full = self.root.join(rel_key);
        match std::fs::read(&full) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(BackendError::Io(e)),
        }
    }
}

/// Normalise a mem-relative path to a forward-slash key. Rejects
/// empty paths, absolute paths, and any path that escapes the mem
/// root via `..`. Mirrors `git_tree::normalise_rel_path` so the two
/// adapters reject the same inputs.
///
/// `pub(crate)` so the in-memory backend reuses the exact same
/// rejection rules — a third hand-rolled copy would be free to drift.
pub(crate) fn normalise_rel_path(rel_path: &Path) -> Result<String, BackendError> {
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

impl crate::backend::MemBackend for FilesystemBackend {
    fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        walk_for_md(&self.root, &self.root, &mut out)?;
        Ok(out)
    }

    /// Folder-mem drift cursor: the changelog's last-line `ts` (or the
    /// commit-pending marker while a decided commit is unapplied) — the
    /// same RFC3339-millisecond dialect `folder_changes_since` accepts
    /// as its cursor, so drift heads feed straight into delta reads.
    /// Every mutation appends a changelog line (in this process or a
    /// sibling's), advancing the cursor; the drift check then treats
    /// the advance exactly like a git-branch tip move. Absent
    /// changelog (a mem never mutated through the engine) keeps the
    /// historical no-drift-signal `None`. Appends go through
    /// `append_change_monotonic`, so the cursor strictly advances even
    /// for same-millisecond commits; only a read-append race between
    /// separate processes can momentarily share a cursor value —
    /// detection then rides the next append.
    fn current_head(&self) -> Result<Option<String>, BackendError> {
        // A commit record on disk is a decided commit not yet applied:
        // report a cursor no ledger line carries, so the staleness
        // probe reloads the mem and the reload completes the commit.
        if std::fs::symlink_metadata(self.record_path()).is_ok() {
            return Ok(Some(COMMIT_PENDING_HEAD.to_string()));
        }
        let log_path = crate::filesystem::changelog::changelog_path(&self.root);
        let raw = match std::fs::read_to_string(&log_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(BackendError::Other(format!(
                    "reading folder changelog {}: {e}",
                    log_path.display()
                )));
            }
        };
        let last_ts = raw
            .lines()
            .rev()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }
                serde_json::from_str::<serde_json::Value>(trimmed)
                    .ok()?
                    .get("ts")?
                    .as_str()
                    .map(str::to_string)
            })
            .next();
        Ok(last_ts)
    }

    /// Inside the mem's own `.memstead/`, in a self-ignoring `locks/`
    /// subtree, so every process that mounts this folder, from any
    /// workspace, contends on the same file.
    fn write_lock_path(&self) -> Option<PathBuf> {
        Some(self.root.join(".memstead").join("locks").join("write.lock"))
    }

    fn read_entity(&self, rel_path: &Path) -> Result<Option<Vec<u8>>, BackendError> {
        let key = normalise_rel_path(rel_path)?;
        let pending = self.pending.lock().map_err(|_| {
            BackendError::Other("filesystem writer pending state poisoned".to_string())
        })?;
        if let Some(state) = pending.ops.get(&key) {
            return Ok(match state {
                PendingState::Upsert(bytes) => Some(bytes.clone()),
                PendingState::Delete => None,
            });
        }
        let full = self.root.join(&key);
        match std::fs::read(&full) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(BackendError::Io(e)),
        }
    }

    /// Metadata-class existence probe: pending buffer first (same
    /// precedence as `read_entity`), then one `symlink_metadata` call —
    /// no file open, no byte read.
    /// The mem root directory exists. A folder mount whose path is
    /// gone (a moved checkout, a mem never materialised) lists zero
    /// entities exactly like an empty one; this is how boot tells the
    /// two apart.
    fn storage_present(&self) -> Result<bool, BackendError> {
        Ok(self.root.is_dir())
    }

    fn entity_exists(&self, rel_path: &Path) -> Result<bool, BackendError> {
        let key = normalise_rel_path(rel_path)?;
        let pending = self.pending.lock().map_err(|_| {
            BackendError::Other("filesystem writer pending state poisoned".to_string())
        })?;
        if let Some(state) = pending.ops.get(&key) {
            return Ok(matches!(state, PendingState::Upsert(_)));
        }
        drop(pending);
        match std::fs::symlink_metadata(self.root.join(&key)) {
            Ok(md) => Ok(md.is_file()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(BackendError::Io(e)),
        }
    }

    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), BackendError> {
        let key = normalise_rel_path(rel_path)?;
        let mut pending = self.pending.lock().map_err(|_| {
            BackendError::Path("filesystem writer pending state poisoned".to_string())
        })?;
        pending
            .ops
            .insert(key, PendingState::Upsert(content.to_vec()));
        Ok(())
    }

    fn delete_entity(&self, rel_path: &Path) -> Result<(), BackendError> {
        let key = normalise_rel_path(rel_path)?;
        let mut pending = self.pending.lock().map_err(|_| {
            BackendError::Path("filesystem writer pending state poisoned".to_string())
        })?;
        pending.ops.insert(key, PendingState::Delete);
        Ok(())
    }

    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), BackendError> {
        let from_key = normalise_rel_path(from)?;
        let to_key = normalise_rel_path(to)?;
        let mut pending = self.pending.lock().map_err(|_| {
            BackendError::Path("filesystem writer pending state poisoned".to_string())
        })?;

        let bytes = match pending.ops.remove(&from_key) {
            Some(PendingState::Upsert(b)) => b,
            Some(PendingState::Delete) => {
                pending.ops.insert(from_key, PendingState::Delete);
                return Err(BackendError::Path(format!(
                    "move source {} is already pending deletion",
                    from.display()
                )));
            }
            None => match self.read_source(&pending, &from_key)? {
                Some(b) => b,
                None => {
                    return Err(BackendError::Path(format!(
                        "move source {} does not exist",
                        from.display()
                    )));
                }
            },
        };

        if matches!(pending.ops.get(&to_key), Some(PendingState::Upsert(_))) {
            return Err(BackendError::Path(format!(
                "move target {} already has a pending write",
                to.display()
            )));
        }
        pending.ops.insert(from_key, PendingState::Delete);
        pending.ops.insert(to_key, PendingState::Upsert(bytes));
        Ok(())
    }

    fn discard_pending(&self) -> Result<(), BackendError> {
        // Drop the staged operations and their provenance records
        // without replaying them: nothing staged has touched disk.
        let mut pending = self.pending.lock().map_err(|_| {
            BackendError::Other("filesystem writer pending state poisoned".to_string())
        })?;
        pending.clear();
        Ok(())
    }

    fn commit(&self, _message: &str, _ctx: &CommitContext<'_>) -> Result<CommitId, BackendError> {
        let mut pending = self.pending.lock().map_err(|_| {
            BackendError::Path("filesystem writer pending state poisoned".to_string())
        })?;
        // Taken out before anything can fail: whatever this commit
        // comes to, nothing stays staged behind it, so no later read
        // can serve a write that never landed.
        let staged = std::mem::replace(&mut *pending, Pending::new());
        if staged.ops.is_empty() && staged.records.is_empty() {
            return Ok(make_commit_id());
        }

        // A record already on disk is an earlier commit, decided and
        // not yet applied. Putting a new record over it would lose it.
        match self.io.exists(&self.record_path()) {
            Ok(false) => {}
            Ok(true) => {
                return Err(BackendError::Other(
                    "an earlier commit of this mem is decided and not yet applied; \
                     the mem must be loaded, which completes it, before it is written again"
                        .to_string(),
                ));
            }
            Err(e) => return Err(BackendError::Io(e)),
        }

        // Path order, so the commit's steps and its record read the
        // same on every run.
        let mut ops: Vec<(String, PendingState)> = staged.ops.into_iter().collect();
        ops.sort_by(|a, b| a.0.cmp(&b.0));

        // Failure handling is spelled out here rather than left to a
        // drop guard: a process that dies mid-commit runs none of it,
        // and the recovery at load must cope with exactly that.
        let mut attempt = Attempt::default();
        let record = match self.prepare_commit(&ops, &staged.records, &mut attempt) {
            Ok(record) => record,
            Err(e) => {
                self.abandon(&attempt);
                return Err(BackendError::Io(e));
            }
        };

        // The record is in place: the commit is decided. From here an
        // error leaves a commit that the next load completes, never a
        // failed write.
        let not_applied = |e: std::io::Error| BackendError::CommitNotApplied {
            detail: e.to_string(),
        };
        self.io.sync_dir(&self.meta_dir()).map_err(not_applied)?;
        self.apply_record(&record).map_err(not_applied)?;
        Ok(make_commit_id())
    }

    /// Recovery writes, so it excludes a live writer first: the record
    /// it applies and the temps it removes belong to a writer that is
    /// gone only if no one holds the mem's write lock. A record on disk
    /// waits for that lock (a live writer finishes its own commit and
    /// the record is gone by then); leftover temps are removed only if
    /// the lock is free at once, since they are invisible to readers
    /// and a load must not wait on hygiene. A mem with nothing to
    /// recover is not touched at all, lock file included.
    fn complete_interrupted_commit(&self) -> Result<Option<CompletedCommit>, BackendError> {
        let lock = self.root.join(".memstead").join("locks").join("write.lock");
        if self
            .io
            .exists(&self.record_path())
            .map_err(BackendError::Io)?
        {
            // A lock that cannot be taken (a read-only location) is not
            // a reason to serve the mem half applied: the completion
            // below then fails with its own error.
            let locked = path_lock::acquire(&lock).is_ok();
            let result = self.complete_decided_commit();
            if locked {
                path_lock::release(&lock);
            }
            return result;
        }
        // Best effort from here: a temp that stays is removed by a
        // later load and changes nothing a reader sees.
        if self.has_leftovers() && matches!(path_lock::try_acquire(&lock), Ok(true)) {
            let _ = self.sweep_leftovers();
            path_lock::release(&lock);
        }
        Ok(None)
    }

    fn read_mem_config(&self) -> Result<Option<Vec<u8>>, BackendError> {
        // Folder backend reads `<root>/.memstead/config.json`.
        // Missing file → Ok(None); other IO errors propagate as
        // BackendError.
        let config_path = self.root.join(crate::mem::MEM_META_DIR).join("config.json");
        match std::fs::read(&config_path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(BackendError::Io(e)),
        }
    }

    fn write_mem_config(
        &self,
        bytes: &[u8],
        _ctx: &crate::vcs::CommitContext<'_>,
    ) -> Result<(), BackendError> {
        // Folder backend writes `<root>/.memstead/config.json` to disk.
        // Creates the `.memstead/` directory if missing. Existing config
        // is overwritten — caller's responsibility to gate against
        // overwrites if that's the contract (the unified
        // `mem_management::create_mem` Step 4 already does the
        // refusal-to-overwrite check upstream).
        let memstead_dir = self.root.join(crate::mem::MEM_META_DIR);
        std::fs::create_dir_all(&memstead_dir).map_err(BackendError::Io)?;
        let config_path = memstead_dir.join("config.json");
        std::fs::write(&config_path, bytes).map_err(BackendError::Io)
    }

    /// Compare-and-set under an exclusive lock file, which is what makes the
    /// check and the write one step. `create_new` on the
    /// lock is the atomic primitive: exactly one process wins it, so no other
    /// engine can slip a write between this one's compare and its write.
    ///
    /// A stale lock (a process killed mid-write) is broken after a short wait
    /// rather than blocking forever: a config write that hangs is its own
    /// outage, and the compare inside still refuses to overwrite content it
    /// did not observe. A hand edit made in that window is still detected,
    /// because the compare reads the file, not the lock.
    fn write_mem_config_cas(
        &self,
        expected: Option<&[u8]>,
        bytes: &[u8],
        _ctx: &crate::vcs::CommitContext<'_>,
    ) -> Result<bool, BackendError> {
        let memstead_dir = self.root.join(crate::mem::MEM_META_DIR);
        std::fs::create_dir_all(&memstead_dir).map_err(BackendError::Io)?;
        let config_path = memstead_dir.join("config.json");
        let lock_path = memstead_dir.join("config.json.lock");

        let mut held = None;
        for attempt in 0..50 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(f) => {
                    held = Some(f);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // 50 x 10ms. Past that the holder is presumed dead: a
                    // config write is milliseconds of work, so half a second
                    // of contention is not a busy writer.
                    if attempt == 49 {
                        let _ = std::fs::remove_file(&lock_path);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => return Err(BackendError::Io(e)),
            }
        }
        let _lock = match held {
            Some(f) => f,
            None => std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&lock_path)
                .map_err(BackendError::Io)?,
        };
        // Release on every exit path, including the mismatch return.
        let release = || {
            let _ = std::fs::remove_file(&lock_path);
        };

        let current = match std::fs::read(&config_path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                release();
                return Err(BackendError::Io(e));
            }
        };
        if let Some(expected) = expected
            && current.as_deref() != Some(expected)
        {
            release();
            return Ok(false);
        }
        let result = std::fs::write(&config_path, bytes).map_err(BackendError::Io);
        release();
        result.map(|_| true)
    }

    fn read_anchors_sidecar(&self) -> Result<Option<Vec<u8>>, BackendError> {
        // Read via the entity path so a staged (pending) sidecar write is
        // visible before its commit, symmetric with the other backends.
        self.read_entity(Path::new(crate::anchor::ANCHOR_SIDECAR_PATH))
    }

    fn write_anchors_sidecar(&self, bytes: &[u8]) -> Result<(), BackendError> {
        // Stage into the same op buffer entity writes use so the sidecar
        // rides the entity mutation's commit. `list_entities`
        // (`walk_for_md`) skips `.memstead/`, so it never lists as an
        // entity.
        <Self as MemBackend>::write_entity(
            self,
            Path::new(crate::anchor::ANCHOR_SIDECAR_PATH),
            bytes,
        )
    }

    fn stage_provenance(&self, record: &Provenance) -> Result<(), BackendError> {
        let mut pending = self.pending.lock().map_err(|_| {
            BackendError::Other("filesystem writer pending state poisoned".to_string())
        })?;
        pending.records.push(record.clone());
        Ok(())
    }

    fn append_provenance(&self, record: &Provenance) -> Result<(), BackendError> {
        let log = changelog_path(&self.root);
        let existed = log.exists();
        // Monotonic variant: the last-line `ts` is this backend's
        // drift cursor (`current_head()`), so same-millisecond lines
        // must still advance it — see `append_change_monotonic`.
        changelog::append_change_monotonic(&self.root, &change_entry(record), record.timestamp)
            .map_err(|e| BackendError::Other(format!("changelog append: {e}")))?;
        // A line reported as written must survive a power loss, and a
        // ledger this call created needs its name synced too.
        self.io.sync_file(&log).map_err(BackendError::Io)?;
        if !existed && let Some(parent) = log.parent() {
            self.io.sync_dir(parent).map_err(BackendError::Io)?;
        }
        Ok(())
    }

    fn read_provenance(&self, cursor: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
        let log_path = changelog_path(&self.root);
        let raw = match std::fs::read_to_string(&log_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(BackendError::Io(e)),
        };
        let mut out = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: serde_json::Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let ts_str = value.get("ts").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(c) = cursor
                && ts_str <= c
            {
                continue;
            }
            let timestamp = parse_rfc3339_utc(ts_str).unwrap_or(std::time::UNIX_EPOCH);
            let kind = value
                .get("kind")
                .and_then(|v| v.as_str())
                .and_then(ProvenanceKind::parse)
                .unwrap_or(ProvenanceKind::Update);
            let entity = value
                .get("entity")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let actor = value
                .get("actor")
                .and_then(|v| v.as_str())
                .and_then(Actor::from_trailer)
                .unwrap_or(Actor::Unknown);
            let client = value
                .get("client")
                .and_then(|v| v.as_str())
                .and_then(parse_client_id);
            let note = value
                .get("note")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let logical_operation_id = value
                .get("logical_op")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let mut record = Provenance::new(timestamp, kind, entity, actor, client, note);
            if let Some(id) = logical_operation_id {
                record = record.with_logical_operation_id(id);
            }
            if let Some(role) = value
                .get("role")
                .and_then(|v| v.as_str())
                .and_then(crate::vcs::Role::from_wire)
            {
                record = record.with_role(role);
            }
            record = record.with_identity(
                value
                    .get("identity")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            );
            out.push(record);
        }
        Ok(out)
    }
}

/// Walk `dir` for `.md` files, accumulating mem-relative paths in
/// `out`. Skips the mem's `.memstead/` umbrella so the engine never
/// confuses changelog / config / schema files with entity-bearing
/// markdown, and `README.md` — repository documentation beside the
/// entity files, never an entity (mirrors the entity-source walker's
/// skip; entities are slug-named after their titles, so no legitimate
/// entity file carries this name).
fn walk_for_md(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), BackendError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(BackendError::Io(e)),
    };
    for entry in entries {
        let entry = entry.map_err(BackendError::Io)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(BackendError::Io)?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if name == crate::mem::MEM_META_DIR {
                continue;
            }
            walk_for_md(root, &path, out)?;
        } else if file_type.is_file()
            && path.extension().and_then(|s| s.to_str()) == Some("md")
            && entry.file_name() != "README.md"
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

/// The ledger entry a provenance record writes.
fn change_entry(record: &Provenance) -> ChangeEntry<'_> {
    let kind: MutationKind = record.kind.into();
    ChangeEntry {
        kind,
        entity: record.entity.as_deref(),
        actor: record.actor,
        client: record.client.as_ref(),
        note: record.note.as_deref(),
        logical_operation_id: record.logical_operation_id.as_deref(),
        role: record.role,
        identity: record.identity.as_deref(),
    }
}

/// The mem-relative key of `tmp`, the temp file beside the target at
/// `key`: same directory, the temp's file name.
fn sibling_key(key: &str, tmp: &Path) -> String {
    let name = tmp
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match key.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/{name}"),
        None => name,
    }
}

/// Is `name` a temp file of this backend ([`make_tmp_path`]):
/// `.<target>.tmp.<hex>-<hex>`?
fn is_temp_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.') else {
        return false;
    };
    let Some((target, suffix)) = rest.rsplit_once(".tmp.") else {
        return false;
    };
    let Some((nanos, counter)) = suffix.split_once('-') else {
        return false;
    };
    let hex = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit());
    !target.is_empty() && hex(nanos) && hex(counter)
}

/// Build a sibling temp path of the form `.<name>.tmp.<suffix>` next
/// to `target`. The leading dot keeps the temp file out of the way of
/// directory listings; the suffix combines UNIX-nanos with a process-
/// scoped counter so concurrent writes never collide.
fn make_tmp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "_".to_string());
    let suffix = unique_suffix();
    let tmp_name = format!(".{name}.tmp.{suffix}");
    target.with_file_name(tmp_name)
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static COMMIT_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{counter:x}")
}

/// `pub(crate)` so the in-memory backend mints the same synthetic
/// write-id shape (UNIX-nanos + counter, hex) the folder backend
/// produces — both are history-free backends and must hand callers an
/// identically-shaped opaque identity. It is NOT a change cursor:
/// the change feed's cursor is an RFC3339 timestamp, and passing this
/// token as `since` refuses with `INVALID_CURSOR` (it would otherwise
/// sort below every timestamp and replay the whole history).
pub(crate) fn make_commit_id() -> CommitId {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COMMIT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:032x}{counter:016x}")
}

#[cfg(test)]
mod commit_point_tests;
#[cfg(test)]
pub(crate) mod fault_io;
#[cfg(test)]
mod folder_drift_tests;
pub(crate) mod io;
#[cfg(test)]
mod tests;
