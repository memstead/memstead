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
//! The lock is reentrant per thread ([`crate::storage::path_lock`]): a
//! mutation that calls another mutation entry point (a batch over
//! single-item paths, a merge over creates) takes the same lock again
//! without blocking on itself. Two threads, or two engines on two
//! threads, still exclude each other.
//!
//! The guard is also the one exit every mutation shares for its staged
//! writes. A mutation stages bytes into the backend and commits them
//! later; an error between the two (a refused sidecar write, a failed
//! commit, a `?` in any verb) used to leave the staged bytes in the
//! backend, where the next read served them as content that never
//! landed. When the outermost guard of a thread drops, it discards
//! whatever is still staged on the mems it locked. After a commit
//! nothing is staged and the discard is a no-op, so a call site has no
//! failure path to remember: holding the lock is the cleanup.
//!
//! [`MemBackend::write_lock_path`]: crate::backend::MemBackend::write_lock_path

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::backend::{BackendError, MemBackend};
use crate::storage::path_lock;

use super::{Engine, EngineError};

/// Held for the duration of one mutation; dropping it discards the
/// writes still staged on the mems it covers and releases every lock
/// it took (both when the last nested holder drops).
#[must_use = "the write lock is released as soon as the guard drops"]
pub struct MemWriteGuard {
    /// Each covered mem's backend with its lock-file path. A backend
    /// with no shared on-disk state has no path and takes no OS lock,
    /// but its staged writes are discarded like any other's.
    held: Vec<Held>,
}

/// One mem a guard covers.
struct Held {
    path: Option<PathBuf>,
    backend: Arc<dyn MemBackend>,
    /// The mount's record of a discard that failed
    /// ([`super::MountedBackend::strays`]). The guard cannot reach the
    /// engine when it drops, so it shares this flag with the mount.
    strays: Arc<std::sync::atomic::AtomicBool>,
}

thread_local! {
    /// How many guards of this thread cover a backend, by its address.
    /// Only the outermost one discards: an inner guard dropping must
    /// not empty a staged set the outer mutation is still building.
    static COVERED: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
}

fn backend_key(backend: &Arc<dyn MemBackend>) -> usize {
    Arc::as_ptr(backend) as *const () as usize
}

impl MemWriteGuard {
    fn empty() -> Self {
        Self { held: Vec::new() }
    }

    fn cover(&mut self, held: Held) {
        COVERED.with(|c| {
            *c.borrow_mut()
                .entry(backend_key(&held.backend))
                .or_insert(0) += 1
        });
        self.held.push(held);
    }
}

impl Drop for MemWriteGuard {
    fn drop(&mut self) {
        for Held {
            path,
            backend,
            strays,
        } in self.held.iter().rev()
        {
            let outermost = COVERED.with(|c| {
                let mut covered = c.borrow_mut();
                let key = backend_key(backend);
                match covered.get_mut(&key) {
                    Some(count) if *count > 1 => {
                        *count -= 1;
                        false
                    }
                    _ => {
                        covered.remove(&key);
                        true
                    }
                }
            });
            // Discard before the lock goes, so no other writer can
            // stage between the two. A discard that fails leaves the
            // staged writes where a read or the next commit would pick
            // them up, so the backend is remembered and the engine
            // discards again before its next operation.
            if outermost {
                strays.store(
                    backend.discard_pending().is_err(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }
            if let Some(path) = path {
                path_lock::release(path);
            }
        }
    }
}

impl Engine {
    /// Discard what a failed mutation left staged on `mem`'s backend,
    /// if an earlier discard failed. Run before every operation, ahead
    /// of anything that reads the backend: a staged write is served in
    /// place of the committed bytes, and the next commit would land it.
    /// An error means the staged writes are still there.
    pub(super) fn discard_strays(&self, mem: &str) -> Result<(), BackendError> {
        let Some(state) = self.mounts.iter().find(|m| m.mount.mem == mem) else {
            return Ok(());
        };
        if !state.strays.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        state.backend.discard_pending()?;
        state
            .strays
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

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
            Ok(MemWriteGuard::empty())
        }
    }

    /// Take the cross-process write lock of every named mem, in sorted
    /// order (so two multi-mem writers can never deadlock), and hold it
    /// until the returned guard drops. Call it before the
    /// reload-before-operation probe of a mutation and keep the guard
    /// alive past the commit. Mems that are not mounted are skipped:
    /// the mutation's own lookup reports an unknown mem with its usual
    /// error. A backend with no shared state takes no OS lock and is
    /// still covered for the staged-write discard.
    pub fn lock_mems_for_write<S: AsRef<str>>(
        &self,
        mems: &[S],
    ) -> Result<MemWriteGuard, EngineError> {
        let mut locks: Vec<Held> = mems
            .iter()
            .filter_map(|name| {
                let m = self.mounts.iter().find(|m| m.mount.mem == name.as_ref())?;
                Some(Held {
                    path: m.backend.write_lock_path(),
                    backend: Arc::clone(&m.backend),
                    strays: Arc::clone(&m.strays),
                })
            })
            .collect();
        // Sorted by lock path so two multi-mem writers take their locks
        // in one order; pathless backends take no lock and sort first.
        locks.sort_by(|a, b| a.path.cmp(&b.path));
        locks.dedup_by(|a, b| backend_key(&a.backend) == backend_key(&b.backend));
        let mut guard = MemWriteGuard::empty();
        for held in locks {
            if let Some(path) = &held.path {
                path_lock::acquire(path).map_err(|e| EngineError::Backend(BackendError::Io(e)))?;
            }
            guard.cover(held);
        }
        Ok(guard)
    }
}
