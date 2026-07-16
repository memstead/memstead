//! `MemBackend` — uniform trait surface over folder, git-branch,
//! and archive storage.
//!
//! Bytes-level: list / read / write / delete / move / commit /
//! append-provenance / read-provenance. The one-engine architecture
//! that the workspace-store rebuild produces sits above this trait;
//! entity-mutation logic, validation, the in-memory store, and the
//! search index live in one place regardless of which backend serves
//! a given mount.
//!
//! Today's [`crate::storage::MemWriter`] is a write-side subset of
//! this trait. As each backend gains its `MemBackend` impl the
//! `MemWriter` references in that backend's call sites collapse
//! into the unified surface; `MemWriter` stays in
//! `crate::storage::filesystem` for now as the on-disk write helpers
//! it embodies are reused by the folder-backend `MemBackend` impl.
//!
//! ## Per-backend write semantics
//!
//! - **Folder** — writes go to the workspace's mem subdirectory;
//!   commit is a no-op CAS-token mint (no history).
//! - **Git-branch** — writes buffer in memory, commit produces a real
//!   git commit on the per-mem branch with the trailer block.
//! - **Archive** — writes return [`BackendError::Sealed`] without
//!   touching disk. Read methods return live content from inside the
//!   sealed `.mem` zip.

use std::path::{Path, PathBuf};

use crate::provenance::Provenance;
use crate::storage::{CommitId, MemWriterError};
use crate::vcs::CommitContext;

/// Mem-backend trait. Implementations live next to the backend's
/// other code (folder under `crate::storage::filesystem`; git-branch
/// in the renamed-from-`memstead-git-branch` crate; archive under the
/// archive read-paths in `crate::entity` once that wiring lands).
///
/// Methods are not split into `Read` / `Write` sub-traits because
/// the engine's mutation paths frequently need both surfaces on the
/// same backend handle (read current bytes, validate, write new
/// bytes). Backends that cannot write return [`BackendError::Sealed`]
/// from the write methods — typed and stable so callers branch on
/// the discriminant rather than parsing a message string.
pub trait MemBackend: Send + Sync {
    /// Mem-relative paths of every entity-bearing file the backend
    /// holds. Order is not specified; callers that need stable
    /// ordering sort.
    fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError>;

    /// Read raw bytes at `rel_path`. `Ok(None)` for a missing path
    /// (idempotent reads); `Err` for IO or backend-specific failures.
    fn read_entity(&self, rel_path: &Path) -> Result<Option<Vec<u8>>, BackendError>;

    /// Upsert `content` at `rel_path`. Pending until [`Self::commit`].
    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), BackendError>;

    /// Remove `rel_path`. Idempotent: no-op when the path is already
    /// absent. Pending until [`Self::commit`].
    fn delete_entity(&self, rel_path: &Path) -> Result<(), BackendError>;

    /// Rename `from` to `to`. Pending until [`Self::commit`]. Errors
    /// when `to` already exists.
    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), BackendError>;

    /// Discard every pending (uncommitted) mutation, returning the
    /// staging buffer to empty *without* producing a commit. The
    /// transactional escape hatch for stage-then-commit callers:
    /// the atomic `batch_update` stages each item's write into the
    /// pending set, and when a later item fails validation it calls
    /// this to drop the already-staged writes rather than commit a
    /// half-applied batch. Idempotent — discarding an empty buffer
    /// is a no-op.
    ///
    /// Default impl is a no-op: backends that never stage writes
    /// (archive / any sealed backend) have no buffer to clear. The
    /// folder and git-branch backends override to clear their
    /// pending buffer (the git-branch backend also drops the
    /// captured parent snapshot, symmetric with what `commit` does
    /// on success).
    fn discard_pending(&self) -> Result<(), BackendError> {
        Ok(())
    }

    /// Flush pending mutations into a single commit. Returns the
    /// resulting opaque [`CommitId`]; backends without history
    /// return a synthetic id (UNIX-nanos + counter, hex) so callers
    /// always get a non-empty cursor.
    fn commit(&self, message: &str, ctx: &CommitContext<'_>) -> Result<CommitId, BackendError>;

    /// Commit pending mutations with a parent-ref pinning guard.
    /// When `expected_parent` is `Some`, the backend MUST refuse the
    /// commit (`Err(BackendError::ParentMismatch { ... })`) if its
    /// current head no longer matches the supplied ref — a sibling
    /// writer advanced the on-disk state between the snapshot the
    /// caller pinned and now. When `expected_parent` is `None`, the
    /// call is equivalent to [`Self::commit`].
    ///
    /// Used by atomic multi-file mutations (notably the planned
    /// referrer-rewriting rename) to surface drift mid-operation
    /// rather than between operations. Backends without history
    /// (folder, archive) inherit the default impl: they ignore
    /// `expected_parent` because there's no concept of a parent to
    /// pin against — drift detection on those mounts is a no-op
    /// today and stays a no-op here. The git-branch backend
    /// overrides to check the per-mem branch tip and surfaces
    /// the mismatch with a typed error the engine layer can map to
    /// `MEM_RELOADED` / `RENAME_PARTIAL_FAILURE`.
    ///
    /// Default impl: ignore `expected_parent` and delegate to
    /// [`Self::commit`]. Bisect-safe — existing callers using
    /// `Self::commit` directly are unaffected.
    fn commit_with_expected_parent(
        &self,
        message: &str,
        ctx: &CommitContext<'_>,
        _expected_parent: Option<&str>,
    ) -> Result<CommitId, BackendError> {
        self.commit(message, ctx)
    }

    /// Append a [`Provenance`] record to the backend's mutation log.
    /// Persistence form differs per backend — JSONL line, commit
    /// trailer, etc. — but the in-memory shape is identical.
    fn append_provenance(&self, record: &Provenance) -> Result<(), BackendError>;

    /// Read provenance entries since `cursor` (opaque,
    /// backend-defined: a commit SHA for git-branch, an RFC-3339
    /// timestamp for folder, ignored for archive). `None` cursor
    /// means "from the beginning".
    fn read_provenance(&self, cursor: Option<&str>) -> Result<Vec<Provenance>, BackendError>;

    /// Opaque cursor pointing at the backend's current state. The
    /// engine compares against a per-mount cached cursor to detect
    /// drift — a sibling writer (another `Engine` instance, an
    /// out-of-band `git pull`, etc.) advancing the on-disk state past
    /// what the engine last read. Backends without history (folder,
    /// archive) inherit the default impl returning `Ok(None)`; the
    /// engine treats `None` as "no drift signal available" and skips
    /// drift detection for that mount. The git-branch backend
    /// overrides to return the per-mem branch tip's commit SHA hex;
    /// the filesystem backend overrides to return the changelog's
    /// last-line timestamp cursor (folder mems with no changelog yet
    /// keep `None`). Archive and in-memory backends stay on the
    /// default.
    ///
    /// Returning `Err` is reserved for backend-internal failures
    /// (refdb hiccup, archive read failure, etc.); the engine logs
    /// the error and treats it as a transient None — drift detection
    /// is best-effort and never blocks the read it accompanies.
    fn current_head(&self) -> Result<Option<String>, BackendError> {
        Ok(None)
    }

    /// Read the per-mem `.memstead/config.json` payload, if any.
    ///
    /// Returns the raw bytes the backend has for the mem's
    /// config. The engine parses via
    /// [`memstead_schema::config::parse_mem_config`] and stores the
    /// result on the [`crate::Engine::mem_config_for`] accessor.
    ///
    /// Default impl returns `Ok(None)` — backends that don't
    /// surface a config (or haven't yet implemented this primitive)
    /// inherit and signal "no config available". The engine
    /// treats `None` the same as a parse failure: `mem_config_for`
    /// returns `None` for the affected mem, and consumers
    /// (`memstead_health { include_config: true }`) emit empty
    /// `writeGuidance` + `extra` blocks for that mem.
    ///
    /// Mirrors the pattern of [`Self::current_head`] —
    /// backend-internal capability with a sensible no-op default.
    ///
    /// Implementations:
    /// - Folder backend reads `<root>/.memstead/config.json`.
    /// - Archive backend reads `.memstead/config.json` from inside the
    ///   zip.
    /// - Git-branch backend reads `__MEMSTEAD:mems/<mem>/config.json`.
    fn read_mem_config(&self) -> Result<Option<Vec<u8>>, BackendError> {
        Ok(None)
    }

    /// Read the optional authoring-provenance payload
    /// (`.memstead/provenance.json`) the archive carries, if any.
    ///
    /// Returns the raw bytes the engine parses into a
    /// [`memstead_schema::ArchiveProvenance`] and surfaces via
    /// [`crate::Engine::archive_provenance_for`]. Default impl returns
    /// `Ok(None)` — a backend with no provenance member (a pre-provenance
    /// archive, the folder/git-branch backends until their read paths
    /// lift) inherits and signals "provenance absent". Mirrors
    /// [`Self::read_mem_config`].
    fn read_archive_provenance(&self) -> Result<Option<Vec<u8>>, BackendError> {
        Ok(None)
    }

    /// Write the per-mem `.memstead/config.json` payload. Symmetric
    /// counterpart to [`Self::read_mem_config`].
    ///
    /// Backends that cannot persist a config (today: archive)
    /// inherit the default and return [`BackendError::Sealed`]. The
    /// engine's create / migrate paths branch on the discriminant
    /// before calling.
    ///
    /// Implementations:
    /// - Folder backend writes `<root>/.memstead/config.json` to disk.
    /// - Git-branch backend writes
    ///   `__MEMSTEAD:mems/<mem>/config.json` (workspace-level ref) —
    ///   its own commit, separate from any per-mem-branch
    ///   mutation.
    /// - Archive backend returns [`BackendError::Sealed`] — sealed
    ///   archives never re-write configs.
    ///
    /// Mirrors the symmetry pattern of
    /// [`Self::read_entity`] / [`Self::write_entity`]: the trait
    /// surface stays balanced so the engine doesn't branch on
    /// backend type for write paths.
    fn write_mem_config(&self, _bytes: &[u8]) -> Result<(), BackendError> {
        Err(BackendError::Sealed)
    }

    /// Like [`Self::write_mem_config`] but records `note` (an optional
    /// agent/operator-supplied provenance reason) on the resulting
    /// commit body. The default delegates to the note-less form, so
    /// backends without a commit (folder) simply ignore the note; the
    /// git-branch backend overrides this to thread `note` into the
    /// `__MEMSTEAD`-ref commit. Lets `set_mem_version` carry a `--note`
    /// like the other commit-producing mem-lifecycle operations.
    fn write_mem_config_with_note(
        &self,
        bytes: &[u8],
        _note: Option<&str>,
    ) -> Result<(), BackendError> {
        self.write_mem_config(bytes)
    }

    /// Record provenance for a pipeline-config edit (mediums / facets /
    /// projections / ingests). The canonical pipeline config is a plain
    /// JSON file under `.memstead/` on the workspace root — it has no
    /// commit of its own — so backends with a commit timeline mirror the
    /// edit into their provenance record; the commit is the audit trail,
    /// the disk file stays the read path.
    ///
    /// `edits`: `(config_name, Some(bytes))` upserts the mirrored blob,
    /// `(config_name, None)` removes it (a rename passes both). `kind`
    /// is the primitive's plural (`mediums`, `facets`, `projections`,
    /// `ingests`); `verb` names the operation for the commit subject.
    ///
    /// The default is a successful no-op: folder and archive backends
    /// have no commit timeline, so the note is accepted and dropped —
    /// the same posture as [`Self::write_mem_config_with_note`]. The
    /// git-branch backend overrides this to commit the mirror under
    /// `__MEMSTEAD:pipeline/<kind>/<mem>/<name>.json` with `note` on
    /// the commit body.
    fn record_pipeline_edit(
        &self,
        _kind: &str,
        _edits: &[(String, Option<Vec<u8>>)],
        _note: Option<&str>,
        _verb: &str,
    ) -> Result<(), BackendError> {
        Ok(())
    }

    /// Read the engine-owned anchors sidecar
    /// ([`crate::anchor::ANCHOR_SIDECAR_PATH`]) bytes, if any.
    ///
    /// The sidecar lives on the mem branch under the `.memstead/`
    /// umbrella every external reader already filters, so it never
    /// surfaces as an entity. Returns the raw bytes the engine parses via
    /// [`crate::anchor::AnchorSidecar::from_bytes`]; `Ok(None)` for a mem
    /// that has never written anchors.
    ///
    /// Default impl returns `Ok(None)` — a backend that does not persist
    /// anchors (a pre-anchor archive, any read-only mount) inherits and
    /// signals "no anchors". Mirrors [`Self::read_mem_config`]. The
    /// git-branch and in-memory backends override to read the sidecar
    /// from their store (pending-buffer precedence, so a staged sidecar
    /// write is visible before its commit).
    fn read_anchors_sidecar(&self) -> Result<Option<Vec<u8>>, BackendError> {
        Ok(None)
    }

    /// Stage a write of the engine-owned anchors sidecar so it rides the
    /// **same commit** as the entity mutation that produced it — the
    /// atomicity guarantee anchors depend on (rename's referrer-rewrite,
    /// delete's anchor removal, and branch_reset's rewind all move
    /// entity + anchor state together).
    ///
    /// Pending until the next [`Self::commit`] — callers stage the entity
    /// write, then the sidecar write, then commit once. Backends that
    /// cannot persist anchors (archive / any sealed backend) inherit the
    /// default returning [`BackendError::Sealed`]; the engine's write
    /// path branches on mount capability before calling. The git-branch
    /// and in-memory backends override to buffer the sidecar under
    /// [`crate::anchor::ANCHOR_SIDECAR_PATH`] in the same pending set the
    /// entity write used.
    fn write_anchors_sidecar(&self, _bytes: &[u8]) -> Result<(), BackendError> {
        Err(BackendError::Sealed)
    }

    /// Drop every backend-side artifact for this mem — the
    /// symmetric counterpart to the writes performed by
    /// `memstead_mem_create` (entity-seed commit on the per-mem
    /// branch + [`Self::write_mem_config`] on `__MEMSTEAD`). Called by
    /// `memstead_mem_delete` orchestration when `delete_files=true` and
    /// the delete rule matched, to give the backend a chance to
    /// prune ref-store state the engine alone has the git authority
    /// to touch.
    ///
    /// Idempotent: safe to call on a backend whose artifacts already
    /// went away (a sibling engine pruned them, the branch was
    /// deleted manually, etc.). The default impl returns `Ok(())` —
    /// backends whose on-disk state is fully captured by the mem
    /// directory (folder, archive) inherit the no-op. The
    /// orchestrator handles its `remove_dir_all` separately at the
    /// outer layer.
    ///
    /// Implementations:
    /// - Folder backend keeps the default — its disk state is the
    ///   mem directory, which the orchestrator rmdirs.
    /// - Archive backend keeps the default — sealed archives have
    ///   nothing additional to prune.
    /// - Git-branch backend deletes `refs/heads/<branch_leaf>` and
    ///   commits a tree edit on `refs/heads/__MEMSTEAD` that removes
    ///   `mems/<branch_leaf>/config.json`. `<branch_leaf>` is the
    ///   mem's full hierarchical path (e.g.
    ///   `planning/plan-q4-revamp` or the bare `<name>` for flat
    ///   layouts).
    fn delete_artifacts(&self) -> Result<(), BackendError> {
        Ok(())
    }
}

/// Errors surfaced by [`MemBackend`].
///
/// The `Sealed` variant is the typed read-only signal — backends
/// that physically cannot write (archive) return it from every
/// mutating method. Callers (the engine's mutation pipeline) branch
/// on the discriminant before reaching the backend; a `Sealed`
/// reaching this layer is a programming error in the upstream
/// capability check.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// Re-thrown from the existing [`MemWriterError`] surface so
    /// folder-backend implementations can lift `MemWriter`
    /// failures without lossy conversion.
    #[error(transparent)]
    MemWriter(#[from] MemWriterError),
    /// Backend physically rejects writes. Returned by the archive
    /// backend and any future read-only backend (e.g. registry pin).
    #[error("backend is sealed (writes rejected)")]
    Sealed,
    /// Filesystem IO failure outside the [`MemWriterError`] path.
    #[error("backend io error: {0}")]
    Io(#[from] std::io::Error),
    /// Backend-specific failure not modelled by the variants above.
    /// Carries an agent-readable message; structured backend errors
    /// add their own variant.
    #[error("backend error: {0}")]
    Other(String),
    /// Parent-ref pinning guard tripped on
    /// [`MemBackend::commit_with_expected_parent`] — the backend's
    /// current head no longer matches the caller's `expected_parent`.
    /// A sibling writer (another `Engine` instance, an out-of-band
    /// `git pull`, a manual git operation) advanced the on-disk state
    /// between the snapshot the caller pinned and now. The engine
    /// layer maps this into `MEM_RELOADED` /
    /// `RENAME_PARTIAL_FAILURE` depending on whether other mems
    /// already committed in the same logical operation. Today only
    /// the git-branch backend (planned override) produces this
    /// variant; folder and archive backends inherit the default impl
    /// of `commit_with_expected_parent` which delegates to `commit`
    /// without parent checking.
    #[error(
        "parent-ref mismatch: expected {expected}, found {actual} — sibling writer advanced the mem"
    )]
    ParentMismatch { expected: String, actual: String },
}
