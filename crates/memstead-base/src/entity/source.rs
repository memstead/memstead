//! Entity source abstraction — where markdown comes from.
//!
//! Two backend-agnostic shapes live here: a directory on disk (for
//! live, writable workspaces) and a sealed `.mem` zip archive (for read-only
//! attached mems). The git-tree shape lives in
//! `memstead-git-branch::entity::git_tree_source`. All shapes feed the
//! same parse pipeline via the helper [`crate::entity::loader::parse_entries`].

use std::path::{Path, PathBuf};

use super::loader::LoadError;
use crate::validator::{BoundedZipRead, ValidatorLimits, read_zip_entry_bounded};

/// A backend-agnostic source of markdown entities.
pub enum EntitySource {
    /// A directory on disk. Walked recursively; engine-internal
    /// directories (`.git/`, `.memstead/`) are always skipped.
    Directory { root: PathBuf },
    /// A sealed `.mem` mem archive: a zip containing the same markdown
    /// tree a `Directory` would. Read-only; loaded as a dep alongside
    /// the primary mem. Zip-slip protected via `enclosed_name`.
    ZipArchive(PathBuf),
}

/// One successfully-read markdown entry produced by an `EntitySource`.
#[derive(Debug, Clone)]
pub struct SourceEntry {
    /// Path relative to the source root. Used as `Entity.file_path`.
    /// Uses the platform's native separator for `Directory`; will use
    /// POSIX separators for `ZipArchive` when that variant lands.
    pub relative_path: String,
    /// Source-specific path for error reporting — absolute on disk,
    /// archive-relative for zips. Opaque to the parser; kept so callers
    /// can surface a human-useful location in error messages without
    /// re-joining paths.
    pub source_path: PathBuf,
    /// Raw file contents.
    pub content: String,
}

/// A per-file read failure. Non-fatal: the walker continues past these
/// and hands them back to the caller alongside the successful entries,
/// preserving the loader's "collect errors, don't stop" behavior.
#[derive(Debug)]
pub struct SourceReadError {
    pub source_path: PathBuf,
    pub error: std::io::Error,
}

impl EntitySource {
    /// Read every `.md` entry from this source. Returns the successful
    /// entries and any per-file read errors.
    ///
    /// Source-level failures (missing directory, unreadable directory
    /// listing) surface as `Err(LoadError::…)`. Individual file read
    /// failures go into the `SourceReadError` bucket.
    pub fn read_all(&self) -> Result<(Vec<SourceEntry>, Vec<SourceReadError>), LoadError> {
        match self {
            EntitySource::Directory { root } => read_directory(root),
            EntitySource::ZipArchive(archive) => read_zip_archive(archive),
        }
    }
}

fn read_directory(
    root: &Path,
) -> Result<(Vec<SourceEntry>, Vec<SourceReadError>), LoadError> {
    if !root.exists() {
        return Err(LoadError::DirNotFound(root.display().to_string()));
    }

    let mut files = Vec::new();
    find_markdown_files(root, root, &mut files)?;
    files.sort();

    let mut entries = Vec::with_capacity(files.len());
    let mut errors = Vec::new();

    for file in &files {
        match std::fs::read_to_string(file) {
            Ok(content) => {
                let relative_path = file
                    .strip_prefix(root)
                    .unwrap_or(file)
                    .to_string_lossy()
                    .to_string();
                entries.push(SourceEntry {
                    relative_path,
                    source_path: file.clone(),
                    content,
                });
            }
            Err(error) => errors.push(SourceReadError {
                source_path: file.clone(),
                error,
            }),
        }
    }

    Ok((entries, errors))
}

fn read_zip_archive(
    archive_path: &Path,
) -> Result<(Vec<SourceEntry>, Vec<SourceReadError>), LoadError> {
    if !archive_path.is_file() {
        return Err(LoadError::ArchiveNotFound(
            archive_path.display().to_string(),
        ));
    }

    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let limits = ValidatorLimits::DEFAULT;
    if archive.len() as u32 > limits.max_file_count {
        return Err(LoadError::InvalidArchive(format!(
            "archive contains {} entries, exceeding the {}-entry cap",
            archive.len(),
            limits.max_file_count
        )));
    }

    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut uncompressed_total: u64 = 0;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let raw_name = entry.name().to_string();

        // Symlinks are rejected outright — a malicious archive could
        // point one at /etc/passwd or any other absolute path. We never
        // extract, but we also never want to surface their content.
        if entry.is_symlink() {
            return Err(LoadError::InvalidArchive(format!(
                "entry '{raw_name}': symlinks are not allowed in sealed mem archives"
            )));
        }

        // `enclosed_name` is the zip crate's blessed zip-slip guard:
        // returns `Some(path)` only if the entry name is safe (relative,
        // no `..` escape, no absolute prefix, no drive letters). `None`
        // means the archive is trying to write outside its own root.
        let safe_path = match entry.enclosed_name() {
            Some(p) => p,
            None => {
                return Err(LoadError::InvalidArchive(format!(
                    "entry '{raw_name}': path escapes archive root \
                     (absolute, '..'-components, or otherwise unsafe)"
                )));
            }
        };

        if entry.is_dir() {
            continue;
        }

        let relative_path = safe_path.to_string_lossy().to_string();
        if !relative_path.ends_with(".md") {
            // Non-markdown entries (including the meta-dir config) are
            // silently skipped here. The entity source only yields
            // entity content; mem metadata is read by
            // `mem_cache::read_published_config` on a separate pass.
            continue;
        }

        // Bounded read: decompression is never sized by the entry's
        // declared header, and a bomb refuses with a typed error —
        // the same caps the ingress validator enforces.
        let bytes = match read_zip_entry_bounded(&mut entry, limits.max_uncompressed_entry)? {
            BoundedZipRead::Within(bytes) => bytes,
            BoundedZipRead::ExceedsCap => {
                return Err(LoadError::InvalidArchive(format!(
                    "entry '{relative_path}' exceeds the {}-byte uncompressed cap",
                    limits.max_uncompressed_entry
                )));
            }
        };
        uncompressed_total = uncompressed_total.saturating_add(bytes.len() as u64);
        if uncompressed_total > limits.max_uncompressed_archive {
            return Err(LoadError::InvalidArchive(format!(
                "archive exceeds the {}-byte total uncompressed cap",
                limits.max_uncompressed_archive
            )));
        }
        match String::from_utf8(bytes) {
            Ok(content) => entries.push(SourceEntry {
                relative_path: relative_path.clone(),
                source_path: PathBuf::from(&relative_path),
                content,
            }),
            Err(error) => errors.push(SourceReadError {
                source_path: PathBuf::from(&relative_path),
                error: std::io::Error::new(std::io::ErrorKind::InvalidData, error),
            }),
        }
    }

    // Sort for deterministic ordering — the zip crate yields entries in
    // archive order, which our deterministic-export contract already
    // sorts by path, but external archives may not.
    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    Ok((entries, errors))
}

/// Recursively find all .md files under a directory.
///
/// Skip rules:
/// - `.git/` — external git metadata, never entity territory.
/// - `.memstead/` — engine-internal (config, schemas cache). Always
///   hidden at every depth.
///
/// All other directories (including unrelated dot-prefixed dirs like
/// `.obsidian/`, `.idea/`) are walked.
fn find_markdown_files(
    root: &Path,
    dir: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), LoadError> {
    let entries = std::fs::read_dir(dir)?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if path.is_dir() {
            if name.as_ref() == ".git" || name.as_ref() == crate::mem::MEM_META_DIR {
                continue;
            }
            find_markdown_files(root, &path, files)?;
        } else if name.ends_with(".md") {
            files.push(path);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn directory_reads_markdown_in_sorted_order() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("b.md"), "b").unwrap();
        fs::write(dir.path().join("a.md"), "a").unwrap();

        let (entries, errors) = EntitySource::Directory {
            root: dir.path().to_path_buf(),
        }
            .read_all()
            .unwrap();
        assert!(errors.is_empty());
        let paths: Vec<_> = entries.iter().map(|e| e.relative_path.as_str()).collect();
        assert_eq!(paths, vec!["a.md", "b.md"]);
    }

    #[test]
    fn directory_skips_engine_internal_dirs_and_non_md() {
        // `.git/` and `.memstead/` are always skipped (engine-internal).
        // Other dot-prefixed dirs (e.g. `.obsidian/`) walk normally.
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("keep.md"), "k").unwrap();
        fs::write(dir.path().join("ignore.txt"), "i").unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/secret.md"), "s").unwrap();
        fs::create_dir_all(dir.path().join(".memstead")).unwrap();
        fs::write(dir.path().join(".memstead/note.md"), "n").unwrap();
        fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
        fs::write(dir.path().join(".obsidian/vis.md"), "v").unwrap();

        let (entries, _) = EntitySource::Directory {
            root: dir.path().to_path_buf(),
        }
            .read_all()
            .unwrap();
        let paths: Vec<_> = entries.iter().map(|e| e.relative_path.as_str()).collect();
        assert!(paths.contains(&"keep.md"), "keep.md must load: {paths:?}");
        assert!(
            paths.iter().any(|p| p.ends_with("vis.md")),
            ".obsidian/vis.md must load: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains(".git")),
            ".git/* must be skipped: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains(".memstead")),
            ".memstead/* must be skipped: {paths:?}"
        );
    }

    #[test]
    fn directory_missing_root_returns_error() {
        let err = EntitySource::Directory {
            root: PathBuf::from("/nonexistent/path/xyz"),
        }
        .read_all()
        .unwrap_err();
        assert!(matches!(err, LoadError::DirNotFound(_)));
    }

    // --- zip archive ---

    use std::io::Write;
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    /// Build a minimal zip with the given `(name, content)` entries.
    /// Caller controls entry names exactly — used to test zip-slip.
    fn write_zip(path: &Path, entries: &[(&str, &str)]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, content) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn zip_archive_reads_markdown_in_sorted_order() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("pkg.mem");
        write_zip(
            &archive,
            &[
                ("b.md", "b"),
                ("a.md", "a"),
                ("meta.json", "{\"name\":\"pkg\"}"),
                (".memstead/config.json", "{}"),
                ("readme.txt", "ignored"),
                ("nested/c.md", "c"),
            ],
        );

        let (entries, errors) = EntitySource::ZipArchive(archive).read_all().unwrap();
        assert!(errors.is_empty());
        let paths: Vec<_> = entries.iter().map(|e| e.relative_path.as_str()).collect();
        assert_eq!(paths, vec!["a.md", "b.md", "nested/c.md"]);
        let contents: Vec<_> = entries.iter().map(|e| e.content.as_str()).collect();
        assert_eq!(contents, vec!["a", "b", "c"]);
    }

    #[test]
    fn zip_archive_missing_file_returns_error() {
        let err = EntitySource::ZipArchive(PathBuf::from("/nonexistent/pkg.mem"))
            .read_all()
            .unwrap_err();
        assert!(matches!(err, LoadError::ArchiveNotFound(_)));
    }

    #[test]
    fn zip_archive_corrupt_file_returns_zip_error() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("bad.mem");
        fs::write(&archive, b"not a zip file at all, just bytes").unwrap();

        let err = EntitySource::ZipArchive(archive).read_all().unwrap_err();
        assert!(
            matches!(err, LoadError::Zip(_)),
            "corrupt archive should surface as LoadError::Zip, got {err:?}"
        );
    }

    #[test]
    fn zip_archive_rejects_parent_dir_escape() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("evil.mem");
        write_zip(&archive, &[("../escape.md", "bad")]);

        let err = EntitySource::ZipArchive(archive).read_all().unwrap_err();
        let msg = format!("{err}");
        assert!(matches!(err, LoadError::InvalidArchive(_)));
        assert!(
            msg.contains("escape") || msg.contains("..") || msg.contains("unsafe"),
            "zip-slip error should explain the rejection: {msg}"
        );
    }

    #[test]
    fn zip_archive_rejects_nested_parent_dir_escape() {
        // `subdir/../../outside.md` has `..`-components that normalize to
        // "one level above the archive root". `enclosed_name` must reject
        // this — a single `..` check that only looks at the first path
        // segment would miss it.
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("evil.mem");
        write_zip(&archive, &[("subdir/../../outside.md", "bad")]);

        let err = EntitySource::ZipArchive(archive).read_all().unwrap_err();
        assert!(matches!(err, LoadError::InvalidArchive(_)));
    }

    #[test]
    fn zip_archive_rejects_oversized_entry() {
        // A deflate bomb: highly compressible content one byte past the
        // per-entry uncompressed cap. Must refuse with a typed error —
        // and the read must stop at the cap, not decompress it all.
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("bomb.mem");
        let big = "a".repeat((ValidatorLimits::DEFAULT.max_uncompressed_entry + 1) as usize);
        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("bomb.md", opts).unwrap();
        zip.write_all(big.as_bytes()).unwrap();
        zip.finish().unwrap();

        let err = EntitySource::ZipArchive(archive).read_all().unwrap_err();
        let msg = format!("{err}");
        assert!(matches!(err, LoadError::InvalidArchive(_)), "got {err:?}");
        assert!(msg.contains("cap"), "error should name the cap: {msg}");
    }

    // Note on absolute entry paths: the standard `zip::ZipWriter::start_file`
    // normalizes leading separators away, so crafting a `/etc/evil.md` entry
    // via the writer API is impossible — the writer stores it as
    // `etc/evil.md` (relative). `enclosed_name` covers the hand-crafted
    // malicious-archive case by rejecting anything that doesn't resolve to
    // a relative path, including Windows drive letters. The two `..`-based
    // tests above verify the guard is actually wired up; we trust
    // `enclosed_name` for the rest.

    // Git-tree adapter parity tests live alongside the GitTreeSource
    // implementation in `memstead-git-branch::entity::git_tree_source`.
}
