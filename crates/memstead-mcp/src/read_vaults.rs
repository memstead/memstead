//! Batch-install helper for the `--read-vault` CLI flag.
//!
//! Wraps [`memstead_git_branch::vault_cache::install_read_vault`] in a small loop
//! so the binary entry point stays thin and integration tests can drive
//! the behavior without spawning the MCP server.

use std::path::{Path, PathBuf};

use memstead_git_branch::vault_cache::{self, InstallError, InstallOutcome, TargetVault};
use memstead_git_branch::vcs::CommitContext;

/// Outcome of processing a single `--read-vault` argument.
///
/// Owning enum instead of `Result` so callers can iterate the full batch
/// and decide per entry how loudly to surface it — the binary warn-logs,
/// tests inspect structure.
#[derive(Debug)]
pub enum ReadVaultResult {
    /// Validator accepted the archive; side effects captured in
    /// `outcome` (cache write + config registration, either or both may
    /// have been no-ops for an already-present vault).
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

/// Install every `--read-vault` archive against `target`, one by one,
/// collecting per-archive outcomes.
///
/// **Warn-and-continue semantics.** A malformed archive does not abort
/// the batch — the caller receives a `Failed` entry and keeps going.
/// The write vault stays useful on its own; tearing the server down
/// over one bad `--read-vault` is worse DX than a visible warning plus
/// a running server. Hard-fail remains a future option via a dedicated
/// flag if a use case emerges; not built now.
///
/// Relative `archive` paths resolve against `cwd`. The `target` selects
/// disk vs. vault-repo registration shape; `ctx` + `commit_message` ride
/// along for the vault-repo arm.
pub fn install_read_vaults(
    archives: &[PathBuf],
    target: TargetVault<'_>,
    ctx: &CommitContext<'_>,
    commit_message: &str,
    cwd: &Path,
    writable_vault_names: &[&str],
) -> Vec<ReadVaultResult> {
    archives
        .iter()
        .map(|archive| {
            let archive = if archive.is_absolute() {
                archive.clone()
            } else {
                cwd.join(archive)
            };
            match vault_cache::install_read_vault(
                &archive,
                target,
                ctx,
                commit_message,
                writable_vault_names,
            ) {
                Ok(outcome) => ReadVaultResult::Installed { archive, outcome },
                Err(error) => ReadVaultResult::Failed { archive, error },
            }
        })
        .collect()
}
