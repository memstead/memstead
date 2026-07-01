//! Type definitions, schema loading, and vault-config validation for Memstead.
//!
//! Schemas are first-class packages — named, versioned bundles of type
//! definitions + relationship vocabulary + LLM-facing documentation. The
//! engine holds a `SchemaRegistry` mapping `(name, version)` to `Arc<Schema>`.
//! Each vault pins exactly one schema via `VaultConfig.schema: SchemaRef`.

pub mod archive_provenance;
pub mod base_metadata;
pub mod builtins;
pub mod config;
pub mod loader;
pub mod manifest;
pub mod meta_schema;
pub mod schema;
pub mod source;
pub mod types;
pub mod workspace_config;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use archive_provenance::{
    ARCHIVE_PROVENANCE_FORMAT, ArchiveProvenance, EntityProvenance, History,
};
pub use config::{
    ARCHIVE_CONFIG_PATH, ARCHIVE_EXTENSION, ARCHIVE_META_DIR, ARCHIVE_PROVENANCE_PATH,
    ARCHIVE_SCHEMA_PREFIX,
    CommunityOverride, ConfigCheckResult, ConfigError, LEGACY_ARCHIVE_EXTENSIONS,
    PUBLISHED_VAULT_FORMAT, PublishConfig, PublishConversionError, PublishedVaultConfig,
    ReadVaultSource, ReadVaultSpec, RoleConfig, SchemaRef, VAULT_META_DIR, VaultConfig,
    VcsConfig, check_config, load_and_validate, load_config, parse_vault_config,
    published_config_from,
};
pub use loader::{SchemaLoadError, load_schema_from_dir, load_schema_from_memory};
pub use manifest::{
    Cardinality, CommunityConfig, CrossVaultRelationshipEntry, DefaultWritingGuidance,
    ManualAuthoring, PerEdgeDescription, RelationshipDef, RelationshipMode,
    RelationshipVocabulary, SchemaManifest,
};
pub use schema::Schema;
pub use source::{SchemaSourceError, SchemaSourceFile, collect_schema_source};
pub use types::{
    FieldType, Filterable, MetadataFieldDef, RequiredCardinality, RequiredOutgoing, SectionDef,
    Serialization, TypeDefinition, TypeExample,
};

/// Name constants for the 10 built-in knowledge types shipped in the
/// `default` schema. Kept as a module to catch typos at compile time.
pub mod builtin_names {
    pub const SPEC: &str = "spec";
    pub const MEMO: &str = "memo";
    pub const ASSERTION: &str = "assertion";
    pub const CONCEPT: &str = "concept";
    pub const INQUIRY: &str = "inquiry";
    pub const MODEL: &str = "model";
    pub const NARRATIVE: &str = "narrative";
    pub const PERSPECTIVE: &str = "perspective";
    pub const PRINCIPLE: &str = "principle";
    pub const PROCESS: &str = "process";

    pub const ALL: [&str; 10] = [
        SPEC, MEMO, ASSERTION, CONCEPT, INQUIRY, MODEL, NARRATIVE, PERSPECTIVE, PRINCIPLE, PROCESS,
    ];
}

/// Registry holding every loaded schema keyed by `(name, version)`.
#[derive(Debug)]
pub struct SchemaRegistry {
    schemas: HashMap<(String, semver::Version), Arc<Schema>>,
}

#[derive(Debug, thiserror::Error)]
pub enum SchemaRegistryError {
    #[error(
        "schema '{name}' version '{version}' is already registered — cannot reinsert"
    )]
    AlreadyRegistered {
        name: String,
        version: semver::Version,
    },
}

/// Error returned by [`SchemaRegistry::resolve_by_name`] when a bare-name
/// lookup matches multiple registered versions. Surfaced by the
/// `memstead_schema(name=...)` discovery surface — callers must supply an
/// exact `<name>@<version>` to disambiguate.
#[derive(Debug, thiserror::Error)]
#[error(
    "schema name '{name}' is ambiguous: {} versions registered ({}). \
     Use a versioned pin (e.g. \"{name}@{}\") to disambiguate.",
    .versions.len(),
    .versions.join(", "),
    .versions.first().map(String::as_str).unwrap_or("")
)]
pub struct SchemaNameAmbiguous {
    pub name: String,
    pub versions: Vec<String>,
}

/// Errors raised while scanning workspace-level or cache schema directories.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceSchemaLoadError {
    /// A schema directory failed semantic or structural validation. The
    /// offending directory path is captured alongside the underlying
    /// loader error so operators can jump straight to the broken file.
    #[error("failed to load schema at {}: {source}", .path.display())]
    Invalid {
        path: PathBuf,
        #[source]
        source: SchemaLoadError,
    },

    /// Filesystem error while iterating a schemas/ or .memstead.cache/schemas/
    /// directory.
    #[error("i/o error scanning {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Two distinct schema directories share the same `<name>-<version>`
    /// cache key. Surfaces as `SCHEMA_CACHE_COLLISION` to MCP callers —
    /// silent overwrite would mask a bug in the extraction pipeline.
    #[error(
        "schema cache collision: '{name}-{version}' has more than one source directory ({} and {})",
        .first.display(),
        .second.display()
    )]
    CacheCollision {
        name: String,
        version: semver::Version,
        first: PathBuf,
        second: PathBuf,
    },
}

impl SchemaRegistry {
    pub fn empty() -> Self {
        Self {
            schemas: HashMap::new(),
        }
    }

    /// Preloaded with every schema embedded in the binary.
    pub fn builtin() -> Self {
        let mut reg = Self::empty();
        for schema in builtins::load_builtin_schemas()
            .expect("built-in schemas must load cleanly — bug in shipped YAML")
        {
            reg.schemas
                .insert((schema.manifest.name.clone(), schema.version.clone()), schema);
        }
        reg
    }

    /// Build a registry starting from the embedded builtins, then layer
    /// in the workspace-level shared schemas and the workspace-wide
    /// schema cache.
    ///
    /// Precedence (highest wins on identical `(name, version)`):
    /// 1. `<workspace_schemas_dir>/<schema>/` — workspace-level shared schemas
    /// 2. Embedded builtins (`default@1.0.0`, ...)
    /// 3. `<workspace_root>/.memstead.cache/schemas/<schema>-<version>/` —
    ///    extracted from read-vault archives (workspace-wide cache)
    ///
    /// Different versions of the same schema coexist; a vault picks by exact
    /// pin via `VaultConfig.schema`.
    ///
    /// Hidden directories (name starts with `.`) are skipped at every scan
    /// level so VCS metadata and OS dotdirs cannot be mistaken for a schema
    /// definition.
    ///
    /// Returns the first validation failure it hits so a broken schema can
    /// never silently shadow a working builtin. Two distinct cache
    /// directories sharing the same `<name>-<version>` key surface as
    /// [`WorkspaceSchemaLoadError::CacheCollision`] — the extraction
    /// pipeline must guarantee uniqueness, and silent overwrite would mask
    /// the bug.
    ///
    /// `workspace_root` and `workspace_schemas_dir` are independent
    /// optionals: passing `None` for both yields the builtins-only registry
    /// (the `Engine::init` no-settings variant).
    pub fn load_for_workspace(
        workspace_root: Option<&Path>,
        workspace_schemas_dir: Option<&Path>,
    ) -> Result<Self, WorkspaceSchemaLoadError> {
        let mut reg = Self::empty();

        // Pass 1 (lowest precedence): cache schemas extracted from archives.
        if let Some(ws_root) = workspace_root {
            let cache_dir = ws_root.join(".memstead.cache").join("schemas");
            // Track seen `(name, version)` against the directory that
            // contributed the schema — a second hit means the extraction
            // pipeline produced two cache entries for the same key, which
            // is a bug we surface rather than mask.
            let mut seen: HashMap<(String, semver::Version), PathBuf> = HashMap::new();
            for path in list_schema_subdirs(&cache_dir)? {
                let schema = loader::load_schema_from_dir(&path).map_err(|source| {
                    WorkspaceSchemaLoadError::Invalid {
                        path: path.clone(),
                        source,
                    }
                })?;
                let key = (schema.manifest.name.clone(), schema.version.clone());
                if let Some(prev) = seen.get(&key) {
                    return Err(WorkspaceSchemaLoadError::CacheCollision {
                        name: key.0,
                        version: key.1,
                        first: prev.clone(),
                        second: path,
                    });
                }
                seen.insert(key.clone(), path);
                reg.schemas.insert(key, Arc::new(schema));
            }
        }

        // Pass 2: embedded builtins override cache at the same key.
        for schema in builtins::load_builtin_schemas()
            .expect("built-in schemas must load cleanly — bug in shipped YAML")
        {
            let key = (schema.manifest.name.clone(), schema.version.clone());
            reg.schemas.insert(key, schema);
        }

        // Pass 3 (highest precedence): workspace-level schemas override
        // both cache and builtins.
        if let Some(ws_dir) = workspace_schemas_dir {
            for path in list_schema_subdirs(ws_dir)? {
                let schema =
                    loader::load_schema_from_dir(&path).map_err(|source| {
                        WorkspaceSchemaLoadError::Invalid {
                            path: path.clone(),
                            source,
                        }
                    })?;
                let key = (schema.manifest.name.clone(), schema.version.clone());
                reg.schemas.insert(key, Arc::new(schema));
            }
        }

        Ok(reg)
    }

    /// Resolve a schema by name alone. Used by the `memstead_schema(name=...)`
    /// lookup surface: the registry is expected to hold at most one
    /// schema with the given name after workspace-level loading.
    ///
    /// Returns:
    /// - `Ok(Some(schema))` when exactly one schema is registered with
    ///   this name (any version — the version is metadata).
    /// - `Ok(None)` when no schema with that name is registered.
    /// - `Err(SchemaNameAmbiguous)` when multiple versions of the same
    ///   name are registered (cache + builtin collision, or mixed
    ///   workspace-level versions). Callers surface this — bare-name
    ///   lookups need a unique winner.
    pub fn resolve_by_name(&self, name: &str) -> Result<Option<Arc<Schema>>, SchemaNameAmbiguous> {
        let candidates: Vec<&Arc<Schema>> = self
            .schemas
            .iter()
            .filter(|((n, _), _)| n == name)
            .map(|(_, s)| s)
            .collect();
        match candidates.len() {
            0 => Ok(None),
            1 => Ok(Some(candidates[0].clone())),
            _ => {
                let mut versions: Vec<String> =
                    candidates.iter().map(|s| s.version.to_string()).collect();
                versions.sort();
                Err(SchemaNameAmbiguous {
                    name: name.to_string(),
                    versions,
                })
            }
        }
    }

    /// Merge another registry into this one. `name@version` keys already
    /// present are left untouched — the caller controls precedence by the
    /// order it merges. Used by the engine to build one aggregate registry
    /// across writable vaults without copying arcs twice.
    pub fn merge_from(&mut self, other: &SchemaRegistry) {
        for (key, schema) in &other.schemas {
            self.schemas.entry(key.clone()).or_insert_with(|| schema.clone());
        }
    }

    /// Insert a schema, replacing any existing entry at the same
    /// `(name, version)` key. Used by storage backends that source
    /// workspace-level schemas from outside the disk-walker (e.g. the
    /// `gix`-tree-backed loader in `memstead-git-branch::vault_repo_schemas`) to
    /// overlay workspace schemas on top of the cache + builtins layers
    /// loaded by [`Self::load_for_workspace`] with `workspace_schemas_dir
    /// = None`.
    ///
    /// **Shadowing semantics:** this method overwrites builtin entries
    /// at the same `(name, version)`. That is intentional for the
    /// canonical use case — a workspace's `software@1.0.0` schema
    /// overlay legitimately replaces the `default@1.0.0` builtin's
    /// slot only when the names happen to collide, which is the
    /// vault-repo overlay pattern. Callers MUST NOT use this
    /// method to silently shadow an unrelated builtin name unless
    /// they own the workspace-overlay precedence story; the only
    /// in-tree caller is `memstead-git-branch::lib::build_workspace_schema_registry`.
    pub fn insert_overwriting(&mut self, schema: Arc<Schema>) {
        let key = (schema.manifest.name.clone(), schema.version.clone());
        self.schemas.insert(key, schema);
    }

    pub fn get(&self, name: &str, version: &semver::Version) -> Option<Arc<Schema>> {
        self.schemas
            .get(&(name.to_string(), version.clone()))
            .cloned()
    }

    pub fn iter(&self) -> impl Iterator<Item = Arc<Schema>> + '_ {
        self.schemas.values().cloned()
    }

    pub fn available_versions(&self, name: &str) -> Vec<semver::Version> {
        let mut versions: Vec<semver::Version> = self
            .schemas
            .keys()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .collect();
        versions.sort();
        versions
    }

    /// Closest-match schema name by Levenshtein edit distance against the
    /// currently-registered schemas. Returns `None` if the registry is
    /// empty or every candidate's distance from `name` is 0 (exact match,
    /// shouldn't be called in that case) — callers get a clean `Option`
    /// to plug into error messages without format-plumbing.
    pub fn suggest_name(&self, name: &str) -> Option<String> {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut best: Option<(usize, String)> = None;
        for (n, _) in self.schemas.keys() {
            if !seen.insert(n.as_str()) {
                continue;
            }
            let d = strsim::levenshtein(name, n);
            match &best {
                Some((bd, _)) if *bd <= d => {}
                _ => best = Some((d, n.clone())),
            }
        }
        best.and_then(|(d, n)| if d > 0 { Some(n) } else { None })
    }

    /// List every registered `(name, version)` pair, stably sorted so
    /// iteration is deterministic.
    pub fn identities(&self) -> Vec<(String, semver::Version)> {
        let mut ids: Vec<(String, semver::Version)> = self.schemas.keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty()
    }

    pub fn len(&self) -> usize {
        self.schemas.len()
    }

    pub fn insert(&mut self, schema: Arc<Schema>) -> Result<(), SchemaRegistryError> {
        let key = (schema.manifest.name.clone(), schema.version.clone());
        if self.schemas.contains_key(&key) {
            return Err(SchemaRegistryError::AlreadyRegistered {
                name: key.0,
                version: key.1,
            });
        }
        self.schemas.insert(key, schema);
        Ok(())
    }
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

/// Lookup by short type name against the built-in `default` schema.
///
/// Kept as a convenience because ~100 engine call sites use short names to
/// resolve type definitions. Production consumers should prefer
/// `vault.schema.get_type(name)` for user-defined schemas; this helper
/// exists for test fixtures, CLI one-offs, and callers that legitimately
/// target the built-in `default` schema.
pub fn type_by_name(name: &str) -> Option<Arc<TypeDefinition>> {
    Schema::builtin_default().get_type(name)
}

/// Enumerate immediate subdirectories of `dir` that look like schema roots.
///
/// Missing directories are treated as "no schemas here" rather than an
/// error — a workspace without `.memstead/schemas/` is legitimate. Hidden
/// directories (leading `.`) are skipped so `.cache`, `.git`, `.DS_Store`,
/// and other VCS/OS metadata cannot masquerade as a schema package.
/// Non-directory entries (stray files like `README.md`) are ignored too.
fn list_schema_subdirs(dir: &Path) -> Result<Vec<PathBuf>, WorkspaceSchemaLoadError> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| WorkspaceSchemaLoadError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| WorkspaceSchemaLoadError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        out.push(path);
    }
    // Deterministic order so log output and error messages match across runs.
    out.sort();
    Ok(out)
}

/// Every type in the built-in `default` schema, in declaration order.
pub fn all_types() -> Vec<Arc<TypeDefinition>> {
    let schema = Schema::builtin_default();
    // Preserve `manifest.types` order so callers iterating this get a
    // stable sequence instead of HashMap iteration order.
    schema
        .manifest
        .types
        .iter()
        .filter_map(|name| schema.get_type(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_contains_default() {
        let reg = SchemaRegistry::builtin();
        assert!(!reg.is_empty());
        let versions = reg.available_versions("default");
        assert_eq!(versions.len(), 1);
    }

    #[test]
    fn builtin_default_has_ten_types() {
        assert_eq!(all_types().len(), 10);
        for name in builtin_names::ALL {
            assert!(type_by_name(name).is_some(), "missing type: {name}");
        }
    }

    #[test]
    fn registry_rejects_duplicate_insert() {
        let schema = Schema::builtin_default();
        let mut reg = SchemaRegistry::empty();
        reg.insert(schema.clone()).unwrap();
        let err = reg.insert(schema).unwrap_err();
        assert!(matches!(err, SchemaRegistryError::AlreadyRegistered { .. }));
    }

    /// `software@0.1.0` lifecycle-stage date fields are optional: a
    /// brand-new entity in its default stage (`verification_status:
    /// unverified`, `deprecation_status: current`) must author without
    /// supplying a date for an event that has not happened. Sibling
    /// non-lifecycle fields stay required. Locks the requiredness
    /// decision so a future schema edit can't silently re-require them.
    #[test]
    fn software_lifecycle_date_fields_are_optional() {
        let reg = SchemaRegistry::builtin();
        let software = reg
            .resolve_by_name("software")
            .unwrap()
            .expect("software builtin present");

        let requirement = software.get_type("requirement").expect("requirement type");
        assert!(
            requirement.metadata_field("verified_on").unwrap().optional,
            "verified_on must be optional — an unverified requirement has no verification date"
        );
        // Sibling required field is untouched.
        assert!(
            !requirement.metadata_field("source").unwrap().optional,
            "source stays required"
        );

        let contract = software.get_type("contract").expect("contract type");
        for field in ["deprecated_on", "removal_on"] {
            assert!(
                contract.metadata_field(field).unwrap().optional,
                "{field} must be optional — a current contract has no deprecation/removal date"
            );
        }
        // Sibling required fields are untouched.
        for field in ["protocol", "version"] {
            assert!(
                !contract.metadata_field(field).unwrap().optional,
                "{field} stays required"
            );
        }
    }

    mod workspace_layer {
        use super::*;
        use tempfile::TempDir;

        /// Minimal schema fixture writer — builds `schema.yaml` + one type.
        fn write_schema(dir: &Path, name: &str, version: &str) {
            std::fs::create_dir_all(dir.join("types")).unwrap();
            let manifest = format!(
                r#"name: {name}
version: {version}
description: test
when_to_use: test
types:
  - spec
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
"#
            );
            std::fs::write(dir.join("schema.yaml"), manifest).unwrap();
            std::fs::write(
                dir.join("types/spec.yaml"),
                r#"name: spec
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
"#,
            )
            .unwrap();
        }

        #[test]
        fn workspace_schema_resolves_by_name() {
            let tmp = TempDir::new().unwrap();
            let workspace_schemas = tmp.path().join("schemas");

            // Use a name that does not collide with any registered
            // builtin schema (default / ingest / planning / project /
            // software all ship as builtins).
            write_schema(&workspace_schemas.join("test-isolated"), "test-isolated", "1.0.0");

            let reg =
                SchemaRegistry::load_for_workspace(Some(tmp.path()), Some(&workspace_schemas))
                    .expect("workspace load succeeds");

            let resolved = reg
                .resolve_by_name("test-isolated")
                .expect("unique name resolves");
            assert!(resolved.is_some(), "workspace schema must be registered");
        }

        #[test]
        fn per_vault_schema_override_no_longer_resolves() {
            // Pre-cutover, a per-vault `<vault>/.memstead/schemas/<name>/`
            // shadowed builtins and the workspace layer. That level is
            // gone — the builtin survives even
            // with a per-vault directory present on disk.
            let tmp = TempDir::new().unwrap();
            let vault_override = tmp.path().join("vault/.memstead/schemas/default");
            write_schema(&vault_override, "default", "1.0.0");

            let reg =
                SchemaRegistry::load_for_workspace(Some(tmp.path()), None).unwrap();
            let schema = reg
                .get("default", &semver::Version::new(1, 0, 0))
                .expect("builtin default still registered");
            // Builtin ships 10 types; the per-vault override would have
            // been a 1-type schema if the level still existed.
            assert_eq!(
                schema.types.len(),
                10,
                "per-vault override must no longer shadow the builtin"
            );
        }

        #[test]
        fn workspace_layer_falls_through_to_builtins() {
            let tmp = TempDir::new().unwrap();
            // No workspace dir → only builtins remain.
            let reg = SchemaRegistry::load_for_workspace(Some(tmp.path()), None)
                .expect("builtin-only load succeeds");
            let resolved = reg.resolve_by_name("default").expect("unique");
            assert!(
                resolved.is_some(),
                "builtin default must be registered when no other layers contribute"
            );
        }

        #[test]
        fn workspace_overrides_builtin_at_same_key() {
            // Workspace-level schemas sit above builtins. A workspace-level
            // `default@1.0.0` with a single type must replace the 10-type
            // shipped builtin.
            let tmp = TempDir::new().unwrap();
            let workspace_schemas = tmp.path().join("schemas");

            write_schema(&workspace_schemas.join("default"), "default", "1.0.0");

            let reg =
                SchemaRegistry::load_for_workspace(Some(tmp.path()), Some(&workspace_schemas))
                    .expect("workspace override loads");

            let schema = reg
                .get("default", &semver::Version::new(1, 0, 0))
                .expect("default@1.0.0 still registered");
            // Builtin ships 10 types; the workspace override carries 1.
            assert_eq!(
                schema.types.len(),
                1,
                "workspace-level schema must replace the builtin shape"
            );
        }

        #[test]
        fn workspace_cache_path_is_workspace_wide() {
            // The cache lives at `<workspace_root>/.memstead.cache/schemas/`
            // — never under any vault directory.
            let tmp = TempDir::new().unwrap();
            let cache_dir = tmp.path().join(".memstead.cache/schemas/recipe-1.0.0");
            write_schema(&cache_dir, "recipe", "1.0.0");

            let reg =
                SchemaRegistry::load_for_workspace(Some(tmp.path()), None).unwrap();
            assert!(
                reg.get("recipe", &semver::Version::new(1, 0, 0)).is_some(),
                "workspace-wide cache schema must register"
            );
        }

        #[test]
        fn schema_cache_collision_yields_error() {
            // Two cache directories sharing the same `(name, version)` key
            // are a bug — surface the collision instead of silently
            // overwriting one with the other.
            let tmp = TempDir::new().unwrap();
            let first = tmp.path().join(".memstead.cache/schemas/dup-a");
            let second = tmp.path().join(".memstead.cache/schemas/dup-b");
            write_schema(&first, "shared", "1.0.0");
            write_schema(&second, "shared", "1.0.0");

            let err =
                SchemaRegistry::load_for_workspace(Some(tmp.path()), None).unwrap_err();
            assert!(
                matches!(err, WorkspaceSchemaLoadError::CacheCollision { .. }),
                "expected CacheCollision, got {err:?}"
            );
        }

        #[test]
        fn three_level_chain_resolves_correctly() {
            // Workspace > builtins > cache. Place a unique schema at
            // each level and confirm fall-through and override semantics.
            let tmp = TempDir::new().unwrap();
            let workspace_schemas = tmp.path().join("schemas");

            // Cache only — falls through to it when nothing else matches.
            let cache_only = tmp.path().join(".memstead.cache/schemas/cache-only-1.0.0");
            write_schema(&cache_only, "cache-only", "1.0.0");

            // Workspace overrides builtin default.
            write_schema(&workspace_schemas.join("default"), "default", "1.0.0");

            // Workspace also overrides the cache at the same key.
            let cache_overridden =
                tmp.path().join(".memstead.cache/schemas/overridden-1.0.0");
            write_schema(&cache_overridden, "overridden", "1.0.0");
            write_schema(&workspace_schemas.join("overridden"), "overridden", "1.0.0");

            let reg =
                SchemaRegistry::load_for_workspace(Some(tmp.path()), Some(&workspace_schemas))
                    .unwrap();

            assert!(
                reg.get("cache-only", &semver::Version::new(1, 0, 0))
                    .is_some(),
                "cache-only schema must register via cache layer"
            );
            // Builtin had 10 types; workspace override has 1.
            assert_eq!(
                reg.get("default", &semver::Version::new(1, 0, 0))
                    .expect("default registered")
                    .types
                    .len(),
                1,
                "workspace-level default must override the 10-type builtin"
            );
            assert!(
                reg.get("overridden", &semver::Version::new(1, 0, 0))
                    .is_some(),
                "workspace-level schema overrides the cache copy at the same key"
            );
        }

        #[test]
        fn unknown_name_resolves_to_none() {
            let tmp = TempDir::new().unwrap();
            let reg = SchemaRegistry::load_for_workspace(Some(tmp.path()), None).unwrap();
            assert!(reg.resolve_by_name("does-not-exist").unwrap().is_none());
        }

        #[test]
        fn ambiguous_name_surfaces_versions() {
            let tmp = TempDir::new().unwrap();
            let workspace_schemas = tmp.path().join("schemas");

            // Two different versions of the same schema name registered
            // under two different directory names so both get loaded.
            // Uses a name that does not collide with any builtin
            // schema (those would add a third version).
            write_schema(
                &workspace_schemas.join("test-ambig-1"),
                "test-ambig",
                "1.0.0",
            );
            write_schema(
                &workspace_schemas.join("test-ambig-2"),
                "test-ambig",
                "2.0.0",
            );

            let reg =
                SchemaRegistry::load_for_workspace(Some(tmp.path()), Some(&workspace_schemas))
                    .unwrap();
            let err = reg
                .resolve_by_name("test-ambig")
                .expect_err("two versions under same name must be ambiguous");
            assert_eq!(err.versions.len(), 2);
            assert!(err.versions.iter().any(|v| v == "1.0.0"));
            assert!(err.versions.iter().any(|v| v == "2.0.0"));
            let msg = format!("{err}");
            assert!(msg.contains("ambiguous"));
            assert!(msg.contains("1.0.0"));
        }
    }
}
