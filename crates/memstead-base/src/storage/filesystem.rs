//! Filesystem-backed [`MemWriter`](super::MemWriter) — the gix-free
//! companion to [`memstead_git_branch::storage::git_tree::GitTreeMemWriter`].
//! Used by filesystem mems, where entities live as plain files under a
//! workspace root and there is no commit history.
//!
//! ## Buffer + commit
//!
//! Mutations buffer in memory until [`FilesystemMemWriter::commit`].
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

use super::{CommitId, MemWriter, MemWriterError};
// `MemBackend` is referenced via fully-qualified path in the impl
// declaration below so it does NOT enter this module's name lookup.
// Both traits expose `write_entity` / `delete_entity` / etc.; importing
// both at module scope would make every dot-syntax call on a
// `FilesystemMemWriter` ambiguous (existing tests included). Tests
// that exercise the `MemBackend` impl pull it in via a local `use`
// at the function level.
use crate::backend::BackendError;
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

/// Filesystem-backed [`MemWriter`]. Mutations buffer in memory until
/// [`Self::commit`]; commit replays them against the directory at
/// `root` with per-file write-to-temp + rename atomicity.
pub struct FilesystemMemWriter {
    root: PathBuf,
    pending: Mutex<Pending>,
}

impl FilesystemMemWriter {
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
    ) -> Result<Option<Vec<u8>>, MemWriterError> {
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
            Err(e) => Err(MemWriterError::Io(e)),
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
pub(crate) fn normalise_rel_path(rel_path: &Path) -> Result<String, MemWriterError> {
    if rel_path.as_os_str().is_empty() {
        return Err(MemWriterError::Path(
            "mem-relative path is empty".to_string(),
        ));
    }
    let mut parts: Vec<String> = Vec::new();
    for component in rel_path.components() {
        use std::path::Component;
        match component {
            Component::Normal(s) => match s.to_str() {
                Some(p) if !p.is_empty() => parts.push(p.to_string()),
                _ => {
                    return Err(MemWriterError::Path(format!(
                        "non-utf-8 or empty path component in {}",
                        rel_path.display()
                    )));
                }
            },
            Component::CurDir => continue,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(MemWriterError::Path(format!(
                    "path traversal or absolute component in {}",
                    rel_path.display()
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(MemWriterError::Path(
            "mem-relative path is empty after normalisation".to_string(),
        ));
    }
    Ok(parts.join("/"))
}

impl MemWriter for FilesystemMemWriter {
    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), MemWriterError> {
        let key = normalise_rel_path(rel_path)?;
        let mut pending = self.pending.lock().map_err(|_| {
            MemWriterError::Path("filesystem writer pending state poisoned".to_string())
        })?;
        pending
            .ops
            .insert(key, PendingState::Upsert(content.to_vec()));
        Ok(())
    }

    fn delete_entity(&self, rel_path: &Path) -> Result<(), MemWriterError> {
        let key = normalise_rel_path(rel_path)?;
        let mut pending = self.pending.lock().map_err(|_| {
            MemWriterError::Path("filesystem writer pending state poisoned".to_string())
        })?;
        pending.ops.insert(key, PendingState::Delete);
        Ok(())
    }

    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), MemWriterError> {
        let from_key = normalise_rel_path(from)?;
        let to_key = normalise_rel_path(to)?;
        let mut pending = self.pending.lock().map_err(|_| {
            MemWriterError::Path("filesystem writer pending state poisoned".to_string())
        })?;

        let bytes = match pending.ops.remove(&from_key) {
            Some(PendingState::Upsert(b)) => b,
            Some(PendingState::Delete) => {
                pending.ops.insert(from_key, PendingState::Delete);
                return Err(MemWriterError::Path(format!(
                    "move source {} is already pending deletion",
                    from.display()
                )));
            }
            None => match self.read_source(&pending, &from_key)? {
                Some(b) => b,
                None => {
                    return Err(MemWriterError::Path(format!(
                        "move source {} does not exist",
                        from.display()
                    )));
                }
            },
        };

        if matches!(pending.ops.get(&to_key), Some(PendingState::Upsert(_))) {
            return Err(MemWriterError::Path(format!(
                "move target {} already has a pending write",
                to.display()
            )));
        }
        pending.ops.insert(from_key, PendingState::Delete);
        pending.ops.insert(to_key, PendingState::Upsert(bytes));
        Ok(())
    }

    fn commit(&self, _message: &str, _ctx: &CommitContext<'_>) -> Result<CommitId, MemWriterError> {
        let mut pending = self.pending.lock().map_err(|_| {
            MemWriterError::Path("filesystem writer pending state poisoned".to_string())
        })?;

        for (key, state) in pending.ops.iter() {
            let target = self.root.join(key);
            match state {
                PendingState::Upsert(bytes) => atomic_write(&target, bytes)?,
                PendingState::Delete => match std::fs::remove_file(&target) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(MemWriterError::Io(e)),
                },
            }
        }

        pending.clear();
        Ok(make_commit_id())
    }
}

impl crate::backend::MemBackend for FilesystemMemWriter {
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
    /// historical no-drift-signal `None`. Same-millisecond sibling
    /// appends can momentarily share a cursor value — detection then
    /// rides the next append (the changelog dialect's precision,
    /// pre-existing).
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

    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), BackendError> {
        <Self as MemWriter>::write_entity(self, rel_path, content).map_err(Into::into)
    }

    fn delete_entity(&self, rel_path: &Path) -> Result<(), BackendError> {
        <Self as MemWriter>::delete_entity(self, rel_path).map_err(Into::into)
    }

    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), BackendError> {
        <Self as MemWriter>::move_entity(self, from, to).map_err(Into::into)
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

    fn commit(&self, message: &str, ctx: &CommitContext<'_>) -> Result<CommitId, BackendError> {
        <Self as MemWriter>::commit(self, message, ctx).map_err(Into::into)
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

    fn write_mem_config(&self, bytes: &[u8]) -> Result<(), BackendError> {
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
        <Self as MemWriter>::write_entity(
            self,
            Path::new(crate::anchor::ANCHOR_SIDECAR_PATH),
            bytes,
        )
        .map_err(Into::into)
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
        };
        changelog::append_change_at(&self.root, &entry, record.timestamp)
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
fn atomic_write(target: &Path, bytes: &[u8]) -> Result<(), MemWriterError> {
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(MemWriterError::Io)?;
    }
    let tmp = make_tmp_path(target);
    std::fs::write(&tmp, bytes).map_err(MemWriterError::Io)?;
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(MemWriterError::Io(e));
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
/// commit-id shape (UNIX-nanos + counter, hex) the folder backend
/// produces — both are history-free backends and must hand callers an
/// identically-shaped opaque cursor.
pub(crate) fn make_commit_id() -> CommitId {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = COMMIT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:032x}{counter:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::{Actor, ClientId, CommitContext};
    use tempfile::TempDir;

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

    #[test]
    fn write_then_commit_round_trip() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        writer
            .write_entity(Path::new("notes/hello.md"), b"# hi\n")
            .unwrap();
        let id = writer.commit("first commit", &ctx_for_test()).unwrap();
        assert!(!id.is_empty());

        let bytes = std::fs::read(tmp.path().join("notes/hello.md")).unwrap();
        assert_eq!(bytes, b"# hi\n");
    }

    #[test]
    fn delete_removes_path() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        writer.write_entity(Path::new("a.md"), b"a").unwrap();
        writer.write_entity(Path::new("b.md"), b"b").unwrap();
        writer.commit("seed", &ctx_for_test()).unwrap();

        writer.delete_entity(Path::new("a.md")).unwrap();
        writer.commit("drop a", &ctx_for_test()).unwrap();

        assert!(!tmp.path().join("a.md").exists());
        assert!(tmp.path().join("b.md").exists());
    }

    #[test]
    fn delete_of_missing_path_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        writer.delete_entity(Path::new("never-existed.md")).unwrap();
        writer.commit("noop delete", &ctx_for_test()).unwrap();
    }

    #[test]
    fn move_renames_path() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        writer
            .write_entity(Path::new("from.md"), b"payload")
            .unwrap();
        writer.commit("seed", &ctx_for_test()).unwrap();

        writer
            .move_entity(Path::new("from.md"), Path::new("nested/to.md"))
            .unwrap();
        writer.commit("rename", &ctx_for_test()).unwrap();

        assert!(!tmp.path().join("from.md").exists());
        let moved = std::fs::read(tmp.path().join("nested/to.md")).unwrap();
        assert_eq!(moved, b"payload");
    }

    #[test]
    fn move_with_pending_upsert_carries_bytes() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        writer.write_entity(Path::new("a.md"), b"alpha").unwrap();
        writer
            .move_entity(Path::new("a.md"), Path::new("b.md"))
            .unwrap();
        writer.commit("write+move", &ctx_for_test()).unwrap();

        assert!(!tmp.path().join("a.md").exists());
        assert_eq!(std::fs::read(tmp.path().join("b.md")).unwrap(), b"alpha");
    }

    #[test]
    fn move_missing_source_errors() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        let err = writer
            .move_entity(Path::new("ghost.md"), Path::new("here.md"))
            .unwrap_err();
        assert!(matches!(err, MemWriterError::Path(_)));
    }

    #[test]
    fn move_with_pending_target_upsert_errors() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        writer.write_entity(Path::new("from.md"), b"x").unwrap();
        writer.write_entity(Path::new("to.md"), b"y").unwrap();
        let err = writer
            .move_entity(Path::new("from.md"), Path::new("to.md"))
            .unwrap_err();
        assert!(matches!(err, MemWriterError::Path(_)));
    }

    #[test]
    fn multi_op_commit() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        writer.write_entity(Path::new("doomed.md"), b"x").unwrap();
        writer.commit("seed", &ctx_for_test()).unwrap();

        writer.write_entity(Path::new("a.md"), b"alpha").unwrap();
        writer
            .write_entity(Path::new("nested/b.md"), b"beta")
            .unwrap();
        writer.delete_entity(Path::new("doomed.md")).unwrap();
        writer.commit("multi-op", &ctx_for_test()).unwrap();

        assert!(!tmp.path().join("doomed.md").exists());
        assert_eq!(std::fs::read(tmp.path().join("a.md")).unwrap(), b"alpha");
        assert_eq!(
            std::fs::read(tmp.path().join("nested/b.md")).unwrap(),
            b"beta"
        );
    }

    #[test]
    fn rejects_path_traversal() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        let err = writer
            .write_entity(Path::new("../escape.md"), b"x")
            .unwrap_err();
        assert!(matches!(err, MemWriterError::Path(_)));
    }

    #[test]
    fn rejects_absolute_path() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        let err = writer
            .write_entity(Path::new("/etc/passwd"), b"x")
            .unwrap_err();
        assert!(matches!(err, MemWriterError::Path(_)));
    }

    #[test]
    fn rejects_empty_path() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        let err = writer.write_entity(Path::new(""), b"x").unwrap_err();
        assert!(matches!(err, MemWriterError::Path(_)));
    }

    #[test]
    fn write_overwrites_existing_file() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        writer.write_entity(Path::new("a.md"), b"first").unwrap();
        writer.commit("c1", &ctx_for_test()).unwrap();
        writer.write_entity(Path::new("a.md"), b"second").unwrap();
        writer.commit("c2", &ctx_for_test()).unwrap();

        assert_eq!(std::fs::read(tmp.path().join("a.md")).unwrap(), b"second");
    }

    #[test]
    fn no_temp_files_left_after_commit() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        writer.write_entity(Path::new("a.md"), b"a").unwrap();
        writer.write_entity(Path::new("nested/b.md"), b"b").unwrap();
        writer.commit("c", &ctx_for_test()).unwrap();

        // Walk the tree and ensure no `.tmp.` artefacts survived.
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let p = entry.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(tmp.path(), &mut files);
        for f in &files {
            let name = f.file_name().unwrap().to_string_lossy();
            assert!(
                !name.contains(".tmp."),
                "stray temp file after commit: {}",
                f.display()
            );
        }
    }

    #[test]
    fn commit_id_is_unique_across_calls() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        writer.write_entity(Path::new("a.md"), b"a").unwrap();
        let id1 = writer.commit("c1", &ctx_for_test()).unwrap();
        writer.write_entity(Path::new("b.md"), b"b").unwrap();
        let id2 = writer.commit("c2", &ctx_for_test()).unwrap();

        assert_ne!(id1, id2);
    }

    #[test]
    fn pending_buffer_clears_on_commit() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        writer.write_entity(Path::new("a.md"), b"a").unwrap();
        writer.commit("c1", &ctx_for_test()).unwrap();
        // A second commit with no mutations writes nothing new but
        // still returns a fresh id.
        let id2 = writer.commit("noop", &ctx_for_test()).unwrap();
        assert!(!id2.is_empty());
        // a.md still has its original content (no zombie pending op).
        assert_eq!(std::fs::read(tmp.path().join("a.md")).unwrap(), b"a");
    }

    // --- MemBackend impl ----------------------------------------

    /// Folder backend's `write_mem_config` writes
    /// `<root>/.memstead/config.json`, creating the umbrella directory
    /// if needed. The subsequent `read_mem_config` round-trips the
    /// bytes.
    #[test]
    fn backend_write_mem_config_round_trips_via_read() {
        use crate::backend::MemBackend;

        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        let backend: &dyn MemBackend = &writer;

        // No config yet — read returns None.
        assert!(backend.read_mem_config().unwrap().is_none());

        // Write — creates `.memstead/` umbrella + the config blob.
        let bytes = br#"{"version":"0.1.0","schema":"default@1.0.0"}"#.to_vec();
        backend.write_mem_config(&bytes).unwrap();

        // Read returns the same bytes.
        let read_back = backend.read_mem_config().unwrap();
        assert_eq!(read_back, Some(bytes.clone()));
        // Config file lands at `<root>/.memstead/config.json`.
        let on_disk = std::fs::read(tmp.path().join(".memstead/config.json")).unwrap();
        assert_eq!(on_disk, bytes);
    }

    #[test]
    fn backend_list_entities_returns_only_md_outside_meta_dirs() {
        use crate::backend::MemBackend;

        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        // With both traits in scope, dot-syntax `writer.foo(...)`
        // is ambiguous — seed via fully-qualified MemWriter calls.
        <FilesystemMemWriter as MemWriter>::write_entity(&writer, Path::new("a.md"), b"a").unwrap();
        <FilesystemMemWriter as MemWriter>::write_entity(&writer, Path::new("nested/b.md"), b"b")
            .unwrap();
        <FilesystemMemWriter as MemWriter>::write_entity(&writer, Path::new("notes.json"), b"{}")
            .unwrap();
        <FilesystemMemWriter as MemWriter>::commit(&writer, "seed", &ctx_for_test()).unwrap();
        // The current `.memstead/` meta dir is skipped by the walker.
        // An ordinary dot-dir (`.other/`) is not special, so markdown
        // under it is walked like any other non-meta path.
        std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
        std::fs::write(tmp.path().join(".memstead/config.json"), b"{}").unwrap();
        std::fs::write(tmp.path().join(".memstead/notes.md"), b"#").unwrap();
        std::fs::create_dir_all(tmp.path().join(".other")).unwrap();
        std::fs::write(tmp.path().join(".other/notes.md"), b"#").unwrap();

        let backend: &dyn MemBackend = &writer;
        let mut paths: Vec<String> = backend
            .list_entities()
            .unwrap()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        paths.sort();
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
    fn backend_read_entity_consults_pending_then_disk() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());

        // Seed via the legacy MemWriter trait — both traits expose
        // `write_entity`; importing `MemBackend` later in the test
        // makes the dot-syntax ambiguous, so we route the seed
        // through the trait that's still implicitly in scope here.
        <FilesystemMemWriter as MemWriter>::write_entity(&writer, Path::new("on_disk.md"), b"disk")
            .unwrap();
        <FilesystemMemWriter as MemWriter>::commit(&writer, "seed", &ctx_for_test()).unwrap();

        use crate::backend::MemBackend;
        let backend: &dyn MemBackend = &writer;
        // Disk path → reads from disk.
        assert_eq!(
            backend.read_entity(Path::new("on_disk.md")).unwrap(),
            Some(b"disk".to_vec())
        );
        // Buffered upsert wins over disk.
        backend
            .write_entity(Path::new("on_disk.md"), b"buffered")
            .unwrap();
        assert_eq!(
            backend.read_entity(Path::new("on_disk.md")).unwrap(),
            Some(b"buffered".to_vec())
        );
        // Buffered delete masks disk.
        backend.delete_entity(Path::new("on_disk.md")).unwrap();
        assert_eq!(backend.read_entity(Path::new("on_disk.md")).unwrap(), None);
        // Unknown path → None (idempotent).
        assert_eq!(backend.read_entity(Path::new("never.md")).unwrap(), None);
    }

    #[test]
    fn backend_provenance_round_trips_through_jsonl() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        use crate::backend::MemBackend;
        let backend: &dyn MemBackend = &writer;

        let client = ClientId {
            name: "claude-code".into(),
            version: "2.1.0".into(),
        };
        let earlier = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let later = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_777_077_296);

        backend
            .append_provenance(&Provenance::new(
                earlier,
                ProvenanceKind::Create,
                Some("v:e1".into()),
                Actor::Agent,
                Some(client.clone()),
                Some("first".into()),
            ))
            .unwrap();
        backend
            .append_provenance(&Provenance::new(
                later,
                ProvenanceKind::Update,
                Some("v:e1".into()),
                Actor::Cli,
                None,
                None,
            ))
            .unwrap();

        let all = backend.read_provenance(None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].kind, ProvenanceKind::Create);
        assert_eq!(all[0].entity.as_deref(), Some("v:e1"));
        assert_eq!(all[0].actor, Actor::Agent);
        assert_eq!(all[0].note.as_deref(), Some("first"));
        assert_eq!(
            all[0]
                .client
                .as_ref()
                .map(|c| (c.name.as_str(), c.version.as_str())),
            Some(("claude-code", "2.1.0"))
        );
        assert_eq!(all[0].timestamp, earlier);
        assert_eq!(all[1].kind, ProvenanceKind::Update);
        assert_eq!(all[1].timestamp, later);
        assert!(all[1].note.is_none());
        assert!(all[1].client.is_none());
    }

    #[test]
    fn backend_provenance_cursor_filters_by_timestamp() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        use crate::backend::MemBackend;
        let backend: &dyn MemBackend = &writer;

        for (secs, label) in [
            (1_700_000_000u64, "first"),
            (1_750_000_000, "middle"),
            (1_800_000_000, "last"),
        ] {
            backend
                .append_provenance(&Provenance::new(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
                    ProvenanceKind::Create,
                    Some(format!("v:{label}")),
                    Actor::Cli,
                    None,
                    None,
                ))
                .unwrap();
        }
        // Cursor between first and middle should drop the first entry.
        let cursor = changelog::format_rfc3339_utc(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_725_000_000),
        );
        let after = backend.read_provenance(Some(&cursor)).unwrap();
        let entities: Vec<_> = after.iter().filter_map(|p| p.entity.clone()).collect();
        assert_eq!(entities, vec!["v:middle".to_string(), "v:last".to_string()]);
    }

    #[test]
    fn backend_read_provenance_handles_missing_log() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        use crate::backend::MemBackend;
        let backend: &dyn MemBackend = &writer;
        // No mutations yet → no `.memstead/changes.jsonl` → empty result, no error.
        assert!(backend.read_provenance(None).unwrap().is_empty());
    }

    // ---- folder JSONL synthesis (memstead_base::ops::folder_changes_since) ----

    /// Helper: append a single Provenance event with an explicit
    /// timestamp, so tests get deterministic JSONL ordering.
    fn append_at(
        backend: &dyn crate::backend::MemBackend,
        secs: u64,
        kind: ProvenanceKind,
        entity: &str,
    ) {
        backend
            .append_provenance(&Provenance::new(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
                kind,
                Some(entity.to_string()),
                Actor::Cli,
                None,
                None,
            ))
            .unwrap();
    }

    #[test]
    fn folder_changes_since_no_log_returns_empty_at_cursor() {
        // Fresh mem, no `.memstead/changes.jsonl` → empty BackendChanges
        // with `head` echoing the cursor.
        let tmp = TempDir::new().unwrap();
        let result =
            crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA)
                .unwrap();
        assert_eq!(result.since, crate::ops::EMPTY_TREE_SHA);
        assert_eq!(result.head, crate::ops::EMPTY_TREE_SHA);
        assert!(result.changes.is_empty());
    }

    #[test]
    fn folder_changes_since_create_only_yields_added_envelope() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        append_at(
            &writer,
            1_700_000_000,
            ProvenanceKind::Create,
            "specs--alpha",
        );

        let result =
            crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA)
                .unwrap();
        assert_eq!(result.changes.len(), 1);
        match &result.changes[0] {
            crate::ops::ChangeEnvelope::Added {
                id,
                title,
                entity_type,
            } => {
                assert_eq!(id.0, "specs--alpha");
                assert!(title.is_none(), "id-only contract");
                assert!(entity_type.is_none(), "id-only contract");
            }
            other => panic!("expected Added, got {other:?}"),
        }
        // head advances to the event's timestamp.
        assert_ne!(result.head, crate::ops::EMPTY_TREE_SHA);
    }

    #[test]
    fn folder_changes_since_create_then_delete_cancels_to_no_envelope() {
        // Within the cursor window, an entity that was created and then
        // deleted nets out to Removed (final state wins for Delete).
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        append_at(
            &writer,
            1_700_000_000,
            ProvenanceKind::Create,
            "specs--ephemeral",
        );
        append_at(
            &writer,
            1_700_000_001,
            ProvenanceKind::Delete,
            "specs--ephemeral",
        );

        let result =
            crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA)
                .unwrap();
        assert_eq!(result.changes.len(), 1);
        match &result.changes[0] {
            crate::ops::ChangeEnvelope::Removed { id, .. } => {
                assert_eq!(id.0, "specs--ephemeral");
            }
            other => panic!("expected Removed (Delete wins), got {other:?}"),
        }
    }

    #[test]
    fn folder_changes_since_update_only_yields_updated_envelope() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        append_at(
            &writer,
            1_700_000_000,
            ProvenanceKind::Update,
            "specs--alpha",
        );

        let result =
            crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA)
                .unwrap();
        assert_eq!(result.changes.len(), 1);
        assert!(matches!(
            result.changes[0],
            crate::ops::ChangeEnvelope::Updated { .. }
        ));
    }

    #[test]
    fn folder_changes_since_cursor_filters_to_window() {
        // Three events at three timestamps; cursor between first and
        // second drops the first event from the window.
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        append_at(
            &writer,
            1_700_000_000,
            ProvenanceKind::Create,
            "specs--first",
        );
        append_at(
            &writer,
            1_750_000_000,
            ProvenanceKind::Create,
            "specs--middle",
        );
        append_at(
            &writer,
            1_800_000_000,
            ProvenanceKind::Create,
            "specs--last",
        );

        let cursor = changelog::format_rfc3339_utc(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_725_000_000),
        );
        let result = crate::ops::folder_changes_since(tmp.path(), "specs", &cursor).unwrap();
        assert_eq!(result.changes.len(), 2);
        let ids: Vec<_> = result
            .changes
            .iter()
            .map(|e| match e {
                crate::ops::ChangeEnvelope::Added { id, .. } => id.0.clone(),
                _ => panic!("expected Added"),
            })
            .collect();
        // BTreeMap iteration order — ids sort lexicographically.
        assert_eq!(
            ids,
            vec!["specs--last".to_string(), "specs--middle".to_string()]
        );
    }

    #[test]
    fn folder_changes_since_skips_events_for_other_mems() {
        // Defensive: changelog drift could carry events for another
        // mem prefix; the impl filters them out so envelopes only
        // surface for the queried mem.
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        append_at(
            &writer,
            1_700_000_000,
            ProvenanceKind::Create,
            "specs--mine",
        );
        append_at(
            &writer,
            1_700_000_001,
            ProvenanceKind::Create,
            "other--theirs",
        );

        let result =
            crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA)
                .unwrap();
        assert_eq!(result.changes.len(), 1);
        match &result.changes[0] {
            crate::ops::ChangeEnvelope::Added { id, .. } => {
                assert_eq!(id.0, "specs--mine");
            }
            other => panic!("expected Added, got {other:?}"),
        }
    }

    #[test]
    fn folder_changes_since_skips_batch_events_with_no_entity() {
        // Batch events have entity=null. They don't surface as
        // envelopes (no per-entity id to attach to).
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        use crate::backend::MemBackend;
        // append_at requires an entity; use append_provenance directly
        // for the batch-with-no-entity case.
        writer
            .append_provenance(&Provenance::new(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
                ProvenanceKind::Batch,
                None,
                Actor::Cli,
                None,
                None,
            ))
            .unwrap();
        append_at(
            &writer,
            1_700_000_001,
            ProvenanceKind::Create,
            "specs--real",
        );

        let result =
            crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA)
                .unwrap();
        // Only the Create-Real event surfaces; the batch is dropped.
        assert_eq!(result.changes.len(), 1);
        match &result.changes[0] {
            crate::ops::ChangeEnvelope::Added { id, .. } => {
                assert_eq!(id.0, "specs--real");
            }
            other => panic!("expected Added, got {other:?}"),
        }
    }

    #[test]
    fn folder_changes_since_head_echoes_cursor_when_no_events_in_window() {
        // Events exist but all before the cursor → head echoes cursor.
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        append_at(&writer, 1_700_000_000, ProvenanceKind::Create, "specs--old");

        let cursor = changelog::format_rfc3339_utc(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_900_000_000),
        );
        let result = crate::ops::folder_changes_since(tmp.path(), "specs", &cursor).unwrap();
        assert!(result.changes.is_empty());
        assert_eq!(result.head, cursor);
    }

    #[test]
    fn backend_writes_delegate_to_memwriter() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        use crate::backend::MemBackend;
        let backend: &dyn MemBackend = &writer;

        backend.write_entity(Path::new("a.md"), b"alpha").unwrap();
        backend.commit("seed", &ctx_for_test()).unwrap();
        assert_eq!(std::fs::read(tmp.path().join("a.md")).unwrap(), b"alpha");

        backend.delete_entity(Path::new("a.md")).unwrap();
        backend.commit("drop", &ctx_for_test()).unwrap();
        assert!(!tmp.path().join("a.md").exists());
    }

    #[test]
    fn parse_client_id_splits_on_last_at() {
        let c = parse_client_id("claude-code@2.1.0").unwrap();
        assert_eq!(c.name, "claude-code");
        assert_eq!(c.version, "2.1.0");
        // Edge: name with `.`
        let c = parse_client_id("foo.bar@1.0").unwrap();
        assert_eq!(c.name, "foo.bar");
        // Bare strings without `@` → None (forward-compat: tolerant
        // readers ignore rather than mis-construct).
        assert!(parse_client_id("naked").is_none());
        assert!(parse_client_id("@1.0").is_none());
        assert!(parse_client_id("name@").is_none());
    }
}

#[cfg(test)]
mod folder_drift_tests {
    use super::*;

    const ENTITY: &str = "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Seed\n\n## Identity\n\nSeed.\n";
    const SIBLING_ENTITY: &str = "---\ntype: spec\ncreated_date: 2026-01-02\nlast_modified: 2026-01-02\nlevel: M0\n---\n# Sibling\n\n## Identity\n\nWritten out-of-band.\n";

    fn folder_engine(dir: std::path::PathBuf) -> crate::Engine {
        let mount = crate::Mount {
            mem: "specs".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: crate::MountStorage::Folder { path: dir.clone() },
            capability: crate::MountCapability::Write,
            lifecycle: crate::MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        let backend = Box::new(FilesystemMemWriter::new(dir)) as Box<dyn crate::MemBackend>;
        crate::Engine::from_mounts(vec![(mount, backend)]).unwrap()
    }

    /// A sibling process's folder commit is drift: the changelog-ts
    /// cursor advances, `reload_if_stale` reloads, surfaces
    /// MEM_RELOADED, stashes the structured notice — and the engine's
    /// own writes never masquerade as drift (the recorded head is
    /// probe-corrected to the cursor dialect).
    #[test]
    fn sibling_folder_write_is_drift_and_self_write_is_not() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("specs");
        std::fs::create_dir_all(&dir).unwrap();
        // Seed through a writer WITH provenance so a baseline cursor exists.
        let seeder = FilesystemMemWriter::new(dir.clone());
        MemWriter::write_entity(&seeder, std::path::Path::new("seed.md"), ENTITY.as_bytes())
            .unwrap();
        MemWriter::commit(&seeder, "seed", &CommitContext::internal()).unwrap();
        crate::backend::MemBackend::append_provenance(
            &seeder,
            &Provenance::new(
                std::time::SystemTime::now(),
                ProvenanceKind::Create,
                Some("specs--seed".into()),
                Actor::Cli,
                None,
                None,
            ),
        )
        .unwrap();

        let mut engine = folder_engine(dir.clone());
        // First probe captures the baseline silently.
        assert!(engine.reload_if_stale(None).is_empty());

        // Self-write through the engine: no spurious drift afterwards.
        engine
            .create_entity(
                crate::CreateEntityArgs {
                    mem: "specs".to_string(),
                    title: "Self Made".to_string(),
                    entity_type: "spec".to_string(),
                    sections: [
                        ("identity".to_string(), "self".to_string()),
                        ("purpose".to_string(), "prove no self-drift".to_string()),
                    ]
                    .into_iter()
                    .collect(),
                    metadata: Default::default(),
                    relations: Vec::new(),
                    anchors: Vec::new(),
                    dry_run: false,
                },
                crate::vcs::Actor::Cli,
                None,
                None,
            )
            .unwrap();
        assert!(
            engine.reload_if_stale(None).is_empty(),
            "the engine's own write must not read as sibling drift"
        );
        assert!(engine.take_mem_changed_notices().is_empty());

        // Sibling write: a separate writer instance (a stand-in for a
        // second process) commits + appends provenance out-of-band.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let sibling = FilesystemMemWriter::new(dir);
        MemWriter::write_entity(
            &sibling,
            std::path::Path::new("sibling.md"),
            SIBLING_ENTITY.as_bytes(),
        )
        .unwrap();
        MemWriter::commit(&sibling, "sibling", &CommitContext::internal()).unwrap();
        crate::backend::MemBackend::append_provenance(
            &sibling,
            &Provenance::new(
                std::time::SystemTime::now(),
                ProvenanceKind::Create,
                Some("specs--sibling".into()),
                Actor::Cli,
                None,
                None,
            ),
        )
        .unwrap();

        let warnings = engine.reload_if_stale(None);
        assert_eq!(
            warnings.len(),
            1,
            "sibling drift must surface: {warnings:?}"
        );
        match &warnings[0] {
            crate::ops::WarningHint::MemReloaded { mem, .. } => assert_eq!(mem, "specs"),
            other => panic!("expected MemReloaded, got {other:?}"),
        }
        let notices = engine.take_mem_changed_notices();
        assert_eq!(notices.len(), 1);
        // Post-reload the sibling entity is visible.
        assert!(
            engine
                .get_entity(&crate::EntityId("specs--sibling".to_string()))
                .is_some(),
            "reload must surface the sibling's entity"
        );
        // Idempotent probe: no repeat notice.
        assert!(engine.reload_if_stale(None).is_empty());
    }
}
