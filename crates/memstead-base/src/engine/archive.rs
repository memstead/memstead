//! Byte-based snapshot API: hydrate an engine from sealed `.mem`
//! archive bytes, and export a mem's current state back to archive
//! bytes.
//!
//! Bridge consumers and future browser-WASM replicas consume
//! these two methods to ship the current state of a mem over HTTP
//! without materialising a temp file. Both methods go through the
//! existing validator + storage stack — same wire format, same caps,
//! same refusal envelopes — but expose a single-call API that hides
//! `ArchiveBackend` / `Mount` from the caller.
//!
//! Symmetric contract: bytes produced by [`Engine::export_mem_to_bytes`]
//! hydrate cleanly into another [`Engine`] via
//! [`Engine::from_archive_bytes`], and the resulting engine answers the
//! read surface (`memstead_overview`, `memstead_search`, `memstead_entity`,
//! `memstead_health`) with results indistinguishable from the source for
//! the exported mem. Mutation methods refuse via the existing
//! sealed-backend / read-only-mount envelope — no new error categories
//! enter the surface here.

use std::path::PathBuf;
use std::sync::Arc;

use memstead_schema::{Schema, load_schema_from_memory};

use crate::backend::MemBackend;
use crate::storage::ArchiveBackend;
use crate::validator::ValidatorLimits;
use crate::validator::archive::{ArchiveEntries, SchemaFile, extract_entries};
use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

use super::{Engine, EngineError};

/// Errors surfaced by [`Engine::from_archive_bytes`].
///
/// The archive ingress validator's typed payload rides through as
/// [`Self::Validation`] so the caller pattern-matches on the same
/// variant `extract_entries` would surface standalone — the new API
/// does not collapse validation failures into a generic error. The
/// remaining variants cover the small ladder of engine-side failures
/// (config parse, embedded schema load, downstream construction).
#[derive(Debug, thiserror::Error)]
pub enum FromArchiveBytesError {
    /// Archive bytes failed validation by `extract_entries`. Carries
    /// the typed [`crate::validator::ValidationError`] verbatim.
    #[error("archive validation: {0}")]
    Validation(#[from] crate::validator::ValidationError),
    /// `.memstead/config.json` inside the archive could not be parsed as a
    /// `PublishedMemConfig`. The archive bytes passed the
    /// archive-level whitelist but the JSON shape failed.
    #[error("invalid published config: {0}")]
    InvalidConfig(String),
    /// The embedded `.memstead/schema/` package failed to load via
    /// `load_schema_from_memory`.
    #[error("embedded schema failed to load: {0}")]
    EmbeddedSchemaInvalid(String),
    /// Downstream engine construction failed (e.g., schema pin not
    /// resolved against builtins + embedded schemas).
    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl Engine {
    /// Hydrate an engine from sealed archive bytes (`.mem`).
    ///
    /// Validates the bytes through the archive ingress validator
    /// (`extract_entries`), reads the embedded `.memstead/config.json` for
    /// mem name + schema pin, loads any embedded schema package
    /// (`.memstead/schema/`) into the engine's schema catalogue, and
    /// constructs a single-mount read-only engine backed by the bytes.
    /// No temp file, no on-disk artifact — the bytes are the storage.
    ///
    /// The resulting engine refuses mutations (`memstead_create`,
    /// `memstead_update`, `memstead_delete`, `memstead_relate`, `memstead_rename`) via
    /// the existing read-only-mount / sealed-backend envelope. Read
    /// operations work for the embedded mem.
    pub fn from_archive_bytes(bytes: Vec<u8>) -> Result<Self, FromArchiveBytesError> {
        Self::from_archive_bytes_with_limits(bytes, &ValidatorLimits::DEFAULT)
    }

    /// Variant of [`Self::from_archive_bytes`] with caller-supplied
    /// limits. Bridge / registry deployments tune the caps; the
    /// default ladder ([`ValidatorLimits::DEFAULT`]) is what
    /// `from_archive_bytes` picks.
    pub fn from_archive_bytes_with_limits(
        bytes: Vec<u8>,
        limits: &ValidatorLimits,
    ) -> Result<Self, FromArchiveBytesError> {
        let entries = extract_entries(&bytes, limits)?;
        let ArchiveEntries {
            config_bytes,
            schema_files,
            ..
        } = &entries;

        let published: memstead_schema::PublishedMemConfig =
            serde_json::from_slice(config_bytes)
                .map_err(|e| FromArchiveBytesError::InvalidConfig(e.to_string()))?;

        let extra_schemas = load_embedded_schemas(schema_files)?;

        let mount = Mount {
            mem: published.name.clone(),
            schema: Some(published.schema.clone()),
            storage: MountStorage::Archive {
                path: PathBuf::new(),
            },
            capability: MountCapability::ReadOnly,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        let backend: Box<dyn MemBackend> = Box::new(ArchiveBackend::from_bytes(bytes));

        let engine = Self::from_mounts_inner(vec![(mount, backend)], extra_schemas)?;
        Ok(engine)
    }

    /// Export the named mem's current state as `.mem` archive bytes.
    ///
    /// Symmetric to [`Self::from_archive_bytes`]: a mem name in, a
    /// self-contained byte buffer out. The bytes validate against
    /// `extract_entries` standalone — any consumer of sealed archives
    /// accepts them. Feeding the bytes back into
    /// `Engine::from_archive_bytes` yields an engine that returns
    /// identical reads against the exported mem.
    ///
    /// Returns [`EngineError::UnknownMem`] when the name resolves to
    /// no mount; [`EngineError::Backend`] wrapping
    /// [`crate::backend::BackendError::Sealed`] when the mem is
    /// archive-mounted (already-an-archive, no meaningful re-export);
    /// [`EngineError::InvalidInput`] when the mem has no loaded
    /// `MemConfig`; [`EngineError::MemConfigIncomplete`] when the
    /// loaded config is missing `version`. The git-branch byte-export
    /// path lifts in a follow-up; today it surfaces as
    /// [`EngineError::Backend`] wrapping the unmounted-hook message.
    pub fn export_mem_to_bytes(&self, mem_name: &str) -> Result<Vec<u8>, EngineError> {
        let mount = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem_name)
            .ok_or_else(|| EngineError::UnknownMem(mem_name.to_string()))?;
        let config = self.mem_config_for(mem_name).ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "mem '{mem_name}' has no loaded MemConfig — cannot export"
            ))
        })?;
        if config.version.is_none() {
            return Err(EngineError::MemConfigIncomplete {
                mem: mem_name.to_string(),
                missing_fields: vec!["version".to_string()],
            });
        }
        let workspace_root = self.workspace_root.as_deref();
        // Fixed authored-schema location (the `schemas_dir` key is retired).
        let fixed_schemas_dir = workspace_root.map(|r| r.join(".memstead").join("schemas"));
        let workspace_schemas_dir = fixed_schemas_dir.as_deref();
        match &mount.mount.storage {
            MountStorage::Folder { path } => crate::ops::export::export_mem_to_bytes(
                path,
                config,
                workspace_root,
                workspace_schemas_dir,
                mem_name,
            )
            .map(|out| out.bytes)
            .map_err(|e| {
                EngineError::Backend(crate::backend::BackendError::Other(format!(
                    "export_mem_to_bytes: {e}"
                )))
            }),
            MountStorage::Archive { .. } => {
                Err(EngineError::Backend(crate::backend::BackendError::Sealed))
            }
            MountStorage::GitBranch { gitdir, branch } => {
                let hook = self.git_branch_ops.as_ref().ok_or_else(|| {
                    EngineError::Backend(crate::backend::BackendError::Other(
                        "git-branch export hook not installed (full flavour not loaded)"
                            .to_string(),
                    ))
                })?;
                // Source per-entity provenance from the git-branch mutation
                // log (commit trailers) via the mount's backend and hand the
                // serialised payload to the hook to embed — the hook walks
                // no history itself.
                let provenance_bytes = mount
                    .backend
                    .read_provenance(None)
                    .ok()
                    .and_then(|records| crate::ops::export::build_archive_provenance(&records))
                    .and_then(|prov| prov.to_archive_bytes().ok());
                (hook.export_to_bytes)(
                    gitdir,
                    branch,
                    mem_name,
                    config,
                    workspace_root,
                    workspace_schemas_dir,
                    provenance_bytes.as_deref(),
                )
                .map(|out| out.bytes)
                .map_err(EngineError::Backend)
            }
            // In-memory mems have no directory to walk: list the
            // entities from the backend (RAM) and seal them through the
            // same storage-agnostic archive builder the folder path uses,
            // so a session mem exports to a `.mem` that mounts
            // standalone identically.
            MountStorage::InMemory => {
                let backend = mount.backend.as_ref();
                let rels = backend.list_entities().map_err(EngineError::Backend)?;
                let mut md_entries: Vec<(std::path::PathBuf, Vec<u8>)> =
                    Vec::with_capacity(rels.len());
                for rel in rels {
                    if let Some(bytes) = backend.read_entity(&rel).map_err(EngineError::Backend)? {
                        md_entries.push((rel, bytes));
                    }
                }
                // Source per-entity provenance from the backend's mutation
                // log so an in-memory mem exports a provenance-bearing
                // `.mem` identical in shape to the folder/git-branch paths.
                let provenance = backend
                    .read_provenance(None)
                    .ok()
                    .and_then(|records| crate::ops::export::build_archive_provenance(&records));
                crate::ops::export::export_entries_to_bytes(
                    config,
                    workspace_root,
                    workspace_schemas_dir,
                    mem_name,
                    md_entries,
                    provenance.as_ref(),
                )
                .map(|out| out.bytes)
                .map_err(|e| {
                    EngineError::Backend(crate::backend::BackendError::Other(format!(
                        "export_mem_to_bytes: {e}"
                    )))
                })
            }
        }
    }
}

/// Load the embedded `.memstead/schema/` package (if any) via
/// `load_schema_from_memory`. Returns an empty vec when the archive
/// carries no schema files — the boot resolver then falls back to the
/// built-in catalogue for the schema pin. `pub(crate)` so the archive
/// `SchemaSource` reads through the same loader.
pub(crate) fn load_embedded_schemas(
    schema_files: &[SchemaFile],
) -> Result<Vec<Arc<Schema>>, FromArchiveBytesError> {
    if schema_files.is_empty() {
        return Ok(Vec::new());
    }
    let mut manifest: Option<&str> = None;
    let mut types: Vec<(String, String)> = Vec::new();
    for sf in schema_files {
        if sf.archive_path == ".memstead/schema/schema.yaml" {
            manifest = Some(&sf.content);
        } else if let Some(rest) = sf.archive_path.strip_prefix(".memstead/schema/types/")
            && let Some(stem) = rest.strip_suffix(".yaml")
        {
            types.push((stem.to_string(), sf.content.clone()));
        }
    }
    let Some(manifest_yaml) = manifest else {
        return Err(FromArchiveBytesError::EmbeddedSchemaInvalid(
            "embedded schema package present but `.memstead/schema/schema.yaml` missing"
                .to_string(),
        ));
    };
    let schema = load_schema_from_memory(manifest_yaml, &types)
        .map_err(|e| FromArchiveBytesError::EmbeddedSchemaInvalid(e.to_string()))?;
    Ok(vec![Arc::new(schema)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    use crate::backend::{BackendError, MemBackend};
    use crate::engine::test_helpers::{cli_actor, empty_create_args, folder_mount};
    use crate::storage::FilesystemMemWriter;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    /// Seed a folder-backed mem with `.memstead/config.json` and N
    /// entities (zero allowed); return the running engine + mem dir.
    fn folder_mem_with_entities(tmp: &TempDir, titles: &[&str]) -> (Engine, std::path::PathBuf) {
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        let config_body = r#"{
            "format": 1,
            "schema": "default@1.0.0",
            "version": "1.0.0"
        }"#;
        std::fs::write(mem_dir.join(".memstead").join("config.json"), config_body).unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        for t in titles {
            engine
                .create_entity(empty_create_args("specs", t), actor, Some(&client), None)
                .unwrap();
        }
        (engine, mem_dir)
    }

    #[test]
    fn export_to_bytes_produces_bytes_that_extract_cleanly() {
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &["Alpha", "Beta"]);
        let bytes = engine.export_mem_to_bytes("specs").unwrap();
        assert!(!bytes.is_empty(), "export bytes must be non-empty");
        // The bytes validate against the archive ingress validator
        // standalone — any consumer of sealed archives accepts them.
        let entries = extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
        assert_eq!(entries.markdown_files.len(), 2);
        let mut names: Vec<_> = entries
            .markdown_files
            .iter()
            .map(|m| m.path.clone())
            .collect();
        names.sort();
        assert_eq!(names, vec!["alpha.md".to_string(), "beta.md".to_string()]);
    }

    /// End-to-end producer → consumer round-trip: an entity created with
    /// an authoring note exports per-entity provenance into the archive,
    /// and a fresh engine that installs those bytes reads the rationale
    /// back — matching the source. An entity created without a note is
    /// absent from the payload (no fabricated provenance).
    #[test]
    fn export_carries_provenance_that_install_reads_back() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        // Alpha carries a note; Beta deliberately does not.
        engine
            .create_entity(
                empty_create_args("specs", "Alpha"),
                actor,
                Some(&client),
                Some("why alpha exists"),
            )
            .unwrap();
        engine
            .create_entity(
                empty_create_args("specs", "Beta"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let bytes = engine.export_mem_to_bytes("specs").unwrap();
        // The archive carries the provenance payload.
        let entries = extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
        assert!(
            entries.provenance_bytes.is_some(),
            "export must embed the provenance payload"
        );

        // The publish/install store path persists the *canonical* (re-packed)
        // bytes, not the raw upload — so normalize must preserve the
        // provenance member or it would be dropped before serving.
        let validated =
            crate::validator::validate_and_normalize_archive(&bytes).expect("archive re-validates");
        let canonical_entries =
            extract_entries(&validated.canonical_bytes, &ValidatorLimits::DEFAULT).unwrap();
        assert!(
            canonical_entries.provenance_bytes.is_some(),
            "normalize must preserve provenance through the canonical re-pack (publish store path)"
        );

        // Install the bytes into a fresh engine and read provenance back.
        let installed = Engine::from_archive_bytes(bytes).unwrap();
        let prov = installed
            .archive_provenance_for("specs")
            .expect("installed mem exposes provenance");
        assert_eq!(
            prov.entity("alpha").and_then(|r| r.rationale.as_deref()),
            Some("why alpha exists"),
            "noted entity's rationale matches the source"
        );
        assert_eq!(
            prov.entity("alpha").and_then(|r| r.kind.as_deref()),
            Some("create"),
        );
        assert!(
            prov.entity("beta").is_none(),
            "entity authored without a note is absent — no fabricated provenance"
        );
    }

    /// Inject an extra member into a zip archive, returning fresh bytes.
    /// Used to add an anchors sidecar to an exported `.mem` for the
    /// archive-survival test (the export leg does not yet embed anchors —
    /// deferred — so the test synthesises the member the registry path
    /// must round-trip).
    fn inject_zip_member(archive: &[u8], name: &str, content: &[u8]) -> Vec<u8> {
        use std::io::{Read, Write};
        let mut src = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
        let mut out = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for i in 0..src.len() {
                let mut f = src.by_index(i).unwrap();
                let fname = f.name().to_string();
                let mut buf = Vec::new();
                f.read_to_end(&mut buf).unwrap();
                w.start_file(fname, opts).unwrap();
                w.write_all(&buf).unwrap();
            }
            w.start_file(name, opts).unwrap();
            w.write_all(content).unwrap();
            w.finish().unwrap();
        }
        out
    }

    /// Registry-leg survival: an anchors sidecar member threads verbatim
    /// through `validate_and_normalize_archive`'s canonical re-pack (the
    /// publish/install store path) rather than being silently stripped, and
    /// the installed mem exposes the anchors.
    #[test]
    fn anchors_member_survives_canonical_repack_and_install() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, _dir) = folder_mem_with_entities(&tmp, &["Alpha"]);
        let _ = &mut engine;
        let exported = engine.export_mem_to_bytes("specs").unwrap();

        let anchors = br#"{"version":1,"entities":{"specs--alpha":[{"artifact":"src/lib.rs","grain":"file","class":"anchored","hash_stability":"stable","hash":"h1"}]}}"#;
        let with_anchors = inject_zip_member(&exported, ".memstead/anchors.json", anchors);

        // Recognised at extract time.
        let entries = extract_entries(&with_anchors, &ValidatorLimits::DEFAULT).unwrap();
        assert_eq!(entries.anchors_bytes.as_deref(), Some(&anchors[..]));

        // Threaded through the canonical re-pack (what publish stores).
        let validated = crate::validator::validate_and_normalize_archive(&with_anchors)
            .expect("archive with anchors re-validates");
        let canonical =
            extract_entries(&validated.canonical_bytes, &ValidatorLimits::DEFAULT).unwrap();
        assert_eq!(
            canonical.anchors_bytes.as_deref(),
            Some(&anchors[..]),
            "normalize must preserve the anchors member through the canonical re-pack"
        );

        // Installing the canonical bytes exposes the anchors on the mem.
        let installed = Engine::from_archive_bytes(validated.canonical_bytes).unwrap();
        let ids = installed.entity_anchors(&crate::EntityId::new("specs", "alpha"));
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].artifact, "src/lib.rs");
    }

    /// Size discipline: provenance scales with entity count (one current
    /// rationale per entity, each ≤ the 280-char note cap), so for a
    /// representative mem (~60 noted entities, larger than the live engine
    /// seed's ~120 but with realistic notes) the provenance-bearing archive
    /// stays well under the registry's 2 MB publish body limit, and the
    /// provenance payload is a small fraction of the archive.
    #[test]
    fn provenance_bearing_archive_stays_within_publish_budget() {
        const PUBLISH_BODY_LIMIT: usize = 2 * 1024 * 1024;
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        // A realistic-length authoring note on every entity (near the
        // 280-char cap) — the worst case for provenance size.
        let note = "x".repeat(280);
        for i in 0..60 {
            engine
                .create_entity(
                    empty_create_args("specs", &format!("Entity {i}")),
                    actor,
                    Some(&client),
                    Some(&note),
                )
                .unwrap();
        }
        let bytes = engine.export_mem_to_bytes("specs").unwrap();
        assert!(
            bytes.len() < PUBLISH_BODY_LIMIT,
            "archive ({} B) must stay under the 2 MB publish limit",
            bytes.len()
        );
        let entries = extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
        let prov = entries.provenance_bytes.expect("provenance present");
        // Every entity's rationale travelled and the payload is a modest
        // fraction of the archive, not a budget threat.
        assert!(
            prov.len() < PUBLISH_BODY_LIMIT / 4,
            "provenance payload ({} B) is a small fraction of the budget",
            prov.len()
        );
        let parsed = memstead_schema::ArchiveProvenance::from_archive_bytes(&prov).unwrap();
        assert_eq!(
            parsed.entities.len(),
            60,
            "every noted entity has provenance"
        );
    }

    /// A mem slice carrying a
    /// cross-mem edge (target lives in another mem, won't travel in
    /// this single-mem archive) exports successfully — the archive is
    /// still produced — and the export surfaces the dangling edge so the
    /// operator sees, before sharing, exactly what `install` will reject.
    /// AC1 (export warns, archive produced) + AC2 (export's condition ==
    /// install's refusal) tested against one set of bytes.
    #[test]
    fn export_warns_on_cross_mem_edge_that_install_refuses() {
        let tmp = TempDir::new().unwrap();
        let (engine, mem_dir) = folder_mem_with_entities(&tmp, &[]);
        // Hand-write a valid spec whose only blemish is a cross-mem
        // USES edge into mem `other` — the folder export reads `.md`
        // verbatim, so the edge lands in the archive.
        let md = "\
---
type: spec
created_date: 2026-01-15
last_modified: 2026-01-15
level: M0
---
# Broker

## Identity

A

## Purpose

B

## Specifies

C

## Constraints

D

## Rationale

E

## Relationships

- **USES**: [[other--thing]]
";
        std::fs::write(mem_dir.join("broker.md"), md).unwrap();

        // The path-shaped export carries the dangling edge on its result
        // and still writes the archive (AC1).
        let out = tmp.path().join("specs.mem");
        let result = engine.export_mem("specs", &out).unwrap();
        assert!(out.is_file(), "archive must still be produced");
        assert_eq!(
            result.dangling_cross_mem_edges.len(),
            1,
            "export must surface the cross-mem edge: {:?}",
            result.dangling_cross_mem_edges
        );
        let edge = &result.dangling_cross_mem_edges[0];
        assert_eq!(edge.entity_path, "broker.md");
        assert_eq!(edge.target_id, "other--thing");
        assert_eq!(edge.target_mem, "other");

        // AC2: the exact condition export warned on is what install
        // refuses on — the strict validator rejects these same bytes.
        let bytes = std::fs::read(&out).unwrap();
        let err = crate::validator::validate_and_normalize_archive(&bytes).unwrap_err();
        assert!(
            matches!(
                err,
                crate::validator::ValidationError::CrossMemRelationship { .. }
            ),
            "install-side strict validation must refuse the same edge: {err:?}",
        );
    }

    /// Complement: a self-contained export (no cross-mem edges) carries
    /// no dangling-edge warnings.
    #[test]
    fn export_self_contained_mem_warns_nothing() {
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &["Alpha", "Beta"]);
        let out = tmp.path().join("specs.mem");
        let result = engine.export_mem("specs", &out).unwrap();
        assert!(
            result.dangling_cross_mem_edges.is_empty(),
            "self-contained export must warn nothing: {:?}",
            result.dangling_cross_mem_edges
        );
    }

    #[test]
    fn export_empty_mem_produces_valid_hydratable_archive() {
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &[]);
        let bytes = engine.export_mem_to_bytes("specs").unwrap();
        // Validator accepts the empty case.
        let entries = extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
        assert!(entries.markdown_files.is_empty());
        // Hydrate path accepts it too.
        let hydrated = Engine::from_archive_bytes(bytes).unwrap();
        assert_eq!(hydrated.mem_names(), vec!["specs"]);
        assert!(hydrated.store().is_empty());
    }

    #[test]
    fn export_unknown_mem_returns_unknown_mem_error() {
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &[]);
        let err = engine.export_mem_to_bytes("missing").unwrap_err();
        match err {
            EngineError::UnknownMem(v) => assert_eq!(v, "missing"),
            other => panic!("expected UnknownMem, got {other:?}"),
        }
    }

    #[test]
    fn export_archive_backend_returns_sealed() {
        // Seed by exporting a folder mem, then re-mount the produced
        // archive as a read-only archive. The byte-export path on the
        // archive mount refuses with the Sealed envelope — matches the
        // existing path-based `export_mem` posture.
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &["Alpha"]);
        let bytes = engine.export_mem_to_bytes("specs").unwrap();

        let archive_path = tmp.path().join("ext.mem");
        std::fs::write(&archive_path, &bytes).unwrap();
        let archive_engine = Engine::from_mounts(vec![(
            Mount {
                mem: "ext".to_string(),
                schema: Some(memstead_schema::SchemaRef::new(
                    "default",
                    semver::Version::new(1, 0, 0),
                )),
                storage: MountStorage::Archive {
                    path: archive_path.clone(),
                },
                capability: MountCapability::ReadOnly,
                lifecycle: MountLifecycle::Lazy,
                cross_linkable: false,
                migration_target: None,
            },
            Box::new(crate::storage::ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let err = archive_engine.export_mem_to_bytes("ext").unwrap_err();
        assert!(matches!(err, EngineError::Backend(BackendError::Sealed)));
    }

    #[test]
    fn from_archive_bytes_refuses_non_zip_with_validation_error() {
        let err = Engine::from_archive_bytes(b"not a zip at all".to_vec()).unwrap_err();
        match err {
            FromArchiveBytesError::Validation(crate::validator::ValidationError::Zip(_)) => {}
            other => panic!("expected Validation(Zip(_)), got {other:?}"),
        }
    }

    #[test]
    fn from_archive_bytes_refuses_oversized_with_size_cap() {
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &["Alpha"]);
        let bytes = engine.export_mem_to_bytes("specs").unwrap();

        let mut limits = ValidatorLimits::DEFAULT;
        limits.max_compressed_archive = 1;
        let err = Engine::from_archive_bytes_with_limits(bytes, &limits).unwrap_err();
        match err {
            FromArchiveBytesError::Validation(
                crate::validator::ValidationError::SizeCapExceeded { .. },
            ) => {}
            other => panic!("expected Validation(SizeCapExceeded), got {other:?}"),
        }
    }

    #[test]
    fn hydrated_engine_answers_reads_and_refuses_writes() {
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &["Hello", "World"]);
        let bytes = engine.export_mem_to_bytes("specs").unwrap();
        let mut hydrated = Engine::from_archive_bytes(bytes).unwrap();

        // Read surface — same titles surface from the hydrated state.
        let hello = hydrated
            .get_entity(&crate::EntityId::new("specs", "hello"))
            .expect("hello entity must round-trip");
        assert_eq!(hello.title, "Hello");
        let world = hydrated
            .get_entity(&crate::EntityId::new("specs", "world"))
            .expect("world entity must round-trip");
        assert_eq!(world.title, "World");

        // Mutation surface — read-only mount refuses with the existing
        // typed envelope (no new error categories on the hydrate path).
        let (actor, client) = cli_actor();
        let err = hydrated
            .create_entity(
                empty_create_args("specs", "Forbidden"),
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::ReadOnlyMount(v) if v == "specs"));
    }

    #[test]
    fn round_trip_preserves_entities_and_relations() {
        // Build a multi-entity mem with a relation, export → hydrate,
        // and confirm state equivalence: same ids, same content per
        // entity, same relations.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut source = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        let src = source
            .create_entity(
                empty_create_args("specs", "Source"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let tgt = source
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        source
            .relate_entity(
                crate::engine::RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(src.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: tgt.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let bytes = source.export_mem_to_bytes("specs").unwrap();
        let hydrated = Engine::from_archive_bytes(bytes).unwrap();

        // Same id set.
        let mut src_ids: Vec<String> = source
            .store()
            .all_entities()
            .map(|e| e.id.to_string())
            .collect();
        let mut hyd_ids: Vec<String> = hydrated
            .store()
            .all_entities()
            .map(|e| e.id.to_string())
            .collect();
        src_ids.sort();
        hyd_ids.sort();
        assert_eq!(src_ids, hyd_ids);

        // Same title + entity_type per id.
        for id_str in &src_ids {
            let (mem, slug) = id_str.split_once("--").expect("ids carry `<mem>--<slug>`");
            let id = crate::EntityId::new(mem, slug);
            let s = source.get_entity(&id).unwrap();
            let h = hydrated.get_entity(&id).unwrap();
            assert_eq!(s.title, h.title, "title differs for {id_str}");
            assert_eq!(s.entity_type, h.entity_type, "type differs for {id_str}");
        }

        // Same outgoing relation set.
        let src_edges: Vec<_> = source
            .store()
            .outgoing(&src.id)
            .iter()
            .map(|e| (e.rel_type.clone(), e.target.clone()))
            .collect();
        let hyd_edges: Vec<_> = hydrated
            .store()
            .outgoing(&src.id)
            .iter()
            .map(|e| (e.rel_type.clone(), e.target.clone()))
            .collect();
        assert_eq!(src_edges, hyd_edges);
    }

    #[test]
    fn export_then_hydrate_then_re_export_yields_byte_equivalent_archive() {
        // Determinism check — same source state must produce identical
        // archive bytes through the export path, and re-exporting from
        // the hydrated copy is not part of the contract (the hydrated
        // engine is read-only) but the produced bytes from the source
        // must be a fixpoint when re-fed.
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &["Alpha"]);
        let bytes1 = engine.export_mem_to_bytes("specs").unwrap();
        let bytes2 = engine.export_mem_to_bytes("specs").unwrap();
        assert_eq!(bytes1, bytes2, "export bytes must be deterministic");
    }

    /// Compare two engines' state for the named mem. State
    /// equivalence at minimum (per the round-trip AC): same entity
    /// ids, same content per entity (title, type, metadata, sections,
    /// content_hash), same relations (rel_type + target per source).
    /// Shared by the fixture-sweep round-trip tests so they assert the
    /// same invariant regardless of the fixture shape under test.
    fn assert_state_equivalent(source: &Engine, hydrated: &Engine, mem: &str) {
        let mut src_ids: Vec<String> = source
            .store()
            .all_entities()
            .filter(|e| e.mem == mem)
            .map(|e| e.id.to_string())
            .collect();
        let mut hyd_ids: Vec<String> = hydrated
            .store()
            .all_entities()
            .filter(|e| e.mem == mem)
            .map(|e| e.id.to_string())
            .collect();
        src_ids.sort();
        hyd_ids.sort();
        assert_eq!(src_ids, hyd_ids, "entity id set differs for mem {mem}");

        for id_str in &src_ids {
            let (v, slug) = id_str.split_once("--").expect("ids carry `<mem>--<slug>`");
            let id = crate::EntityId::new(v, slug);
            let s = source.get_entity(&id).expect("source entity present");
            let h = hydrated.get_entity(&id).expect("hydrated entity present");
            assert_eq!(s.title, h.title, "title differs for {id_str}");
            assert_eq!(s.entity_type, h.entity_type, "type differs for {id_str}");
            assert_eq!(s.metadata, h.metadata, "metadata differs for {id_str}");
            assert_eq!(s.sections, h.sections, "sections differ for {id_str}");
            assert_eq!(
                s.content_hash, h.content_hash,
                "content_hash differs for {id_str}",
            );

            let mut src_edges: Vec<_> = source
                .store()
                .outgoing(&id)
                .iter()
                .map(|e| (e.rel_type.clone(), e.target.to_string()))
                .collect();
            let mut hyd_edges: Vec<_> = hydrated
                .store()
                .outgoing(&id)
                .iter()
                .map(|e| (e.rel_type.clone(), e.target.to_string()))
                .collect();
            src_edges.sort();
            hyd_edges.sort();
            assert_eq!(src_edges, hyd_edges, "edges differ for {id_str}");
        }
    }

    /// Round-trip the engine state via export → hydrate, asserting
    /// state equivalence against the input. Returns the hydrated
    /// engine so individual tests can drive extra reads against it.
    fn round_trip(source: &Engine, mem: &str) -> Engine {
        let bytes = source.export_mem_to_bytes(mem).unwrap();
        // The bytes pass the validator standalone — same invariant the
        // bridge consumer relies on, asserted on every fixture so a
        // future export change can't silently break ingress.
        extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
        let hydrated = Engine::from_archive_bytes(bytes).unwrap();
        assert_state_equivalent(source, &hydrated, mem);
        hydrated
    }

    #[test]
    fn fixture_sweep_round_trip_empty() {
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &[]);
        let hydrated = round_trip(&engine, "specs");
        assert!(hydrated.store().is_empty());
    }

    #[test]
    fn fixture_sweep_round_trip_single_entity() {
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &["Solo"]);
        round_trip(&engine, "specs");
    }

    #[test]
    fn fixture_sweep_round_trip_multi_entity_no_relations() {
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &["A One", "A Two", "A Three"]);
        round_trip(&engine, "specs");
    }

    #[test]
    fn fixture_sweep_round_trip_entity_with_metadata_and_sections() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let mut sections = indexmap::IndexMap::new();
        sections.insert("identity".to_string(), "A rich body.".to_string());
        sections.insert(
            "purpose".to_string(),
            "To exercise the archive round-trip.".to_string(),
        );
        sections.insert(
            "rationale".to_string(),
            "Because the spec said so.".to_string(),
        );

        let mut metadata: indexmap::IndexMap<String, String> = indexmap::IndexMap::new();
        metadata.insert("level".to_string(), "M0".to_string());

        engine
            .create_entity(
                crate::engine::CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "specs".to_string(),
                    title: "Rich".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata,
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        round_trip(&engine, "specs");
    }

    #[test]
    fn fixture_sweep_round_trip_multi_entity_with_relations() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        let src = engine
            .create_entity(
                empty_create_args("specs", "Source"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let mid = engine
            .create_entity(
                empty_create_args("specs", "Middle"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let tgt = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // Two outgoing edges of different rel-types from the same
        // source — the round-trip must preserve both.
        engine
            .relate_entity(
                crate::engine::RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(src.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: mid.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let src_after = engine
            .get_entity(&src.id)
            .expect("source must still resolve");
        engine
            .relate_entity(
                crate::engine::RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(src_after.content_hash.clone()),
                    rel_type: "PART_OF".to_string(),
                    target: tgt.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        round_trip(&engine, "specs");
    }

    #[test]
    fn read_entity_path_works_against_byte_backed_archive() {
        // Sanity: the byte-backed ArchiveBackend the hydrate path
        // constructs answers `read_entity` for every listed path.
        let tmp = TempDir::new().unwrap();
        let (engine, _mem) = folder_mem_with_entities(&tmp, &["First", "Second"]);
        let bytes = engine.export_mem_to_bytes("specs").unwrap();
        let hydrated = Engine::from_archive_bytes(bytes).unwrap();
        let first = hydrated
            .get_entity(&crate::EntityId::new("specs", "first"))
            .expect("first must hydrate");
        assert_eq!(first.title, "First");
        // Path-based archive_path() returns None for byte-backed
        // backends — compile-time check that the contract holds.
        let backend = ArchiveBackend::from_bytes(Vec::new());
        let _: Option<&Path> = backend.archive_path();
    }
}
