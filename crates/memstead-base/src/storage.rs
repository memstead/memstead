//! The storage backends this crate ships and the commit-id type they
//! share. The one write-side abstraction is [`crate::backend::MemBackend`]
//! (bytes-level list / read / write / delete / move / commit); every
//! backend implements it, and every error it raises is a
//! [`crate::backend::BackendError`]. Three backends live here: the
//! folder backend ([`filesystem::FilesystemBackend`], no gix, no commit
//! history), the sealed archive ([`archive::ArchiveBackend`], reads
//! only) and the in-memory backend ([`in_memory::InMemoryBackend`]);
//! the git-tree backend lives in `memstead_git_branch::storage::git_tree`.
//!
//! Until 2026-09-11 a second writer trait carried the four write
//! methods with its own error type; the backend trait absorbed it, so
//! one name and one error type cover the write path.

pub mod archive;
pub mod filesystem;
pub mod in_memory;

pub use archive::ArchiveBackend;
pub use filesystem::FilesystemBackend;
pub use in_memory::InMemoryBackend;

/// Opaque commit identifier returned by
/// [`crate::backend::MemBackend::commit`]. Backend-defined string: the
/// git-tree backend formats it as a hex-encoded object id (40 chars for
/// sha-1, 64 for sha-256), the folder backend mints a synthetic token;
/// callers treat it as opaque. Carried back into the engine's
/// `HashMismatch.current` envelope when a backend detects a commit-tip
/// CAS conflict.
pub type CommitId = String;
