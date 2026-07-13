//! Workspace-shape `.memstead/config.json` for filesystem mems.
//!
//! Distinct from the archive-shape config in
//! [`super::super::validator::config`]: the workspace shape adds a
//! `deps` list (cross-mem dependencies on registry-published mems)
//! and pins `format` to a different version namespace so a workspace
//! file accidentally fed to the archive validator (or vice versa)
//! surfaces as a typed mismatch rather than a generic serde error.
//!
//! ## Shape (as written to disk)
//!
//! ```json
//! {
//!   "format": 1,
//!   "schema": "default@1.0.0",
//!   "deps": ["anthropic/core"]
//! }
//! ```
//!
//! - `format`: workspace-config format integer. Bumped on breaking
//!   shape changes; current = [`FILESYSTEM_WORKSPACE_FORMAT`].
//! - `name`: mem slug — **path-derived**, not persisted. Identity of
//!   record is the mounts roster (`state/mounts.json`); on read an absent
//!   `name` is filled from the workspace-root basename. The schema
//!   validator tombstones a stray `name` in `config.json`.
//! - `schema`: mem schema pin in exact `<name>@<version>` form
//!   (e.g. `"default@1.0.0"`) — bare-name pins are rejected at parse.
//! - `deps`: cross-mem dependencies, each in `scope/name` form. The
//!   list ordering is preserved on round-trip; duplicates are
//!   rejected at parse time.
//! - `version`, `description`, `authors`: optional fields used by
//!   `memstead publish` to populate the archive shape. Carried through
//!   so the workspace remains the source of truth for publish
//!   metadata.
//!
//! ## Publish projection
//!
//! [`Self::to_published`] converts the workspace shape to a strict
//! [`PublishedMemConfig`] for `memstead publish`, dropping `deps` and
//! enforcing the archive's stricter requirements (versioned schema,
//! present `version` field). Errors surface via
//! [`PublishConversionError`] from `memstead-schema`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use memstead_schema::{
    PUBLISHED_MEM_FORMAT, PublishConversionError, PublishedMemConfig, SchemaRef,
};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Format integer for the filesystem workspace `.memstead/config.json`. Bumped
/// on breaking shape changes; current consumers (`memstead init`,
/// `memstead link`, `memstead publish`, the filesystem engine) all check this
/// before parsing the rest. Distinct from
/// [`memstead_schema::PUBLISHED_MEM_FORMAT`] so a misfiled archive
/// config inside a workspace surfaces as
/// [`WorkspaceConfigError::UnsupportedFormat`].
pub const FILESYSTEM_WORKSPACE_FORMAT: u32 = 1;

/// Cross-mem dependency entry. Mirrors the Tier 3 wiki-link
/// addressing scheme `[[scope/name:slug]]` and the `memstead link
/// <scope>/<name>` CLI shorthand.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DepRef {
    pub scope: String,
    pub name: String,
}

impl DepRef {
    /// Round-trip display form used in `deps`: `<scope>/<name>`.
    pub fn as_display(&self) -> String {
        format!("{}/{}", self.scope, self.name)
    }
}

impl std::fmt::Display for DepRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_display())
    }
}

impl std::str::FromStr for DepRef {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("dep ref must not be empty (expected \"scope/name\")".into());
        }
        let (scope, name) = trimmed
            .split_once('/')
            .ok_or_else(|| format!("dep ref '{trimmed}' must be in 'scope/name' form"))?;
        check_slug(scope, "scope")?;
        check_slug(name, "name")?;
        Ok(DepRef {
            scope: scope.to_string(),
            name: name.to_string(),
        })
    }
}

impl Serialize for DepRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_display())
    }
}

impl<'de> Deserialize<'de> for DepRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse::<DepRef>().map_err(serde::de::Error::custom)
    }
}

/// Workspace-shape `.memstead/config.json` for a filesystem mem. Distinct
/// from [`PublishedMemConfig`] (the archive shape).
///
/// Unknown fields are preserved, not refused: the engine's own runtime
/// machinery writes fields this struct does not model (`syncState` from
/// the projection sync baseline, `writeGuidance` from per-mem guidance
/// additions), and a strict reader would both break export for any
/// projection-maintained mem and drop those fields on rewrite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceConfig {
    /// Format integer. Always [`FILESYSTEM_WORKSPACE_FORMAT`] on
    /// successful load.
    pub format: u32,
    /// Mem slug — path-derived under the unified layout. The mem's identity
    /// of record lives in the mounts roster (`state/mounts.json`); the engine no
    /// longer writes `name` into `config.json` (the schema validator tombstones a
    /// stray `name`), so it is omitted on serialize and, when absent on read,
    /// filled from the workspace-root basename by `read_workspace_config`.
    #[serde(default, skip_serializing)]
    pub name: String,
    /// Schema pin. Exact `<name>@<version>` only — bare-name pins are
    /// rejected at parse.
    pub schema: SchemaRef,
    /// Mem version, used by `memstead publish`. Optional in the
    /// workspace shape; required when publishing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<semver::Version>,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional author list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    /// Cross-mem dependencies. Order preserved; duplicates rejected
    /// at parse time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<DepRef>,
    /// Engine-owned runtime fields this shape does not model
    /// (`syncState`, `writeGuidance`, …) — carried verbatim so a
    /// read-modify-write round-trip never destroys them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl WorkspaceConfig {
    /// Build a fresh workspace config with `format` set to
    /// [`FILESYSTEM_WORKSPACE_FORMAT`], the engine default `version`
    /// (`0.1.0`), and an empty deps list. Convenience for `memstead init`.
    /// F1: every mem carries a populated `version` from creation
    /// onward — operators bump via `memstead mem set-version` before
    /// publishing.
    pub fn new(name: impl Into<String>, schema: SchemaRef) -> Self {
        Self {
            format: FILESYSTEM_WORKSPACE_FORMAT,
            name: name.into(),
            schema,
            version: Some(semver::Version::new(0, 1, 0)),
            description: None,
            authors: None,
            deps: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }

    /// Add a dep entry to the list. Idempotent — re-adding an existing
    /// dep is a no-op so `memstead link` can be invoked twice in a row
    /// without producing a duplicate. Returns `true` when the entry
    /// was newly added, `false` when it was already present.
    pub fn add_dep(&mut self, dep: DepRef) -> bool {
        if self.deps.iter().any(|d| d == &dep) {
            return false;
        }
        self.deps.push(dep);
        true
    }

    /// Project the workspace config to the strict archive shape used
    /// by sealed `.mem` archives. Drops `deps` and applies the same
    /// requirements as [`memstead_schema::published_config_from`]
    /// (versioned schema, present `version`).
    pub fn to_published(&self) -> Result<PublishedMemConfig, PublishConversionError> {
        let version = self
            .version
            .clone()
            .ok_or(PublishConversionError::MissingVersion)?;
        Ok(PublishedMemConfig {
            format: PUBLISHED_MEM_FORMAT,
            name: self.name.clone(),
            version,
            description: self.description.clone(),
            authors: self.authors.clone(),
            schema: self.schema.clone(),
        })
    }
}

/// Errors surfaced by [`WorkspaceConfig`] load + parse.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceConfigError {
    #[error("workspace config not found at {0}")]
    NotFound(PathBuf),
    #[error("workspace config io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("workspace config malformed: {0}")]
    Malformed(String),
    #[error(
        "workspace config format {got} is not supported (expected {expected}) — \
         re-run `memstead init` against a fresh folder"
    )]
    UnsupportedFormat { got: u32, expected: u32 },
    #[error("workspace config invalid name: {0}")]
    InvalidName(String),
    #[error("workspace config has duplicate dep: {0}")]
    DuplicateDep(String),
}

fn name_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$").unwrap())
}

fn check_slug(value: &str, label: &str) -> Result<(), String> {
    if !name_regex().is_match(value) {
        return Err(format!(
            "{label} {value:?} must match ^[a-z0-9][a-z0-9-]{{0,62}}[a-z0-9]$"
        ));
    }
    Ok(())
}

/// Validate a mem name against the slug shape. Since the name is now
/// path-derived (no longer persisted in `config.json`), callers that accept a
/// mem name as input — e.g. `memstead init --name` — validate it here at the
/// boundary instead of relying on a config round-trip to reject a bad value.
pub fn validate_mem_name(name: &str) -> Result<(), String> {
    check_slug(name, "name")
}

/// Conventional path of the workspace config inside a workspace root.
pub fn config_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(crate::mem::MEM_META_DIR)
        .join("config.json")
}

/// Parse workspace config bytes, enforcing the format pin, the name
/// shape, and the deps-uniqueness invariant.
pub fn parse_workspace_config(bytes: &[u8]) -> Result<WorkspaceConfig, WorkspaceConfigError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| WorkspaceConfigError::Malformed(e.to_string()))?;

    if !value.is_object() {
        return Err(WorkspaceConfigError::Malformed(
            "expected a JSON object".to_string(),
        ));
    }

    // Pull `format` out first so a wrong-format file produces a
    // typed error instead of a serde mismatch on a downstream field.
    let format = value
        .get("format")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| WorkspaceConfigError::Malformed("missing or non-integer 'format'".into()))?;
    if format != FILESYSTEM_WORKSPACE_FORMAT as u64 {
        return Err(WorkspaceConfigError::UnsupportedFormat {
            got: format as u32,
            expected: FILESYSTEM_WORKSPACE_FORMAT,
        });
    }

    let config: WorkspaceConfig = serde_json::from_value(value)
        .map_err(|e| WorkspaceConfigError::Malformed(e.to_string()))?;

    // A legacy `name` carried by an older on-disk config is still shape-checked;
    // engine-written configs omit it (path-derived) and parse with an empty name
    // that `read_workspace_config` fills from the workspace-root basename.
    if !config.name.is_empty() {
        check_slug(&config.name, "name").map_err(WorkspaceConfigError::InvalidName)?;
    }

    // Reject duplicate deps at parse time so the on-disk file matches
    // the in-memory invariant `add_dep` upholds.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for dep in &config.deps {
        let key = dep.as_display();
        if !seen.insert(key.clone()) {
            return Err(WorkspaceConfigError::DuplicateDep(key));
        }
    }

    Ok(config)
}

/// Read + parse the workspace config at `<workspace_root>/.memstead/config.json`.
pub fn read_workspace_config(
    workspace_root: &Path,
) -> Result<WorkspaceConfig, WorkspaceConfigError> {
    let path = config_path(workspace_root);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(WorkspaceConfigError::NotFound(path));
        }
        Err(e) => {
            return Err(WorkspaceConfigError::Io { path, source: e });
        }
    };
    let mut config = parse_workspace_config(&bytes)?;
    // Path-derived identity: an engine-written config omits `name`, so fall back
    // to the workspace-root basename (matches the unified-layout rule the schema
    // validator enforces and `standalone_workspace` already applies).
    if config.name.is_empty() {
        config.name = workspace_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "mem".to_string());
    }
    Ok(config)
}

/// Write the workspace config to `<workspace_root>/.memstead/config.json`
/// atomically (write-to-temp + rename). Creates the `.memstead/` parent
/// directory if absent. Pretty-printed with 2-space indent for human
/// inspection (the file is the operator's primary debugging surface).
pub fn write_workspace_config(
    workspace_root: &Path,
    config: &WorkspaceConfig,
) -> Result<(), WorkspaceConfigError> {
    let target = config_path(workspace_root);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WorkspaceConfigError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    // Serialize in struct-field order (format, schema, version?, description?,
    // authors?, deps). `name` is path-derived and intentionally omitted.
    let mut bytes = serde_json::to_vec_pretty(config)
        .map_err(|e| WorkspaceConfigError::Malformed(format!("serialise: {e}")))?;
    bytes.push(b'\n');

    let tmp = make_tmp_path(&target);
    std::fs::write(&tmp, &bytes).map_err(|e| WorkspaceConfigError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(WorkspaceConfigError::Io {
            path: target,
            source: e,
        });
    }
    Ok(())
}

/// Initialise a brand-new filesystem (folder-backed) mem at `root` — the
/// engine-owned counterpart of `memstead init` for a single collapsed mem.
/// Writes the canonical `.memstead/config.json`, the `cache/` + `memstead-io/`
/// subdirs, the `workspace.toml` adapter marker, and the `state/mounts.json`
/// one-folder-mount roster, so the result roots directly through
/// [`crate::Engine::from_workspace_root`]. The mem root *is* the workspace
/// root (collapsed single-mem form).
///
/// This is the engine entry external embedders (the macOS app's bootstrap)
/// route through instead of hand-writing `.memstead/config.json` from their
/// own code — the engine owns the seed structure. Creates `root` (and the
/// `.memstead/` tree) if absent; the caller is responsible for refusing a
/// non-empty target if that matters.
pub fn init_filesystem_mem(root: &Path, name: &str, schema: &SchemaRef) -> std::io::Result<()> {
    use crate::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, Workspace, WorkspaceSettings,
    };
    use crate::workspace_store::{FileWorkspaceStore, WorkspaceStoreAdapter};

    let config = WorkspaceConfig::new(name, schema.clone());
    write_workspace_config(root, &config).map_err(std::io::Error::other)?;

    let memstead_dir = root.join(crate::WORKSPACE_STORE_DIR);
    std::fs::create_dir_all(memstead_dir.join("cache"))?;
    std::fs::create_dir_all(memstead_dir.join("memstead-io"))?;
    // Two-layer file adapter marker — `from_workspace_root` recognises a
    // workspace by `.memstead/workspace.toml`. The filesystem mem collapses
    // workspace = mem root: one folder mount carries every entity.
    std::fs::write(
        memstead_dir.join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )?;

    let workspace = Workspace {
        mounts: vec![Mount {
            mem: name.to_string(),
            schema: Some(schema.clone()),
            storage: MountStorage::Folder {
                path: root.to_path_buf(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }],
        settings: WorkspaceSettings::default(),
    };
    FileWorkspaceStore::new()
        .save_state(root, &workspace)
        .map_err(std::io::Error::other)?;
    Ok(())
}

fn make_tmp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "_".to_string());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    target.with_file_name(format!(".{name}.tmp.{nanos:x}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn versioned(name: &str, version: &str) -> SchemaRef {
        SchemaRef::new(name, semver::Version::parse(version).unwrap())
    }

    #[test]
    fn init_filesystem_mem_produces_a_rootable_workspace() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("notes");
        init_filesystem_mem(&root, "notes", &versioned("default", "1.0.0")).unwrap();

        // Seed structure landed: config + adapter marker + mounts roster.
        assert!(config_path(&root).is_file());
        assert!(root.join(".memstead").join("workspace.toml").is_file());
        assert!(
            root.join(".memstead")
                .join("state")
                .join("mounts.json")
                .is_file()
        );

        // And it roots directly through the engine, listing the one mem.
        let engine = crate::Engine::from_workspace_root(&root).unwrap();
        assert!(
            engine
                .mem_router()
                .writable_mems()
                .iter()
                .any(|v| v == "notes"),
            "init'd mem must be writable in the rooted engine"
        );
    }

    fn ok_config_value() -> serde_json::Value {
        serde_json::json!({
            "format": FILESYSTEM_WORKSPACE_FORMAT,
            "name": "demo-mem",
            "schema": "default@1.0.0",
        })
    }

    fn parse(value: serde_json::Value) -> Result<WorkspaceConfig, WorkspaceConfigError> {
        parse_workspace_config(value.to_string().as_bytes())
    }

    #[test]
    fn parses_minimal_config() {
        let cfg = parse(ok_config_value()).unwrap();
        assert_eq!(cfg.format, FILESYSTEM_WORKSPACE_FORMAT);
        assert_eq!(cfg.name, "demo-mem");
        assert_eq!(cfg.schema.as_display(), "default@1.0.0");
        assert!(cfg.deps.is_empty());
        assert!(cfg.version.is_none());
    }

    #[test]
    fn parses_full_config() {
        let v = serde_json::json!({
            "format": FILESYSTEM_WORKSPACE_FORMAT,
            "name": "demo-mem",
            "schema": "default@1.0.0",
            "version": "0.1.0",
            "description": "demo",
            "authors": ["alice"],
            "deps": ["anthropic/core", "anthropic/agents"],
        });
        let cfg = parse(v).unwrap();
        assert_eq!(cfg.deps.len(), 2);
        assert_eq!(cfg.deps[0].as_display(), "anthropic/core");
        assert_eq!(cfg.deps[1].as_display(), "anthropic/agents");
        assert_eq!(cfg.version.unwrap().to_string(), "0.1.0");
        assert_eq!(cfg.authors.unwrap(), vec!["alice".to_string()]);
    }

    #[test]
    fn rejects_bare_name_schema_pin() {
        let mut v = ok_config_value();
        v["schema"] = serde_json::json!("default");
        let err = parse(v).unwrap_err();
        assert!(
            matches!(err, WorkspaceConfigError::Malformed(_)),
            "expected Malformed for bare-name pin, got {err:?}"
        );
    }

    #[test]
    fn rejects_unsupported_format() {
        let mut v = ok_config_value();
        v["format"] = serde_json::json!(99);
        let err = parse(v).unwrap_err();
        match err {
            WorkspaceConfigError::UnsupportedFormat { got: 99, expected } => {
                assert_eq!(expected, FILESYSTEM_WORKSPACE_FORMAT);
            }
            other => panic!("expected UnsupportedFormat, got {other:?}"),
        }
    }

    #[test]
    fn rejects_archive_format_in_workspace_position() {
        // An archive's `format: 3` config sneaking into the workspace
        // position must surface as a typed mismatch — the two
        // namespaces overlap on the filename but not on the format
        // integer.
        let mut v = ok_config_value();
        v["format"] = serde_json::json!(PUBLISHED_MEM_FORMAT);
        let err = parse(v).unwrap_err();
        assert!(matches!(
            err,
            WorkspaceConfigError::UnsupportedFormat { .. }
        ));
    }

    #[test]
    fn preserves_unknown_top_level_fields() {
        // Engine-owned runtime fields (`syncState`, `writeGuidance`, …)
        // land in the same file this shape reads; they must survive a
        // read-modify-write round-trip instead of refusing the parse
        // (a strict reader broke export for projection-maintained mems).
        let mut v = ok_config_value();
        v["syncState"] = serde_json::json!({"public-docs": "abc123"});
        let cfg = parse(v).unwrap();
        assert_eq!(
            cfg.extra.get("syncState"),
            Some(&serde_json::json!({"public-docs": "abc123"}))
        );
        let back = serde_json::to_value(&cfg).unwrap();
        assert_eq!(back["syncState"]["public-docs"], "abc123");
    }

    #[test]
    fn rejects_invalid_name() {
        let mut v = ok_config_value();
        v["name"] = serde_json::json!("Invalid Name");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, WorkspaceConfigError::InvalidName(_)));
    }

    #[test]
    fn rejects_invalid_schema_pin() {
        let mut v = ok_config_value();
        v["schema"] = serde_json::json!("default@^1.0.0");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, WorkspaceConfigError::Malformed(_)));
    }

    #[test]
    fn rejects_dep_without_scope() {
        let mut v = ok_config_value();
        v["deps"] = serde_json::json!(["just-a-name"]);
        let err = parse(v).unwrap_err();
        assert!(matches!(err, WorkspaceConfigError::Malformed(_)));
    }

    #[test]
    fn rejects_duplicate_deps_on_disk() {
        let mut v = ok_config_value();
        v["deps"] = serde_json::json!(["anthropic/core", "anthropic/core"]);
        let err = parse(v).unwrap_err();
        assert!(matches!(err, WorkspaceConfigError::DuplicateDep(_)));
    }

    #[test]
    fn add_dep_is_idempotent() {
        let mut cfg = WorkspaceConfig::new("demo", versioned("default", "1.0.0"));
        let dep = DepRef {
            scope: "anthropic".into(),
            name: "core".into(),
        };
        assert!(cfg.add_dep(dep.clone()));
        assert!(!cfg.add_dep(dep.clone()));
        assert_eq!(cfg.deps.len(), 1);
    }

    #[test]
    fn round_trip_through_disk_preserves_dep_order() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = WorkspaceConfig::new("demo", versioned("default", "1.0.0"));
        cfg.add_dep("anthropic/core".parse().unwrap());
        cfg.add_dep("anthropic/agents".parse().unwrap());
        cfg.add_dep("openai/sdk".parse().unwrap());

        write_workspace_config(tmp.path(), &cfg).unwrap();
        let read = read_workspace_config(tmp.path()).unwrap();
        assert_eq!(
            read.deps.iter().map(|d| d.as_display()).collect::<Vec<_>>(),
            vec![
                "anthropic/core".to_string(),
                "anthropic/agents".to_string(),
                "openai/sdk".to_string(),
            ]
        );
    }

    #[test]
    fn read_missing_config_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let err = read_workspace_config(tmp.path()).unwrap_err();
        assert!(matches!(err, WorkspaceConfigError::NotFound(_)));
    }

    #[test]
    fn engine_written_config_omits_name_and_read_derives_basename() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("my-mem");
        std::fs::create_dir_all(&root).unwrap();
        let cfg = WorkspaceConfig::new("my-mem", versioned("default", "1.0.0"));
        write_workspace_config(&root, &cfg).unwrap();

        // The persisted config carries no `name` (the schema validator
        // tombstones it; identity is path-derived).
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config_path(&root)).unwrap()).unwrap();
        assert!(
            raw.get("name").is_none(),
            "config.json must not carry a path-derived `name`"
        );

        // Read fills the identity from the basename.
        let read = read_workspace_config(&root).unwrap();
        assert_eq!(read.name, "my-mem");
    }

    #[test]
    fn read_tolerates_a_legacy_name_field() {
        let tmp = TempDir::new().unwrap();
        let v = serde_json::json!({
            "format": FILESYSTEM_WORKSPACE_FORMAT,
            "name": "legacy-name",
            "schema": "default@1.0.0",
        });
        std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
        std::fs::write(config_path(tmp.path()), v.to_string()).unwrap();
        let read = read_workspace_config(tmp.path()).unwrap();
        // A present legacy name is read as-is (basename fallback only kicks in
        // when absent), so old mems keep working.
        assert_eq!(read.name, "legacy-name");
    }

    #[test]
    fn write_creates_memstead_parent_directory() {
        let tmp = TempDir::new().unwrap();
        assert!(!tmp.path().join(".memstead").exists());
        let cfg = WorkspaceConfig::new("demo", versioned("default", "1.0.0"));
        write_workspace_config(tmp.path(), &cfg).unwrap();
        assert!(tmp.path().join(".memstead").is_dir());
        assert!(tmp.path().join(".memstead").join("config.json").is_file());
    }

    #[test]
    fn to_published_drops_deps() {
        let mut cfg = WorkspaceConfig::new("demo", versioned("default", "1.0.0"));
        cfg.version = Some(semver::Version::parse("0.1.0").unwrap());
        cfg.add_dep("anthropic/core".parse().unwrap());

        let published = cfg.to_published().unwrap();
        assert_eq!(published.format, PUBLISHED_MEM_FORMAT);
        assert_eq!(published.name, "demo");
        assert_eq!(published.version.to_string(), "0.1.0");
        assert_eq!(published.schema.name, "default");
        // PublishedMemConfig has no `deps` field — the projection
        // simply drops them. Verify it still serialises clean.
        let serialised = serde_json::to_value(&published).unwrap();
        assert!(serialised.get("deps").is_none());
    }

    #[test]
    fn to_published_requires_version() {
        // F1: mem-init populates `version` with `0.1.0` by default
        // so `to_published` no longer trips on a freshly-created
        // config. Simulate the pre-gate / externally-imported config
        // by clearing `version` explicitly.
        let mut cfg = WorkspaceConfig::new("demo", versioned("default", "1.0.0"));
        cfg.version = None;
        let err = cfg.to_published().unwrap_err();
        assert!(matches!(err, PublishConversionError::MissingVersion));
    }

    #[test]
    fn dep_ref_roundtrip_via_serde() {
        let dep = DepRef {
            scope: "scope".into(),
            name: "name".into(),
        };
        let s = serde_json::to_string(&dep).unwrap();
        assert_eq!(s, "\"scope/name\"");
        let back: DepRef = serde_json::from_str(&s).unwrap();
        assert_eq!(back, dep);
    }

    #[test]
    fn dep_ref_rejects_uppercase() {
        let err: Result<DepRef, _> = "Scope/name".parse();
        assert!(err.is_err());
    }

    #[test]
    fn dep_ref_rejects_empty_segments() {
        let err: Result<DepRef, _> = "/name".parse();
        assert!(err.is_err());
        let err: Result<DepRef, _> = "scope/".parse();
        assert!(err.is_err());
    }
}
