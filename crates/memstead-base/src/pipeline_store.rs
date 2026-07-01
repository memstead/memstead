//! File-adapter persistence for the four pipeline primitives.
//!
//! Reads and writes [`Medium`] / [`Facet`] / [`Projection`] / [`Ingest`] JSON
//! under the workspace store ([`WORKSPACE_STORE_DIR`]):
//!
//! - `<root>/.memstead/mediums/<vault>/<name>.json`
//! - `<root>/.memstead/facets/<vault>/<name>.json`
//! - `<root>/.memstead/projections/<vault>/<name>.json`
//! - `<root>/.memstead/ingests/<name>.json`  — flat; ingests are not per-vault
//!
//! (The plan's acceptance-criteria text lists `projections`/`ingests` under a
//! `.graph/` path; that is a typo for `.memstead/` — the AC header, Goal, and
//! Constraints all place every pipeline config in the `.memstead/` workspace
//! store. All four primitives live under `.memstead/` here.)
//!
//! Mediums, facets, and projections are per-vault (a `<vault>` subdirectory
//! tier preserves the "vault owns its territory" framing); ingests are flat,
//! matching the legacy `ingests/<name>.json` layout. The record's
//! identity is `(vault, name)` derived from the file path; `name` is the file
//! stem.
//!
//! The loader's job is load + validate + expose read-only: a malformed config
//! surfaces a typed [`StoreError::Parse`] naming the offending file (the
//! early-validation value), rather than being silently skipped.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

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

/// A per-vault pipeline record paired with the vault and name (file stem) that
/// identify it on disk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VaultPipelineRecord<T> {
    /// The vault subdirectory this record lives under.
    pub vault: String,
    /// The record's name — the file stem (e.g. `source-tree`).
    pub name: String,
    /// The parsed config.
    pub config: T,
}

/// A flat (non-per-vault) pipeline record — used for ingests.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PipelineRecord<T> {
    /// The record's name — the file stem (e.g. `macos-graph`).
    pub name: String,
    /// The parsed config.
    pub config: T,
}

/// Every pipeline config in a workspace store, in the four-primitive shape.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct PipelineConfigs {
    /// Per-vault mediums.
    pub mediums: Vec<VaultPipelineRecord<Medium>>,
    /// Per-vault facets.
    pub facets: Vec<VaultPipelineRecord<Facet>>,
    /// Per-vault projections.
    pub projections: Vec<VaultPipelineRecord<Projection>>,
    /// Flat ingests.
    pub ingests: Vec<PipelineRecord<Ingest>>,
}

/// The `<root>/.memstead/<primitive>` directory for a given primitive.
fn primitive_dir(workspace_root: &Path, primitive: &str) -> PathBuf {
    workspace_root.join(WORKSPACE_STORE_DIR).join(primitive)
}

/// File path of a per-vault record: `<root>/.memstead/<primitive>/<vault>/<name>.json`.
fn vault_scoped_path(workspace_root: &Path, primitive: &str, vault: &str, name: &str) -> PathBuf {
    primitive_dir(workspace_root, primitive)
        .join(vault)
        .join(format!("{name}.json"))
}

/// File path of a flat (non-per-vault) record: `<root>/.memstead/<primitive>/<name>.json`.
fn flat_path(workspace_root: &Path, primitive: &str, name: &str) -> PathBuf {
    primitive_dir(workspace_root, primitive).join(format!("{name}.json"))
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

/// Load every `<primitive>/<vault>/<name>.json` under the store, parsed.
/// Absent primitive directory → empty (a workspace may declare no pipelines).
/// A malformed file surfaces a typed parse error naming the path.
fn load_vault_scoped<T: DeserializeOwned>(
    workspace_root: &Path,
    primitive: &str,
) -> Result<Vec<VaultPipelineRecord<T>>, StoreError> {
    let dir = primitive_dir(workspace_root, primitive);
    let mut out: Vec<VaultPipelineRecord<T>> = Vec::new();
    let vault_dirs = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(StoreError::Io { path: dir, source: e }),
    };
    for vault_entry in vault_dirs.flatten() {
        let vault_path = vault_entry.path();
        if !vault_path.is_dir() {
            continue;
        }
        let vault = vault_entry.file_name().to_string_lossy().into_owned();
        let files = match std::fs::read_dir(&vault_path) {
            Ok(rd) => rd,
            Err(e) => {
                return Err(StoreError::Io {
                    path: vault_path,
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
            out.push(VaultPipelineRecord {
                vault: vault.clone(),
                name,
                config,
            });
        }
    }
    // Deterministic order so callers (and tests) see a stable enumeration.
    out.sort_by(|a, b| (a.vault.as_str(), a.name.as_str()).cmp(&(b.vault.as_str(), b.name.as_str())));
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
        Err(e) => return Err(StoreError::Io { path: dir, source: e }),
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

/// Write a medium to `<root>/.memstead/mediums/<vault>/<name>.json`.
pub fn write_medium(
    workspace_root: &Path,
    vault: &str,
    name: &str,
    medium: &Medium,
) -> Result<(), StoreError> {
    write_json(&vault_scoped_path(workspace_root, MEDIUMS_DIR, vault, name), medium)
}

/// Write a facet to `<root>/.memstead/facets/<vault>/<name>.json`.
pub fn write_facet(
    workspace_root: &Path,
    vault: &str,
    name: &str,
    facet: &Facet,
) -> Result<(), StoreError> {
    write_json(&vault_scoped_path(workspace_root, FACETS_DIR, vault, name), facet)
}

/// Write a projection to `<root>/.memstead/projections/<vault>/<name>.json`.
pub fn write_projection(
    workspace_root: &Path,
    vault: &str,
    name: &str,
    projection: &Projection,
) -> Result<(), StoreError> {
    write_json(
        &vault_scoped_path(workspace_root, PROJECTIONS_DIR, vault, name),
        projection,
    )
}

/// Write an ingest to `<root>/.memstead/ingests/<name>.json` (flat).
pub fn write_ingest(
    workspace_root: &Path,
    name: &str,
    ingest: &Ingest,
) -> Result<(), StoreError> {
    write_json(&flat_path(workspace_root, INGESTS_DIR, name), ingest)
}

/// Delete a medium file. Missing → [`StoreError::Io`]; callers that want a
/// friendly "no such medium" pre-check existence via [`load_pipeline_configs`].
pub fn delete_medium(workspace_root: &Path, vault: &str, name: &str) -> Result<(), StoreError> {
    remove_file(&vault_scoped_path(workspace_root, MEDIUMS_DIR, vault, name))
}

/// Delete a facet file. See [`delete_medium`] for missing-file semantics.
pub fn delete_facet(workspace_root: &Path, vault: &str, name: &str) -> Result<(), StoreError> {
    remove_file(&vault_scoped_path(workspace_root, FACETS_DIR, vault, name))
}

/// Delete a projection file. See [`delete_medium`] for missing-file semantics.
pub fn delete_projection(workspace_root: &Path, vault: &str, name: &str) -> Result<(), StoreError> {
    remove_file(&vault_scoped_path(workspace_root, PROJECTIONS_DIR, vault, name))
}

/// Delete an ingest file (flat). See [`delete_medium`] for missing-file semantics.
pub fn delete_ingest(workspace_root: &Path, name: &str) -> Result<(), StoreError> {
    remove_file(&flat_path(workspace_root, INGESTS_DIR, name))
}

// Rename is exposed only for the *nameless* records (projection, ingest),
// whose identity is the file stem alone. Mediums and facets carry an embedded
// `name` field that must equal the stem (facets reference mediums by name,
// projections reference facets by name); a pure file move would leave that
// field stale, so their rename lives in the `pipeline_edit` layer, which
// rewrites the embedded name and dependent references together.

/// Rename a projection within its vault (`old` → `new`, same `<vault>` tier).
/// Refuses to clobber an existing target. A projection has no embedded name,
/// so a file move is its whole rename; rewriting dependent ingest `projection`
/// references is the calling layer's job.
pub fn rename_projection(
    workspace_root: &Path,
    vault: &str,
    old: &str,
    new: &str,
) -> Result<(), StoreError> {
    rename_file(
        &vault_scoped_path(workspace_root, PROJECTIONS_DIR, vault, old),
        &vault_scoped_path(workspace_root, PROJECTIONS_DIR, vault, new),
    )
}

/// Rename an ingest (flat). Refuses to clobber an existing target. An ingest
/// has no embedded name and nothing references it, so a file move is the whole
/// rename.
pub fn rename_ingest(workspace_root: &Path, old: &str, new: &str) -> Result<(), StoreError> {
    rename_file(
        &flat_path(workspace_root, INGESTS_DIR, old),
        &flat_path(workspace_root, INGESTS_DIR, new),
    )
}

/// Load all four primitives from the workspace store. Absent directories
/// resolve to empty; a malformed file surfaces a typed [`StoreError::Parse`].
pub fn load_pipeline_configs(workspace_root: &Path) -> Result<PipelineConfigs, StoreError> {
    Ok(PipelineConfigs {
        mediums: load_vault_scoped(workspace_root, MEDIUMS_DIR)?,
        facets: load_vault_scoped(workspace_root, FACETS_DIR)?,
        projections: load_vault_scoped(workspace_root, PROJECTIONS_DIR)?,
        ingests: load_flat(workspace_root, INGESTS_DIR)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{
        IngestMode, IngestTrigger, MediumType, PatternEntry, PatternMode,
    };
    use tempfile::TempDir;

    fn sample() -> (Medium, Facet, Projection, Ingest) {
        let medium = Medium {
            name: "source-tree".to_string(),
            medium_type: MediumType::Codebase,
            pointer: "../macos".to_string(),
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
            reference_vaults: vec!["engine".to_string()],
            destination_vault: "macos".to_string(),
        };
        let ingest = Ingest {
            projection: "macos/graph".to_string(),
            mode: IngestMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 20,
            deny_paths: vec!["VISION.md".to_string()],
        };
        (medium, facet, projection, ingest)
    }

    #[test]
    fn empty_store_loads_empty_configs() {
        let tmp = TempDir::new().unwrap();
        let configs = load_pipeline_configs(tmp.path()).unwrap();
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
        assert!(root.join(".memstead/mediums/macos/source-tree.json").is_file());
        assert!(root.join(".memstead/facets/macos/source-files.json").is_file());
        assert!(root.join(".memstead/projections/macos/graph.json").is_file());
        assert!(root.join(".memstead/ingests/macos-graph.json").is_file());

        let configs = load_pipeline_configs(root).unwrap();
        assert_eq!(configs.mediums.len(), 1);
        assert_eq!(configs.mediums[0].vault, "macos");
        assert_eq!(configs.mediums[0].name, "source-tree");
        assert_eq!(configs.mediums[0].config, medium);
        assert_eq!(configs.facets[0].config, facet);
        assert_eq!(configs.projections[0].config, projection);
        assert_eq!(configs.ingests.len(), 1);
        assert_eq!(configs.ingests[0].name, "macos-graph");
        assert_eq!(configs.ingests[0].config, ingest);
    }

    #[test]
    fn load_enumeration_is_sorted_and_per_vault() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let (medium, _, _, _) = sample();
        write_medium(root, "engine", "z-medium", &medium).unwrap();
        write_medium(root, "engine", "a-medium", &medium).unwrap();
        write_medium(root, "macos", "m-medium", &medium).unwrap();

        let configs = load_pipeline_configs(root).unwrap();
        let keys: Vec<_> = configs
            .mediums
            .iter()
            .map(|r| (r.vault.as_str(), r.name.as_str()))
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

        let err = load_pipeline_configs(root).unwrap_err();
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

        assert!(!root.join(".memstead/mediums/macos/source-tree.json").exists());
        assert!(!root.join(".memstead/ingests/macos-graph.json").exists());
        let configs = load_pipeline_configs(root).unwrap();
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

        assert!(!root.join(".memstead/projections/macos/old-name.json").exists());
        let configs = load_pipeline_configs(root).unwrap();
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
}
