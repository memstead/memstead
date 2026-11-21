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

/// Take the lock at `path`, waiting at most `limit` for another holder
/// to release it; `None` waits without bound, as [`acquire`] does.
/// `Ok(true)` means it is held now and needs one [`release`];
/// `Ok(false)` means the limit passed with the lock still held
/// elsewhere, and nothing is held here.
pub(crate) fn acquire_within(path: &Path, limit: Option<std::time::Duration>) -> io::Result<bool> {
    let Some(limit) = limit else {
        return acquire(path).map(|()| true);
    };
    if nest(path) {
        return Ok(true);
    }
    let file = open(path)?;
    let started = std::time::Instant::now();
    // Ordinary contention between two writers resolves in
    // milliseconds, so the first retries come quickly; a holder that
    // stays is polled at a pace that costs nothing.
    let mut pause = std::time::Duration::from_millis(1);
    loop {
        match file.try_lock() {
            Ok(()) => {
                HELD.with(|held| held.borrow_mut().insert(path.to_path_buf(), (file, 1)));
                return Ok(true);
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
        let waited = started.elapsed();
        if waited >= limit {
            return Ok(false);
        }
        std::thread::sleep(pause.min(limit - waited));
        pause = (pause * 2).min(std::time::Duration::from_millis(50));
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A holder on another thread: takes the lock, says so, and lets go
    /// when told to.
    fn hold(path: &Path) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let (held_tx, held_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let path = path.to_path_buf();
        let handle = std::thread::spawn(move || {
            acquire(&path).unwrap();
            held_tx.send(()).unwrap();
            let _ = go_rx.recv();
            release(&path);
        });
        held_rx.recv().unwrap();
        (go_tx, handle)
    }

    #[test]
    fn a_held_lock_ends_the_wait_at_the_limit_and_holds_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("write.lock");
        let (go, holder) = hold(&path);

        let started = Instant::now();
        let limit = Duration::from_millis(300);
        assert!(!acquire_within(&path, Some(limit)).unwrap());
        let waited = started.elapsed();
        assert!(waited >= limit, "waited only {waited:?}");
        assert!(waited < Duration::from_secs(5), "waited {waited:?}");
        assert!(
            !try_acquire(&path).unwrap(),
            "the timed-out wait left no hold behind on this thread"
        );

        go.send(()).unwrap();
        holder.join().unwrap();
        assert!(acquire_within(&path, Some(limit)).unwrap());
        release(&path);
    }

    #[test]
    fn a_lock_released_inside_the_limit_is_taken() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("write.lock");
        let (go, holder) = hold(&path);
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            go.send(()).unwrap();
        });
        let started = Instant::now();
        assert!(acquire_within(&path, Some(Duration::from_secs(10))).unwrap());
        assert!(started.elapsed() >= Duration::from_millis(100));
        release(&path);
        releaser.join().unwrap();
        holder.join().unwrap();
    }

    #[test]
    fn no_limit_waits_until_the_holder_lets_go() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("write.lock");
        let (go, holder) = hold(&path);
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            go.send(()).unwrap();
        });
        let started = Instant::now();
        assert!(acquire_within(&path, None).unwrap());
        assert!(started.elapsed() >= Duration::from_millis(250));
        release(&path);
        releaser.join().unwrap();
        holder.join().unwrap();
    }
}
