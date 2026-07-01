//! Markdown and vault-archive export.
//!
//! `export_markdown` regenerates entity files from the store, only writing
//! files that actually changed (incremental). Compares generated markdown
//! against the current file on disk byte-for-byte.
//!
//! `export_vault` zips a vault directory into a portable `.mem` archive
//! with deterministic output (sorted entries, fixed mtime).

use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use memstead_schema::{
    ARCHIVE_CONFIG_PATH, ARCHIVE_SCHEMA_PREFIX, TypeDefinition, VaultConfig,
    collect_schema_source, published_config_from, type_by_name,
};
#[cfg(feature = "git-object-storage")]
use memstead_base::ops::VaultExportBytes;
use zip::{CompressionMethod, DateTime, write::SimpleFileOptions};

use super::{ExportResult, VaultExportResult};
use crate::entity::generator::generate_markdown;
use crate::entity::writer::write_entity;
use crate::store::Store;
use crate::validator::canonical::canonical_json;
#[cfg(feature = "git-object-storage")]
use crate::storage::git_tree::{BranchReadError, read_branch_blobs};

/// Regenerate all entity markdown files from the in-memory store.
/// Only writes files that have changed (incremental export).
pub fn export_markdown(
    store: &Store,
    default_schema: &TypeDefinition,
    vault_dir: &Path,
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
        let full_path = vault_dir.join(&entity.file_path);
        let needs_write = match std::fs::read_to_string(&full_path) {
            Ok(existing) => existing != generated,
            Err(_) => true, // File doesn't exist — write it
        };

        if needs_write {
            let _ = write_entity(entity, vault_dir, schema);
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
    vault_dir: &Path,
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

    let full_path = vault_dir.join(&entity.file_path);
    let needs_write = match std::fs::read_to_string(&full_path) {
        Ok(existing) => existing != generated,
        Err(_) => true,
    };

    if needs_write {
        write_entity(entity, vault_dir, schema).map_err(|e| e.to_string())?;
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
// Vault (.mem) archive export
// ---------------------------------------------------------------------------

// Stage 1.7-2: VaultExportError and the folder-shaped `export_vault`
// live in `memstead-base::ops::export`. Re-export here so downstream
// callers that imported via the workspace path keep working. The
// gix-bound `export_vault_from_branch` (below) stays here.
pub use memstead_base::ops::export::{VaultExportError, export_vault};

#[cfg(feature = "git-object-storage")]
fn branch_read_into_vault_export(e: BranchReadError) -> VaultExportError {
    VaultExportError::BranchRead(e.to_string())
}

/// Export a vault as a portable `.mem` archive by walking the
/// `vault-repo-git` branch tree directly — the git-object storage path's
/// counterpart to [`export_vault`]. No working tree is consulted; all
/// `.md` content comes from the per-vault branch tip, sorted by path
/// for deterministic archive bytes.
///
/// Wire format matches [`export_vault`]:
/// `.memstead/config.json` carries the whitelist projection,
/// `.memstead/schema/` embeds the pinned schema's source files, and the rest of the
/// archive carries vault-relative `.md` blobs.
///
/// `vault_repo_gitdir` is the multi-root repo (`<workspace>/vault-repo/.git/`).
/// The function reads vault content from `refs/heads/<vault_name>` and
/// (for now) resolves the schema via [`collect_schema_source`] using
/// `workspace_root` + `workspace_schemas_dir` — the same chain used by
/// the disk path. Reading schemas from `refs/heads/__MEMSTEAD:schemas/<name>/`
/// is a follow-up; the current chain still satisfies the wire-format
/// contract because schemas are workspace-tracked alongside the cache.
#[cfg(feature = "git-object-storage")]
pub fn export_vault_from_branch(
    vault_repo_gitdir: &Path,
    vault_name: &str,
    config: &VaultConfig,
    output_path: &Path,
    workspace_root: Option<&Path>,
    workspace_schemas_dir: Option<&Path>,
    provenance_bytes: Option<&[u8]>,
) -> Result<VaultExportResult, VaultExportError> {
    let out = export_vault_from_branch_to_bytes(
        vault_repo_gitdir,
        vault_name,
        config,
        workspace_root,
        workspace_schemas_dir,
        provenance_bytes,
    )?;

    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, &out.bytes)?;

    let size_bytes = fs::metadata(output_path)?.len();
    Ok(VaultExportResult {
        archive_path: output_path.display().to_string(),
        name: out.name,
        version: out.version,
        entity_count: out.entity_count,
        size_bytes,
        dangling_cross_vault_edges: out.dangling_cross_vault_edges,
    })
}

/// Byte-shaped counterpart to [`export_vault_from_branch`]. Same wire
/// format, same determinism contract, no on-disk artifact — the bytes
/// are the output. The engine's `export_vault_to_bytes` dispatches to
/// this for git-branch mounts via the [`memstead_base::GitBranchOps`]
/// bundle so byte-snapshot consumers (the bridge, future WASM
/// replicas) treat folder and git-branch backends symmetrically.
#[cfg(feature = "git-object-storage")]
pub fn export_vault_from_branch_to_bytes(
    vault_repo_gitdir: &Path,
    vault_name: &str,
    config: &VaultConfig,
    workspace_root: Option<&Path>,
    workspace_schemas_dir: Option<&Path>,
    provenance_bytes: Option<&[u8]>,
) -> Result<VaultExportBytes, VaultExportError> {
    let published = published_config_from(config, vault_name)?;
    let config_bytes = canonical_json(&published)
        .map_err(|e| VaultExportError::Canonical(e.to_string()))?
        .into_bytes();

    let schema_files =
        collect_schema_source(workspace_root, workspace_schemas_dir, &published.schema)?;

    let ref_name = match workspace_root {
        Some(root) => crate::vault_repo_config::branch_ref_for_vault(root, vault_name),
        None => format!("refs/heads/{vault_name}"),
    };
    let blobs = match read_branch_blobs(vault_repo_gitdir, &ref_name) {
        Ok(b) => b,
        Err(BranchReadError::BranchMissing { .. }) => Vec::new(),
        Err(e) => return Err(branch_read_into_vault_export(e)),
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
    // rationale, keyed by vault-relative path). The engine walks the log;
    // this assembler only places the member.
    if let Some(prov) = provenance_bytes {
        all_entries.push((
            memstead_schema::ARCHIVE_PROVENANCE_PATH.to_string(),
            prov.to_vec(),
        ));
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

    // Surface every cross-vault
    // edge whose target won't travel inside this single-vault archive —
    // exactly what `install` will refuse on. Detected via the shared
    // predicate so export and install agree. Cross-vault-only (tolerant
    // parse): the git-branch export keeps its lenient-on-drift posture;
    // this adds the missing cross-vault signal so the failure no longer
    // surfaces silently at install time on the share boundary.
    let dangling_cross_vault_edges =
        memstead_base::validator::collect_dangling_cross_vault_edges_from_bytes(&buf)
            .map_err(|e| VaultExportError::ArchiveValidationFailed(e.to_string()))?;

    Ok(VaultExportBytes {
        bytes: buf,
        name: published.name.clone(),
        version: published.version.to_string(),
        entity_count,
        dangling_cross_vault_edges,
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
            vault: "specs".into(),
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
            vault: "memos".into(),
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
            vault: "assertions".into(),
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

    // ---- vault archive export ---------------------------------------------

    fn write_vault_fixture(dir: &Path) {
        std::fs::create_dir_all(dir.join(".memstead")).unwrap();
        // Author-side config with every flavor of author-only field the
        // whitelist projection has to strip. If the export pipeline ever
        // leaks one of these into the archive, the round-trip assertion
        // below catches it.
        std::fs::write(
            dir.join(".memstead/config.json"),
            r#"{"version":"1.2.0","description":"AWS patterns","schema":"default@1.0.0","writeGuidance":{"context":"secret"},"mediums":{},"projections":{},"readVaults":{}}"#,
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
    fn export_vault_writes_whitelisted_config_and_markdown() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("aws-patterns");
        write_vault_fixture(&vault);

        let config = memstead_schema::load_and_validate(&vault).unwrap();
        let out = tmp.path().join("aws-patterns.mem");

        // `vault` acts as the source root for schema resolution. It has no
        // `.memstead/schemas/` dir of its own, so the default pin falls through
        // to the embedded builtin.
        let result = export_vault(&vault, &config, &out, None, None).unwrap();
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
        assert_eq!(written["format"], memstead_schema::PUBLISHED_VAULT_FORMAT);
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
            "readVaults",
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
    fn export_vault_is_deterministic() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("aws-patterns");
        write_vault_fixture(&vault);

        let config = memstead_schema::load_and_validate(&vault).unwrap();
        let out1 = tmp.path().join("a.mem");
        let out2 = tmp.path().join("b.mem");

        export_vault(&vault, &config, &out1, None, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        export_vault(&vault, &config, &out2, None, None).unwrap();

        let a = std::fs::read(&out1).unwrap();
        let b = std::fs::read(&out2).unwrap();
        assert_eq!(a, b, "vault archive exports must be byte-stable");
    }

    #[test]
    fn export_vault_errors_when_version_missing() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("no-version");
        std::fs::create_dir_all(vault.join(".memstead")).unwrap();
        std::fs::write(
            vault.join(".memstead/config.json"),
            r#"{"schema":"default@1.0.0","mediums":{},"projections":{}}"#,
        )
        .unwrap();
        std::fs::write(
            vault.join("a.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# A\n\n## Identity\n\nA.\n",
        ).unwrap();

        let config = memstead_schema::load_and_validate(&vault).unwrap();
        let out = tmp.path().join("out.mem");
        let err = export_vault(&vault, &config, &out, None, None).unwrap_err();
        assert!(matches!(
            err,
            VaultExportError::Convert(memstead_schema::PublishConversionError::MissingVersion)
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

    // ---- vault archive export from git-object branch ----------------------

    #[cfg(feature = "git-object-storage")]
    mod git_object_export {
        use super::*;
        use crate::storage::git_tree::GitTreeVaultWriter;
        use crate::storage::VaultWriter;
        use crate::vcs::CommitContext;

        /// Build a fresh `vault-repo-git`-style bare repo and a side-by-side
        /// vault config dir so [`export_vault_from_branch`] has both inputs
        /// it needs: the gitdir + ref to walk, plus a disk-resident
        /// `<vault>/.memstead/config.json` for the metadata projection.
        fn seed_vault_branch(
            workspace: &Path,
            vault_name: &str,
            entries: &[(&str, &str)],
        ) -> (PathBuf, PathBuf) {
            let gitdir = workspace.join("vault-repo").join(".git");
            std::fs::create_dir_all(&gitdir).unwrap();
            gix::init_bare(&gitdir).unwrap();

            // Per-vault on-disk config dir mirrors the cutover layout —
            // schemas resolve through workspace paths regardless of the
            // adapter, and the engine still loads VaultConfig from disk.
            let vault_dir = workspace.join(vault_name);
            std::fs::create_dir_all(vault_dir.join(".memstead")).unwrap();
            std::fs::write(
                vault_dir.join(".memstead/config.json"),
                r#"{"version":"1.0.0","description":"fixture","schema":"default@1.0.0"}"#,
            )
            .unwrap();

            // Commit each entry to `refs/heads/<vault_name>` via the
            // production write path so the test exercises the same tree
            // shape `memstead-cli`'s mutations would produce.
            let writer = GitTreeVaultWriter::new(
                gitdir.clone(),
                format!("refs/heads/{vault_name}"),
            );
            for (rel, content) in entries {
                writer
                    .write_entity(Path::new(rel), content.as_bytes())
                    .unwrap();
            }
            writer
                .commit("seed", &CommitContext::internal())
                .unwrap();
            (gitdir, vault_dir)
        }

        #[test]
        fn publish_from_branch_produces_correct_tarball() {
            let tmp = TempDir::new().unwrap();
            let (gitdir, vault_dir) = seed_vault_branch(
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

            let config = memstead_schema::load_and_validate(&vault_dir).unwrap();
            let out = tmp.path().join("fixture.mem");
            let result = export_vault_from_branch(
                &gitdir,
                "fixture",
                &config,
                &out,
                None,
                None,
                None,
            )
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
            let (gitdir, vault_dir) = seed_vault_branch(
                tmp.path(),
                "fixture",
                &[(
                    "a.md",
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# A\n\n## Identity\n\nA.\n",
                )],
            );

            let config = memstead_schema::load_and_validate(&vault_dir).unwrap();
            let out = tmp.path().join("fixture.mem");
            export_vault_from_branch(&gitdir, "fixture", &config, &out, None, None, None).unwrap();

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
            let (gitdir, vault_dir) = seed_vault_branch(
                tmp.path(),
                "fixture",
                &[(
                    "a.md",
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# A\n\n## Identity\n\nA.\n",
                )],
            );
            let config = memstead_schema::load_and_validate(&vault_dir).unwrap();
            let out = tmp.path().join("fixture.mem");
            export_vault_from_branch(&gitdir, "fixture", &config, &out, None, None, None).unwrap();
            let path_bytes = std::fs::read(&out).unwrap();
            let byte_bytes = export_vault_from_branch_to_bytes(
                &gitdir, "fixture", &config, None, None, None,
            )
            .unwrap()
            .bytes;
            assert_eq!(path_bytes, byte_bytes);
        }

        #[test]
        fn byte_export_validates_and_hydrates_via_engine() {
            // The bridge consumer's contract: bytes out of
            // `export_vault_to_bytes` validate against `extract_entries`
            // standalone and hydrate into a new engine with the same
            // read surface for the exported vault.
            let tmp = TempDir::new().unwrap();
            let (gitdir, vault_dir) = seed_vault_branch(
                tmp.path(),
                "fixture",
                &[(
                    "alpha.md",
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nA round-trip seed.\n",
                )],
            );
            let config = memstead_schema::load_and_validate(&vault_dir).unwrap();
            let bytes = export_vault_from_branch_to_bytes(
                &gitdir, "fixture", &config, None, None, None,
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
        fn publish_re_runs_yield_byte_identical_tarballs() {
            let tmp = TempDir::new().unwrap();
            let (gitdir, vault_dir) = seed_vault_branch(
                tmp.path(),
                "fixture",
                &[(
                    "a.md",
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# A\n\n## Identity\n\nA.\n",
                )],
            );

            let config = memstead_schema::load_and_validate(&vault_dir).unwrap();
            let out1 = tmp.path().join("a.mem");
            let out2 = tmp.path().join("b.mem");
            export_vault_from_branch(&gitdir, "fixture", &config, &out1, None, None, None).unwrap();
            // Sleep a few ms to defeat any wallclock-based determinism leak.
            std::thread::sleep(std::time::Duration::from_millis(10));
            export_vault_from_branch(&gitdir, "fixture", &config, &out2, None, None, None).unwrap();
            let a = std::fs::read(&out1).unwrap();
            let b = std::fs::read(&out2).unwrap();
            assert_eq!(
                a, b,
                "branch-walk archive exports must be byte-stable across re-runs"
            );
        }
    }
}
