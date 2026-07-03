//! Archive [`MemBackend`] — a sealed `.mem` zip mem, read-only.
//!
//! The third sibling of folder and git-branch backends. Mem content
//! lives compressed inside a zip; the engine reads markdown entries
//! through this backend without unzipping to disk. Write methods
//! return [`BackendError::Sealed`] — archive-mounted mems are
//! distribution artifacts, not edit targets.
//!
//! ## Reading
//!
//! `list_entities` and `read_entity` open the archive on each call.
//! No in-memory caching for V1: the read paths exist primarily for
//! the engine's load-on-mount step, which calls `list_entities` once
//! and `read_entity` once per entry. Hot-path callers (e.g. interactive
//! `memstead_entity` tools against a mounted archive) re-pay the open cost
//! per request — acceptable for V1, optimisable later behind the same
//! trait surface.
//!
//! Mirrors the read semantics of [`crate::entity::source::EntitySource::ZipArchive`]:
//! - Symlinks rejected (the archive backend refuses to surface them
//!   even though it never extracts).
//! - Zip-slip protected via `enclosed_name`.
//! - Only `.md` files outside the `.memstead/` namespace surface as
//!   entities; archive-internal config / schema files are skipped at
//!   this layer (separate read paths consume them).
//!
//! ## Writes and provenance
//!
//! Every write method returns [`BackendError::Sealed`]. `read_provenance`
//! returns an empty vector — archives carry no mutation log; the
//! distribution artifact is by definition history-free at the engine
//! seam. `append_provenance` returns `Sealed` rather than silently
//! dropping; the engine's mutation pipeline branches on capability
//! before reaching the backend, so a `Sealed` here signals an upstream
//! bug.

use std::io::{Cursor, Seek};
use std::path::{Path, PathBuf};

use crate::backend::BackendError;
use crate::provenance::Provenance;
use crate::storage::CommitId;
use crate::validator::{BoundedZipRead, ValidatorLimits, read_zip_entry_bounded};
use crate::vcs::CommitContext;

/// Source of the archive bytes. Either an on-disk `.mem` file or an
/// in-memory buffer. The in-memory variant powers the byte-based
/// hydrate path on `Engine` (snapshot delivery to the bridge / WASM
/// engine) without forcing the caller to materialise a temp file.
enum ArchiveSource {
    Path(PathBuf),
    Bytes(Vec<u8>),
}

/// Archive-backed [`crate::backend::MemBackend`]. Holds either the
/// on-disk path to the sealed zip or an in-memory byte buffer; opens
/// the archive lazily on each read call.
pub struct ArchiveBackend {
    source: ArchiveSource,
}

impl ArchiveBackend {
    /// Build a backend pointing at `archive_path`. The file is not
    /// opened until the first read; constructor failure modes are
    /// limited to argument validation done at the engine layer
    /// (existence checks, extension checks).
    pub fn new(archive_path: PathBuf) -> Self {
        Self {
            source: ArchiveSource::Path(archive_path),
        }
    }

    /// Build a backend wrapping an in-memory archive byte buffer. The
    /// engine's byte-based hydrate path constructs this variant after
    /// validating the bytes through `extract_entries`. Reads parse the
    /// zip lazily on each call — same lifecycle as the path-based
    /// variant.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            source: ArchiveSource::Bytes(bytes),
        }
    }

    /// Path of the archive this backend reads from, when the backend
    /// was built from an on-disk file. `None` for byte-backed
    /// instances.
    pub fn archive_path(&self) -> Option<&Path> {
        match &self.source {
            ArchiveSource::Path(p) => Some(p),
            ArchiveSource::Bytes(_) => None,
        }
    }
}

impl ArchiveBackend {
    /// Run `visit` against a reader for the archive contents. Picks
    /// `File::open` for path-backed instances and a `Cursor` over the
    /// stored bytes for byte-backed instances. Centralises the
    /// source-dispatch so the trait impl below stays source-agnostic.
    fn with_archive_reader<F, T>(&self, f: F) -> Result<T, BackendError>
    where
        F: FnOnce(&mut dyn ReadSeek) -> Result<T, BackendError>,
    {
        match &self.source {
            ArchiveSource::Path(p) => {
                if !p.is_file() {
                    return Err(BackendError::Other(format!(
                        "archive not found: {}",
                        p.display()
                    )));
                }
                let mut file = std::fs::File::open(p).map_err(BackendError::Io)?;
                f(&mut file)
            }
            ArchiveSource::Bytes(bytes) => {
                let mut cursor = Cursor::new(bytes.as_slice());
                f(&mut cursor)
            }
        }
    }
}

/// Combined `Read + Seek` trait object the zip reader needs and that
/// both `File` and `Cursor<&[u8]>` satisfy.
trait ReadSeek: std::io::Read + Seek {}
impl<T: std::io::Read + Seek + ?Sized> ReadSeek for T {}

impl crate::backend::MemBackend for ArchiveBackend {
    fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
        let mut out = Vec::new();
        self.with_archive_reader(|reader| {
            for_each_md_entry(reader, |relative_path, _bytes| {
                out.push(PathBuf::from(relative_path));
                Ok(())
            })
        })?;
        Ok(out)
    }

    fn read_entity(&self, rel_path: &Path) -> Result<Option<Vec<u8>>, BackendError> {
        let want = rel_path.to_string_lossy().replace('\\', "/");
        let mut found: Option<Vec<u8>> = None;
        self.with_archive_reader(|reader| {
            for_each_md_entry(reader, |relative_path, bytes| {
                if relative_path == want {
                    found = Some(bytes.to_vec());
                }
                Ok(())
            })
        })?;
        Ok(found)
    }

    fn write_entity(&self, _rel_path: &Path, _content: &[u8]) -> Result<(), BackendError> {
        Err(BackendError::Sealed)
    }

    fn delete_entity(&self, _rel_path: &Path) -> Result<(), BackendError> {
        Err(BackendError::Sealed)
    }

    fn move_entity(&self, _from: &Path, _to: &Path) -> Result<(), BackendError> {
        Err(BackendError::Sealed)
    }

    fn commit(
        &self,
        _message: &str,
        _ctx: &CommitContext<'_>,
    ) -> Result<CommitId, BackendError> {
        Err(BackendError::Sealed)
    }

    fn append_provenance(&self, _record: &Provenance) -> Result<(), BackendError> {
        Err(BackendError::Sealed)
    }

    fn read_provenance(
        &self,
        _cursor: Option<&str>,
    ) -> Result<Vec<Provenance>, BackendError> {
        // Archives are history-free at the engine seam.
        Ok(Vec::new())
    }

    fn read_mem_config(&self) -> Result<Option<Vec<u8>>, BackendError> {
        // Archives bundle the per-mem config inside the zip at
        // `.memstead/config.json`. Return the raw bytes on hit,
        // Ok(None) on miss.
        // Path-backed archives that are absent return Ok(None) to
        // match the previous (non-existent-file) behaviour rather
        // than surfacing the missing file as an error.
        if let ArchiveSource::Path(p) = &self.source
            && !p.is_file()
        {
            return Ok(None);
        }
        self.with_archive_reader(|reader| {
            let mut archive = zip::ZipArchive::new(reader)
                .map_err(|e| BackendError::Other(format!("zip open: {e}")))?;
            // Take the mutable entry borrow only if the config member is
            // present (`by_name` holds `&mut archive`).
            let config_name = memstead_schema::ARCHIVE_CONFIG_PATH;
            if archive.index_for_name(config_name).is_none() {
                return Ok(None);
            }
            let mut entry = archive
                .by_name(config_name)
                .map_err(|e| BackendError::Other(format!("zip lookup: {e}")))?;
            let cap = ValidatorLimits::DEFAULT.max_config_file;
            match read_zip_entry_bounded(&mut entry, cap).map_err(BackendError::Io)? {
                BoundedZipRead::Within(bytes) => Ok(Some(bytes)),
                BoundedZipRead::ExceedsCap => Err(BackendError::Other(format!(
                    "archive config '{config_name}' exceeds the {cap}-byte cap"
                ))),
            }
        })
    }

    fn read_archive_provenance(&self) -> Result<Option<Vec<u8>>, BackendError> {
        // The optional provenance payload lives at
        // `.memstead/provenance.json` inside the zip. Same shape as
        // `read_mem_config`: raw bytes on hit, Ok(None) on miss (a
        // pre-provenance archive simply omits the member).
        if let ArchiveSource::Path(p) = &self.source
            && !p.is_file()
        {
            return Ok(None);
        }
        self.with_archive_reader(|reader| {
            let mut archive = zip::ZipArchive::new(reader)
                .map_err(|e| BackendError::Other(format!("zip open: {e}")))?;
            let prov_name = memstead_schema::ARCHIVE_PROVENANCE_PATH;
            if archive.index_for_name(prov_name).is_none() {
                return Ok(None);
            }
            let mut entry = archive
                .by_name(prov_name)
                .map_err(|e| BackendError::Other(format!("zip lookup: {e}")))?;
            let cap = ValidatorLimits::DEFAULT.max_uncompressed_entry;
            match read_zip_entry_bounded(&mut entry, cap).map_err(BackendError::Io)? {
                BoundedZipRead::Within(bytes) => Ok(Some(bytes)),
                BoundedZipRead::ExceedsCap => Err(BackendError::Other(format!(
                    "archive provenance '{prov_name}' exceeds the {cap}-byte cap"
                ))),
            }
        })
    }
}

/// Walk every `.md` entry in the archive, calling `visit` with the
/// POSIX-relative path and the entry's bytes. Centralises the
/// symlink / zip-slip / extension checks so list and read paths
/// cannot diverge.
fn for_each_md_entry<R, F>(reader: &mut R, mut visit: F) -> Result<(), BackendError>
where
    R: std::io::Read + Seek + ?Sized,
    F: FnMut(&str, &[u8]) -> Result<(), BackendError>,
{
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| BackendError::Other(format!("open archive: {e}")))?;
    let limits = ValidatorLimits::DEFAULT;
    if archive.len() as u32 > limits.max_file_count {
        return Err(BackendError::Other(format!(
            "archive contains {} entries, exceeding the {}-entry cap",
            archive.len(),
            limits.max_file_count
        )));
    }
    let mut uncompressed_total: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| BackendError::Other(format!("archive entry {i}: {e}")))?;
        let raw_name = entry.name().to_string();
        if entry.is_symlink() {
            return Err(BackendError::Other(format!(
                "entry '{raw_name}': symlinks are not allowed in sealed mem archives"
            )));
        }
        let safe_path = match entry.enclosed_name() {
            Some(p) => p,
            None => {
                return Err(BackendError::Other(format!(
                    "entry '{raw_name}': path escapes archive root \
                     (absolute, '..'-components, or otherwise unsafe)"
                )));
            }
        };
        if entry.is_dir() {
            continue;
        }
        let relative_path = safe_path.to_string_lossy().replace('\\', "/");
        if !relative_path.ends_with(".md") {
            continue;
        }
        // Skip the archive's `.memstead/` meta umbrella so config /
        // schema files don't surface as entity content. (They have
        // separate read paths.) Matches the folder backend's meta-dir
        // skip.
        if relative_path.starts_with(".memstead/") {
            continue;
        }
        let bytes = match read_zip_entry_bounded(&mut entry, limits.max_uncompressed_entry)
            .map_err(BackendError::Io)?
        {
            BoundedZipRead::Within(bytes) => bytes,
            BoundedZipRead::ExceedsCap => {
                return Err(BackendError::Other(format!(
                    "entry '{relative_path}' exceeds the {}-byte uncompressed cap",
                    limits.max_uncompressed_entry
                )));
            }
        };
        uncompressed_total = uncompressed_total.saturating_add(bytes.len() as u64);
        if uncompressed_total > limits.max_uncompressed_archive {
            return Err(BackendError::Other(format!(
                "archive exceeds the {}-byte total uncompressed cap",
                limits.max_uncompressed_archive
            )));
        }
        visit(&relative_path, &bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MemBackend;
    use std::io::Write as _;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    /// Build a sealed archive at `tmp/<name>.mem` from
    /// `(relative_path, bytes)` pairs. Returns the archive path.
    fn build_archive(tmp: &Path, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = tmp.join(format!("{name}.mem"));
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts: SimpleFileOptions = SimpleFileOptions::default();
        for (rel, bytes) in entries {
            writer.start_file(*rel, opts).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    fn ctx_for_test<'a>() -> CommitContext<'a> {
        CommitContext::internal()
    }

    #[test]
    fn list_returns_only_md_outside_memstead_namespace() {
        let tmp = TempDir::new().unwrap();
        let archive = build_archive(
            tmp.path(),
            "pkg",
            &[
                ("a.md", b"# a"),
                ("nested/b.md", b"# b"),
                ("notes.json", b"{}"),
                (".memstead/config.json", b"{}"),
                (".memstead/notes.md", b"# skip me"),
            ],
        );
        let backend = ArchiveBackend::new(archive);
        let mut paths: Vec<String> = backend
            .list_entities()
            .unwrap()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        paths.sort();
        assert_eq!(paths, vec!["a.md".to_string(), "nested/b.md".to_string()]);
    }

    /// Only the `.memstead/` meta layout is read: a config under any
    /// other dir (`.other/config.json`) is not served — the sole config
    /// member path is `.memstead/config.json`.
    #[test]
    fn foreign_layout_config_is_not_read() {
        let tmp = TempDir::new().unwrap();
        let archive = build_archive(
            tmp.path(),
            "foreign",
            &[("a.md", b"# a"), (".other/config.json", b"{\"foreign\":true}")],
        );
        let backend = ArchiveBackend::new(archive);
        assert_eq!(
            backend.read_mem_config().unwrap(),
            None,
            "a `.other/config.json` archive must not serve config"
        );
    }

    #[test]
    fn read_entity_returns_bytes_for_known_path() {
        let tmp = TempDir::new().unwrap();
        let archive = build_archive(
            tmp.path(),
            "pkg",
            &[("a.md", b"# alpha"), ("b/c.md", b"# nested")],
        );
        let backend = ArchiveBackend::new(archive);
        assert_eq!(
            backend.read_entity(Path::new("a.md")).unwrap(),
            Some(b"# alpha".to_vec())
        );
        assert_eq!(
            backend.read_entity(Path::new("b/c.md")).unwrap(),
            Some(b"# nested".to_vec())
        );
    }

    #[test]
    fn read_entity_refuses_oversized_entry() {
        // Deflate bomb one byte past the per-entry uncompressed cap:
        // the read stops at the cap and refuses with a typed error
        // instead of decompressing the whole entry into memory.
        let tmp = TempDir::new().unwrap();
        let big = vec![b'a'; (ValidatorLimits::DEFAULT.max_uncompressed_entry + 1) as usize];
        let archive = build_archive(tmp.path(), "bomb", &[("bomb.md", big.as_slice())]);
        let backend = ArchiveBackend::new(archive);
        let err = backend.read_entity(Path::new("bomb.md")).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("cap"), "error should name the cap: {msg}");
    }

    #[test]
    fn read_entity_returns_none_for_unknown_path() {
        let tmp = TempDir::new().unwrap();
        let archive = build_archive(tmp.path(), "pkg", &[("a.md", b"# a")]);
        let backend = ArchiveBackend::new(archive);
        assert_eq!(backend.read_entity(Path::new("missing.md")).unwrap(), None);
    }

    #[test]
    fn writes_return_sealed() {
        let tmp = TempDir::new().unwrap();
        let archive = build_archive(tmp.path(), "pkg", &[("a.md", b"# a")]);
        let backend = ArchiveBackend::new(archive);
        assert!(matches!(
            backend.write_entity(Path::new("x.md"), b"x"),
            Err(BackendError::Sealed)
        ));
        assert!(matches!(
            backend.delete_entity(Path::new("x.md")),
            Err(BackendError::Sealed)
        ));
        assert!(matches!(
            backend.move_entity(Path::new("a.md"), Path::new("b.md")),
            Err(BackendError::Sealed)
        ));
        assert!(matches!(
            backend.commit("msg", &ctx_for_test()),
            Err(BackendError::Sealed)
        ));
    }

    #[test]
    fn provenance_append_is_sealed_read_is_empty() {
        let tmp = TempDir::new().unwrap();
        let archive = build_archive(tmp.path(), "pkg", &[("a.md", b"# a")]);
        let backend = ArchiveBackend::new(archive);
        let record = Provenance::new(
            std::time::UNIX_EPOCH,
            crate::provenance::ProvenanceKind::Create,
            Some("v:e".into()),
            crate::vcs::Actor::Unknown,
            None,
            None,
        );
        assert!(matches!(
            backend.append_provenance(&record),
            Err(BackendError::Sealed)
        ));
        assert!(backend.read_provenance(None).unwrap().is_empty());
        // Cursor parameter is accepted but ignored — same empty result.
        assert!(backend.read_provenance(Some("anything")).unwrap().is_empty());
    }

    #[test]
    fn missing_archive_returns_typed_error_not_panic() {
        let backend = ArchiveBackend::new(PathBuf::from("/nonexistent/missing.mem"));
        match backend.list_entities() {
            Err(BackendError::Other(msg)) => assert!(msg.contains("archive not found")),
            other => panic!("expected archive-not-found Other error, got {other:?}"),
        }
    }

    #[test]
    fn from_bytes_lists_and_reads_same_as_path() {
        // Same archive content via path vs in-memory bytes must
        // produce identical list/read results — the source dispatch is
        // transparent to the trait surface.
        let tmp = TempDir::new().unwrap();
        let archive = build_archive(
            tmp.path(),
            "pkg",
            &[("a.md", b"# alpha"), ("dir/b.md", b"# nested")],
        );
        let bytes = std::fs::read(&archive).unwrap();
        let from_path = ArchiveBackend::new(archive);
        let from_bytes = ArchiveBackend::from_bytes(bytes);

        let mut path_list: Vec<String> = from_path
            .list_entities()
            .unwrap()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let mut bytes_list: Vec<String> = from_bytes
            .list_entities()
            .unwrap()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        path_list.sort();
        bytes_list.sort();
        assert_eq!(path_list, bytes_list);

        for rel in &path_list {
            let p_bytes = from_path.read_entity(Path::new(rel)).unwrap();
            let b_bytes = from_bytes.read_entity(Path::new(rel)).unwrap();
            assert_eq!(p_bytes, b_bytes, "mismatch reading {rel}");
        }
    }

    #[test]
    fn from_bytes_writes_return_sealed() {
        let backend = ArchiveBackend::from_bytes(build_archive(
            TempDir::new().unwrap().path(),
            "pkg",
            &[("a.md", b"# a")],
        )
        .as_os_str()
        .to_string_lossy()
        .as_bytes()
        .to_vec());
        // Even with bogus bytes, the write methods short-circuit on
        // Sealed before parsing the archive — covers the symmetry
        // contract that byte-backed archives are also read-only.
        assert!(matches!(
            backend.write_entity(Path::new("x.md"), b"x"),
            Err(BackendError::Sealed)
        ));
        assert!(matches!(
            backend.commit("msg", &ctx_for_test()),
            Err(BackendError::Sealed)
        ));
    }

    #[test]
    fn from_bytes_archive_path_is_none() {
        let backend = ArchiveBackend::from_bytes(Vec::new());
        assert!(backend.archive_path().is_none());
    }

    #[test]
    fn list_then_read_for_every_listed_path() {
        // The two read paths must agree on what's in the archive: every
        // path returned by `list_entities` must be readable via
        // `read_entity` and yield non-empty bytes.
        let tmp = TempDir::new().unwrap();
        let archive = build_archive(
            tmp.path(),
            "pkg",
            &[
                ("alpha.md", b"# a"),
                ("dir/beta.md", b"# b"),
                ("dir/sub/gamma.md", b"# g"),
            ],
        );
        let backend = ArchiveBackend::new(archive);
        for path in backend.list_entities().unwrap() {
            let bytes = backend
                .read_entity(&path)
                .unwrap()
                .unwrap_or_else(|| panic!("listed but unread: {path:?}"));
            assert!(!bytes.is_empty(), "empty entry: {path:?}");
        }
    }
}
