//! Cross-process write serialisation per mem.
//!
//! Every mutation runs reload-before-operation, compares the caller's
//! `expected_hash`, then commits. Those three steps must be one atomic
//! unit across processes: if a sibling process commits between this
//! process's reload and its commit, the compare runs against a snapshot
//! that is already behind, and a stale write lands over the sibling's
//! (a lost update reported as success). The guard below holds an
//! exclusive OS file lock on the mem's [`MemBackend::write_lock_path`]
//! for the whole unit, so a second writer blocks until the first has
//! committed and then reloads, sees the new content, and gets its honest
//! `HASH_MISMATCH`.
//!
//! The lock is reentrant per thread: a mutation that calls another
//! mutation entry point (a batch over single-item paths, a merge over
//! creates) takes the same lock again without blocking on itself. Two
//! threads, or two engines on two threads, still exclude each other,
//! because each takes its own open file description and OS file locks
//! conflict between those even inside one process.
//!
//! [`MemBackend::write_lock_path`]: crate::backend::MemBackend::write_lock_path

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;

use crate::backend::BackendError;

use super::{Engine, EngineError};

thread_local! {
    /// Locks this thread holds, by lock-file path, with a nesting count.
    static HELD: RefCell<HashMap<PathBuf, (File, usize)>> = RefCell::new(HashMap::new());
}

/// Held for the duration of one mutation; dropping it releases every
/// lock it took (the OS lock goes when the last nested holder drops).
#[must_use = "the write lock is released as soon as the guard drops"]
pub struct MemWriteGuard {
    paths: Vec<PathBuf>,
}

impl Drop for MemWriteGuard {
    fn drop(&mut self) {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            for path in self.paths.iter().rev() {
                let release = match held.get_mut(path) {
                    Some((_, count)) => {
                        *count -= 1;
                        *count == 0
                    }
                    None => false,
                };
                if release && let Some((file, _)) = held.remove(path) {
                    // Closing the descriptor releases the lock too; the
                    // explicit unlock just makes the release immediate.
                    let _ = file.unlock();
                }
            }
        });
    }
}

fn acquire(path: &PathBuf) -> Result<(), BackendError> {
    let already = HELD.with(|held| {
        if let Some((_, count)) = held.borrow_mut().get_mut(path) {
            *count += 1;
            true
        } else {
            false
        }
    });
    if already {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        // Self-ignoring, like the findings and friction stores: a lock
        // file inside a tracked folder mem never shows up as git noise.
        let gitignore = dir.join(".gitignore");
        if !gitignore.exists() {
            let _ = std::fs::write(&gitignore, "*\n");
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.lock()?;
    HELD.with(|held| held.borrow_mut().insert(path.clone(), (file, 1)));
    Ok(())
}

impl Engine {
    /// [`Self::lock_mems_for_write`] when `writes` is true; an empty
    /// guard otherwise. A rehearsal (`dry_run`) commits nothing, so it
    /// takes no lock and leaves the workspace byte-identical, lock files
    /// included.
    pub fn lock_mems_for_write_if<S: AsRef<str>>(
        &self,
        writes: bool,
        mems: &[S],
    ) -> Result<MemWriteGuard, EngineError> {
        if writes {
            self.lock_mems_for_write(mems)
        } else {
            Ok(MemWriteGuard { paths: Vec::new() })
        }
    }

    /// Take the cross-process write lock of every named mem, in sorted
    /// order (so two multi-mem writers can never deadlock), and hold it
    /// until the returned guard drops. Call it before the
    /// reload-before-operation probe of a mutation and keep the guard
    /// alive past the commit. Mems that are not mounted, or whose
    /// backend has no shared state, are skipped: the mutation's own
    /// lookup reports an unknown mem with its usual error.
    pub fn lock_mems_for_write<S: AsRef<str>>(
        &self,
        mems: &[S],
    ) -> Result<MemWriteGuard, EngineError> {
        let mut paths: Vec<PathBuf> = mems
            .iter()
            .filter_map(|name| {
                self.mounts
                    .iter()
                    .find(|m| m.mount.mem == name.as_ref())
                    .and_then(|m| m.backend.write_lock_path())
            })
            .collect();
        paths.sort();
        paths.dedup();
        let mut guard = MemWriteGuard {
            paths: Vec::with_capacity(paths.len()),
        };
        for path in paths {
            acquire(&path).map_err(EngineError::Backend)?;
            guard.paths.push(path);
        }
        Ok(guard)
    }
}
