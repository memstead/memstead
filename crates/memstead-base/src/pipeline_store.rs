//! File-adapter persistence for the four pipeline primitives.
//!
//! Reads and writes [`Medium`] / [`Facet`] / [`Projection`] / [`Ingest`] JSON
//! under the workspace store ([`WORKSPACE_STORE_DIR`]):
//!
//! - `<root>/.memstead/mediums/<mem>/<name>.json`
//! - `<root>/.memstead/facets/<mem>/<name>.json`
//! - `<root>/.memstead/projections/<mem>/<name>.json`
//! - `<root>/.memstead/ingests/<name>.json`  — flat; ingests are not per-mem
//!
//! (The plan's acceptance-criteria text lists `projections`/`ingests` under a
//! `.graph/` path; that is a typo for `.memstead/` — the AC header, Goal, and
//! Constraints all place every pipeline config in the `.memstead/` workspace
//! store. All four primitives live under `.memstead/` here.)
//!
//! Mediums, facets, and projections are per-mem (a `<mem>` subdirectory
//! tier preserves the "mem owns its territory" framing); ingests are flat,
//! matching the legacy `ingests/<name>.json` layout. The record's
//! identity is `(mem, name)` derived from the file path; `name` is the file
//! stem.
//!
//! The loader's job is load + validate + expose read-only: a malformed config
//! surfaces a typed [`StoreError::Parse`] naming the offending file (the
//! early-validation value), rather than being silently skipped.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::binding::BindingV1;
use crate::pipeline::{Facet, Ingest, Medium, Projection};
use crate::workspace_store::{StoreError, WORKSPACE_STORE_DIR};

/// Per-primitive subdirectory names under the workspace store.
pub const MEDIUMS_DIR: &str = "mediums";
/// See [`MEDIUMS_DIR`].
pub const FACETS_DIR: &str = "facets";
/// See [`MEDIUMS_DIR`].
pub const PROJECTIONS_DIR: &str = "projections";
/// See [`MEDIUMS_DIR`].
pub const INGESTS_DIR: &str = "ingests";

/// A per-mem pipeline record paired with the mem and name (file stem) that
/// identify it on disk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MemPipelineRecord<T> {
    /// The mem subdirectory this record lives under.
    pub mem: String,
    /// The record's name — the file stem (e.g. `source-tree`).
    pub name: String,
    /// The parsed config.
    pub config: T,
}

/// A flat (non-per-mem) pipeline record — used for ingests.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PipelineRecord<T> {
    /// The record's name — the file stem (e.g. `macos-graph`).
    pub name: String,
    /// The parsed config.
    pub config: T,
}

/// Every pipeline config in a workspace store, in the four-primitive shape.
///
/// This is the **legacy** (gen-2) shape — produced only by
/// [`load_legacy_pipeline_configs`], consumed by the referential-integrity
/// edit layer, the `projection migrate` transform, and the macOS
/// `pipeline_configs_json` surface. The live loader
/// ([`load_pipeline_configs`]) returns [`BindingConfigs`] instead.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct PipelineConfigs {
    /// Per-mem mediums.
    pub mediums: Vec<MemPipelineRecord<Medium>>,
    /// Per-mem facets.
    pub facets: Vec<MemPipelineRecord<Facet>>,
    /// Per-mem projections.
    pub projections: Vec<MemPipelineRecord<Projection>>,
    /// Flat ingests.
    pub ingests: Vec<PipelineRecord<Ingest>>,
}

/// Every pipeline config in a workspace store, in the **binding v1** shape
/// (D1/D2) — mediums, facets, and one [`BindingV1`] per source→mem obligation
/// (the projection + flat-ingest split collapsed into a single versioned
/// record). This is what the live loader [`load_pipeline_configs`] returns and
/// the brief / selection / cursor paths consume.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct BindingConfigs {
    /// Per-mem mediums.
    pub mediums: Vec<MemPipelineRecord<Medium>>,
    /// Per-mem facets.
    pub facets: Vec<MemPipelineRecord<Facet>>,
    /// Per-mem v1 bindings (occupying the `projections/<mem>/<name>.json` tier,
    /// stem-identity preserved). Canonical id is `<mem>/<stem>` (D3).
    pub bindings: Vec<MemPipelineRecord<BindingV1>>,
}

/// The `<root>/.memstead/<primitive>` directory for a given primitive.
fn primitive_dir(workspace_root: &Path, primitive: &str) -> PathBuf {
    workspace_root.join(WORKSPACE_STORE_DIR).join(primitive)
}

/// Refuse a `mem`/`name` value that is not a single, plain path
/// component: separators, traversal segments, drive/stream colons, NULs,
/// and empty values would let a caller-supplied name write or delete
/// outside the workspace's own metadata directory. Validated here — the
/// one place every mutation's path is built — so no surface above
/// (CLI, UniFFI, engine) can bypass it.
fn validate_component(kind: &str, value: &str) -> Result<(), StoreError> {
    let invalid = value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.contains(':')
        || value.contains('\0');
    if invalid {
        return Err(StoreError::Other(format!(
            "invalid {kind} '{}': must be a single path component \
             (no separators, traversal segments, ':' or NUL)",
            value.escape_default()
        )));
    }
    Ok(())
}

/// File path of a per-mem record: `<root>/.memstead/<primitive>/<mem>/<name>.json`.
fn mem_scoped_path(
    workspace_root: &Path,
    primitive: &str,
    mem: &str,
    name: &str,
) -> Result<PathBuf, StoreError> {
    validate_component("mem", mem)?;
    validate_component("name", name)?;
    Ok(primitive_dir(workspace_root, primitive)
        .join(mem)
        .join(format!("{name}.json")))
}

/// File path of a flat (non-per-mem) record: `<root>/.memstead/<primitive>/<name>.json`.
fn flat_path(workspace_root: &Path, primitive: &str, name: &str) -> Result<PathBuf, StoreError> {
    validate_component("name", name)?;
    Ok(primitive_dir(workspace_root, primitive).join(format!("{name}.json")))
}

/// Remove the file at `path`, mapping IO failures (including a missing
/// file) to a typed [`StoreError::Io`] naming the path. Dumb counterpart
/// to [`write_json`] — referential-integrity / existence checks belong to
/// the calling layer, matching the write-is-upsert / load-validates split.
fn remove_file(path: &Path) -> Result<(), StoreError> {
    std::fs::remove_file(path).map_err(|e| StoreError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Rename the record file `from` → `to`. Refuses to clobber an existing
/// target (silent overwrite would lose a distinct record); that guard is
/// the one non-dumb concession here because the failure mode is data loss.
/// A missing source surfaces as [`StoreError::Io`]. Reference rewriting in
/// dependent primitives is the calling layer's job.
fn rename_file(from: &Path, to: &Path) -> Result<(), StoreError> {
    if to.exists() {
        return Err(StoreError::Other(format!(
            "rename target already exists: {}",
            to.display()
        )));
    }
    std::fs::rename(from, to).map_err(|e| StoreError::Io {
        path: from.to_path_buf(),
        source: e,
    })
}

/// Serialise `config` (pretty JSON) into `path`, creating parent directories.
fn write_json<T: Serialize>(path: &Path, config: &T) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| StoreError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    let bytes = serde_json::to_vec_pretty(config).map_err(|e| StoreError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    std::fs::write(path, bytes).map_err(|e| StoreError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Load every `<primitive>/<mem>/<name>.json` under the store, parsed.
/// Absent primitive directory → empty (a workspace may declare no pipelines).
/// A malformed file surfaces a typed parse error naming the path.
fn load_mem_scoped<T: DeserializeOwned>(
    workspace_root: &Path,
    primitive: &str,
) -> Result<Vec<MemPipelineRecord<T>>, StoreError> {
    let dir = primitive_dir(workspace_root, primitive);
    let mut out: Vec<MemPipelineRecord<T>> = Vec::new();
    let mem_dirs = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => {
            return Err(StoreError::Io {
                path: dir,
                source: e,
            });
        }
    };
    for mem_entry in mem_dirs.flatten() {
        let mem_path = mem_entry.path();
        if !mem_path.is_dir() {
            continue;
        }
        let mem = mem_entry.file_name().to_string_lossy().into_owned();
        let files = match std::fs::read_dir(&mem_path) {
            Ok(rd) => rd,
            Err(e) => {
                return Err(StoreError::Io {
                    path: mem_path,
                    source: e,
                });
            }
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            let config = read_json::<T>(&path)?;
            out.push(MemPipelineRecord {
                mem: mem.clone(),
                name,
                config,
            });
        }
    }
    // Deterministic order so callers (and tests) see a stable enumeration.
    out.sort_by(|a, b| (a.mem.as_str(), a.name.as_str()).cmp(&(b.mem.as_str(), b.name.as_str())));
    Ok(out)
}

/// Load every `ingests/<name>.json` (flat), parsed.
fn load_flat<T: DeserializeOwned>(
    workspace_root: &Path,
    primitive: &str,
) -> Result<Vec<PipelineRecord<T>>, StoreError> {
    let dir = primitive_dir(workspace_root, primitive);
    let mut out: Vec<PipelineRecord<T>> = Vec::new();
    let files = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => {
            return Err(StoreError::Io {
                path: dir,
                source: e,
            });
        }
    };
    for file in files.flatten() {
        let path = file.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        let config = read_json::<T>(&path)?;
        out.push(PipelineRecord { name, config });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Read + parse one JSON file into `T`, mapping IO/parse failures to typed
/// [`StoreError`]s naming the path.
fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    let bytes = std::fs::read(path).map_err(|e| StoreError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    serde_json::from_slice(&bytes).map_err(|e| StoreError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

/// Write a medium to `<root>/.memstead/mediums/<mem>/<name>.json`.
pub fn write_medium(
    workspace_root: &Path,
    mem: &str,
    name: &str,
    medium: &Medium,
) -> Result<(), StoreError> {
    write_json(
        &mem_scoped_path(workspace_root, MEDIUMS_DIR, mem, name)?,
        medium,
    )
}

/// Write a facet to `<root>/.memstead/facets/<mem>/<name>.json`.
pub fn write_facet(
    workspace_root: &Path,
    mem: &str,
    name: &str,
    facet: &Facet,
) -> Result<(), StoreError> {
    write_json(
        &mem_scoped_path(workspace_root, FACETS_DIR, mem, name)?,
        facet,
    )
}

/// Write a projection to `<root>/.memstead/projections/<mem>/<name>.json`.
pub fn write_projection(
    workspace_root: &Path,
    mem: &str,
    name: &str,
    projection: &Projection,
) -> Result<(), StoreError> {
    write_json(
        &mem_scoped_path(workspace_root, PROJECTIONS_DIR, mem, name)?,
        projection,
    )
}

/// Write an ingest to `<root>/.memstead/ingests/<name>.json` (flat).
pub fn write_ingest(workspace_root: &Path, name: &str, ingest: &Ingest) -> Result<(), StoreError> {
    write_json(&flat_path(workspace_root, INGESTS_DIR, name)?, ingest)
}

/// Write a v1 binding to `<root>/.memstead/projections/<mem>/<name>.json`.
///
/// A binding (`BindingV1`) occupies the *same* per-mem projections tier and
/// file identity a gen-2 [`Projection`] did (stem-identity preserved, D1/D3),
/// so this overwrites the gen-2 projection file in place when promoting a
/// workspace. Additive counterpart to [`write_projection`]; nothing in the
/// live loader reads the v1 shape yet (that gate is a later slice).
pub fn write_binding(
    workspace_root: &Path,
    mem: &str,
    name: &str,
    binding: &BindingV1,
) -> Result<(), StoreError> {
    write_json(
        &mem_scoped_path(workspace_root, PROJECTIONS_DIR, mem, name)?,
        binding,
    )
}

/// Read the v1 binding at `<root>/.memstead/projections/<mem>/<name>.json`.
///
/// The read counterpart of [`write_binding`] — reads the *same* per-mem
/// projections tier and file identity, parsed as a [`BindingV1`]. A missing
/// file surfaces [`StoreError::Io`] (kind `NotFound`); a file present but not a
/// v1 binding (e.g. a not-yet-migrated gen-2 projection) surfaces
/// [`StoreError::Parse`]. Callers wanting a friendly "no such binding" message
/// pre-check existence and keep the two apart. Additive; the live loader does
/// not consult this yet (that gate is a later slice).
pub fn read_binding(workspace_root: &Path, mem: &str, name: &str) -> Result<BindingV1, StoreError> {
    read_json(&mem_scoped_path(
        workspace_root,
        PROJECTIONS_DIR,
        mem,
        name,
    )?)
}

/// Delete a medium file. Missing → [`StoreError::Io`]; callers that want a
/// friendly "no such medium" pre-check existence via [`load_pipeline_configs`].
pub fn delete_medium(workspace_root: &Path, mem: &str, name: &str) -> Result<(), StoreError> {
    remove_file(&mem_scoped_path(workspace_root, MEDIUMS_DIR, mem, name)?)
}

/// Delete a facet file. See [`delete_medium`] for missing-file semantics.
pub fn delete_facet(workspace_root: &Path, mem: &str, name: &str) -> Result<(), StoreError> {
    remove_file(&mem_scoped_path(workspace_root, FACETS_DIR, mem, name)?)
}

/// Delete a projection file. See [`delete_medium`] for missing-file semantics.
pub fn delete_projection(workspace_root: &Path, mem: &str, name: &str) -> Result<(), StoreError> {
    remove_file(&mem_scoped_path(
        workspace_root,
        PROJECTIONS_DIR,
        mem,
        name,
    )?)
}

/// Delete an ingest file (flat). See [`delete_medium`] for missing-file semantics.
pub fn delete_ingest(workspace_root: &Path, name: &str) -> Result<(), StoreError> {
    remove_file(&flat_path(workspace_root, INGESTS_DIR, name)?)
}

// Rename is exposed only for the *nameless* records (projection, ingest),
// whose identity is the file stem alone. Mediums and facets carry an embedded
// `name` field that must equal the stem (facets reference mediums by name,
// projections reference facets by name); a pure file move would leave that
// field stale, so their rename lives in the `pipeline_edit` layer, which
// rewrites the embedded name and dependent references together.

/// Rename a projection within its mem (`old` → `new`, same `<mem>` tier).
/// Refuses to clobber an existing target. A projection has no embedded name,
/// so a file move is its whole rename; rewriting dependent ingest `projection`
/// references is the calling layer's job.
pub fn rename_projection(
    workspace_root: &Path,
    mem: &str,
    old: &str,
    new: &str,
) -> Result<(), StoreError> {
    rename_file(
        &mem_scoped_path(workspace_root, PROJECTIONS_DIR, mem, old)?,
        &mem_scoped_path(workspace_root, PROJECTIONS_DIR, mem, new)?,
    )
}

/// Rename an ingest (flat). Refuses to clobber an existing target. An ingest
/// has no embedded name and nothing references it, so a file move is the whole
/// rename.
pub fn rename_ingest(workspace_root: &Path, old: &str, new: &str) -> Result<(), StoreError> {
    rename_file(
        &flat_path(workspace_root, INGESTS_DIR, old)?,
        &flat_path(workspace_root, INGESTS_DIR, new)?,
    )
}

/// Load the **legacy** (gen-2) four-primitive store from the workspace.
/// Absent directories resolve to empty; a malformed file surfaces a typed
/// [`StoreError::Parse`]. This reader is the counterpart of the version-gated
/// [`load_pipeline_configs`]: it deliberately reads the old
/// `Projection` + flat-`Ingest` shape (parsing a v1 binding file lossily as a
/// [`Projection`], which ignores `version`/`operations`), so
/// `projection migrate`, the referential-integrity edit layer, and the macOS
/// `pipeline_configs_json` surface keep working. It performs **no** version
/// gate — it is the escape hatch the gate points migrations at.
pub fn load_legacy_pipeline_configs(workspace_root: &Path) -> Result<PipelineConfigs, StoreError> {
    Ok(PipelineConfigs {
        mediums: load_mem_scoped(workspace_root, MEDIUMS_DIR)?,
        facets: load_mem_scoped(workspace_root, FACETS_DIR)?,
        projections: load_mem_scoped(workspace_root, PROJECTIONS_DIR)?,
        ingests: load_flat(workspace_root, INGESTS_DIR)?,
    })
}

/// Load every `projections/<mem>/<name>.json` as a **v1 binding**, version-gated
/// (D2). Absent directory → empty. For each file:
///
/// - no `version` field → [`StoreError::LegacyProjectionStore`] (the pre-v1
///   layout the loader no longer serves; the message names
///   `memstead projection migrate`);
/// - `version` present but not `1` → [`StoreError::UnknownBindingVersion`];
/// - `version: 1` → parsed as [`BindingV1`] (a malformed operations block etc.
///   surfaces [`StoreError::Parse`] naming the file).
///
/// The gate refuses the whole load on the first offending file — a version-less
/// workspace fails loudly (pointing at migrate) rather than loading half-served.
fn load_bindings(workspace_root: &Path) -> Result<Vec<MemPipelineRecord<BindingV1>>, StoreError> {
    let dir = primitive_dir(workspace_root, PROJECTIONS_DIR);
    let mut out: Vec<MemPipelineRecord<BindingV1>> = Vec::new();
    let mem_dirs = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => {
            return Err(StoreError::Io {
                path: dir,
                source: e,
            });
        }
    };
    for mem_entry in mem_dirs.flatten() {
        let mem_path = mem_entry.path();
        if !mem_path.is_dir() {
            continue;
        }
        let mem = mem_entry.file_name().to_string_lossy().into_owned();
        let files = match std::fs::read_dir(&mem_path) {
            Ok(rd) => rd,
            Err(e) => {
                return Err(StoreError::Io {
                    path: mem_path,
                    source: e,
                });
            }
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            // Peek at `version` before committing to the BindingV1 shape so a
            // pre-v1 (version-less) file yields the migrate-naming error rather
            // than an opaque "missing field `version`" parse error.
            let value: serde_json::Value = read_json(&path)?;
            match value.get("version") {
                None => return Err(StoreError::LegacyProjectionStore { path }),
                Some(v) => {
                    let n = v.as_i64();
                    if n != Some(i64::from(crate::binding::BINDING_VERSION)) {
                        return Err(StoreError::UnknownBindingVersion {
                            path,
                            version: n.unwrap_or(-1),
                        });
                    }
                }
            }
            let config: BindingV1 =
                serde_json::from_value(value).map_err(|e| StoreError::Parse {
                    path: path.clone(),
                    message: e.to_string(),
                })?;
            out.push(MemPipelineRecord {
                mem: mem.clone(),
                name,
                config,
            });
        }
    }
    out.sort_by(|a, b| (a.mem.as_str(), a.name.as_str()).cmp(&(b.mem.as_str(), b.name.as_str())));
    Ok(out)
}

/// Load the live **binding v1** store from the workspace (D2). Mediums and
/// facets load as before; `projections/` is read as version-gated v1 bindings
/// via [`load_bindings`] — a version-less (pre-v1) `projections/` refuses with
/// [`StoreError::LegacyProjectionStore`] naming `memstead projection migrate`.
/// The flat `ingests/` directory is **never read** by this path (bindings carry
/// their operations); it is served only by [`load_legacy_pipeline_configs`] for
/// migration.
pub fn load_pipeline_configs(workspace_root: &Path) -> Result<BindingConfigs, StoreError> {
    Ok(BindingConfigs {
        mediums: load_mem_scoped(workspace_root, MEDIUMS_DIR)?,
        facets: load_mem_scoped(workspace_root, FACETS_DIR)?,
        bindings: load_bindings(workspace_root)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{IngestMode, IngestTrigger, MediumType, PatternEntry, PatternMode};
    use tempfile::TempDir;

    fn sample() -> (Medium, Facet, Projection, Ingest) {
        let medium = Medium {
            name: "source-tree".to_string(),
            medium_type: MediumType::Codebase,
            pointer: "../macos".to_string(),
            change_detection: None,
        };
        let facet = Facet {
            name: "source-files".to_string(),
            medium: "source-tree".to_string(),
            scope: vec![PatternEntry {
                path: "../macos/**/*.swift".to_string(),
                mode: PatternMode::Allow,
            }],
            engagement: None,
            preparation: None,
        };
        let projection = Projection {
            intent: Some("Swift macOS app source.".to_string()),
            source_facets: vec!["source-files".to_string()],
            reference_mems: vec!["engine".to_string()],
            destination_mem: "macos".to_string(),
            rules: None,
        };
        let ingest = Ingest {
            projection: "macos/graph".to_string(),
            mode: IngestMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 20,
            deny_paths: vec!["VISION.md".to_string()],
            post_actions: None,
        };
        (medium, facet, projection, ingest)
    }

    #[test]
    fn mutations_refuse_traversal_in_mem_and_name() {
        // Every mutation path-builds from caller-supplied mem/name; a
        // separator or traversal segment must refuse with a typed error
        // and leave nothing on disk outside the store.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let (medium, _, _, ingest) = sample();

        let evil_values = [
            "..",
            ".",
            "",
            "../escape",
            "a/b",
            "a\\b",
            "..\\up",
            "c:evil",
            "nul\0byte",
        ];
        for evil in evil_values {
            assert!(
                write_medium(root, evil, "ok", &medium).is_err(),
                "mem '{}' must refuse",
                evil.escape_default()
            );
            assert!(
                write_medium(root, "ok", evil, &medium).is_err(),
                "name '{}' must refuse",
                evil.escape_default()
            );
            assert!(write_ingest(root, evil, &ingest).is_err());
            assert!(delete_medium(root, evil, "ok").is_err());
            assert!(delete_ingest(root, evil).is_err());
            assert!(rename_projection(root, evil, "a", "b").is_err());
            assert!(rename_projection(root, "ok", evil, "b").is_err());
            assert!(rename_projection(root, "ok", "a", evil).is_err());
            assert!(rename_ingest(root, evil, "b").is_err());
            assert!(rename_ingest(root, "a", evil).is_err());
        }

        // A traversal write must not have escaped: the only thing under
        // the temp root may be the (empty) store dir, and the parent of
        // the temp root gained no `escape.json`.
        assert!(
            !root.parent().unwrap().join("escape.json").exists(),
            "no write may land outside the workspace"
        );

        // Existing valid names keep working.
        write_medium(root, "macos", "source-tree", &medium).unwrap();
        assert!(
            root.join(".memstead/mediums/macos/source-tree.json")
                .is_file()
        );
    }

    #[test]
    fn empty_store_loads_empty_configs() {
        let tmp = TempDir::new().unwrap();
        let configs = load_legacy_pipeline_configs(tmp.path()).unwrap();
        assert_eq!(configs, PipelineConfigs::default());
    }

    #[test]
    fn write_then_load_round_trips_all_four_primitives() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let (medium, facet, projection, ingest) = sample();

        write_medium(root, "macos", "source-tree", &medium).unwrap();
        write_facet(root, "macos", "source-files", &facet).unwrap();
        write_projection(root, "macos", "graph", &projection).unwrap();
        write_ingest(root, "macos-graph", &ingest).unwrap();

        // Files land at the documented `.memstead/` locations.
        assert!(
            root.join(".memstead/mediums/macos/source-tree.json")
                .is_file()
        );
        assert!(
            root.join(".memstead/facets/macos/source-files.json")
                .is_file()
        );
        assert!(
            root.join(".memstead/projections/macos/graph.json")
                .is_file()
        );
        assert!(root.join(".memstead/ingests/macos-graph.json").is_file());

        let configs = load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.mediums.len(), 1);
        assert_eq!(configs.mediums[0].mem, "macos");
        assert_eq!(configs.mediums[0].name, "source-tree");
        assert_eq!(configs.mediums[0].config, medium);
        assert_eq!(configs.facets[0].config, facet);
        assert_eq!(configs.projections[0].config, projection);
        assert_eq!(configs.ingests.len(), 1);
        assert_eq!(configs.ingests[0].name, "macos-graph");
        assert_eq!(configs.ingests[0].config, ingest);
    }

    #[test]
    fn load_enumeration_is_sorted_and_per_mem() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let (medium, _, _, _) = sample();
        write_medium(root, "engine", "z-medium", &medium).unwrap();
        write_medium(root, "engine", "a-medium", &medium).unwrap();
        write_medium(root, "macos", "m-medium", &medium).unwrap();

        let configs = load_legacy_pipeline_configs(root).unwrap();
        let keys: Vec<_> = configs
            .mediums
            .iter()
            .map(|r| (r.mem.as_str(), r.name.as_str()))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("engine", "a-medium"),
                ("engine", "z-medium"),
                ("macos", "m-medium"),
            ]
        );
    }

    #[test]
    fn malformed_config_surfaces_typed_parse_error_naming_the_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let bad = root.join(".memstead/mediums/macos");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("broken.json"), b"{ not valid json").unwrap();

        let err = load_legacy_pipeline_configs(root).unwrap_err();
        match err {
            StoreError::Parse { path, .. } => {
                assert!(path.ends_with("broken.json"), "got {path:?}");
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn delete_removes_the_record_and_load_reflects_it() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let (medium, _, _, ingest) = sample();
        write_medium(root, "macos", "source-tree", &medium).unwrap();
        write_ingest(root, "macos-graph", &ingest).unwrap();

        delete_medium(root, "macos", "source-tree").unwrap();
        delete_ingest(root, "macos-graph").unwrap();

        assert!(
            !root
                .join(".memstead/mediums/macos/source-tree.json")
                .exists()
        );
        assert!(!root.join(".memstead/ingests/macos-graph.json").exists());
        let configs = load_legacy_pipeline_configs(root).unwrap();
        assert!(configs.mediums.is_empty());
        assert!(configs.ingests.is_empty());
    }

    #[test]
    fn delete_of_missing_record_surfaces_io_error() {
        let tmp = TempDir::new().unwrap();
        let err = delete_medium(tmp.path(), "macos", "nope").unwrap_err();
        match err {
            StoreError::Io { source, .. } => {
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[test]
    fn rename_moves_the_record_preserving_config() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let (_, _, projection, _) = sample();
        write_projection(root, "macos", "old-name", &projection).unwrap();

        rename_projection(root, "macos", "old-name", "new-name").unwrap();

        assert!(
            !root
                .join(".memstead/projections/macos/old-name.json")
                .exists()
        );
        let configs = load_legacy_pipeline_configs(root).unwrap();
        assert_eq!(configs.projections.len(), 1);
        assert_eq!(configs.projections[0].name, "new-name");
        assert_eq!(configs.projections[0].config, projection);
    }

    #[test]
    fn rename_refuses_to_clobber_an_existing_target() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let (_, _, projection, _) = sample();
        write_projection(root, "macos", "a", &projection).unwrap();
        write_projection(root, "macos", "b", &projection).unwrap();

        let err = rename_projection(root, "macos", "a", "b").unwrap_err();
        assert!(matches!(err, StoreError::Other(_)), "got {err:?}");
        // Both records survive — nothing was lost.
        assert!(root.join(".memstead/projections/macos/a.json").exists());
        assert!(root.join(".memstead/projections/macos/b.json").exists());
    }

    #[test]
    fn rename_of_missing_source_surfaces_io_error() {
        let tmp = TempDir::new().unwrap();
        let err = rename_ingest(tmp.path(), "missing", "whatever").unwrap_err();
        assert!(matches!(err, StoreError::Io { .. }), "got {err:?}");
    }

    // ── binding v1 loader (D2 version gate) ──────────────────────────────

    fn sample_binding() -> BindingV1 {
        use crate::binding::{
            BINDING_VERSION, BuildMode, BuildOperation, CoverageSemantics, Operations,
        };
        use crate::pipeline::IngestTrigger;
        BindingV1 {
            version: BINDING_VERSION,
            intent: Some("prose".to_string()),
            source_facets: vec!["source-tree".to_string()],
            reference_mems: vec![],
            destination_mem: "engine".to_string(),
            deny_paths: vec![],
            coverage_semantics: CoverageSemantics::Exhaustive,
            rules: None,
            operations: Operations {
                build: BuildOperation {
                    mode: BuildMode::Discovery,
                    trigger: IngestTrigger::Loop,
                    batch_size: 20,
                    post_actions: None,
                },
                sync: None,
                verify: None,
            },
        }
    }

    #[test]
    fn empty_store_loads_empty_binding_configs() {
        let tmp = TempDir::new().unwrap();
        let configs = load_pipeline_configs(tmp.path()).unwrap();
        assert_eq!(configs, BindingConfigs::default());
    }

    #[test]
    fn binding_loader_round_trips_a_v1_binding() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let binding = sample_binding();
        write_binding(root, "engine", "graph", &binding).unwrap();

        let configs = load_pipeline_configs(root).unwrap();
        assert_eq!(configs.bindings.len(), 1);
        assert_eq!(configs.bindings[0].mem, "engine");
        assert_eq!(configs.bindings[0].name, "graph");
        assert_eq!(configs.bindings[0].config, binding);
    }

    #[test]
    fn version_less_projection_refuses_with_migrate_naming_error() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // A gen-2 (version-less) projection file.
        let projection = Projection {
            intent: Some("legacy".to_string()),
            source_facets: vec!["f".to_string()],
            reference_mems: vec![],
            destination_mem: "engine".to_string(),
            rules: None,
        };
        write_projection(root, "engine", "graph", &projection).unwrap();

        let err = load_pipeline_configs(root).unwrap_err();
        match err {
            StoreError::LegacyProjectionStore { path } => {
                assert!(path.ends_with("graph.json"), "got {path:?}");
                assert!(
                    err_message(&StoreError::LegacyProjectionStore { path })
                        .contains("memstead projection migrate")
                );
            }
            other => panic!("expected LegacyProjectionStore, got {other:?}"),
        }
    }

    #[test]
    fn unknown_binding_version_refuses() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = root.join(".memstead/projections/engine");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("graph.json"),
            br#"{"version": 99, "destination_mem": "engine", "operations": {"build": {"mode": "discovery", "trigger": "loop", "batch_size": 20}}}"#,
        )
        .unwrap();

        let err = load_pipeline_configs(root).unwrap_err();
        assert!(
            matches!(err, StoreError::UnknownBindingVersion { version: 99, .. }),
            "got {err:?}"
        );
    }

    fn err_message(e: &StoreError) -> String {
        e.to_string()
    }
}
