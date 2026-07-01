//! Vault write-side trait. The [`VaultWriter`] surface is
//! backend-neutral: it deals in vault-relative paths, raw bytes, and
//! opaque [`CommitId`] strings. Two adapters live in the workspace
//! today: the git-tree adapter in `memstead_git_branch::storage::git_tree`
//! (vault-repo writes) and [`filesystem::FilesystemVaultWriter`]
//! in this crate (filesystem-only writes — no gix, no commit
//! history).
//!
//! # Trait surface
//!
//! Four operations cover today's mutation surface:
//! - [`VaultWriter::write_entity`] — upsert raw bytes at a vault-relative path.
//! - [`VaultWriter::delete_entity`] — remove a vault-relative path.
//! - [`VaultWriter::move_entity`] — rename within the vault.
//! - [`VaultWriter::commit`] — flush pending mutations into a single commit.
//!
//! # Errors
//!
//! All four methods return [`VaultWriterError`]. Engine-layer code in
//! `memstead-git-branch` wraps this into its `EngineError::VaultWriter` (a
//! `#[from]` conversion) and the MCP layer wraps it as a
//! `VAULT_WRITER_ERROR`-coded envelope.

pub mod archive;
pub mod filesystem;
pub mod in_memory;

pub use archive::ArchiveBackend;
pub use filesystem::FilesystemVaultWriter;
pub use in_memory::InMemoryBackend;

use std::path::Path;

use crate::vcs::CommitContext;

/// Opaque commit identifier returned by [`VaultWriter::commit`].
/// Backend-defined string — the git-tree adapter formats it as a
/// hex-encoded object id (40 chars for sha-1, 64 for sha-256), but the
/// trait surface treats it as opaque. Callers carry it back into the
/// engine's `HashMismatch.current` envelope when the adapter detects a
/// CAS conflict.
pub type CommitId = String;

/// Write-side abstraction for vault content. Implementations are
/// `Send + Sync` so the engine can hold a `Box<dyn VaultWriter>` on
/// each `VaultState` and reach it from any caller.
pub trait VaultWriter: Send + Sync {
    /// Upsert `content` at `rel_path` (vault-relative). Pending until
    /// [`Self::commit`].
    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), VaultWriterError>;

    /// Remove `rel_path` (vault-relative). Idempotent: no-op when the
    /// path is already absent. Pending until [`Self::commit`].
    fn delete_entity(&self, rel_path: &Path) -> Result<(), VaultWriterError>;

    /// Rename `from` to `to` (both vault-relative). Pending until
    /// [`Self::commit`]. Errors if `to` already exists.
    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), VaultWriterError>;

    /// Flush pending mutations into a single commit. The implementation
    /// picks up the actor / committer / trailer information from `ctx`.
    /// Returns the resulting [`CommitId`] (opaque; the git-tree adapter
    /// formats it as a hex object id).
    fn commit(
        &self,
        message: &str,
        ctx: &CommitContext<'_>,
    ) -> Result<CommitId, VaultWriterError>;
}

/// Errors surfaced by [`VaultWriter`].
#[derive(Debug, thiserror::Error)]
pub enum VaultWriterError {
    #[error("vault writer io error: {0}")]
    Io(#[from] std::io::Error),
    /// Commit-time CAS conflict: the adapter snapshotted parent commit
    /// `X` at write-time, but by commit-time the underlying store has
    /// advanced to `current`. Surfaced by adapters that perform
    /// commit-tip CAS (the git-tree adapter); mapped onto the engine's
    /// `HashMismatch` envelope so MCP agents see a single
    /// `HASH_MISMATCH` code regardless of whether the conflict was
    /// detected at the entity-hash level or at the commit level.
    #[error("vault writer cas conflict: current commit is now {current}")]
    HashMismatch {
        /// New commit identifier observed when the CAS check failed.
        /// Opaque to base callers; the git-tree adapter populates it
        /// with a hex commit object id.
        current: CommitId,
    },
    /// Path-related rejection that does not map onto the IO case —
    /// e.g. an empty relative path or a path that escapes the vault
    /// root.
    #[error("vault writer path error: {0}")]
    Path(String),
}
