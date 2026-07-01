//! Batch-install helper for the `--read-mem` CLI flag.
//!
//! Wraps [`memstead_git_branch::mem_cache::install_read_mem`] in a small loop
//! so the binary entry point stays thin and integration tests can drive
//! the behavior without spawning the MCP server.

use std::path::{Path, PathBuf};

use memstead_git_branch::mem_cache::{self, InstallError, InstallOutcome, TargetMem};
use memstead_git_branch::vcs::CommitContext;

/// Outcome of processing a single `--read-mem` argument.
///
/// Owning enum instead of `Result` so callers can iterate the full batch
/// and decide per entry how loudly to surface it — the binary warn-logs,
/// tests inspect structure.
#[derive(Debug)]
pub enum ReadMemResult {
    /// Validator accepted the archive; side effects captured in
    /// `outcome` (cache write + config registration, either or both may
    /// have been no-ops for an already-present mem).
    Installed {
        archive: PathBuf,
        outcome: InstallOutcome,
    },
    /// Validator rejected the archive, or an I/O step around it failed.
    /// `InstallError::Validation`'s `Display` preserves path + reason
    /// from the underlying `ValidationError`, so a warn log over the
    /// error value is actionable without unwrapping the variant.
    Failed {
        archive: PathBuf,
        error: InstallError,
    },
}

/// Install every `--read-mem` archive against `target`, one by one,
/// collecting per-archive outcomes.
///
/// **Warn-and-continue semantics.** A malformed archive does not abort
/// the batch — the caller receives a `Failed` entry and keeps going.
/// The write mem stays useful on its own; tearing the server down
/// over one bad `--read-mem` is worse DX than a visible warning plus
/// a running server. Hard-fail remains a future option via a dedicated
/// flag if a use case emerges; not built now.
///
/// Relative `archive` paths resolve against `cwd`. The `target` selects
/// disk vs. mem-repo registration shape; `ctx` + `commit_message` ride
/// along for the mem-repo arm.
pub fn install_read_mems(
    archives: &[PathBuf],
    target: TargetMem<'_>,
    ctx: &CommitContext<'_>,
    commit_message: &str,
    cwd: &Path,
    writable_mem_names: &[&str],
) -> Vec<ReadMemResult> {
    archives
        .iter()
        .map(|archive| {
            let archive = if archive.is_absolute() {
                archive.clone()
            } else {
                cwd.join(archive)
            };
            match mem_cache::install_read_mem(
                &archive,
                target,
                ctx,
                commit_message,
                writable_mem_names,
            ) {
                Ok(outcome) => ReadMemResult::Installed { archive, outcome },
                Err(error) => ReadMemResult::Failed { archive, error },
            }
        })
        .collect()
}
