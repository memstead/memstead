//! Where check rows live.
//!
//! A git-branch mem keeps its check rows in the store that holds it, on
//! one ref per mem beside its branch ([`crate::workspace::checks_ref_for_mem`],
//! a tree with one file of transport lines). Every workspace that mounts
//! the mem reads the same rows from there, a copy of the mem-repo carries
//! them, and adopting the mem into another store brings them along. A
//! check still writes nothing on the mem's branch: the ref is not a mem,
//! moves no tip a reader or a proposal pins, and touches no content hash.
//!
//! Every other row lives in the workspace's ledger file: a folder mem's,
//! a row recorded through a mount that may not write into its store, and
//! rows a workspace held before the store was the home. A reading folds
//! both ([`crate::check::CheckLedger::composed`]); a row present in both is
//! one row, because a row is its bytes.
//!
//! An append to a store reads the ref's rows, unites the new ones and
//! commits on the tip it read; a commit that lost the race to another
//! process reads again and retries, so concurrent recorders in several
//! engines and several workspaces never lose a row.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::backend::{BackendError, MemBackend};
use crate::check::{CheckLedger, CheckRecord};
use crate::engine::{Engine, EngineError};
use crate::workspace::{MountCapability, MountLifecycle, MountStorage};

/// One store's rows of one mem, and the checks ref's tip they were read
/// at (`None`: the ref did not exist).
pub(crate) type CachedRows = (Option<String>, Arc<Vec<CheckRecord>>);

/// The rows each store's checks ref held at the tip they were read at,
/// keyed by (git directory, mem).
#[derive(Debug, Default)]
pub struct ChecksCache {
    pub(crate) rows: HashMap<(PathBuf, String), CachedRows>,
}

/// Where a new check row of a mem is appended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckHome {
    /// The workspace's ledger file.
    File,
    /// The checks ref of the mem in the store at `gitdir`.
    Store { gitdir: PathBuf },
}

/// How often an append to a store retries after another writer moved the
/// ref between the read and the commit.
const STORE_APPEND_ATTEMPTS: u32 = 64;

/// The name of the file a workspace keeps the ledger length in at which
/// it last moved its rows into the stores.
const MIGRATION_MARKER: &str = "store-migrated";

/// What a migration of a workspace's file rows into the stores did, per
/// mem.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MigratedChecks {
    pub mem: String,
    /// Rows the mem's store did not hold before and holds now.
    pub moved: usize,
    /// The file's rows of the mem the store holds after the run, moved
    /// now or before (an earlier run, the automatic one at opening).
    pub held: usize,
}

impl Engine {
    /// A backend over `ref_name` in `gitdir`, for reading and writing a
    /// checks tree. Never mounted, never registered.
    pub(crate) fn checks_ref_backend(
        &self,
        gitdir: &Path,
        ref_name: &str,
    ) -> Result<Box<dyn MemBackend>, EngineError> {
        let mount = crate::workspace::Mount {
            mem: ref_name.to_string(),
            schema: None,
            storage: MountStorage::GitBranch {
                gitdir: gitdir.to_path_buf(),
                branch: ref_name.to_string(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
            check: None,
        };
        self.instantiate_backend(&mount)
            .map_err(|e| EngineError::Mem(format!("checks ref backend: {e}")))
    }

    /// The git directory whose checks ref holds `mem`'s rows: the mount's
    /// for a mounted git-branch mem, the workspace's own mem-repo for a mem
    /// that is not mounted (its rows outlive the mount), none for a folder,
    /// archive or in-memory mount, or where the engine has no git-branch
    /// operations.
    fn checks_store_gitdir(&self, mem: &str) -> Option<PathBuf> {
        let gitdir = match self.mounts.iter().find(|m| m.mount.mem == mem) {
            Some(m) => match &m.mount.storage {
                MountStorage::GitBranch { gitdir, .. } => Some(gitdir.clone()),
                _ => None,
            },
            None => self
                .workspace_root()
                .map(|root| root.join("mem-repo").join(".git"))
                .filter(|g| g.is_dir()),
        }?;
        self.can_hold_store_checks(&gitdir).then_some(gitdir)
    }

    /// Whether this engine can read and write checks refs in `gitdir`: it
    /// has git-branch operations and a backend factory that builds a
    /// git-branch backend (an engine built from the base crate alone
    /// keeps every row in the workspace file).
    fn can_hold_store_checks(&self, gitdir: &Path) -> bool {
        self.git_branch_ops.is_some()
            && self
                .checks_ref_backend(gitdir, &crate::workspace::checks_ref_for_mem("_"))
                .is_ok()
    }

    /// Where a new check row of `mem` is appended: the mem's store when
    /// the mem is a mounted git-branch mem this workspace may write into,
    /// the workspace file otherwise.
    pub(crate) fn check_home(&self, mem: &str) -> CheckHome {
        if self.git_branch_ops.is_none() {
            return CheckHome::File;
        }
        match self.mounts.iter().find(|m| m.mount.mem == mem) {
            Some(m) => match &m.mount.storage {
                MountStorage::GitBranch { gitdir, .. }
                    if m.mount.may_check() && self.can_hold_store_checks(gitdir) =>
                {
                    CheckHome::Store {
                        gitdir: gitdir.clone(),
                    }
                }
                _ => CheckHome::File,
            },
            None => CheckHome::File,
        }
    }

    /// The rows the store at `gitdir` holds for `mem`, read again only
    /// when the ref's tip moved since the last read. A ref that does not
    /// exist holds none; a store that cannot be read is read as holding
    /// none, so a reading degrades to the workspace file's rows.
    pub(crate) fn store_check_rows(&self, gitdir: &Path, mem: &str) -> Arc<Vec<CheckRecord>> {
        let ref_name = crate::workspace::checks_ref_for_mem(mem);
        let Ok(backend) = self.checks_ref_backend(gitdir, &ref_name) else {
            return Arc::new(Vec::new());
        };
        let Ok(tip) = backend.current_head() else {
            return Arc::new(Vec::new());
        };
        let key = (gitdir.to_path_buf(), mem.to_string());
        if let Ok(cache) = self.checks_cache.lock()
            && let Some((cached_tip, rows)) = cache.rows.get(&key)
            && *cached_tip == tip
        {
            return rows.clone();
        }
        let rows = match tip {
            None => Vec::new(),
            Some(_) => backend
                .read_entity(Path::new(crate::workspace::CHECKS_REF_FILE))
                .ok()
                .flatten()
                .map(|bytes| CheckLedger::parse_lines(&bytes))
                .unwrap_or_default(),
        };
        let rows = Arc::new(rows);
        if let Ok(mut cache) = self.checks_cache.lock() {
            cache.rows.insert(key, (tip, rows.clone()));
        }
        rows
    }

    /// The rows this workspace sees for `mem`: its ledger file beside the
    /// rows the mem's store holds.
    pub fn check_ledger_for_mem(&self, mem: &str) -> CheckLedger {
        let store_rows = self
            .checks_store_gitdir(mem)
            .map(|gitdir| self.store_check_rows(&gitdir, mem).as_ref().clone())
            .unwrap_or_default();
        CheckLedger::composed(self.workspace_root(), store_rows)
    }

    /// Every row this workspace sees: its ledger file beside the rows the
    /// stores of its mounted git-branch mems hold.
    pub fn check_ledger_all(&self) -> CheckLedger {
        let mut store_rows = Vec::new();
        if self.git_branch_ops.is_some() {
            for m in &self.mounts {
                if let MountStorage::GitBranch { gitdir, .. } = &m.mount.storage
                    && self.can_hold_store_checks(gitdir)
                {
                    store_rows.extend(self.store_check_rows(gitdir, &m.mount.mem).iter().cloned());
                }
            }
        }
        CheckLedger::composed(self.workspace_root(), store_rows)
    }

    /// Unite `rows` into the checks ref of `mem` in the store at `gitdir`:
    /// read the ref's rows, add those it does not hold, commit on the tip
    /// that was read. A commit that lost the race to another writer reads
    /// again and retries. Returns the number of rows the store did not
    /// hold before; zero writes nothing.
    pub(crate) fn append_store_check_rows(
        &self,
        gitdir: &Path,
        mem: &str,
        rows: &[CheckRecord],
    ) -> Result<usize, EngineError> {
        if rows.is_empty() {
            return Ok(0);
        }
        let ref_name = crate::workspace::checks_ref_for_mem(mem);
        let file = Path::new(crate::workspace::CHECKS_REF_FILE);
        let mut last: Option<BackendError> = None;
        for attempt in 0..STORE_APPEND_ATTEMPTS {
            if attempt > 0 {
                // Spread the retries of writers that collided.
                let jitter = u64::from(attempt.min(10)) * 3
                    + u64::from(std::process::id() % 7)
                    + u64::from(attempt % 5);
                std::thread::sleep(std::time::Duration::from_millis(jitter));
            }
            let backend = self.checks_ref_backend(gitdir, &ref_name)?;
            let tip = backend.current_head().map_err(EngineError::Backend)?;
            let bytes = backend.read_entity(file).map_err(EngineError::Backend)?;
            if backend.current_head().map_err(EngineError::Backend)? != tip {
                continue;
            }
            let held = bytes
                .as_deref()
                .map(CheckLedger::parse_lines)
                .unwrap_or_default();
            let present: std::collections::HashSet<String> = held
                .iter()
                .filter_map(|r| serde_json::to_string(r).ok())
                .collect();
            let mut seen = present.clone();
            let fresh: Vec<CheckRecord> = rows
                .iter()
                .filter(|r| {
                    serde_json::to_string(r)
                        .map(|c| seen.insert(c))
                        .unwrap_or(false)
                })
                .cloned()
                .collect();
            if fresh.is_empty() {
                return Ok(0);
            }
            let mut all = held;
            all.extend(fresh.iter().cloned());
            backend
                .write_entity(file, &CheckLedger::transport_lines(&all))
                .map_err(EngineError::Backend)?;
            let ctx = crate::vcs::CommitContext::new(
                Some("memstead_check"),
                crate::vcs::Actor::Cli,
                None,
                None,
                self.current_role(),
                self.current_identity().map(str::to_string),
            );
            let subject = format!("memstead: checks {mem}");
            // The tip that was read is the pin, absence included: an empty
            // pin asks that the ref still not exist, so a writer that read
            // no ref never overwrites the first row another writer created.
            match backend.commit_with_expected_parent(
                &subject,
                &ctx,
                Some(tip.as_deref().unwrap_or("")),
            ) {
                Ok(_) => {
                    if let Ok(mut cache) = self.checks_cache.lock() {
                        cache.rows.remove(&(gitdir.to_path_buf(), mem.to_string()));
                    }
                    return Ok(fresh.len());
                }
                Err(
                    e @ (BackendError::HashMismatch { .. } | BackendError::ParentMismatch { .. }),
                ) => {
                    last = Some(e);
                    continue;
                }
                Err(e) => return Err(EngineError::Backend(e)),
            }
        }
        Err(EngineError::CheckNotRecorded {
            reason: format!(
                "the checks ref of mem `{mem}` moved under {STORE_APPEND_ATTEMPTS} consecutive \
                 attempts to append ({})",
                last.map(|e| e.to_string()).unwrap_or_default()
            ),
        })
    }

    /// Append `rows` of `mem` where the mem's rows live
    /// ([`Self::check_home`]). A failure refuses typed
    /// (`CHECK_NOT_RECORDED`): a row the caller believes recorded must be.
    pub(crate) fn append_check_rows(
        &self,
        mem: &str,
        rows: &[CheckRecord],
    ) -> Result<(), EngineError> {
        if rows.is_empty() {
            return Ok(());
        }
        match self.check_home(mem) {
            CheckHome::Store { gitdir } => {
                self.append_store_check_rows(&gitdir, mem, rows)
                    .map_err(|e| match e {
                        e @ EngineError::CheckNotRecorded { .. } => e,
                        other => EngineError::CheckNotRecorded {
                            reason: format!(
                                "append to the checks ref of mem `{mem}` failed: {}",
                                other.prose_render()
                            ),
                        },
                    })?;
                Ok(())
            }
            CheckHome::File => {
                let Some(root) = self.workspace_root() else {
                    return Err(EngineError::CheckNotRecorded {
                        reason: "engine has no workspace root — no durable check store".to_string(),
                    });
                };
                for rec in rows {
                    CheckLedger::for_workspace(root).record(rec).map_err(|e| {
                        EngineError::CheckNotRecorded {
                            reason: format!("ledger append failed: {e}"),
                        }
                    })?;
                }
                Ok(())
            }
        }
    }

    /// Move the rows this workspace's ledger file holds for its writable
    /// git-branch mems into those mems' stores. Idempotent: a row a store
    /// already holds is not added again, and the file keeps its lines
    /// (the reading counts a row present in both once). Rows of folder
    /// mems, of read-only mounts and of mems the workspace does not mount
    /// stay in the file and nowhere else. Returns what moved, per mem.
    pub fn migrate_checks_to_stores(&self) -> Result<Vec<MigratedChecks>, EngineError> {
        let Some(root) = self.workspace_root() else {
            return Ok(Vec::new());
        };
        let file = CheckLedger::for_workspace(root);
        let len_before = file.file_len();
        let mut out = Vec::new();
        let mems: Vec<String> = self.mounts.iter().map(|m| m.mount.mem.clone()).collect();
        for mem in mems {
            let CheckHome::Store { gitdir } = self.check_home(&mem) else {
                continue;
            };
            let rows = file.file_rows_for_mem(&mem);
            if rows.is_empty() {
                continue;
            }
            let moved =
                self.append_store_check_rows(&gitdir, &mem, &rows)
                    .map_err(|e| match e {
                        e @ EngineError::CheckNotRecorded { .. } => e,
                        other => EngineError::CheckNotRecorded {
                            reason: format!(
                                "moving the ledger file's rows of mem `{mem}` into its checks ref \
                             failed: {}; the rows stay in the file, where this workspace \
                             still reads them",
                                other.prose_render()
                            ),
                        },
                    })?;
            out.push(MigratedChecks {
                held: rows.len(),
                mem,
                moved,
            });
        }
        write_marker(root, len_before);
        Ok(out)
    }

    /// [`Self::migrate_checks_to_stores`] when the ledger file changed
    /// since the workspace last moved its rows, so an engine of this
    /// version moves a workspace's rows when it first opens it, and again
    /// after an older engine appended to the file. Best effort: a failure
    /// leaves the rows in the file, where every reading of this
    /// workspace still finds them, and the next opening tries again.
    pub fn migrate_checks_if_ledger_changed(&self) {
        let Some(root) = self.workspace_root() else {
            return;
        };
        let len = CheckLedger::for_workspace(root).file_len();
        if len == 0 || read_marker(root) == Some(len) {
            return;
        }
        let _ = self.migrate_checks_to_stores();
    }
}

fn marker_path(root: &Path) -> PathBuf {
    root.join(crate::workspace_store::WORKSPACE_STORE_DIR)
        .join("state")
        .join("checks")
        .join(MIGRATION_MARKER)
}

fn read_marker(root: &Path) -> Option<u64> {
    std::fs::read_to_string(marker_path(root))
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

fn write_marker(root: &Path, len: u64) {
    let path = marker_path(root);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format!("{len}\n"));
}
