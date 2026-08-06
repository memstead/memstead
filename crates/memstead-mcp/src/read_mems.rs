//! Batch-install helper for the `--read-mem` CLI flag.
//!
//! Wraps [`memstead_git_branch::mem_cache::install_to_cache`] +
//! [`memstead_git_branch::mem_cache::register_cached_archive`] in a
//! small loop so the binary entry point stays thin and integration
//! tests can drive the behavior without spawning the MCP server. Each
//! archive lands in the global cache and registers as a
//! **workspace-level read-only mount** — the same model `memstead
//! install` produces; no writable mem's config is touched.

use std::path::{Path, PathBuf};

use memstead_git_branch::mem_cache::{
    self, CacheInstallOutcome, InstallError, MountRegistration,
};

/// Outcome of processing a single `--read-mem` argument.
///
/// Owning enum instead of `Result` so callers can iterate the full batch
/// and decide per entry how loudly to surface it — the binary warn-logs,
/// tests inspect structure.
#[derive(Debug)]
pub enum ReadMemResult {
    /// Validator accepted the archive; it is cached and mounted
    /// (either or both may have been no-ops for already-present
    /// content).
    Installed {
        archive: PathBuf,
        outcome: CacheInstallOutcome,
        mount: MountRegistration,
    },
    /// Validation, cache I/O, or mount registration failed. The error
    /// `Display` preserves path + reason, so a warn log over the
    /// value is actionable without unwrapping the variant.
    Failed { archive: PathBuf, error: String },
}

/// Install every `--read-mem` archive as a workspace-level read-only
/// mount, one by one, collecting per-archive outcomes.
///
/// **Warn-and-continue semantics.** A malformed archive does not abort
/// the batch — the caller receives a `Failed` entry and keeps going.
/// The write mem stays useful on its own; tearing the server down
/// over one bad `--read-mem` is worse DX than a visible warning plus
/// a running server.
///
/// Relative `archive` paths resolve against `cwd`. The caller persists
/// the mount state once after the batch (`engine.persist_state()`)
/// when any entry reports a `Registered` / `Refreshed` mount.
pub fn install_read_mems(
    engine: &mut memstead_base::Engine,
    archives: &[PathBuf],
    cwd: &Path,
) -> Vec<ReadMemResult> {
    let writable: Vec<String> = engine
        .mem_router()
        .writable_mems()
        .iter()
        .map(|n| n.to_string())
        .collect();

    archives
        .iter()
        .map(|archive| {
            let archive = if archive.is_absolute() {
                archive.clone()
            } else {
                cwd.join(archive)
            };
            let writable_refs: Vec<&str> = writable.iter().map(String::as_str).collect();
            let outcome = match mem_cache::install_to_cache(&archive, &writable_refs) {
                Ok(o) => o,
                Err(e) => {
                    return ReadMemResult::Failed {
                        archive,
                        error: e.to_string(),
                    };
                }
            };
            match mem_cache::register_cached_archive(engine, &outcome, "--read-mem") {
                Ok(mount) => ReadMemResult::Installed {
                    archive,
                    outcome,
                    mount,
                },
                Err(e) => ReadMemResult::Failed {
                    archive,
                    error: e.to_string(),
                },
            }
        })
        .collect()
}

/// Re-exported for callers that log validation failures specifically.
pub type ReadMemInstallError = InstallError;
