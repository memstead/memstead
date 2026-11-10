//! Exclusive OS file locks, reentrant per thread.
//!
//! The one primitive under the engine's per-mem write lock
//! (`crate::engine::write_lock`) and under the folder backend's
//! recovery, which must exclude a live writer before it touches what
//! an interrupted commit left on disk. Both take the same lock file,
//! so they share this table: a recovery that runs inside a mutation
//! (the reload before the operation) finds the lock already held by
//! its own thread and nests instead of blocking on itself.
//!
//! Two threads, or two engines on two threads, still exclude each
//! other, because each takes its own open file description and OS file
//! locks conflict between those even inside one process.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

thread_local! {
    /// Locks this thread holds, by lock-file path, with a nesting count.
    static HELD: RefCell<HashMap<PathBuf, (File, usize)>> = RefCell::new(HashMap::new());
}

fn nest(path: &Path) -> bool {
    HELD.with(|held| {
        if let Some((_, count)) = held.borrow_mut().get_mut(path) {
            *count += 1;
            true
        } else {
            false
        }
    })
}

fn open(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        // Self-ignoring, like the findings and friction stores: a lock
        // file inside a tracked folder mem never shows up as git noise.
        let gitignore = dir.join(".gitignore");
        if !gitignore.exists() {
            let _ = std::fs::write(&gitignore, "*\n");
        }
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Take the lock at `path`, waiting for another holder to release it.
/// Pair every `Ok` with one [`release`].
pub(crate) fn acquire(path: &Path) -> io::Result<()> {
    if nest(path) {
        return Ok(());
    }
    let file = open(path)?;
    file.lock()?;
    HELD.with(|held| held.borrow_mut().insert(path.to_path_buf(), (file, 1)));
    Ok(())
}

/// Take the lock at `path` only if no one else holds it. `Ok(true)`
/// means it is held now and needs one [`release`].
pub(crate) fn try_acquire(path: &Path) -> io::Result<bool> {
    if nest(path) {
        return Ok(true);
    }
    let file = open(path)?;
    match file.try_lock() {
        Ok(()) => {
            HELD.with(|held| held.borrow_mut().insert(path.to_path_buf(), (file, 1)));
            Ok(true)
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// Whether this thread holds a lock, and not the one at `path`. A
/// thread in that position must not wait for `path`: its locks were
/// taken in one sorted order, a lock taken now is out of that order,
/// and a second thread waiting the other way round would wait for
/// ever.
pub(crate) fn holds_another(path: &Path) -> bool {
    HELD.with(|held| {
        let held = held.borrow();
        !held.is_empty() && !held.contains_key(path)
    })
}

/// Give back one hold on `path`; the OS lock goes with the last one.
pub(crate) fn release(path: &Path) {
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        let last = match held.get_mut(path) {
            Some((_, count)) => {
                *count -= 1;
                *count == 0
            }
            None => false,
        };
        if last && let Some((file, _)) = held.remove(path) {
            // Closing the descriptor releases the lock too; the
            // explicit unlock just makes the release immediate.
            let _ = file.unlock();
        }
    });
}
