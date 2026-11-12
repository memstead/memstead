//! The IO seam under the folder backend's commit and recovery.
//!
//! Every step a commit or a recovery takes on the filesystem is one
//! call on [`FolderIo`], so a test can record the exact order of
//! writes, syncs and renames and fail or cut off the sequence at any
//! step. Production runs [`RealIo`], a direct mapping onto `std::fs`.
//! The steps are deliberately small (a write and its sync are two
//! calls): the durability argument is an argument about their order.

use std::io;
use std::path::Path;

/// One directory entry as [`FolderIo::list_dir`] reports it.
pub(crate) struct DirEntry {
    pub(crate) name: String,
    /// A real directory. A symlink to one reports `false`, so a walk
    /// never leaves the mem through a link.
    pub(crate) is_dir: bool,
}

/// The filesystem steps of a folder commit and its recovery.
pub(crate) trait FolderIo: Send + Sync {
    /// Does anything exist at `path` (no symlink follow)?
    fn exists(&self, path: &Path) -> io::Result<bool>;
    /// Whole content of `path`; `None` when it does not exist.
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>>;
    /// The last `max` bytes of `path` (all of it when shorter);
    /// `None` when it does not exist.
    fn read_tail(&self, path: &Path, max: u64) -> io::Result<Option<Vec<u8>>>;
    /// Entries of the directory `path`; empty when it does not exist.
    fn list_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;
    /// Create the one directory `path`; its parent must exist.
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    /// Create or truncate `path` and write `bytes`. Not synced.
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// Append `bytes` to `path`, creating it when absent. Not synced.
    fn append(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// Flush the content of the file `path` to stable storage.
    fn sync_file(&self, path: &Path) -> io::Result<()>;
    /// Flush the entries of the directory `path` to stable storage, so
    /// the names created, renamed or removed in it survive a power
    /// loss.
    fn sync_dir(&self, path: &Path) -> io::Result<()>;
    /// Rename `from` to `to`, replacing `to`.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Remove the file `path`. Absent is an error of kind `NotFound`.
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// Remove the empty directory `path`.
    fn remove_dir(&self, path: &Path) -> io::Result<()>;
}

/// [`FolderIo`] on the real filesystem.
pub(crate) struct RealIo;

impl FolderIo for RealIo {
    fn exists(&self, path: &Path) -> io::Result<bool> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn read_tail(&self, path: &Path, max: u64) -> io::Result<Option<Vec<u8>>> {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let mut file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(max)))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(Some(buf))
    }

    fn list_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let entries = match std::fs::read_dir(path) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry?;
            out.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: entry.file_type()?.is_dir(),
            });
        }
        Ok(out)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir(path)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        std::fs::write(path, bytes)
    }

    fn append(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(bytes)
    }

    fn sync_file(&self, path: &Path) -> io::Result<()> {
        // A sync through a second descriptor flushes what the first
        // one wrote: the flush is of the file, not of the handle.
        // Windows flushes only through a handle with write access
        // (`FlushFileBuffers` refuses a read-only one with access
        // denied), so the handle opens for writing there; it never
        // truncates or appends.
        #[cfg(windows)]
        {
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)?
                .sync_all()
        }
        #[cfg(not(windows))]
        {
            std::fs::File::open(path)?.sync_all()
        }
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        // A directory opens as a file only on unix. Elsewhere the
        // rename itself is the strongest statement available.
        #[cfg(unix)]
        {
            std::fs::File::open(path)?.sync_all()
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Ok(())
        }
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir(path)
    }
}
