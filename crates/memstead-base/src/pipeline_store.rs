//! File-adapter persistence for the binding store.
//!
//! The store is one record kind: a v2 [`Binding`] per pipeline at
//! `<root>/.memstead/projections/<mem>/<name>.json`, read version-gated by
//! [`load_pipeline_configs`]. The record's identity is `(mem, name)` derived
//! from the file path; `name` is the file stem.
//!
//! The loader's job is load + validate + expose read-only: a malformed file
//! quarantines its binding with a typed [`StoreError::Parse`] naming the
//! path rather than being silently skipped. A file in a retired binding
//! format (version-less gen-2, or `version: 1`) quarantines with
//! [`StoreError::LegacyProjectionStore`] (`PROJECTION_STORE_LEGACY`): the
//! engine no longer converts those formats, the binding is re-authored
//! with `memstead projection init`.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::binding::Binding;
use crate::workspace_store::{StoreError, WORKSPACE_STORE_DIR};

/// The subdirectory under the workspace store that holds the bindings.
pub const PROJECTIONS_DIR: &str = "projections";

/// Every directory the workspace store keeps per mem, relative to
/// `.memstead/`: the binding records and the per-binding state the
/// maintenance loop writes (findings, advance). A mem's lifecycle walks
/// this list: a rename moves each, a destructive delete removes each,
/// so a state kind added later joins both by joining the list.
pub const PER_MEM_STORE_DIRS: &[&str] = &[PROJECTIONS_DIR, "state/findings", "state/advance"];

/// The binding records on disk for `mem`, as `(name, bytes)` in name
/// order: what the schema-and-config ref's mirror holds for the mem
/// when the two agree. Empty when the mem has no records.
pub fn mem_binding_records(
    workspace_root: &Path,
    mem: &str,
) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
    // A mem whose name is not a single component (a hierarchical
    // `team/sub`) has no store directory: the store keys by one
    // component, so there is nothing for it here.
    if validate_component("mem", mem).is_err() {
        return Ok(Vec::new());
    }
    let dir = primitive_dir(workspace_root, PROJECTIONS_DIR).join(mem);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| StoreError::Io {
        path: dir.clone(),
        source: e,
    })? {
        let path = entry
            .map_err(|e| StoreError::Io {
                path: dir.clone(),
                source: e,
            })?
            .path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let bytes = std::fs::read(&path).map_err(|e| StoreError::Io {
            path: path.clone(),
            source: e,
        })?;
        out.push((name.to_string(), bytes));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Move every per-mem store directory of `old` under `new` and rewrite
/// every id inside that names the mem: the binding's `destination_mem`,
/// and every `<old>/…` binding or artifact id and `<old>--…` entity id
/// in the records and the state files, keys and values alike.
/// Idempotent: a directory already moved is rewritten in place, so an
/// interrupted rename completes on the next call. Returns the binding
/// records under the new name, for the caller to mirror.
pub fn relocate_mem_store(
    workspace_root: &Path,
    old: &str,
    new: &str,
) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
    if validate_component("mem", old).is_err() || validate_component("mem", new).is_err() {
        return Ok(Vec::new());
    }
    for dir in PER_MEM_STORE_DIRS {
        let old_dir = primitive_dir(workspace_root, dir).join(old);
        let new_dir = primitive_dir(workspace_root, dir).join(new);
        if old_dir.is_dir() && !new_dir.exists() {
            if let Some(parent) = new_dir.parent() {
                std::fs::create_dir_all(parent).map_err(|e| StoreError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
            }
            std::fs::rename(&old_dir, &new_dir).map_err(|e| StoreError::Io {
                path: old_dir.clone(),
                source: e,
            })?;
        }
        if !new_dir.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&new_dir).map_err(|e| StoreError::Io {
            path: new_dir.clone(),
            source: e,
        })? {
            let path = entry
                .map_err(|e| StoreError::Io {
                    path: new_dir.clone(),
                    source: e,
                })?
                .path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(|e| StoreError::Io {
                path: path.clone(),
                source: e,
            })?;
            let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(&text) else {
                // A file this engine cannot parse is left as it is:
                // the store's own loader quarantines it with its reason.
                continue;
            };
            let mut changed = rewrite_mem_ids(&mut doc, old, new);
            if *dir == PROJECTIONS_DIR
                && doc.get("destination_mem").and_then(|v| v.as_str()) == Some(old)
            {
                doc["destination_mem"] = serde_json::Value::String(new.to_string());
                changed = true;
            }
            if changed {
                let mut out = serde_json::to_string_pretty(&doc).unwrap_or(text);
                out.push('\n');
                std::fs::write(&path, out).map_err(|e| StoreError::Io {
                    path: path.clone(),
                    source: e,
                })?;
            }
        }
    }
    mem_binding_records(workspace_root, new)
}

/// Remove every per-mem store directory of `mem`: its binding records
/// and its per-binding state. A directory that is not there is not an
/// error, so a repeated call is a no-op.
pub fn remove_mem_store(workspace_root: &Path, mem: &str) -> Result<(), StoreError> {
    if validate_component("mem", mem).is_err() {
        return Ok(());
    }
    for dir in PER_MEM_STORE_DIRS {
        let path = primitive_dir(workspace_root, dir).join(mem);
        if path.is_dir() {
            std::fs::remove_dir_all(&path).map_err(|e| StoreError::Io {
                path: path.clone(),
                source: e,
            })?;
        }
    }
    Ok(())
}

/// Rewrite every string in `value` (object keys included) that names
/// the mem `old` as an id prefix, `<old>/…` (a binding or artifact id)
/// or `<old>--…` (an entity id), to name `new`. Returns whether
/// anything changed.
fn rewrite_mem_ids(value: &mut serde_json::Value, old: &str, new: &str) -> bool {
    let slash_old = format!("{old}/");
    let dash_old = format!("{old}--");
    fn rewrite_str(s: &str, slash_old: &str, dash_old: &str, new: &str) -> Option<String> {
        if let Some(rest) = s.strip_prefix(slash_old) {
            return Some(format!("{new}/{rest}"));
        }
        if let Some(rest) = s.strip_prefix(dash_old) {
            return Some(format!("{new}--{rest}"));
        }
        None
    }
    match value {
        serde_json::Value::String(s) => match rewrite_str(s, &slash_old, &dash_old, new) {
            Some(r) => {
                *s = r;
                true
            }
            None => false,
        },
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= rewrite_mem_ids(item, old, new);
            }
            changed
        }
        serde_json::Value::Object(map) => {
            let mut changed = false;
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                let mut inner = map.remove(&key).expect("key present");
                changed |= rewrite_mem_ids(&mut inner, old, new);
                let new_key = match rewrite_str(&key, &slash_old, &dash_old, new) {
                    Some(r) => {
                        changed = true;
                        r
                    }
                    None => key,
                };
                map.insert(new_key, inner);
            }
            changed
        }
        _ => false,
    }
}

/// A per-mem binding record paired with the mem and name (file stem) that
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

/// Every binding in a workspace store, in the **v2 single-record** shape:
/// one [`Binding`] per pipeline, nothing else. This is what
/// [`load_pipeline_configs`] returns and the brief / selection / cursor
/// paths consume. Canonical id is `<mem>/<stem>`.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct BindingConfigs {
    /// Per-mem v2 bindings under the `projections/<mem>/<name>.json` tier.
    pub bindings: Vec<MemPipelineRecord<Binding>>,
    /// Bindings whose stored file failed the version gate or parse —
    /// quarantined instead of failing the whole load (degrade, never
    /// disappear). A quarantined binding serves
    /// no operations: resolution sites refuse typed with the entry's
    /// reason (a retired-format file names `memstead projection init`
    /// as the way back). Mems and healthy bindings serve normally.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quarantined: Vec<QuarantinedBinding>,
}

/// One quarantined binding: the store file that failed the v2
/// version gate or parse, with its typed reason.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QuarantinedBinding {
    /// Destination mem (the `projections/<mem>/` tier).
    pub mem: String,
    /// Binding name (`<name>.json`).
    pub name: String,
    /// Offending file path.
    pub path: String,
    /// Typed reason code (`PROJECTION_STORE_LEGACY`,
    /// `UNKNOWN_BINDING_VERSION`, `WORKSPACE_STORE_PARSE`).
    pub reason_code: String,
    /// Full reason message.
    pub reason_message: String,
    /// Reconstruction payload for [`load_pipeline_configs_strict`]:
    /// the declared version on an `UNKNOWN_BINDING_VERSION` entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_version: Option<i64>,
    /// Reconstruction payload: the serde message on a
    /// `WORKSPACE_STORE_PARSE` entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse_message: Option<String>,
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
/// (CLI, MCP, engine embedders) can bypass it.
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

/// Write a v2 binding to `<root>/.memstead/projections/<mem>/<name>.json`,
/// overwriting an existing file of that identity.
pub fn write_binding(
    workspace_root: &Path,
    mem: &str,
    name: &str,
    binding: &Binding,
) -> Result<(), StoreError> {
    write_json(
        &mem_scoped_path(workspace_root, PROJECTIONS_DIR, mem, name)?,
        binding,
    )
}

/// Read the v2 binding at `<root>/.memstead/projections/<mem>/<name>.json`.
///
/// The read counterpart of [`write_binding`] — reads the *same* per-mem
/// projections tier and file identity, parsed as a [`Binding`]. A missing
/// file surfaces [`StoreError::Io`] (kind `NotFound`); a file present but not
/// a v2 binding surfaces [`StoreError::Parse`]. Callers wanting a friendly "no such binding"
/// message pre-check existence and keep the two apart.
pub fn read_binding(workspace_root: &Path, mem: &str, name: &str) -> Result<Binding, StoreError> {
    read_json(&mem_scoped_path(
        workspace_root,
        PROJECTIONS_DIR,
        mem,
        name,
    )?)
}

/// Delete a binding file. Missing → [`StoreError::Io`]; callers that want a
/// friendly "no such binding" pre-check existence via [`load_pipeline_configs`].
pub fn delete_projection(workspace_root: &Path, mem: &str, name: &str) -> Result<(), StoreError> {
    remove_file(&mem_scoped_path(
        workspace_root,
        PROJECTIONS_DIR,
        mem,
        name,
    )?)
}

/// Rename a binding within its mem (`old` → `new`, same `<mem>` tier).
/// Refuses to clobber an existing target. A binding has no embedded name,
/// so a file move is its whole rename.
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

/// Load every `projections/<mem>/<name>.json` as a **v2 binding**,
/// version-gated. Absent directory → empty. For each file:
///
/// - no `version` field (gen-2 projection) or `version: 1` (three-file-store
///   binding) → [`StoreError::LegacyProjectionStore`] — a retired format
///   the loader never serves and the engine no longer converts;
/// - any other `version` but not `2` → [`StoreError::UnknownBindingVersion`];
/// - `version: 2` → parsed as [`Binding`] (a malformed operations block etc.
///   surfaces [`StoreError::Parse`] naming the file).
///
/// An offending file quarantines its own binding; the rest of the store
/// loads.
fn load_bindings(
    workspace_root: &Path,
) -> Result<(Vec<MemPipelineRecord<Binding>>, Vec<QuarantinedBinding>), StoreError> {
    let dir = primitive_dir(workspace_root, PROJECTIONS_DIR);
    let mut out: Vec<MemPipelineRecord<Binding>> = Vec::new();
    let mut quarantined: Vec<QuarantinedBinding> = Vec::new();
    let mem_dirs = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((out, quarantined)),
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
            // Peek at `version` before committing to the Binding shape so a
            // pre-v2 file yields the retired-format error rather than an
            // opaque "missing field" parse error. Version-less (gen-2) and
            // v1 (three-file store) are known retired formats; anything
            // else non-2 is unknown.
            // A failing file QUARANTINES that binding instead of
            // failing the whole load (degrade, never disappear):
            // the typed judgment is unchanged,
            // the blast radius shrinks to the one binding.
            let path_display = path.display().to_string();
            let quarantine =
                |q: &mut Vec<QuarantinedBinding>, mem: &str, name: String, e: StoreError| {
                    let (unknown_version, parse_message) = match &e {
                        StoreError::UnknownBindingVersion { version, .. } => (Some(*version), None),
                        StoreError::Parse { message, .. } => (None, Some(message.clone())),
                        _ => (None, None),
                    };
                    q.push(QuarantinedBinding {
                        mem: mem.to_string(),
                        name,
                        path: path_display.clone(),
                        reason_code: e.code().to_string(),
                        reason_message: e.to_string(),
                        unknown_version,
                        parse_message,
                    });
                };
            let value: serde_json::Value = match read_json(&path) {
                Ok(v) => v,
                Err(e) => {
                    quarantine(&mut quarantined, &mem, name, e);
                    continue;
                }
            };
            let gate_err = match value.get("version") {
                None => Some(StoreError::LegacyProjectionStore { path: path.clone() }),
                Some(v) => match v.as_i64() {
                    Some(1) => Some(StoreError::LegacyProjectionStore { path: path.clone() }),
                    n if n != Some(i64::from(crate::binding::BINDING_VERSION)) => {
                        Some(StoreError::UnknownBindingVersion {
                            path: path.clone(),
                            version: n.unwrap_or(-1),
                        })
                    }
                    _ => None,
                },
            };
            if let Some(e) = gate_err {
                quarantine(&mut quarantined, &mem, name, e);
                continue;
            }
            let config: Binding = match serde_json::from_value(value) {
                Ok(c) => c,
                Err(e) => {
                    quarantine(
                        &mut quarantined,
                        &mem,
                        name,
                        StoreError::Parse {
                            path: path.clone(),
                            message: e.to_string(),
                        },
                    );
                    continue;
                }
            };
            out.push(MemPipelineRecord {
                mem: mem.clone(),
                name,
                config,
            });
        }
    }
    out.sort_by(|a, b| (a.mem.as_str(), a.name.as_str()).cmp(&(b.mem.as_str(), b.name.as_str())));
    quarantined
        .sort_by(|a, b| (a.mem.as_str(), a.name.as_str()).cmp(&(b.mem.as_str(), b.name.as_str())));
    Ok((out, quarantined))
}

/// Load the **v2 single-record** store from the workspace: `projections/`
/// read as version-gated v2 bindings via [`load_bindings`] — a pre-v2 file
/// (version-less gen-2, or v1 three-file store) quarantines with
/// [`StoreError::LegacyProjectionStore`]. Nothing outside `projections/`
/// is read (a v2 binding carries everything inline).
pub fn load_pipeline_configs(workspace_root: &Path) -> Result<BindingConfigs, StoreError> {
    let (bindings, quarantined) = load_bindings(workspace_root)?;
    Ok(BindingConfigs {
        bindings,
        quarantined,
    })
}

/// Strict form for WRITE paths (the pipeline-edit layer): a store
/// carrying ANY quarantined binding refuses with the first entry's
/// underlying [`StoreError`] — the edit layer never writes over a
/// store that holds a retired-format or corrupt file.
/// Read/serve paths use the quarantining [`load_pipeline_configs`].
pub fn load_pipeline_configs_strict(workspace_root: &Path) -> Result<BindingConfigs, StoreError> {
    let configs = load_pipeline_configs(workspace_root)?;
    if let Some(q) = configs.quarantined.first() {
        let path = PathBuf::from(&q.path);
        return Err(match q.reason_code.as_str() {
            "UNKNOWN_BINDING_VERSION" => StoreError::UnknownBindingVersion {
                path,
                version: q.unknown_version.unwrap_or(-1),
            },
            "WORKSPACE_STORE_PARSE" => StoreError::Parse {
                path,
                message: q.parse_message.clone().unwrap_or_default(),
            },
            _ => StoreError::LegacyProjectionStore { path },
        });
    }
    Ok(configs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{MediumType, PatternEntry, PatternMode};
    use tempfile::TempDir;

    #[test]
    fn mutations_refuse_traversal_in_mem_and_name() {
        // Every mutation path-builds from caller-supplied mem/name; a
        // separator or traversal segment must refuse with a typed error
        // and leave nothing on disk outside the store.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let binding = sample_binding();

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
                write_binding(root, evil, "ok", &binding).is_err(),
                "mem '{}' must refuse",
                evil.escape_default()
            );
            assert!(
                write_binding(root, "ok", evil, &binding).is_err(),
                "name '{}' must refuse",
                evil.escape_default()
            );
            assert!(delete_projection(root, evil, "ok").is_err());
            assert!(delete_projection(root, "ok", evil).is_err());
            assert!(rename_projection(root, evil, "a", "b").is_err());
            assert!(rename_projection(root, "ok", evil, "b").is_err());
            assert!(rename_projection(root, "ok", "a", evil).is_err());
        }

        // A traversal write must not have escaped: the only thing under
        // the temp root may be the (empty) store dir, and the parent of
        // the temp root gained no `escape.json`.
        assert!(
            !root.parent().unwrap().join("escape.json").exists(),
            "no write may land outside the workspace"
        );

        // Existing valid names keep working.
        write_binding(root, "engine", "graph", &binding).unwrap();
        assert!(
            root.join(".memstead/projections/engine/graph.json")
                .is_file()
        );
    }

    #[test]
    fn load_enumeration_is_sorted_and_per_mem() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let binding = sample_binding();
        write_binding(root, "engine", "z-binding", &binding).unwrap();
        write_binding(root, "engine", "a-binding", &binding).unwrap();
        write_binding(root, "macos", "m-binding", &binding).unwrap();

        let configs = load_pipeline_configs(root).unwrap();
        let keys: Vec<_> = configs
            .bindings
            .iter()
            .map(|r| (r.mem.as_str(), r.name.as_str()))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("engine", "a-binding"),
                ("engine", "z-binding"),
                ("macos", "m-binding"),
            ]
        );
    }

    #[test]
    fn malformed_config_surfaces_typed_parse_error_naming_the_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let bad = root.join(".memstead/projections/macos");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("broken.json"), b"{ not valid json").unwrap();

        let configs = load_pipeline_configs(root).unwrap();
        assert_eq!(configs.quarantined.len(), 1);
        assert_eq!(configs.quarantined[0].reason_code, "WORKSPACE_STORE_PARSE");
        let err = load_pipeline_configs_strict(root).unwrap_err();
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
        write_binding(root, "engine", "graph", &sample_binding()).unwrap();

        delete_projection(root, "engine", "graph").unwrap();

        assert!(
            !root
                .join(".memstead/projections/engine/graph.json")
                .exists()
        );
        let configs = load_pipeline_configs(root).unwrap();
        assert!(configs.bindings.is_empty());
    }

    #[test]
    fn delete_of_missing_record_surfaces_io_error() {
        let tmp = TempDir::new().unwrap();
        let err = delete_projection(tmp.path(), "engine", "nope").unwrap_err();
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
        let binding = sample_binding();
        write_binding(root, "engine", "old-name", &binding).unwrap();

        rename_projection(root, "engine", "old-name", "new-name").unwrap();

        assert!(
            !root
                .join(".memstead/projections/engine/old-name.json")
                .exists()
        );
        let configs = load_pipeline_configs(root).unwrap();
        assert_eq!(configs.bindings.len(), 1);
        assert_eq!(configs.bindings[0].name, "new-name");
        assert_eq!(configs.bindings[0].config, binding);
    }

    #[test]
    fn rename_refuses_to_clobber_an_existing_target() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let binding = sample_binding();
        write_binding(root, "engine", "a", &binding).unwrap();
        write_binding(root, "engine", "b", &binding).unwrap();

        let err = rename_projection(root, "engine", "a", "b").unwrap_err();
        assert!(matches!(err, StoreError::Other(_)), "got {err:?}");
        // Both records survive — nothing was lost.
        assert!(root.join(".memstead/projections/engine/a.json").exists());
        assert!(root.join(".memstead/projections/engine/b.json").exists());
    }

    #[test]
    fn rename_of_missing_source_surfaces_io_error() {
        let tmp = TempDir::new().unwrap();
        let err = rename_projection(tmp.path(), "engine", "missing", "whatever").unwrap_err();
        assert!(matches!(err, StoreError::Io { .. }), "got {err:?}");
    }

    // ── binding v2 loader (version gate) ─────────────────────────────────

    fn sample_binding() -> Binding {
        use crate::binding::{BINDING_VERSION, BuildMode, BuildOperation, Operations};
        use crate::pipeline::{IngestTrigger, Source};
        Binding {
            version: BINDING_VERSION,
            intent: Some("prose".to_string()),
            sources: vec![Source {
                name: "source-tree".to_string(),
                medium_type: MediumType::Codebase,
                pointer: "../public".to_string(),
                change_detection: None,
                scope: vec![PatternEntry {
                    path: "../public/**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            }],
            reference_mems: vec![],
            destination_mem: "engine".to_string(),
            deny_paths: vec![],
            coverage_semantics: None,
            rules: None,
            prune: None,
            operations: Operations {
                build: Some(BuildOperation {
                    mode: BuildMode::Discovery,
                    trigger: IngestTrigger::Loop,
                    batch_size: 20,
                    post_actions: None,
                }),
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
    fn binding_loader_round_trips_a_v2_binding() {
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
    fn version_less_projection_refuses_as_retired_format() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // A gen-2 (version-less) projection file, written raw: the engine
        // has no type for this shape any more.
        let dir = root.join(".memstead/projections/engine");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("graph.json"),
            br#"{"intent": "legacy", "source_facets": ["f"], "reference_mems": [], "destination_mem": "engine"}"#,
        )
        .unwrap();

        let err = load_pipeline_configs_strict(root).unwrap_err();
        match err {
            StoreError::LegacyProjectionStore { path } => {
                assert!(path.ends_with("graph.json"), "got {path:?}");
                let msg = err_message(&StoreError::LegacyProjectionStore { path });
                assert!(msg.contains("retired binding format"), "got {msg}");
                assert!(msg.contains("memstead projection init"), "got {msg}");
                assert!(!msg.contains("projection migrate"), "got {msg}");
            }
            other => panic!("expected LegacyProjectionStore, got {other:?}"),
        }
    }

    /// REFUSAL: a v1 (three-file-store) binding is a known
    /// retired format: the loader never reads it, surfacing the typed
    /// refusal instead of parsing or reinterpreting.
    #[test]
    fn v1_binding_refuses_as_retired_format() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let dir = root.join(".memstead/projections/engine");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("graph.json"),
            br#"{"version": 1, "source_facets": ["source-tree"], "destination_mem": "engine", "operations": {"build": {"mode": "discovery", "trigger": "loop", "batch_size": 20}}}"#,
        )
        .unwrap();

        let err = load_pipeline_configs_strict(root).unwrap_err();
        match err {
            StoreError::LegacyProjectionStore { path } => {
                assert!(path.ends_with("graph.json"), "got {path:?}");
                let msg = err_message(&StoreError::LegacyProjectionStore { path });
                assert!(msg.contains("retired binding format"), "got {msg}");
                assert!(msg.contains("memstead projection init"), "got {msg}");
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

        let err = load_pipeline_configs_strict(root).unwrap_err();
        assert!(
            matches!(err, StoreError::UnknownBindingVersion { version: 99, .. }),
            "got {err:?}"
        );
    }

    fn err_message(e: &StoreError) -> String {
        e.to_string()
    }
}
