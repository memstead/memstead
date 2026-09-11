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
//! ## Atomicity
//!
//! Per-file writes are atomic via write-to-temp + rename. Multi-op
//! commits are *not* transactional: a failure partway through leaves
//! earlier ops landed and later ops untouched. Single-writer is
//! assumed; concurrent writers against the same mem are out of scope.
//!
//! ## CommitId
//!
//! There is no commit history. [`Self::commit`] returns a synthetic
//! opaque id (UNIX-nanos + counter, hex) to satisfy the trait surface.
//! Callers that pass this through `_hash` envelopes get a unique
//! per-commit token but no CAS guarantee — there is no parent state to
//! compare against.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::CommitId;
use crate::backend::BackendError;
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

/// In-flight mutation buffer. Cleared on a successful commit.
struct Pending {
    ops: HashMap<String, PendingState>,
}

impl Pending {
    fn new() -> Self {
        Self {
            ops: HashMap::new(),
        }
    }

    fn clear(&mut self) {
        self.ops.clear();
    }
}

/// Filesystem-backed [`MemBackend`]. Mutations buffer in memory until
/// [`Self::commit`]; commit replays them against the directory at
/// `root` with per-file write-to-temp + rename atomicity.
pub struct FilesystemBackend {
    root: PathBuf,
    pending: Mutex<Pending>,
}

impl FilesystemBackend {
    /// Build a writer rooted at `root`. The directory must already
    /// exist; sub-directories are created lazily as commits run.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            pending: Mutex::new(Pending::new()),
        }
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

    /// Folder-mem drift cursor: the changelog's last-line `ts` — the
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
        // Drop the in-memory op buffer without replaying it. The
        // atomic batch path calls this to roll back staged writes when
        // a later item refuses the whole batch.
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

        for (key, state) in pending.ops.iter() {
            let target = self.root.join(key);
            match state {
                PendingState::Upsert(bytes) => atomic_write(&target, bytes)?,
                PendingState::Delete => match std::fs::remove_file(&target) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(BackendError::Io(e)),
                },
            }
        }

        pending.clear();
        Ok(make_commit_id())
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

    fn append_provenance(&self, record: &Provenance) -> Result<(), BackendError> {
        let kind: MutationKind = record.kind.into();
        let entry = ChangeEntry {
            kind,
            entity: record.entity.as_deref(),
            actor: record.actor,
            client: record.client.as_ref(),
            note: record.note.as_deref(),
            logical_operation_id: record.logical_operation_id.as_deref(),
            role: record.role,
            identity: record.identity.as_deref(),
        };
        // Monotonic variant: the last-line `ts` is this backend's
        // drift cursor (`current_head()`), so same-millisecond commits
        // must still advance it — see `append_change_monotonic`.
        changelog::append_change_monotonic(&self.root, &entry, record.timestamp)
            .map_err(|e| BackendError::Other(format!("changelog append: {e}")))
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

/// Write `bytes` to `target` atomically: write to a sibling temp file
/// then rename. Creates parent directories as needed. The temp file
/// shares the target's parent so the rename is same-fs (atomic on
/// POSIX). On rename failure, the temp file is best-effort removed.
fn atomic_write(target: &Path, bytes: &[u8]) -> Result<(), BackendError> {
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(BackendError::Io)?;
    }
    let tmp = make_tmp_path(target);
    std::fs::write(&tmp, bytes).map_err(BackendError::Io)?;
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(BackendError::Io(e));
    }
    Ok(())
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
mod folder_drift_tests;
#[cfg(test)]
mod tests;
