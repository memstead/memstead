//! Mem write-side trait. The [`MemWriter`] surface is
//! backend-neutral: it deals in mem-relative paths, raw bytes, and
//! opaque [`CommitId`] strings. Two adapters live in the workspace
//! today: the git-tree adapter in `memstead_git_branch::storage::git_tree`
//! (mem-repo writes) and [`filesystem::FilesystemMemWriter`]
//! in this crate (filesystem-only writes — no gix, no commit
//! history).
//!
//! # Trait surface
//!
//! Four operations cover today's mutation surface:
//! - [`MemWriter::write_entity`] — upsert raw bytes at a mem-relative path.
//! - [`MemWriter::delete_entity`] — remove a mem-relative path.
//! - [`MemWriter::move_entity`] — rename within the mem.
//! - [`MemWriter::commit`] — flush pending mutations into a single commit.
//!
//! # Errors
//!
//! All four methods return [`MemWriterError`]. Engine-layer code in
//! `memstead-git-branch` wraps this into its `EngineError::MemWriter` (a
//! `#[from]` conversion) and the MCP layer wraps it as a
//! `MEM_WRITER_ERROR`-coded envelope.

pub mod archive;
pub mod filesystem;
pub mod in_memory;

pub use archive::ArchiveBackend;
pub use filesystem::FilesystemMemWriter;
pub use in_memory::InMemoryBackend;

use std::path::Path;

use crate::vcs::CommitContext;

/// Opaque commit identifier returned by [`MemWriter::commit`].
/// Backend-defined string — the git-tree adapter formats it as a
/// hex-encoded object id (40 chars for sha-1, 64 for sha-256), but the
/// trait surface treats it as opaque. Callers carry it back into the
/// engine's `HashMismatch.current` envelope when the adapter detects a
/// CAS conflict.
pub type CommitId = String;

/// Write-side abstraction for mem content. Implementations are
/// `Send + Sync` so the engine can hold a `Box<dyn MemWriter>` on
/// each `MemState` and reach it from any caller.
pub trait MemWriter: Send + Sync {
    /// Upsert `content` at `rel_path` (mem-relative). Pending until
    /// [`Self::commit`].
    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), MemWriterError>;

    /// Remove `rel_path` (mem-relative). Idempotent: no-op when the
    /// path is already absent. Pending until [`Self::commit`].
    fn delete_entity(&self, rel_path: &Path) -> Result<(), MemWriterError>;

    /// Rename `from` to `to` (both mem-relative). Pending until
    /// [`Self::commit`]. Errors if `to` already exists.
    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), MemWriterError>;

    /// Flush pending mutations into a single commit. The implementation
    /// picks up the actor / committer / trailer information from `ctx`.
    /// Returns the resulting [`CommitId`] (opaque; the git-tree adapter
    /// formats it as a hex object id).
    fn commit(
        &self,
        message: &str,
        ctx: &CommitContext<'_>,
    ) -> Result<CommitId, MemWriterError>;
}

/// Errors surfaced by [`MemWriter`].
#[derive(Debug, thiserror::Error)]
pub enum MemWriterError {
    #[error("mem writer io error: {0}")]
    Io(#[from] std::io::Error),
    /// Commit-time CAS conflict: the adapter snapshotted parent commit
    /// `X` at write-time, but by commit-time the underlying store has
    /// advanced to `current`. Surfaced by adapters that perform
    /// commit-tip CAS (the git-tree adapter); mapped onto the engine's
    /// `HashMismatch` envelope so MCP agents see a single
    /// `HASH_MISMATCH` code regardless of whether the conflict was
    /// detected at the entity-hash level or at the commit level.
    #[error("mem writer cas conflict: current commit is now {current}")]
    HashMismatch {
        /// New commit identifier observed when the CAS check failed.
        /// Opaque to base callers; the git-tree adapter populates it
        /// with a hex commit object id.
        current: CommitId,
    },
    /// Path-related rejection that does not map onto the IO case —
    /// e.g. an empty relative path or a path that escapes the mem
    /// root.
    #[error("mem writer path error: {0}")]
    Path(String),
}
