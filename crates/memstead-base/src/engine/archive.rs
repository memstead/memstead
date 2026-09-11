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

use memstead_schema::Schema;

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
// The `Validation` variant carries the lower-layer error verbatim, which is
// what makes `#[from]` lifting possible; boxing it to equalise variant sizes
// would trade a cold-path allocation for a colder-path byte count.
#[allow(clippy::large_enum_variant)]
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
    /// The archive declares a `format` this engine does not accept
    /// (`published_format_accepted` refused it). A reader that proceeds
    /// past an unknown format would reinterpret bytes written under a
    /// contract it does not know — refuse, never guess.
    #[error(
        "unsupported archive format {declared} — this engine accepts formats {accepted:?}; \
         re-export the mem with a current engine or upgrade this one"
    )]
    UnsupportedFormat {
        /// The `format` the archive's config declares.
        declared: u32,
        /// The formats this engine accepts.
        accepted: &'static [u32],
    },
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

        // The format gate: every reader path consults the one predicate
        // (`published_format_accepted`) — this byte-hydration path used to
        // skip it, so an archive rewritten to `format: 99` hydrated and
        // served entities through the wasm package. Refuse typed instead.
        if !memstead_schema::published_format_accepted(published.format) {
            return Err(FromArchiveBytesError::UnsupportedFormat {
                declared: published.format,
                accepted: memstead_schema::PUBLISHED_MEM_FORMATS_ACCEPTED,
            });
        }

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

        let engine = Self::from_mounts_inner(vec![(mount, backend)], extra_schemas, Vec::new())?;
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
        self.export_mem_bytes_report(mem_name).map(|out| out.bytes)
    }

    /// The bytes export with its report: the archive bytes plus the
    /// counts the disk export reports (entities, dangling cross-mem
    /// edges, per-class provenance redactions), so a caller that never
    /// touches disk can still say what the archive carries.
    pub fn export_mem_bytes_report(
        &self,
        mem_name: &str,
    ) -> Result<crate::ops::export::MemExportBytes, EngineError> {
        let mount = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
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
                self.ref_schema_source_for(config),
            )
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
                        "git-branch export hook not installed (git-branch ops not wired)"
                            .to_string(),
                    ))
                })?;
                // Source per-entity provenance from the git-branch mutation
                // log (commit trailers) via the mount's backend and hand the
                // serialised payload to the hook to embed — the hook walks
                // no history itself.
                let entity_paths = crate::ops::export::entity_paths_of(
                    &mount
                        .backend
                        .list_entities()
                        .map_err(EngineError::Backend)?,
                );
                let records = mount.backend.read_provenance(None).unwrap_or_default();
                let (provenance, redactions) =
                    crate::ops::export::build_redacted_archive_provenance(&records, &entity_paths);
                let provenance_bytes = provenance.to_archive_bytes().ok();
                // Source the anchors sidecar from the branch tip so the
                // git-branch `.mem` carries anchors like the other backends.
                let anchors_bytes = mount.backend.read_anchors_sidecar().ok().flatten();
                (hook.export_to_bytes)(
                    gitdir,
                    branch,
                    mem_name,
                    config,
                    workspace_root,
                    workspace_schemas_dir,
                    provenance_bytes.as_deref(),
                    anchors_bytes.as_deref(),
                )
                .map(|mut out| {
                    out.redactions = redactions;
                    out
                })
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
                let entity_paths = crate::ops::export::entity_paths_of(&rels);
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
                let records = backend.read_provenance(None).unwrap_or_default();
                let (provenance, redactions) =
                    crate::ops::export::build_redacted_archive_provenance(&records, &entity_paths);
                // Source the anchors sidecar from the in-memory backend so a
                // sketch-session mem exports a `.mem` carrying its anchors —
                // the serve session-export → re-import round-trip.
                let anchors_bytes = backend
                    .read_anchors_sidecar()
                    .map_err(EngineError::Backend)?;
                crate::ops::export::export_entries_to_bytes(
                    config,
                    workspace_root,
                    workspace_schemas_dir,
                    mem_name,
                    md_entries,
                    Some(&provenance),
                    anchors_bytes.as_deref(),
                    self.ref_schema_source_for(config),
                )
                .map(|mut out| {
                    out.redactions = redactions;
                    out
                })
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
    // The archive keeps its sealed generation: marker present ⇒
    // current polarity; absent ⇒ legacy written meaning.
    let marker_path = format!(
        ".memstead/schema/{}",
        memstead_schema::loader::SCHEMA_FORMAT_MARKER_FILE
    );
    let format = if schema_files.iter().any(|sf| sf.archive_path == marker_path) {
        memstead_schema::MetadataPolarityFormat::RequiredOptIn
    } else {
        memstead_schema::MetadataPolarityFormat::Legacy
    };
    let schema =
        memstead_schema::load_schema_from_memory_with_format(manifest_yaml, &types, format)
            .map_err(|e| FromArchiveBytesError::EmbeddedSchemaInvalid(e.to_string()))?;
    Ok(vec![Arc::new(schema)])
}

#[cfg(test)]
mod tests;
