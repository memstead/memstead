//! A [`FolderIo`] for tests: the real filesystem, with every step
//! recorded and one step of choice failed or cut off.
//!
//! Steps are numbered from zero in the order the backend takes them.
//! [`FaultIo::fail_at`] makes step `n` return an IO error instead of
//! running, which is an input or output error at that point.
//! [`FaultIo::kill_at`] panics at step `n` instead of running it, which
//! stands for the process dying there: the backend's error handling
//! does not run, so whatever the earlier steps left on disk stays, the
//! way a kill leaves it. Catch the panic with `std::panic::catch_unwind`
//! and carry on with a fresh backend over the same directory.

#![cfg(test)]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::io::{DirEntry, FolderIo, RealIo};

/// One recorded step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IoOp {
    Exists(PathBuf),
    Read(PathBuf),
    ReadTail(PathBuf),
    ListDir(PathBuf),
    CreateDir(PathBuf),
    Write(PathBuf),
    Append(PathBuf),
    SyncFile(PathBuf),
    SyncDir(PathBuf),
    Rename { from: PathBuf, to: PathBuf },
    RemoveFile(PathBuf),
    RemoveDir(PathBuf),
}

#[derive(Default)]
struct State {
    trace: Vec<IoOp>,
    fail_at: Option<usize>,
    kill_at: Option<usize>,
}

/// The fault-injecting IO. Cheap to clone; clones share one trace, so
/// a test keeps a handle while the backend owns another.
#[derive(Clone, Default)]
pub(crate) struct FaultIo {
    state: Arc<Mutex<State>>,
}

impl FaultIo {
    /// Record every step, fault none.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Step `n` (zero-based) returns an IO error instead of running.
    pub(crate) fn fail_at(n: usize) -> Self {
        let io = Self::default();
        io.lock().fail_at = Some(n);
        io
    }

    /// Step `n` (zero-based) panics instead of running.
    pub(crate) fn kill_at(n: usize) -> Self {
        let io = Self::default();
        io.lock().kill_at = Some(n);
        io
    }

    /// Every step taken so far, the faulted one included.
    pub(crate) fn trace(&self) -> Vec<IoOp> {
        self.lock().trace.clone()
    }

    /// How many steps were taken so far.
    pub(crate) fn op_count(&self) -> usize {
        self.lock().trace.len()
    }

    /// Forget the steps taken so far and disarm any fault, so a test
    /// can set a mem up through the same backend and then arm a fault
    /// for the commit it is about. Steps count from zero again.
    pub(crate) fn reset(&self) {
        let mut state = self.lock();
        state.trace.clear();
        state.fail_at = None;
        state.kill_at = None;
    }

    /// Arm [`Self::fail_at`] on an existing handle.
    pub(crate) fn arm_fail_at(&self, n: usize) {
        self.lock().fail_at = Some(n);
    }

    /// Arm [`Self::kill_at`] on an existing handle.
    pub(crate) fn arm_kill_at(&self, n: usize) {
        self.lock().kill_at = Some(n);
    }

    /// Index of the step that decides a commit: the rename that puts
    /// the commit record in place. A fault at or before this index
    /// strikes before the commit point, a fault after it strikes a
    /// decided commit. `None` when the trace holds no commit.
    pub(crate) fn commit_point(&self) -> Option<usize> {
        self.lock().trace.iter().position(is_record_rename)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A kill poisons the lock by design; the trace is still what
        // the test wants to read.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record `op`, then fault it if it is the chosen step.
    fn step(&self, op: IoOp) -> io::Result<()> {
        let (index, fail, kill) = {
            let mut state = self.lock();
            state.trace.push(op);
            (state.trace.len() - 1, state.fail_at, state.kill_at)
        };
        if kill == Some(index) {
            panic!("fault io: killed at step {index}");
        }
        if fail == Some(index) {
            return Err(io::Error::other(format!(
                "fault io: injected error at step {index}"
            )));
        }
        Ok(())
    }
}

/// The rename that moves a commit record into place.
fn is_record_rename(op: &IoOp) -> bool {
    matches!(op, IoOp::Rename { to, .. } if to.ends_with(super::COMMIT_RECORD_REL))
}

impl FolderIo for FaultIo {
    fn exists(&self, path: &Path) -> io::Result<bool> {
        self.step(IoOp::Exists(path.to_path_buf()))?;
        RealIo.exists(path)
    }

    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        self.step(IoOp::Read(path.to_path_buf()))?;
        RealIo.read(path)
    }

    fn read_tail(&self, path: &Path, max: u64) -> io::Result<Option<Vec<u8>>> {
        self.step(IoOp::ReadTail(path.to_path_buf()))?;
        RealIo.read_tail(path, max)
    }

    fn list_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.step(IoOp::ListDir(path.to_path_buf()))?;
        RealIo.list_dir(path)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        self.step(IoOp::CreateDir(path.to_path_buf()))?;
        RealIo.create_dir(path)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.step(IoOp::Write(path.to_path_buf()))?;
        RealIo.write(path, bytes)
    }

    fn append(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.step(IoOp::Append(path.to_path_buf()))?;
        RealIo.append(path, bytes)
    }

    fn sync_file(&self, path: &Path) -> io::Result<()> {
        self.step(IoOp::SyncFile(path.to_path_buf()))?;
        RealIo.sync_file(path)
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        self.step(IoOp::SyncDir(path.to_path_buf()))?;
        RealIo.sync_dir(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.step(IoOp::Rename {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        })?;
        RealIo.rename(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.step(IoOp::RemoveFile(path.to_path_buf()))?;
        RealIo.remove_file(path)
    }

    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        self.step(IoOp::RemoveDir(path.to_path_buf()))?;
        RealIo.remove_dir(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fault_strikes_its_step_once_and_a_reset_counts_from_zero() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("f");
        let io = FaultIo::new();
        io.write(&file, b"one").unwrap();
        io.sync_file(&file).unwrap();
        assert_eq!(io.op_count(), 2);

        io.reset();
        io.arm_fail_at(1);
        io.write(&file, b"two").unwrap();
        assert!(io.write(&file, b"never").is_err());
        assert_eq!(
            std::fs::read(&file).unwrap(),
            b"two",
            "a failed step does not run"
        );
        io.write(&file, b"three").unwrap();
        assert_eq!(
            io.trace(),
            vec![
                IoOp::Write(file.clone()),
                IoOp::Write(file.clone()),
                IoOp::Write(file.clone())
            ]
        );

        io.reset();
        io.arm_kill_at(0);
        let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = io.write(&file, b"never");
        }));
        assert!(died.is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"three");
        assert_eq!(io.op_count(), 1, "the trace survives the kill");
    }
}
