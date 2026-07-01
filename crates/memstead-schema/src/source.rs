//! Schema source-file collection for publishing mem archives.
//!
//! A published `.mem` archive is portable only if the schema it pins
//! travels inside it — otherwise opening the archive on a foreign
//! machine without the matching schema registered would fail at
//! resolve time. `collect_schema_source` resolves a `SchemaRef` to the
//! raw YAML bytes callers need to zip into the archive's `schema/`
//! tree.
//!
//! Resolution order (first match wins):
//! 1. `<workspace>/<schemas_dir>/<name>/` — workspace-level shared
//!    schemas (optional; when the caller supplies a workspace path)
//! 2. `<workspace>/.memstead.cache/schemas/<name>-<version>/` — cache
//!    extracted from a previously loaded archive (workspace-wide)
//! 3. Embedded built-in (via `include_dir!`) — ships inside the binary
//!
//! In every case the manifest's declared version is checked against the
//! pin; a name collision at the wrong version falls through rather than
//! silently embedding a mismatched schema.

use std::path::{Path, PathBuf};

use crate::builtins::builtin_schemas_dir;
use crate::config::SchemaRef;

/// One source file destined for the archive's `.memstead/schema/` tree.
///
/// `archive_path` is the relative path *inside* the archive (e.g.
/// `"schema.yaml"` or `"types/spec.yaml"`). Callers prefix it with
/// whatever root they want (the archive writer uses `".memstead/schema/"`);
/// the canonical re-pack uses the same prefix so byte-identical
/// archives round-trip.
#[derive(Debug, Clone)]
pub struct SchemaSourceFile {
    pub archive_path: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum SchemaSourceError {
    #[error(
        "schema {schema_ref} not found — candidate paths tried: [{}]",
        .candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    )]
    NotFound {
        schema_ref: String,
        /// Every filesystem location (mem-local, workspace-level,
        /// cache) consulted before falling through to the embedded
        /// builtins. Listed in resolution order so the error message
        /// matches the precedence the resolver walked.
        candidates: Vec<PathBuf>,
    },

    #[error("i/o error reading schema source at {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Two distinct on-disk schema directories share the same
    /// `<name>-<version>` cache key. Indicates a bug in the cache
    /// extraction pipeline — silent overwrite would mask the issue.
    #[error(
        "schema cache collision: '{name}-{version}' has more than one source directory ({} and {})",
        .first.display(),
        .second.display()
    )]
    CacheCollision {
        name: String,
        version: String,
        first: PathBuf,
        second: PathBuf,
    },

    #[error(
        "schema manifest at {} does not declare version '{expected}' (found '{found}')",
        .path.display()
    )]
    VersionMismatch {
        path: PathBuf,
        expected: String,
        found: String,
    },

    #[error("schema manifest at {} is malformed: {reason}", .path.display())]
    MalformedManifest { path: PathBuf, reason: String },
}

/// Resolve the schema pinned by `schema_ref` to a sorted set of source
/// files ready to embed under `.memstead/schema/` in a mem archive.
///
/// The returned vector is sorted by `archive_path` so archive bytes are
/// deterministic — callers don't need to re-sort.
///
/// Resolution order (first match wins):
/// 1. `<workspace_schemas_dir>/<name>/` (when provided)
/// 2. `<workspace_root>/.memstead.cache/schemas/<name>-<version>/`
///    (when `workspace_root` is provided)
/// 3. Embedded builtins
///
/// When every filesystem layer misses, the returned `NotFound` lists
/// every concrete path that was consulted so callers (and agents) can
/// inspect the resolution trace without re-walking the filesystem.
pub fn collect_schema_source(
    workspace_root: Option<&Path>,
    workspace_schemas_dir: Option<&Path>,
    schema_ref: &SchemaRef,
) -> Result<Vec<SchemaSourceFile>, SchemaSourceError> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(ws_dir) = workspace_schemas_dir {
        let ws_schema_dir = ws_dir.join(&schema_ref.name);
        candidates.push(ws_schema_dir.clone());
        if ws_schema_dir.is_dir()
            && let Some(files) = try_collect_dir(&ws_schema_dir, schema_ref)?
        {
            return Ok(files);
        }
    }

    if let Some(ws_root) = workspace_root {
        let cache_dir = ws_root
            .join(".memstead.cache/schemas")
            .join(format!("{}-{}", schema_ref.name, schema_ref.version));
        candidates.push(cache_dir.clone());
        if cache_dir.is_dir()
            && let Some(files) = try_collect_dir(&cache_dir, schema_ref)?
        {
            return Ok(files);
        }
    }

    if let Some(files) = collect_builtin_source(schema_ref)? {
        return Ok(files);
    }

    Err(SchemaSourceError::NotFound {
        schema_ref: schema_ref.as_display(),
        candidates,
    })
}

/// Read `<dir>/schema.yaml` + `<dir>/types/*.yaml` and return them
/// only if the manifest's declared version matches `schema_ref`.
/// A mismatched version returns `Ok(None)` so the caller can fall
/// through to the next resolution layer rather than hard-failing.
fn try_collect_dir(
    dir: &Path,
    schema_ref: &SchemaRef,
) -> Result<Option<Vec<SchemaSourceFile>>, SchemaSourceError> {
    let manifest_path = dir.join("schema.yaml");
    let manifest_bytes = std::fs::read(&manifest_path).map_err(|e| SchemaSourceError::Io {
        path: manifest_path.clone(),
        source: e,
    })?;

    if !manifest_matches(&manifest_bytes, schema_ref, &manifest_path)? {
        return Ok(None);
    }

    let mut out = vec![SchemaSourceFile {
        archive_path: "schema.yaml".to_string(),
        bytes: manifest_bytes,
    }];

    let types_dir = dir.join("types");
    if types_dir.is_dir() {
        let entries = std::fs::read_dir(&types_dir).map_err(|e| SchemaSourceError::Io {
            path: types_dir.clone(),
            source: e,
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| SchemaSourceError::Io {
                path: types_dir.clone(),
                source: e,
            })?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let bytes = std::fs::read(&path).map_err(|e| SchemaSourceError::Io {
                path: path.clone(),
                source: e,
            })?;
            out.push(SchemaSourceFile {
                archive_path: format!("types/{stem}.yaml"),
                bytes,
            });
        }
    }

    out.sort_by(|a, b| a.archive_path.cmp(&b.archive_path));
    Ok(Some(out))
}

fn collect_builtin_source(
    schema_ref: &SchemaRef,
) -> Result<Option<Vec<SchemaSourceFile>>, SchemaSourceError> {
    let Some(schema_dir) = builtin_schemas_dir().get_dir(schema_ref.name.as_str()) else {
        return Ok(None);
    };

    // `include_dir`'s paths are always relative to the include root (the
    // `builtins/schemas` dir), so every entry's path starts with the
    // schema name. Build lookups by constructing the same prefix.
    let schema_name = schema_ref.name.as_str();
    let manifest_key = format!("{schema_name}/schema.yaml");
    let manifest_file = schema_dir.get_file(manifest_key.as_str()).ok_or_else(|| {
        SchemaSourceError::MalformedManifest {
            path: PathBuf::from(&manifest_key),
            reason: "embedded schema directory is missing schema.yaml".into(),
        }
    })?;
    let manifest_bytes = manifest_file.contents().to_vec();
    if !manifest_matches(
        &manifest_bytes,
        schema_ref,
        &PathBuf::from(format!("<builtin:{schema_name}>/schema.yaml")),
    )? {
        return Ok(None);
    }

    let mut out = vec![SchemaSourceFile {
        archive_path: "schema.yaml".to_string(),
        bytes: manifest_bytes,
    }];

    let types_key = format!("{schema_name}/types");
    if let Some(types_dir) = schema_dir.get_dir(types_key.as_str()) {
        for file in types_dir.files() {
            if file.path().extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let Some(stem) = file.path().file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            out.push(SchemaSourceFile {
                archive_path: format!("types/{stem}.yaml"),
                bytes: file.contents().to_vec(),
            });
        }
    }

    out.sort_by(|a, b| a.archive_path.cmp(&b.archive_path));
    Ok(Some(out))
}

/// True when `manifest_bytes` parses and its `name`+`version` match
/// `schema_ref`. Uses `serde_yaml_ng` with a narrow intermediate struct
/// rather than the full `SchemaManifest` so callers can compare versions
/// without paying for the full manifest validation (the caller is often
/// the publish path, where the real loader runs separately over the
/// source directory for full validation).
fn manifest_matches(
    manifest_bytes: &[u8],
    schema_ref: &SchemaRef,
    source_path: &Path,
) -> Result<bool, SchemaSourceError> {
    #[derive(serde::Deserialize)]
    struct ManifestId {
        name: String,
        version: String,
    }
    let id: ManifestId = serde_yaml_ng::from_slice(manifest_bytes).map_err(|e| {
        SchemaSourceError::MalformedManifest {
            path: source_path.to_path_buf(),
            reason: e.to_string(),
        }
    })?;
    if id.name != schema_ref.name {
        return Ok(false);
    }
    let declared =
        semver::Version::parse(&id.version).map_err(|e| SchemaSourceError::MalformedManifest {
            path: source_path.to_path_buf(),
            reason: format!("invalid semver '{}': {e}", id.version),
        })?;
    if declared != schema_ref.version {
        // Surface the mismatch as a hard error only for the on-disk
        // paths that selected `dir` by name — a workspace author bumping
        // the pin without editing the manifest should hear about it
        // loudly, not get a silent fallthrough to the next layer.
        // Builtin dirs are keyed by name only (one version per builtin),
        // so the `collect_builtin_source` path turns `Ok(false)` into
        // "no match; try NotFound" at the call site.
        if source_path.starts_with("<builtin:") {
            return Ok(false);
        }
        return Err(SchemaSourceError::VersionMismatch {
            path: source_path.to_path_buf(),
            expected: schema_ref.version.to_string(),
            found: declared.to_string(),
        });
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_schema(dir: &Path, name: &str, version: &str, types: &[&str]) {
        let manifest = format!(
            r#"name: {name}
version: {version}
description: test
when_to_use: test
types:
  - {type_list}
relationships:
  mode: strict
  definitions:
    - name: _default
      description: default
      default_weight: 1.0
    - name: PART_OF
      description: hier
      default_weight: 3.0
community:
  resolution: 1.0
  seed: 42
"#,
            name = name,
            version = version,
            type_list = types.join("\n  - "),
        );
        std::fs::write(dir.join("schema.yaml"), manifest).unwrap();
        for t in types {
            let td = format!(
                r#"name: {t}
description: test
when_to_use: test
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
metadata_fields: []
title_weight: 1.0
text_fields: [body]
hierarchy_relationship: PART_OF
propagating_relationships: []
updatable_fields: [title, body]
health_required_fields: [body]
staleness_threshold_days: 30
write_rules: []
"#
            );
            std::fs::write(dir.join(format!("types/{t}.yaml")), td).unwrap();
        }
    }

    #[test]
    fn collects_builtin_default_source() {
        let schema_ref = SchemaRef::new("default", semver::Version::new(1, 0, 0));
        let files = collect_schema_source(None, None, &schema_ref).unwrap();

        assert!(
            files.iter().any(|f| f.archive_path == "schema.yaml"),
            "embedded builtin must expose schema.yaml"
        );
        let type_count = files
            .iter()
            .filter(|f| f.archive_path.starts_with("types/"))
            .count();
        assert_eq!(type_count, 10, "default schema has 10 types");
        for pair in files.windows(2) {
            assert!(pair[0].archive_path < pair[1].archive_path, "sorted");
        }
    }

    #[test]
    fn workspace_schema_wins_over_builtin() {
        let tmp = TempDir::new().unwrap();
        let ws_dir = tmp.path().join("schemas");
        let schema_dir = ws_dir.join("default");
        std::fs::create_dir_all(schema_dir.join("types")).unwrap();
        // Override builtin default with a skeletal variant. collect_schema_source
        // must return this, not the 10-type builtin.
        write_schema(&schema_dir, "default", "1.0.0", &["spec"]);
        let schema_ref = SchemaRef::new("default", semver::Version::new(1, 0, 0));
        let files =
            collect_schema_source(Some(tmp.path()), Some(&ws_dir), &schema_ref).unwrap();
        let type_count = files
            .iter()
            .filter(|f| f.archive_path.starts_with("types/"))
            .count();
        assert_eq!(type_count, 1, "workspace override takes priority");
    }

    #[test]
    fn workspace_mismatched_version_errors() {
        let tmp = TempDir::new().unwrap();
        let ws_dir = tmp.path().join("schemas");
        let schema_dir = ws_dir.join("recipe");
        std::fs::create_dir_all(schema_dir.join("types")).unwrap();
        write_schema(&schema_dir, "recipe", "1.0.0", &["spec"]);
        let schema_ref = SchemaRef::new("recipe", semver::Version::new(2, 0, 0));
        let err =
            collect_schema_source(Some(tmp.path()), Some(&ws_dir), &schema_ref).unwrap_err();
        assert!(matches!(err, SchemaSourceError::VersionMismatch { .. }));
    }

    #[test]
    fn cache_schema_resolves_when_workspace_layer_absent() {
        let tmp = TempDir::new().unwrap();
        // Cache dir encodes version in the folder name: `<name>-<version>`.
        let dir = tmp.path().join(".memstead.cache/schemas/recipe-1.0.0");
        std::fs::create_dir_all(dir.join("types")).unwrap();
        write_schema(&dir, "recipe", "1.0.0", &["spec"]);
        let schema_ref = SchemaRef::new("recipe", semver::Version::new(1, 0, 0));
        let files = collect_schema_source(Some(tmp.path()), None, &schema_ref).unwrap();
        assert!(files.iter().any(|f| f.archive_path == "schema.yaml"));
    }

    #[test]
    fn unknown_schema_returns_not_found() {
        let schema_ref = SchemaRef::new("nonexistent", semver::Version::new(1, 0, 0));
        let err = collect_schema_source(None, None, &schema_ref).unwrap_err();
        assert!(matches!(err, SchemaSourceError::NotFound { .. }));
    }

    #[test]
    fn workspace_schema_wins_over_cache() {
        let tmp = TempDir::new().unwrap();
        // Cache layer carries a 2-type variant.
        let cache_dir = tmp.path().join(".memstead.cache/schemas/software-1.0.0");
        std::fs::create_dir_all(cache_dir.join("types")).unwrap();
        write_schema(&cache_dir, "software", "1.0.0", &["spec", "memo"]);
        // Workspace layer carries a 1-type variant — must win.
        let ws_dir = tmp.path().join("schemas");
        let ws_schema = ws_dir.join("software");
        std::fs::create_dir_all(ws_schema.join("types")).unwrap();
        write_schema(&ws_schema, "software", "1.0.0", &["spec"]);

        let schema_ref = SchemaRef::new("software", semver::Version::new(1, 0, 0));
        let files =
            collect_schema_source(Some(tmp.path()), Some(&ws_dir), &schema_ref).unwrap();
        let type_count = files
            .iter()
            .filter(|f| f.archive_path.starts_with("types/"))
            .count();
        assert_eq!(type_count, 1, "workspace layer must win over cache");
    }

    #[test]
    fn not_found_lists_every_candidate_path() {
        let tmp = TempDir::new().unwrap();
        let ws_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&ws_dir).unwrap();

        let schema_ref = SchemaRef::new("missing", semver::Version::new(2, 3, 4));
        let err = collect_schema_source(Some(tmp.path()), Some(&ws_dir), &schema_ref)
            .unwrap_err();
        match err {
            SchemaSourceError::NotFound {
                schema_ref: name,
                candidates,
            } => {
                assert_eq!(name, "missing@2.3.4");
                // Both filesystem candidates listed in resolution order:
                // workspace schemas dir, then workspace cache dir.
                assert_eq!(candidates.len(), 2);
                assert!(candidates[0].ends_with("schemas/missing"));
                assert!(candidates[1]
                    .to_string_lossy()
                    .contains(".memstead.cache/schemas/missing-2.3.4"));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
