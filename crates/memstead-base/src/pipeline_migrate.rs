//! Migration from the legacy JSON-folder pipeline shape to the four-primitive
//! workspace-store shape.
//!
//! The legacy configs live at the *workspace root* (not under `.memstead/`):
//! `<root>/scopes/<mem>/<name>.json`, `<root>/projections/<mem>/<name>.json`,
//! `<root>/ingests/<name>.json`. This module reads them and converts to the
//! [`crate::pipeline`] four-primitive model — the shared core of both the
//! `memstead pipeline migrate` command (read legacy → write the store) and the
//! boot compatibility shim (read legacy on the fly while the store is empty).
//!
//! ## Conversion
//!
//! - A legacy **scope** conflates *territory* and *engagement*; it splits into
//!   a [`Medium`] (the `type` + a `pointer` derived as the longest common
//!   directory prefix of its allow-mode paths) and a [`Facet`] (the
//!   allow/deny tree, referencing the medium). Both take the scope's file-stem
//!   name. The scope's descriptive `label` has no slot in the four-primitive
//!   model and is dropped (it is reconstructable from the medium type + name).
//!   The engagement contract is left unset here — moving the ingest skill's
//!   `mediums.json` engagement metadata into facets is a separate concern.
//! - A legacy **projection**'s `sources[]` split by their reference: a
//!   `scope_ref` (role `primary`) becomes a `source_facets` entry (the scope's
//!   facet shares its name); a `mem` (role `reference`) becomes a
//!   `reference_mems` entry. The first `destinations[].mem` becomes the
//!   single `destination_mem`.
//! - A legacy **ingest** is structurally identical to the new [`Ingest`] and
//!   deserialises directly.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::pipeline::{Facet, Ingest, Medium, MediumType, PatternEntry, PatternMode, Projection};
use crate::pipeline_store::{MemPipelineRecord, PipelineConfigs, PipelineRecord};
use crate::workspace_store::StoreError;

/// Legacy `scopes/<mem>/<name>.json` shape: a medium type, a human label,
/// and an allow/deny `tree`.
#[derive(Debug, Deserialize)]
struct LegacyScope {
    #[serde(rename = "type")]
    medium_type: MediumType,
    #[serde(default)]
    #[allow(dead_code)]
    label: Option<String>,
    scope: LegacyScopeBody,
}

#[derive(Debug, Deserialize)]
struct LegacyScopeBody {
    #[serde(default)]
    tree: Vec<PatternEntry>,
}

/// Legacy `projections/<mem>/<name>.json` shape.
#[derive(Debug, Deserialize)]
struct LegacyProjection {
    #[serde(default)]
    intent: Option<String>,
    #[serde(default)]
    sources: Vec<LegacySource>,
    #[serde(default)]
    destinations: Vec<LegacyDestination>,
}

#[derive(Debug, Deserialize)]
struct LegacySource {
    #[serde(default)]
    scope_ref: Option<String>,
    #[serde(default)]
    mem: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LegacyDestination {
    mem: String,
}

/// The longest common directory prefix of a scope tree's **allow** paths,
/// stopping at the first glob component (`*`). This is the [`Medium`] pointer:
/// the base of the territory the allow patterns carve out. Deny paths do not
/// constrain the territory, so they are excluded. Empty when there are no
/// allow paths.
fn common_dir_prefix(tree: &[PatternEntry]) -> String {
    let allows: Vec<Vec<&str>> = tree
        .iter()
        .filter(|e| e.mode == PatternMode::Allow)
        .map(|e| e.path.split('/').collect())
        .collect();
    let Some(first) = allows.first() else {
        return String::new();
    };
    let mut prefix: Vec<&str> = Vec::new();
    'outer: for (i, comp) in first.iter().enumerate() {
        // A glob component is not a fixed directory — the prefix ends here.
        if comp.contains('*') {
            break;
        }
        // Every allow path must agree on this component.
        for other in &allows[1..] {
            if other.get(i) != Some(comp) {
                break 'outer;
            }
        }
        prefix.push(comp);
    }
    prefix.join("/")
}

/// Split a legacy scope into its [`Medium`] (territory) and [`Facet`]
/// (engagement). Both take `name` — the scope's file stem.
fn convert_scope(name: &str, scope: LegacyScope) -> (Medium, Facet) {
    let pointer = common_dir_prefix(&scope.scope.tree);
    let medium = Medium {
        name: name.to_string(),
        medium_type: scope.medium_type,
        pointer,
    };
    let facet = Facet {
        name: name.to_string(),
        medium: name.to_string(),
        scope: scope.scope.tree,
        engagement: None,
        preparation: None,
    };
    (medium, facet)
}

/// Convert a legacy projection to the four-primitive [`Projection`].
fn convert_projection(p: LegacyProjection) -> Projection {
    let mut source_facets = Vec::new();
    let mut reference_mems = Vec::new();
    for s in p.sources {
        if let Some(scope_ref) = s.scope_ref {
            source_facets.push(scope_ref);
        } else if let Some(mem) = s.mem {
            reference_mems.push(mem);
        }
    }
    Projection {
        intent: p.intent,
        source_facets,
        reference_mems,
        destination_mem: p
            .destinations
            .into_iter()
            .next()
            .map(|d| d.mem)
            .unwrap_or_default(),
    }
}

/// Read + parse one legacy JSON file, mapping failures to typed errors.
fn read_legacy_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    let bytes = std::fs::read(path).map_err(|e| StoreError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    serde_json::from_slice(&bytes).map_err(|e| StoreError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

/// Walk a per-mem legacy directory (`<root>/<primitive>/<mem>/<name>.json`),
/// yielding `(mem, name, path)` triples in sorted order. Absent → empty.
fn walk_mem_scoped(
    root: &Path,
    primitive: &str,
) -> Result<Vec<(String, String, PathBuf)>, StoreError> {
    let dir = root.join(primitive);
    let mut out = Vec::new();
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
        for file in std::fs::read_dir(&mem_path)
            .map_err(|e| StoreError::Io {
                path: mem_path.clone(),
                source: e,
            })?
            .flatten()
        {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) {
                out.push((mem.clone(), name, path));
            }
        }
    }
    out.sort_by(|a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str())));
    Ok(out)
}

/// Read the legacy JSON-folder pipeline configs at the workspace root and
/// convert them to the four-primitive [`PipelineConfigs`]. Absent legacy
/// directories resolve to empty; a malformed legacy file surfaces a typed
/// [`StoreError::Parse`] naming the path.
///
/// This is the shared core of the migration command and the boot compatibility
/// shim. It is read-only — it does not write the store or delete the legacy
/// folders.
pub fn read_legacy_pipeline_configs(workspace_root: &Path) -> Result<PipelineConfigs, StoreError> {
    let mut configs = PipelineConfigs::default();

    for (mem, name, path) in walk_mem_scoped(workspace_root, "scopes")? {
        let scope: LegacyScope = read_legacy_json(&path)?;
        let (medium, facet) = convert_scope(&name, scope);
        configs.mediums.push(MemPipelineRecord {
            mem: mem.clone(),
            name: name.clone(),
            config: medium,
        });
        configs.facets.push(MemPipelineRecord {
            mem,
            name,
            config: facet,
        });
    }

    for (mem, name, path) in walk_mem_scoped(workspace_root, "projections")? {
        let legacy: LegacyProjection = read_legacy_json(&path)?;
        configs.projections.push(MemPipelineRecord {
            mem,
            name,
            config: convert_projection(legacy),
        });
    }

    // Ingests are flat and structurally identical to the new shape.
    let ingests_dir = workspace_root.join("ingests");
    match std::fs::read_dir(&ingests_dir) {
        Ok(rd) => {
            let mut ingests = Vec::new();
            for file in rd.flatten() {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) {
                    let config: Ingest = read_legacy_json(&path)?;
                    ingests.push(PipelineRecord { name, config });
                }
            }
            ingests.sort_by(|a, b| a.name.cmp(&b.name));
            configs.ingests = ingests;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(StoreError::Io {
                path: ingests_dir,
                source: e,
            });
        }
    }

    Ok(configs)
}

/// Migrate the legacy JSON-folder configs into the workspace store: read +
/// convert via [`read_legacy_pipeline_configs`], then write each record to its
/// `.memstead/` location via [`crate::pipeline_store`]. Idempotent — the
/// writes are deterministic, so re-running reproduces identical files. Returns
/// the converted configs that were written.
///
/// Does **not** delete the legacy folders; the operator removes them when
/// ready.
pub fn migrate_legacy_pipeline(workspace_root: &Path) -> Result<PipelineConfigs, StoreError> {
    let configs = read_legacy_pipeline_configs(workspace_root)?;
    for m in &configs.mediums {
        crate::pipeline_store::write_medium(workspace_root, &m.mem, &m.name, &m.config)?;
    }
    for f in &configs.facets {
        crate::pipeline_store::write_facet(workspace_root, &f.mem, &f.name, &f.config)?;
    }
    for p in &configs.projections {
        crate::pipeline_store::write_projection(workspace_root, &p.mem, &p.name, &p.config)?;
    }
    for i in &configs.ingests {
        crate::pipeline_store::write_ingest(workspace_root, &i.name, &i.config)?;
    }
    Ok(configs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::IngestMode;

    #[test]
    fn common_dir_prefix_stops_at_first_glob() {
        let tree = vec![
            PatternEntry {
                path: "../engine/**/*.rs".into(),
                mode: PatternMode::Allow,
            },
            PatternEntry {
                path: "../engine/Cargo.lock".into(),
                mode: PatternMode::Allow,
            },
            PatternEntry {
                path: "../engine/target/**".into(),
                mode: PatternMode::Deny,
            },
        ];
        // `..`,`engine` agree; the third allow component diverges (`**` vs
        // `Cargo.lock`), so the prefix is `../engine`. The deny path is ignored.
        assert_eq!(common_dir_prefix(&tree), "../engine");
    }

    #[test]
    fn common_dir_prefix_falls_back_to_shared_ancestor() {
        let tree = vec![
            PatternEntry {
                path: "../dev/**/*.md".into(),
                mode: PatternMode::Allow,
            },
            PatternEntry {
                path: "../VISION.md".into(),
                mode: PatternMode::Allow,
            },
            PatternEntry {
                path: "../macos/VISION.md".into(),
                mode: PatternMode::Allow,
            },
        ];
        // They diverge at component 1 (`dev` / `VISION.md` / `macos`), so the
        // common prefix is just `..`.
        assert_eq!(common_dir_prefix(&tree), "..");
    }

    #[test]
    fn scope_splits_into_medium_and_facet() {
        let scope = LegacyScope {
            medium_type: MediumType::Codebase,
            label: Some("Rust engine source".into()),
            scope: LegacyScopeBody {
                tree: vec![
                    PatternEntry {
                        path: "../engine/**/*.rs".into(),
                        mode: PatternMode::Allow,
                    },
                    PatternEntry {
                        path: "../engine/target/**".into(),
                        mode: PatternMode::Deny,
                    },
                ],
            },
        };
        let (medium, facet) = convert_scope("source-tree", scope);
        assert_eq!(medium.name, "source-tree");
        assert_eq!(medium.medium_type, MediumType::Codebase);
        assert_eq!(medium.pointer, "../engine");
        assert_eq!(facet.name, "source-tree");
        assert_eq!(facet.medium, "source-tree");
        assert_eq!(facet.scope.len(), 2);
        assert_eq!(facet.preparation, None);
        assert_eq!(facet.engagement, None);
    }

    #[test]
    fn projection_sources_split_into_facets_and_reference_mems() {
        let legacy = LegacyProjection {
            intent: Some("macOS source".into()),
            sources: vec![
                LegacySource {
                    scope_ref: Some("source-tree".into()),
                    mem: None,
                },
                LegacySource {
                    scope_ref: None,
                    mem: Some("engine".into()),
                },
            ],
            destinations: vec![LegacyDestination {
                mem: "macos".into(),
            }],
        };
        let p = convert_projection(legacy);
        assert_eq!(p.source_facets, vec!["source-tree".to_string()]);
        assert_eq!(p.reference_mems, vec!["engine".to_string()]);
        assert_eq!(p.destination_mem, "macos");
        assert_eq!(p.intent.as_deref(), Some("macOS source"));
    }

    #[test]
    fn migrate_reads_legacy_tree_and_writes_store_idempotently() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        // Synthesise a legacy tree at the workspace root.
        std::fs::create_dir_all(root.join("scopes/macos")).unwrap();
        std::fs::write(
            root.join("scopes/macos/source-tree.json"),
            r#"{"type":"codebase","label":"macOS","scope":{"tree":[{"path":"../macos/**/*.swift","mode":"allow"}]}}"#,
        ).unwrap();
        std::fs::create_dir_all(root.join("projections/macos")).unwrap();
        std::fs::write(
            root.join("projections/macos/graph.json"),
            r#"{"intent":"i","sources":[{"role":"primary","scope_ref":"source-tree"},{"role":"reference","mem":"engine"}],"destinations":[{"mem":"macos"}]}"#,
        ).unwrap();
        std::fs::create_dir_all(root.join("ingests")).unwrap();
        std::fs::write(
            root.join("ingests/macos-graph.json"),
            r#"{"projection":"macos/graph","mode":"discovery","trigger":"loop","batch_size":20,"deny_paths":["dev"]}"#,
        ).unwrap();

        let converted = migrate_legacy_pipeline(root).unwrap();
        assert_eq!(converted.mediums.len(), 1);
        assert_eq!(converted.mediums[0].config.pointer, "../macos");
        assert_eq!(converted.facets.len(), 1);
        assert_eq!(
            converted.projections[0].config.reference_mems,
            vec!["engine".to_string()]
        );
        assert_eq!(converted.ingests[0].config.mode, IngestMode::Discovery);

        // Written to the `.memstead/` store and reloadable by the loader.
        let loaded = crate::pipeline_store::load_pipeline_configs(root).unwrap();
        assert_eq!(loaded, converted);

        // Idempotent: a second run reproduces identical store content.
        let again = migrate_legacy_pipeline(root).unwrap();
        assert_eq!(again, converted);
        assert_eq!(
            crate::pipeline_store::load_pipeline_configs(root).unwrap(),
            converted
        );
    }
}
