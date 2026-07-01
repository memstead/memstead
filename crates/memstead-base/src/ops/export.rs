//! Mem-archive export.
//!
//! The folder-shaped export (the gix-free path) lives here so the
//! unified `Engine::export_mem` can call it without crossing into
//! `memstead-git-branch`. The git-branch-shaped variant
//! (`export_mem_from_branch`) stays in `memstead-git-branch::ops::export`
//! because it walks a gitdir.
//!
//! Output wire shape is deterministic and matches the git-branch
//! variant byte-for-byte for equivalent input: `.memstead/config.json`
//! carries the whitelist-projection of the author's `MemConfig`,
//! `.memstead/schema/` embeds the pinned schema's source files, and the
//! mem's `.md` blobs land at their mem-relative paths. Entries are
//! sorted by path and zip-stamped with a fixed mtime so identical
//! input produces byte-identical archives.

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use memstead_schema::{
    ARCHIVE_CONFIG_PATH, ARCHIVE_PROVENANCE_PATH, ARCHIVE_SCHEMA_PREFIX, ArchiveProvenance,
    EntityProvenance, PublishConversionError, SchemaSourceError, MemConfig,
    collect_schema_source, published_config_from,
};
use zip::{CompressionMethod, DateTime, write::SimpleFileOptions};

use crate::entity::EntityId;
use crate::ops::MemExportResult;
use crate::provenance::Provenance;
use crate::validator::canonical::canonical_json;

/// Build the per-entity authoring-provenance payload from a backend's
/// mutation log (`read_provenance`). Keys by each entity's mem-relative
/// path ([`EntityId::path`]) so the payload survives a remount under a
/// different mem name. For each entity, keeps the most recent record
/// that carries a non-empty note — the entity's *current* rationale.
///
/// No-fabrication: records with no entity (batch) or no note are skipped,
/// so an entity authored without rationale is simply absent from the
/// payload (the read path reports it absent). Returns `None` when no
/// entity carried a note — the export then ships no provenance member,
/// distinct from an empty payload.
pub fn build_archive_provenance(records: &[Provenance]) -> Option<ArchiveProvenance> {
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    let mut by_path: BTreeMap<String, (SystemTime, EntityProvenance)> = BTreeMap::new();
    for r in records {
        let Some(entity) = r.entity.as_deref() else {
            continue;
        };
        let Some(note) = r.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) else {
            continue;
        };
        let path = EntityId(entity.to_string()).path().to_string();
        if path.is_empty() {
            continue;
        }
        let candidate = EntityProvenance {
            rationale: Some(note.to_string()),
            kind: Some(r.kind.as_str().to_string()),
            timestamp: Some(crate::filesystem::changelog::format_rfc3339_utc(r.timestamp)),
            actor: Some(r.actor.as_trailer().to_string()),
        };
        match by_path.get(&path) {
            // Keep the existing entry when it is at least as recent.
            Some((ts, _)) if *ts >= r.timestamp => {}
            _ => {
                by_path.insert(path, (r.timestamp, candidate));
            }
        }
    }
    if by_path.is_empty() {
        return None;
    }
    Some(ArchiveProvenance::summarised(
        by_path.into_iter().map(|(k, (_, v))| (k, v)).collect(),
    ))
}

/// Byte-shaped output of [`export_mem_to_bytes`]. Bundles the
/// produced archive bytes with the same metadata
/// [`MemExportResult`] reports for path-based exports.
#[derive(Debug, Clone)]
pub struct MemExportBytes {
    /// The `.mem` archive bytes — self-contained, ready to validate
    /// via `extract_entries` and hydrate via `Engine::from_archive_bytes`.
    pub bytes: Vec<u8>,
    /// Mem name (mirrors `MemExportResult.name`).
    pub name: String,
    /// Mem version (mirrors `MemExportResult.version`).
    pub version: String,
    /// `.md` entity count in the produced archive.
    pub entity_count: usize,
    /// Cross-mem edges whose target won't travel inside this archive —
    /// `install` will reject each. Mirrors
    /// `MemExportResult.dangling_cross_mem_edges`; empty for a
    /// self-contained export.
    pub dangling_cross_mem_edges: Vec<crate::validator::DanglingCrossMemEdge>,
}

#[derive(Debug, thiserror::Error)]
pub enum MemExportError {
    #[error("mem directory not found: {0}")]
    DirNotFound(String),
    #[error(transparent)]
    Convert(#[from] PublishConversionError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("config serialization error: {0}")]
    Canonical(String),
    #[error(transparent)]
    SchemaSource(#[from] SchemaSourceError),
    #[error("branch read error: {0}")]
    BranchRead(String),
    /// The
    /// produced archive failed strict validation. Reaches this state
    /// when the mem carries on-disk drift from a pre-fix engine —
    /// entities created when `MISSING_REQUIRED_SECTION` was a
    /// warning, hand-edited markdown, or archive-imports from
    /// non-canonical sources. Export and install share one strict
    /// validator pass; the trust boundary now fires at export rather
    /// than letting an invalid archive land on disk and refuse only
    /// at the next install attempt.
    #[error("export archive failed strict validation: {0}")]
    ArchiveValidationFailed(String),
}

/// Export a mem directory as a portable `.mem` archive.
///
/// The archive contains `.memstead/config.json` (a **whitelist projection**
/// of the author's `MemConfig` — author-only fields like
/// `writeGuidance`, `mediums`, `projections`, `readMems` never enter
/// the archive), every `.md` file under the mem root, and the pinned
/// schema's source YAML under `schema/`.
///
/// Output is deterministic — entries are sorted by path and written
/// with a fixed modification time — so identical input produces
/// byte-identical archives, and archives produced here round-trip
/// through `validate_and_normalize_archive` without rewriting.
pub fn export_mem(
    mem_dir: &Path,
    config: &MemConfig,
    output_path: &Path,
    workspace_root: Option<&Path>,
    workspace_schemas_dir: Option<&Path>,
) -> Result<MemExportResult, MemExportError> {
    let basename = mem_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let explicit_name = config.name.as_deref().unwrap_or(basename);
    let out = export_mem_to_bytes(
        mem_dir,
        config,
        workspace_root,
        workspace_schemas_dir,
        explicit_name,
    )?;

    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, &out.bytes)?;

    let size_bytes = fs::metadata(output_path)?.len();

    Ok(MemExportResult {
        archive_path: output_path.display().to_string(),
        name: out.name,
        version: out.version,
        entity_count: out.entity_count,
        size_bytes,
        dangling_cross_mem_edges: out.dangling_cross_mem_edges,
    })
}

/// Produce a portable `.mem` archive **as bytes** for a folder-backed
/// mem. Same wire format as [`export_mem`] — same whitelist
/// projection, same embedded schema source, same deterministic sort
/// order and fixed mtime — but the output stays in memory so the
/// bridge / WASM consumers can return it directly over HTTP.
///
/// `explicit_name` is the mem name the publish whitelist receives.
/// Callers reaching this through [`crate::Engine::export_mem_to_bytes`]
/// pass the mount's mem name; callers reaching it directly choose
/// the disk basename or a config-supplied alias.
pub fn export_mem_to_bytes(
    mem_dir: &Path,
    config: &MemConfig,
    workspace_root: Option<&Path>,
    workspace_schemas_dir: Option<&Path>,
    explicit_name: &str,
) -> Result<MemExportBytes, MemExportError> {
    if !mem_dir.is_dir() {
        return Err(MemExportError::DirNotFound(
            mem_dir.display().to_string(),
        ));
    }

    let mut md_files = Vec::new();
    collect_markdown(mem_dir, &mut md_files)?;

    let mut md_entries: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(md_files.len());
    for abs in &md_files {
        let rel = abs
            .strip_prefix(mem_dir)
            .expect("markdown file must live under mem_dir");
        md_entries.push((rel.to_path_buf(), fs::read(abs)?));
    }

    // Source the per-entity authoring provenance from the folder mem's
    // own mutation log (`.memstead/changes.jsonl`), read through the folder
    // backend so the JSONL parsing has one home. A mem with no changelog
    // yields no records → no provenance member (absent, not empty).
    use crate::backend::MemBackend;
    let provenance = crate::storage::FilesystemMemWriter::new(mem_dir.to_path_buf())
        .read_provenance(None)
        .ok()
        .and_then(|records| build_archive_provenance(&records));

    export_entries_to_bytes(
        config,
        workspace_root,
        workspace_schemas_dir,
        explicit_name,
        md_entries,
        provenance.as_ref(),
    )
}

/// Seal already-collected entity bytes into a portable `.mem` archive —
/// the storage-agnostic core shared by the folder exporter
/// ([`export_mem_to_bytes`], which walks a directory) and the
/// in-memory exporter (which lists entities from a
/// [`crate::backend::MemBackend`] holding them in RAM). Same wire
/// format either way: whitelist config projection, embedded schema
/// source, deterministic path-sorted entries, fixed mtime, and the same
/// pre-write lenient validation pass.
///
/// `md_entries` are `(mem-relative path, bytes)` pairs; paths are
/// posix-normalised for the archive. Entries need not be pre-sorted — the
/// archive sort makes the output deterministic regardless of input order.
pub fn export_entries_to_bytes(
    config: &MemConfig,
    workspace_root: Option<&Path>,
    workspace_schemas_dir: Option<&Path>,
    explicit_name: &str,
    md_entries: Vec<(PathBuf, Vec<u8>)>,
    provenance: Option<&ArchiveProvenance>,
) -> Result<MemExportBytes, MemExportError> {
    let published = published_config_from(config, explicit_name)?;
    let config_bytes = canonical_json(&published)
        .map_err(|e| MemExportError::Canonical(e.to_string()))?
        .into_bytes();

    let schema_files =
        collect_schema_source(workspace_root, workspace_schemas_dir, &published.schema)?;

    let entity_count = md_entries.len();
    let mut all_entries: Vec<(String, Vec<u8>)> =
        Vec::with_capacity(2 + schema_files.len() + md_entries.len());
    all_entries.push((ARCHIVE_CONFIG_PATH.to_string(), config_bytes));
    // Embed the authoring-provenance payload when present. Serialised
    // canonically; the validator tolerates it as a recognised meta member
    // and the consumer reads it back via `read_archive_provenance`.
    if let Some(prov) = provenance
        && let Ok(bytes) = prov.to_archive_bytes()
    {
        all_entries.push((ARCHIVE_PROVENANCE_PATH.to_string(), bytes));
    }
    for sf in &schema_files {
        all_entries.push((
            format!("{ARCHIVE_SCHEMA_PREFIX}{}", sf.archive_path),
            sf.bytes.clone(),
        ));
    }
    for (rel, bytes) in md_entries {
        all_entries.push((posix_path(&rel), bytes));
    }
    all_entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut buf: Vec<u8> = Vec::new();
    {
        let cursor = Cursor::new(&mut buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(fixed_mtime())
            .unix_permissions(0o644);

        for (archive_path, bytes) in &all_entries {
            zip.start_file(archive_path, options)?;
            zip.write_all(bytes)?;
        }
        zip.finish()?;
    }

    // Strict
    // archive validation runs against the in-memory bytes before they
    // leave this function. Export and install share one validator
    // pass — the trust boundary fires at export rather than letting an
    // invalid archive land on disk and refuse only at the next install
    // attempt. If the produced archive doesn't validate (legacy
    // on-disk drift, hand-edited markdown, archive-imports from
    // non-canonical sources), surface the typed refusal; the
    // disk-shaped wrapper (`export_mem`) never writes a broken
    // archive because validation happens here, pre-write.
    //
    // The *lenient* variant collects cross-mem edges (whose target
    // won't travel inside this single-mem archive) instead of
    // refusing on them — export warns and still produces, where install
    // refuses. Every other strict check still refuses, so a
    // genuinely-broken archive never lands.
    let validated = crate::validator::validate_and_normalize_archive_lenient(&buf)
        .map_err(|e| MemExportError::ArchiveValidationFailed(e.to_string()))?;

    Ok(MemExportBytes {
        bytes: buf,
        name: published.name.clone(),
        version: published.version.to_string(),
        entity_count,
        dangling_cross_mem_edges: validated.dangling_cross_mem_edges,
    })
}

/// Zip's minimum representable timestamp — 1980-01-01 00:00:00. Used
/// as a fixed mtime so archives are byte-stable across exports.
fn fixed_mtime() -> DateTime {
    DateTime::default()
}

fn posix_path(path: &Path) -> String {
    path.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// Recursively collect `.md` files. Skips hidden directories (same
/// policy as the entity loader).
fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    let mut children: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    children.sort_by_key(|e| e.file_name());

    for entry in children {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if path.is_dir() {
            if name.starts_with('.') {
                continue;
            }
            collect_markdown(&path, out)?;
        } else if name.ends_with(".md") {
            out.push(path);
        }
    }
    Ok(())
}
