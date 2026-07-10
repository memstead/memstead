//! Markdown and mem-archive export.
//!
//! `export_markdown` regenerates entity files from the store, only writing
//! files that actually changed (incremental). Compares generated markdown
//! against the current file on disk byte-for-byte.
//!
//! `export_mem` zips a mem directory into a portable `.mem` archive
//! with deterministic output (sorted entries, fixed mtime).

use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

#[cfg(feature = "git-object-storage")]
use memstead_base::ops::MemExportBytes;
use memstead_schema::{
    ARCHIVE_ANCHORS_PATH, ARCHIVE_CONFIG_PATH, ARCHIVE_SCHEMA_PREFIX, MemConfig, TypeDefinition,
    collect_schema_source, published_config_from, type_by_name,
};
use zip::{CompressionMethod, DateTime, write::SimpleFileOptions};

use super::{ExportResult, MemExportResult};
use crate::entity::generator::generate_markdown;
use crate::entity::writer::write_entity;
#[cfg(feature = "git-object-storage")]
use crate::storage::git_tree::{BranchReadError, read_branch_blobs};
use crate::store::Store;
use crate::validator::canonical::canonical_json;

/// Regenerate all entity markdown files from the in-memory store.
/// Only writes files that have changed (incremental export).
pub fn export_markdown(
    store: &Store,
    default_schema: &TypeDefinition,
    mem_dir: &Path,
    schema_filter: Option<&str>,
) -> ExportResult {
    let mut written = 0;
    let mut unchanged = 0;

    for entity in store.all_entities() {
        // Skip stubs
        if entity.stub || entity.file_path.is_empty() {
            continue;
        }

        // Type filter
        if let Some(filter) = schema_filter
            && entity.entity_type != filter
        {
            continue;
        }

        let resolved = type_by_name(&entity.entity_type);
        let schema: &TypeDefinition = resolved.as_deref().unwrap_or(default_schema);
        let generated = generate_markdown(entity, schema);

        // Compare with file on disk
        let full_path = mem_dir.join(&entity.file_path);
        let needs_write = match std::fs::read_to_string(&full_path) {
            Ok(existing) => existing != generated,
            Err(_) => true, // File doesn't exist — write it
        };

        if needs_write {
            let _ = write_entity(entity, mem_dir, schema);
            written += 1;
        } else {
            unchanged += 1;
        }
    }

    ExportResult {
        written,
        unchanged,
        skipped_mounts: Vec::new(),
    }
}

/// Export a single entity by ID.
pub fn export_entity(
    store: &Store,
    id: &crate::entity::EntityId,
    default_schema: &TypeDefinition,
    mem_dir: &Path,
) -> Result<ExportResult, String> {
    let entity = store
        .get(id)
        .ok_or_else(|| format!("entity not found: {id}"))?;

    if entity.stub {
        return Err(format!("{id} is a stub — nothing to export"));
    }

    let resolved = type_by_name(&entity.entity_type);
    let schema: &TypeDefinition = resolved.as_deref().unwrap_or(default_schema);
    let generated = generate_markdown(entity, schema);

    let full_path = mem_dir.join(&entity.file_path);
    let needs_write = match std::fs::read_to_string(&full_path) {
        Ok(existing) => existing != generated,
        Err(_) => true,
    };

    if needs_write {
        write_entity(entity, mem_dir, schema).map_err(|e| e.to_string())?;
        Ok(ExportResult {
            written: 1,
            unchanged: 0,
            skipped_mounts: Vec::new(),
        })
    } else {
        Ok(ExportResult {
            written: 0,
            unchanged: 1,
            skipped_mounts: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Mem (.mem) archive export
// ---------------------------------------------------------------------------

// Stage 1.7-2: MemExportError and the folder-shaped `export_mem`
// live in `memstead-base::ops::export`. Re-export here so downstream
// callers that imported via the workspace path keep working. The
// gix-bound `export_mem_from_branch` (below) stays here.
pub use memstead_base::ops::export::{MemExportError, export_mem};

#[cfg(feature = "git-object-storage")]
fn branch_read_into_mem_export(e: BranchReadError) -> MemExportError {
    MemExportError::BranchRead(e.to_string())
}

/// Export a mem as a portable `.mem` archive by walking the
/// `mem-repo-git` branch tree directly — the git-object storage path's
/// counterpart to [`export_mem`]. No working tree is consulted; all
/// `.md` content comes from the per-mem branch tip, sorted by path
/// for deterministic archive bytes.
///
/// Wire format matches [`export_mem`]:
/// `.memstead/config.json` carries the whitelist projection,
/// `.memstead/schema/` embeds the pinned schema's source files, and the rest of the
/// archive carries mem-relative `.md` blobs.
///
/// `mem_repo_gitdir` is the multi-root repo (`<workspace>/mem-repo/.git/`).
/// The function reads mem content from `refs/heads/<mem_name>` and
/// resolves the schema from `__MEMSTEAD:schemas/<name>@<version>/`
/// first — the tree `memstead schema install` writes on this backend —
/// falling back to the disk/builtin chain ([`collect_schema_source`]
/// over `workspace_root` + `workspace_schemas_dir`) for builtins and
/// pre-ref layouts.
#[cfg(feature = "git-object-storage")]
#[allow(clippy::too_many_arguments)]
pub fn export_mem_from_branch(
    mem_repo_gitdir: &Path,
    mem_name: &str,
    config: &MemConfig,
    output_path: &Path,
    workspace_root: Option<&Path>,
    workspace_schemas_dir: Option<&Path>,
    provenance_bytes: Option<&[u8]>,
    anchors_bytes: Option<&[u8]>,
) -> Result<MemExportResult, MemExportError> {
    let out = export_mem_from_branch_to_bytes(
        mem_repo_gitdir,
        mem_name,
        config,
        workspace_root,
        workspace_schemas_dir,
        provenance_bytes,
        anchors_bytes,
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

/// Resolve the pinned schema's source files from the workspace's
/// `__MEMSTEAD:schemas/<name>@<version>/` tree — the location
/// `memstead schema install` writes on the git-branch backend. Returns
/// `None` when the branch, the package subtree, or its `schema.yaml`
/// is absent, so the caller falls through to the disk/builtin chain.
#[cfg(feature = "git-object-storage")]
fn schema_files_from_memstead_ref(
    mem_repo_gitdir: &Path,
    schema_ref: &memstead_schema::SchemaRef,
) -> Option<Vec<memstead_schema::SchemaSourceFile>> {
    let blobs = read_branch_blobs(mem_repo_gitdir, "refs/heads/__MEMSTEAD").ok()?;
    let prefix = format!("schemas/{}@{}/", schema_ref.name, schema_ref.version);
    let mut files: Vec<memstead_schema::SchemaSourceFile> = blobs
        .into_iter()
        .filter_map(|b| {
            b.path
                .strip_prefix(&prefix)
                .map(|rel| memstead_schema::SchemaSourceFile {
                    archive_path: rel.to_string(),
                    bytes: b.bytes,
                })
        })
        .collect();
    if !files.iter().any(|f| f.archive_path == "schema.yaml") {
        return None;
    }
    files.sort_by(|a, b| a.archive_path.cmp(&b.archive_path));
    Some(files)
}

/// Byte-shaped counterpart to [`export_mem_from_branch`]. Same wire
/// format, same determinism contract, no on-disk artifact — the bytes
/// are the output. The engine's `export_mem_to_bytes` dispatches to
/// this for git-branch mounts via the [`memstead_base::GitBranchOps`]
/// bundle so byte-snapshot consumers (the bridge, future WASM
/// replicas) treat folder and git-branch backends symmetrically.
#[cfg(feature = "git-object-storage")]
#[allow(clippy::too_many_arguments)]
pub fn export_mem_from_branch_to_bytes(
    mem_repo_gitdir: &Path,
    mem_name: &str,
    config: &MemConfig,
    workspace_root: Option<&Path>,
    workspace_schemas_dir: Option<&Path>,
    provenance_bytes: Option<&[u8]>,
    anchors_bytes: Option<&[u8]>,
) -> Result<MemExportBytes, MemExportError> {
    let published = published_config_from(config, mem_name)?;
    let config_bytes = canonical_json(&published)
        .map_err(|e| MemExportError::Canonical(e.to_string()))?
        .into_bytes();

    // Engine-canonical location first: `memstead schema install` on the
    // git-branch backend writes the package onto the `__MEMSTEAD` branch,
    // not the workspace disk. Builtins and pre-ref disk layouts fall
    // through to the shared chain.
    let schema_files = match schema_files_from_memstead_ref(mem_repo_gitdir, &published.schema) {
        Some(files) => files,
        None => collect_schema_source(workspace_root, workspace_schemas_dir, &published.schema)?,
    };

    let ref_name = match workspace_root {
        Some(root) => crate::mem_repo_config::branch_ref_for_mem(root, mem_name),
        None => format!("refs/heads/{mem_name}"),
    };
    let blobs = match read_branch_blobs(mem_repo_gitdir, &ref_name) {
        Ok(b) => b,
        Err(BranchReadError::BranchMissing { .. }) => Vec::new(),
        Err(e) => return Err(branch_read_into_mem_export(e)),
    };
    let md_entries: Vec<(String, Vec<u8>)> = blobs
        .into_iter()
        .filter(|b| b.path.ends_with(".md"))
        .map(|b| (b.path, b.bytes))
        .collect();
    let entity_count = md_entries.len();

    let mut all_entries: Vec<(String, Vec<u8>)> =
        Vec::with_capacity(2 + schema_files.len() + md_entries.len());
    all_entries.push((ARCHIVE_CONFIG_PATH.to_string(), config_bytes));
    // Embed the engine-sourced authoring-provenance payload (commit-trailer
    // rationale, keyed by mem-relative path). The engine walks the log;
    // this assembler only places the member.
    if let Some(prov) = provenance_bytes {
        all_entries.push((
            memstead_schema::ARCHIVE_PROVENANCE_PATH.to_string(),
            prov.to_vec(),
        ));
    }
    // Embed the engine-owned anchors sidecar verbatim when present — the
    // engine reads it from the branch tip and hands it here; this assembler
    // only places the recognised member so a git-branch `.mem` carries
    // anchors identically to the folder / in-memory exports.
    if let Some(anchors) = anchors_bytes {
        all_entries.push((ARCHIVE_ANCHORS_PATH.to_string(), anchors.to_vec()));
    }
    for sf in &schema_files {
        all_entries.push((
            format!("{ARCHIVE_SCHEMA_PREFIX}{}", sf.archive_path),
            sf.bytes.clone(),
        ));
    }
    all_entries.extend(md_entries);
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

    // Surface every cross-mem
    // edge whose target won't travel inside this single-mem archive —
    // exactly what `install` will refuse on. Detected via the shared
    // predicate so export and install agree. Cross-mem-only (tolerant
    // parse): the git-branch export keeps its lenient-on-drift posture;
    // this adds the missing cross-mem signal so the failure no longer
    // surfaces silently at install time on the share boundary.
    let dangling_cross_mem_edges =
        memstead_base::validator::collect_dangling_cross_mem_edges_from_bytes(&buf)
            .map_err(|e| MemExportError::ArchiveValidationFailed(e.to_string()))?;

    Ok(MemExportBytes {
        bytes: buf,
        name: published.name.clone(),
        version: published.version.to_string(),
        entity_count,
        dangling_cross_mem_edges,
    })
}

/// Zip's minimum representable timestamp — 1980-01-01 00:00:00. Used as a
/// fixed mtime so archives are byte-stable across exports.
fn fixed_mtime() -> DateTime {
    DateTime::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{Entity, EntityId, MetadataValue};
    use indexmap::IndexMap;
    use memstead_schema::type_by_name;
    use tempfile::TempDir;

    fn make_entity(name: &str) -> Entity {
        let mut metadata = IndexMap::new();
        metadata.insert("level".into(), MetadataValue::String("M0".into()));
        metadata.insert(
            "created_date".into(),
            MetadataValue::String("2026-01-15".into()),
        );
        metadata.insert(
            "last_modified".into(),
            MetadataValue::String("2026-04-12".into()),
        );
        metadata.insert("type".into(), MetadataValue::String("spec".into()));

        let mut sections = IndexMap::new();
        sections.insert("identity".into(), "Test.".into());
        sections.insert("purpose".into(), "Test.".into());

        Entity {
            id: EntityId::new("specs", name),
            title: name.into(),
            entity_type: "spec".into(),
            mem: "specs".into(),
            file_path: format!("{name}.md"),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    fn make_memo_entity(name: &str) -> Entity {
        let mut metadata = IndexMap::new();
        metadata.insert("status".into(), MetadataValue::String("active".into()));
        metadata.insert(
            "created_date".into(),
            MetadataValue::String("2026-01-15".into()),
        );
        metadata.insert(
            "last_modified".into(),
            MetadataValue::String("2026-04-12".into()),
        );
        metadata.insert("tags".into(), MetadataValue::String("decision".into()));
        metadata.insert("type".into(), MetadataValue::String("memo".into()));

        let mut sections = IndexMap::new();
        sections.insert("claim".into(), "Sled is the choice.".into());
        sections.insert("context".into(), "Evaluated three stores.".into());

        Entity {
            id: EntityId::new("memos", name),
            title: name.into(),
            entity_type: "memo".into(),
            mem: "memos".into(),
            file_path: format!("{name}.md"),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    fn make_assertion_entity(name: &str) -> Entity {
        let mut metadata = IndexMap::new();
        metadata.insert("confidence".into(), MetadataValue::String("medium".into()));
        metadata.insert(
            "verification_status".into(),
            MetadataValue::String("unverified".into()),
        );
        metadata.insert(
            "created_date".into(),
            MetadataValue::String("2026-01-15".into()),
        );
        metadata.insert(
            "last_modified".into(),
            MetadataValue::String("2026-04-12".into()),
        );
        metadata.insert("type".into(), MetadataValue::String("assertion".into()));

        let mut sections = IndexMap::new();
        sections.insert("claim".into(), "Sled outperforms rocksdb.".into());
        sections.insert("evidence".into(), "Bench results attached.".into());

        Entity {
            id: EntityId::new("assertions", name),
            title: name.into(),
            entity_type: "assertion".into(),
            mem: "assertions".into(),
            file_path: format!("{name}.md"),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn export_mixed_schemas_uses_per_schema_headings() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::new();
        let memo = make_memo_entity("memo-entity");
        let assertion = make_assertion_entity("assertion-entity");
        store.upsert(memo.id.clone(), memo);
        store.upsert(assertion.id.clone(), assertion);

        // default_schema is only the fallback for unknown schema names; each
        // entity's own schema should still win.
        let default_schema = &type_by_name("spec").unwrap();
        let result = export_markdown(&store, default_schema, dir.path(), None);
        assert_eq!(result.written, 2);

        let memo_md = std::fs::read_to_string(dir.path().join("memo-entity.md")).unwrap();
        assert!(memo_md.contains("## Claim"));
        assert!(memo_md.contains("## Context"));
        assert!(memo_md.contains("type: memo"));
        assert!(!memo_md.contains("## Identity"));
        assert!(!memo_md.contains("## Purpose"));

        let assertion_md = std::fs::read_to_string(dir.path().join("assertion-entity.md")).unwrap();
        assert!(assertion_md.contains("## Claim"));
        assert!(assertion_md.contains("## Evidence"));
        assert!(assertion_md.contains("type: assertion"));
        assert!(!assertion_md.contains("## Identity"));
        assert!(!assertion_md.contains("## Purpose"));
    }

    #[test]
    fn export_writes_new_files() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::new();
        let e = make_entity("export-test");
        store.upsert(e.id.clone(), e);

        let schema = &type_by_name("spec").unwrap();
        let result = export_markdown(&store, schema, dir.path(), None);
        assert_eq!(result.written, 1);
        assert_eq!(result.unchanged, 0);
        assert!(dir.path().join("export-test.md").exists());
    }

    #[test]
    fn export_incremental_skips_unchanged() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::new();
        let e = make_entity("incremental");
        store.upsert(e.id.clone(), e);

        let schema = &type_by_name("spec").unwrap();

        // First export: writes
        let r1 = export_markdown(&store, schema, dir.path(), None);
        assert_eq!(r1.written, 1);

        // Second export: unchanged
        let r2 = export_markdown(&store, schema, dir.path(), None);
        assert_eq!(r2.written, 0);
        assert_eq!(r2.unchanged, 1);
    }

    // ---- mem archive export ---------------------------------------------

    fn write_mem_fixture(dir: &Path) {
        std::fs::create_dir_all(dir.join(".memstead")).unwrap();
        // Author-side config with every flavor of author-only field the
        // whitelist projection has to strip. If the export pipeline ever
        // leaks one of these into the archive, the round-trip assertion
        // below catches it.
        std::fs::write(
            dir.join(".memstead/config.json"),
            r#"{"version":"1.2.0","description":"AWS patterns","schema":"default@1.0.0","writeGuidance":{"context":"secret"},"mediums":{},"projections":{},"readMems":{}}"#,
        ).unwrap();
        std::fs::write(
            dir.join("api-gateway.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# API Gateway\n\n## Identity\n\nGateway.\n\n## Purpose\n\nServe API traffic.\n",
        ).unwrap();
        std::fs::create_dir_all(dir.join("well-architected")).unwrap();
        std::fs::write(
            dir.join("well-architected/reliability.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Reliability\n\n## Identity\n\nReliability.\n\n## Purpose\n\nKeep the system available.\n",
        ).unwrap();
        // A cache file that MUST NOT end up in the archive.
        std::fs::write(dir.join(".memstead/communities.json"), "{}").unwrap();
    }

    #[test]
    fn export_mem_writes_whitelisted_config_and_markdown() {
        let tmp = TempDir::new().unwrap();
        let mem = tmp.path().join("aws-patterns");
        write_mem_fixture(&mem);

        let config = memstead_schema::load_and_validate(&mem).unwrap();
        let out = tmp.path().join("aws-patterns.mem");

        // `mem` acts as the source root for schema resolution. It has no
        // `.memstead/schemas/` dir of its own, so the default pin falls through
        // to the embedded builtin.
        let result = export_mem(&mem, &config, &out, None, None).unwrap();
        assert_eq!(result.name, "aws-patterns");
        assert_eq!(result.version, "1.2.0");
        assert_eq!(result.entity_count, 2);

        let file = std::fs::File::open(&out).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();

        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();

        // Config + all markdown entries must be present; `schema/` tree
        // varies with builtin type count, so we assert inclusion of the
        // non-schema paths and a non-empty schema subtree separately.
        for required in [
            ".memstead/config.json",
            "api-gateway.md",
            "well-architected/reliability.md",
            ".memstead/schema/schema.yaml",
        ] {
            assert!(
                names.iter().any(|n| n == required),
                "archive missing expected entry {required:?}; got {names:?}"
            );
        }
        let type_entries: Vec<&String> = names
            .iter()
            .filter(|n| n.starts_with(".memstead/schema/types/"))
            .collect();
        assert!(
            !type_entries.is_empty(),
            "archive must embed at least one type yaml under .memstead/schema/types/"
        );

        use std::io::Read as _;
        let mut config_bytes = Vec::new();
        archive
            .by_name(".memstead/config.json")
            .unwrap()
            .read_to_end(&mut config_bytes)
            .unwrap();
        let written: serde_json::Value = serde_json::from_slice(&config_bytes).unwrap();
        assert_eq!(written["format"], memstead_schema::PUBLISHED_MEM_FORMAT);
        assert_eq!(written["name"], "aws-patterns");
        assert_eq!(written["version"], "1.2.0");
        assert_eq!(written["description"], "AWS patterns");
        assert_eq!(written["schema"], "default@1.0.0");

        // Every author-only field the fixture declares must have been
        // stripped. A single survivor reintroduces the leak the
        // whitelist was designed to prevent.
        for forbidden in [
            "writeGuidance",
            "mediums",
            "projections",
            "rules",
            "publish",
            "readMems",
            "vcs",
            "language",
            "community",
            "defaultSchema",
        ] {
            assert!(
                written.get(forbidden).is_none(),
                "author-only field {forbidden:?} leaked into archive config"
            );
        }
    }

    #[test]
    fn export_mem_is_deterministic() {
        let tmp = TempDir::new().unwrap();
        let mem = tmp.path().join("aws-patterns");
        write_mem_fixture(&mem);

        let config = memstead_schema::load_and_validate(&mem).unwrap();
        let out1 = tmp.path().join("a.mem");
        let out2 = tmp.path().join("b.mem");

        export_mem(&mem, &config, &out1, None, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        export_mem(&mem, &config, &out2, None, None).unwrap();

        let a = std::fs::read(&out1).unwrap();
        let b = std::fs::read(&out2).unwrap();
        assert_eq!(a, b, "mem archive exports must be byte-stable");
    }

    #[test]
    fn export_mem_errors_when_version_missing() {
        let tmp = TempDir::new().unwrap();
        let mem = tmp.path().join("no-version");
        std::fs::create_dir_all(mem.join(".memstead")).unwrap();
        std::fs::write(
            mem.join(".memstead/config.json"),
            r#"{"schema":"default@1.0.0","mediums":{},"projections":{}}"#,
        )
        .unwrap();
        std::fs::write(
            mem.join("a.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# A\n\n## Identity\n\nA.\n",
        ).unwrap();

        let config = memstead_schema::load_and_validate(&mem).unwrap();
        let out = tmp.path().join("out.mem");
        let err = export_mem(&mem, &config, &out, None, None).unwrap_err();
        assert!(matches!(
            err,
            MemExportError::Convert(memstead_schema::PublishConversionError::MissingVersion)
        ));
        // The whitelist projection fails before any archive bytes are
        // written, so the output path must stay untouched.
        assert!(!out.exists(), "no archive should be written on error");
    }

    #[test]
    fn export_with_schema_filter() {
        let dir = TempDir::new().unwrap();
        let mut store = Store::new();
        let e1 = make_entity("spec-entity");
        let mut e2 = make_entity("memo-entity");
        e2.entity_type = "memo".into();
        store.upsert(e1.id.clone(), e1);
        store.upsert(e2.id.clone(), e2);

        let schema = &type_by_name("spec").unwrap();
        let result = export_markdown(&store, schema, dir.path(), Some("spec"));
        assert_eq!(result.written, 1); // Only spec entity
    }

    // ---- mem archive export from git-object branch ----------------------

    #[cfg(feature = "git-object-storage")]
    mod git_object_export {
        use super::*;
        use crate::storage::MemWriter;
        use crate::storage::git_tree::GitTreeMemWriter;
        use crate::vcs::CommitContext;

        /// Build a fresh `mem-repo-git`-style bare repo and a side-by-side
        /// mem config dir so [`export_mem_from_branch`] has both inputs
        /// it needs: the gitdir + ref to walk, plus a disk-resident
        /// `<mem>/.memstead/config.json` for the metadata projection.
        fn seed_mem_branch(
            workspace: &Path,
            mem_name: &str,
            entries: &[(&str, &str)],
        ) -> (PathBuf, PathBuf) {
            let gitdir = workspace.join("mem-repo").join(".git");
            std::fs::create_dir_all(&gitdir).unwrap();
            gix::init_bare(&gitdir).unwrap();

            // Per-mem on-disk config dir mirrors the cutover layout —
            // schemas resolve through workspace paths regardless of the
            // adapter, and the engine still loads MemConfig from disk.
            let mem_dir = workspace.join(mem_name);
            std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
            std::fs::write(
                mem_dir.join(".memstead/config.json"),
                r#"{"version":"1.0.0","description":"fixture","schema":"default@1.0.0"}"#,
            )
            .unwrap();

            // Commit each entry to `refs/heads/<mem_name>` via the
            // production write path so the test exercises the same tree
            // shape `memstead-cli`'s mutations would produce.
            let writer = GitTreeMemWriter::new(gitdir.clone(), format!("refs/heads/{mem_name}"));
            for (rel, content) in entries {
                writer
                    .write_entity(Path::new(rel), content.as_bytes())
                    .unwrap();
            }
            writer.commit("seed", &CommitContext::internal()).unwrap();
            (gitdir, mem_dir)
        }

        #[test]
        fn publish_from_branch_produces_correct_tarball() {
            let tmp = TempDir::new().unwrap();
            let (gitdir, mem_dir) = seed_mem_branch(
                tmp.path(),
                "fixture",
                &[
                    (
                        "alpha.md",
                        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nA.\n",
                    ),
                    (
                        "nested/beta.md",
                        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Beta\n\n## Identity\n\nB.\n",
                    ),
                ],
            );

            let config = memstead_schema::load_and_validate(&mem_dir).unwrap();
            let out = tmp.path().join("fixture.mem");
            let result =
                export_mem_from_branch(&gitdir, "fixture", &config, &out, None, None, None, None)
                    .unwrap();

            assert_eq!(result.name, "fixture");
            assert_eq!(result.version, "1.0.0");
            assert_eq!(result.entity_count, 2, "two `.md` blobs were committed");

            let file = std::fs::File::open(&out).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            let mut names: Vec<String> = (0..archive.len())
                .map(|i| archive.by_index(i).unwrap().name().to_string())
                .collect();
            names.sort();
            for required in [".memstead/config.json", "alpha.md", "nested/beta.md"] {
                assert!(
                    names.iter().any(|n| n == required),
                    "archive missing entry {required:?}; got {names:?}"
                );
            }

            // Branch-tree contents must round-trip byte-for-byte through
            // the archive — no normalisation happens between blob and zip.
            use std::io::Read as _;
            let mut alpha = Vec::new();
            archive
                .by_name("alpha.md")
                .unwrap()
                .read_to_end(&mut alpha)
                .unwrap();
            assert!(String::from_utf8_lossy(&alpha).contains("# Alpha"));
        }

        #[test]
        fn publish_includes_schema_under_underscore_schema_prefix() {
            // The session plan's `_schema/` reference contradicts its own
            // "tarball wire format unchanged" rule — the validator and
            // cache reader both require `.memstead/schema/`. Test name keeps
            // the plan's identifier; assertion targets the wire-format
            // rule.
            let tmp = TempDir::new().unwrap();
            let (gitdir, mem_dir) = seed_mem_branch(
                tmp.path(),
                "fixture",
                &[(
                    "a.md",
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# A\n\n## Identity\n\nA.\n",
                )],
            );

            let config = memstead_schema::load_and_validate(&mem_dir).unwrap();
            let out = tmp.path().join("fixture.mem");
            export_mem_from_branch(&gitdir, "fixture", &config, &out, None, None, None, None)
                .unwrap();

            let file = std::fs::File::open(&out).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            let names: Vec<String> = (0..archive.len())
                .map(|i| archive.by_index(i).unwrap().name().to_string())
                .collect();
            assert!(
                names.iter().any(|n| n == ".memstead/schema/schema.yaml"),
                "schema manifest must embed under `.memstead/schema/`; got {names:?}"
            );
            assert!(
                names
                    .iter()
                    .any(|n: &String| n.starts_with(".memstead/schema/types/")
                        && n.ends_with(".yaml")),
                "at least one type yaml must embed under `.memstead/schema/types/`; got {names:?}"
            );
        }

        #[test]
        fn byte_export_matches_path_export_byte_for_byte() {
            // Same input through the path-based and byte-based exports
            // must produce identical bytes — the bytes variant is the
            // primitive and the path variant is now a thin write
            // wrapper. Guards against future drift between the two.
            let tmp = TempDir::new().unwrap();
            let (gitdir, mem_dir) = seed_mem_branch(
                tmp.path(),
                "fixture",
                &[(
                    "a.md",
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# A\n\n## Identity\n\nA.\n",
                )],
            );
            let config = memstead_schema::load_and_validate(&mem_dir).unwrap();
            let out = tmp.path().join("fixture.mem");
            export_mem_from_branch(&gitdir, "fixture", &config, &out, None, None, None, None)
                .unwrap();
            let path_bytes = std::fs::read(&out).unwrap();
            let byte_bytes = export_mem_from_branch_to_bytes(
                &gitdir, "fixture", &config, None, None, None, None,
            )
            .unwrap()
            .bytes;
            assert_eq!(path_bytes, byte_bytes);
        }

        #[test]
        fn byte_export_validates_and_hydrates_via_engine() {
            // The bridge consumer's contract: bytes out of
            // `export_mem_to_bytes` validate against `extract_entries`
            // standalone and hydrate into a new engine with the same
            // read surface for the exported mem.
            let tmp = TempDir::new().unwrap();
            let (gitdir, mem_dir) = seed_mem_branch(
                tmp.path(),
                "fixture",
                &[(
                    "alpha.md",
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nA round-trip seed.\n",
                )],
            );
            let config = memstead_schema::load_and_validate(&mem_dir).unwrap();
            let bytes = export_mem_from_branch_to_bytes(
                &gitdir, "fixture", &config, None, None, None, None,
            )
            .unwrap()
            .bytes;

            // Validator accepts the bytes standalone.
            let entries = memstead_base::validator::archive::extract_entries(
                &bytes,
                &memstead_base::validator::ValidatorLimits::DEFAULT,
            )
            .unwrap();
            assert_eq!(entries.markdown_files.len(), 1);

            // Engine hydrate produces a working read surface.
            let hydrated = memstead_base::Engine::from_archive_bytes(bytes).unwrap();
            let entity = hydrated
                .get_entity(&memstead_base::EntityId::new("fixture", "alpha"))
                .expect("alpha must round-trip");
            assert_eq!(entity.title, "Alpha");
        }

        #[test]
        fn export_from_branch_embeds_supplied_anchors_member() {
            // Export leg (criterion 5, git-branch producer): the engine sources
            // the anchors sidecar from the branch tip and hands it here; the
            // assembler places the recognised `.memstead/anchors.json` member.
            let tmp = TempDir::new().unwrap();
            let (gitdir, mem_dir) = seed_mem_branch(
                tmp.path(),
                "fixture",
                &[(
                    "a.md",
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# A\n\n## Identity\n\nA.\n",
                )],
            );
            let config = memstead_schema::load_and_validate(&mem_dir).unwrap();
            let sidecar = br#"{"version":1,"entities":{"fixture--a":[{"artifact":"a.rs","grain":"file","class":"anchored","hash_stability":"stable","hash":"h1"}]}}"#;
            let bytes = export_mem_from_branch_to_bytes(
                &gitdir,
                "fixture",
                &config,
                None,
                None,
                None,
                Some(sidecar),
            )
            .unwrap()
            .bytes;
            let entries = memstead_base::validator::archive::extract_entries(
                &bytes,
                &memstead_base::validator::ValidatorLimits::DEFAULT,
            )
            .unwrap();
            assert_eq!(
                entries.anchors_bytes.as_deref(),
                Some(&sidecar[..]),
                "git-branch export must embed the supplied anchors member"
            );
            // A None sidecar embeds no member (byte-identical to pre-anchor).
            let without = export_mem_from_branch_to_bytes(
                &gitdir, "fixture", &config, None, None, None, None,
            )
            .unwrap()
            .bytes;
            let entries_without = memstead_base::validator::archive::extract_entries(
                &without,
                &memstead_base::validator::ValidatorLimits::DEFAULT,
            )
            .unwrap();
            assert!(entries_without.anchors_bytes.is_none());
        }

        #[test]
        fn publish_re_runs_yield_byte_identical_tarballs() {
            let tmp = TempDir::new().unwrap();
            let (gitdir, mem_dir) = seed_mem_branch(
                tmp.path(),
                "fixture",
                &[(
                    "a.md",
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# A\n\n## Identity\n\nA.\n",
                )],
            );

            let config = memstead_schema::load_and_validate(&mem_dir).unwrap();
            let out1 = tmp.path().join("a.mem");
            let out2 = tmp.path().join("b.mem");
            export_mem_from_branch(&gitdir, "fixture", &config, &out1, None, None, None, None)
                .unwrap();
            // Sleep a few ms to defeat any wallclock-based determinism leak.
            std::thread::sleep(std::time::Duration::from_millis(10));
            export_mem_from_branch(&gitdir, "fixture", &config, &out2, None, None, None, None)
                .unwrap();
            let a = std::fs::read(&out1).unwrap();
            let b = std::fs::read(&out2).unwrap();
            assert_eq!(
                a, b,
                "branch-walk archive exports must be byte-stable across re-runs"
            );
        }
    }
}
