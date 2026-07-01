//! Read workspace-level schemas from the unified `__MEMSTEAD` ref.
//!
//! The schema loader delegates to
//! [`crate::storage_memstead::load_schemas_from_memstead_ref`]. All test
//! fixtures and bootstrap paths seed `__MEMSTEAD`;
//! `engine_from_workspace_root` boot promotion covers any
//! pre-existing legacy workspace by running the idempotent migrator
//! at first read.
//!
//! This module remains the [`LoadOutcome`] type owner and the public
//! entry point so call sites keep their import surface stable; the
//! body is now a thin compatibility wrapper.

use std::path::Path;
use std::sync::Arc;

use memstead_schema::{Schema, loader::SchemaLoadError};

/// Errors raised while reading workspace schemas from
/// `vault-repo-git:__MEMSTEAD:schemas/`.
#[derive(Debug, thiserror::Error)]
pub enum VaultRepoSchemasError {
    /// `vault-repo/.git/` exists but cannot be opened (corrupt repo, IO
    /// failure under the object database).
    #[error("could not open vault-repo gitdir: {0}")]
    GixOpen(String),
    /// Generic gix-tree read failure (object missing, corrupt tree, IO
    /// underneath the object database). The wrapped message names the
    /// underlying gix error.
    #[error("git tree read error: {0}")]
    GitTree(String),
    /// A schema blob (`schema.yaml` or a type file) is not valid UTF-8.
    /// The schema name + offending file are folded into the message.
    #[error("schema blob {0} is not valid UTF-8: {1}")]
    NotUtf8(String, String),
    /// Parse / validation failure surfaced from the shared schema
    /// loader pipeline. Wraps `SchemaLoadError` so callers can branch
    /// on the inner kind if needed; the schema name is folded in to
    /// disambiguate when multiple schemas are loaded in one pass.
    #[error("schema '{name}': {source}")]
    Schema {
        name: String,
        #[source]
        source: SchemaLoadError,
    },
}

/// Outcome of [`load_schemas_from_ref`] — three discriminated cases
/// that callers branch on so the engine can decide whether to overlay
/// schemas, fall back to disk, or surface an error.
pub enum LoadOutcome {
    /// `vault-repo/.git/` is missing or carries no `__MEMSTEAD` ref — the
    /// workspace is not a real vault-repo (legacy disk-shaped, empty
    /// stub, or pre-migration). Caller falls back to its disk-based
    /// schema source.
    NoVaultRepo,
    /// Real vault-repo exists but the `__MEMSTEAD:schemas/` subtree is
    /// absent (or its tree carries no schema directories). No schemas
    /// to overlay; caller may still load a disk-based source if
    /// configured.
    NoSchemas,
    /// Schemas successfully loaded from `__MEMSTEAD:schemas/`.
    Schemas(Vec<Arc<Schema>>),
}

/// Load every workspace-level schema from the vault-repo's unified
/// `__MEMSTEAD:schemas/<name>@<version>/` tree.
///
/// Thin wrapper around
/// [`crate::storage_memstead::load_schemas_from_memstead_ref`].
///
/// `workspace_root` is the directory holding `vault-repo/.git/`.
pub fn load_schemas_from_ref(
    workspace_root: &Path,
) -> Result<LoadOutcome, VaultRepoSchemasError> {
    crate::storage_memstead::load_schemas_from_memstead_ref(workspace_root)
}

/// The git-branch backend's [`SchemaSource`](memstead_base::schema_source::SchemaSource):
/// schemas live on the `__MEMSTEAD:schemas/<name>@<version>/` ref of the
/// workspace's `vault-repo/.git`. `read_schemas` wraps
/// [`load_schemas_from_ref`]; `write_schema` wraps
/// [`crate::storage_memstead::write_schema_to_memstead_ref`]. The engine
/// owns vault-repo state, so this type is constructed and used inside the
/// engine layer, never by an external consumer directly.
pub struct GitBranchSchemaSource {
    workspace_root: std::path::PathBuf,
}

impl GitBranchSchemaSource {
    /// Build a git-branch source for a workspace (its `vault-repo/.git`).
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self {
            workspace_root: workspace_root.to_path_buf(),
        }
    }
}

impl memstead_base::schema_source::SchemaSource for GitBranchSchemaSource {
    fn read_schemas(
        &self,
    ) -> Result<Vec<Arc<Schema>>, memstead_base::schema_source::SchemaSourceError> {
        match load_schemas_from_ref(&self.workspace_root) {
            Ok(LoadOutcome::Schemas(schemas)) => Ok(schemas),
            // No vault-repo or no `__MEMSTEAD:schemas/` subtree → nothing
            // to overlay; the catalogue falls back to built-ins.
            Ok(LoadOutcome::NoVaultRepo) | Ok(LoadOutcome::NoSchemas) => Ok(Vec::new()),
            Err(e) => Err(memstead_base::schema_source::SchemaSourceError::Read(e.to_string())),
        }
    }

    fn write_schema(
        &self,
        name: &str,
        version: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), memstead_base::schema_source::SchemaSourceError> {
        let gitdir = self.workspace_root.join("vault-repo").join(".git");
        crate::storage_memstead::write_schema_to_memstead_ref(&gitdir, name, version, files)
            .map(|_| ())
            .map_err(|e| memstead_base::schema_source::SchemaSourceError::Write(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Seed `<root>/vault-repo/.git/` as a real bare repo carrying both
    /// `__SYSTEM` (empty seed) and `__SCHEMAS:<name>/{schema.yaml,
    /// types/*.yaml}` for each supplied schema. Returns the temp root.
    fn seed_vault_repo_with_schemas(
        schemas: &[(&str, &str, &[(&str, &str)])],
    ) -> TempDir {
        let tmp = TempDir::new().unwrap();
        let gitdir = tmp.path().join("vault-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        let repo = gix::init_bare(&gitdir).unwrap();

        let actor = gix::actor::Signature {
            name: "test".into(),
            email: "test@example.com".into(),
            time: gix::date::Time { seconds: 0, offset: 0 },
        };

        // Seed `__SYSTEM` with an empty tree so the "is real vault-repo"
        // gate flips to true.
        {
            let mut buf = gix::date::parse::TimeBuf::default();
            let actor_ref = actor.to_ref(&mut buf);
            repo.commit_as(
                actor_ref,
                actor_ref,
                "refs/heads/__SYSTEM",
                "seed __SYSTEM",
                repo.empty_tree().id().detach(),
                Vec::<gix::ObjectId>::new(),
            )
            .unwrap();
        }

        // Seed `__SCHEMAS` with the supplied schemas keyed at the tree
        // root.
        let mut editor = repo.empty_tree().edit().unwrap();
        for (schema_name, manifest_yaml, type_files) in schemas {
            let manifest_blob = repo.write_blob(manifest_yaml.as_bytes()).unwrap().detach();
            editor
                .upsert(
                    format!("{schema_name}/schema.yaml"),
                    gix::objs::tree::EntryKind::Blob,
                    manifest_blob,
                )
                .unwrap();
            for (stem, contents) in *type_files {
                let blob = repo.write_blob(contents.as_bytes()).unwrap().detach();
                editor
                    .upsert(
                        format!("{schema_name}/types/{stem}.yaml"),
                        gix::objs::tree::EntryKind::Blob,
                        blob,
                    )
                    .unwrap();
            }
        }
        let tree_id = editor.write().unwrap().detach();

        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        repo.commit_as(
            actor_ref,
            actor_ref,
            "refs/heads/__SCHEMAS",
            "seed __SCHEMAS",
            tree_id,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();

        // Project the just-written legacy refs onto `__MEMSTEAD` so the
        // schema reader cutover (s139) doesn't break this fixture.
        // The migrator is idempotent and shares the canonical
        // projection logic with the production migration helper.
        crate::storage_memstead::migrate_to_memstead_ref(&gitdir).unwrap();

        tmp
    }

    /// A minimal `software` schema seeded into
    /// The git-branch `SchemaSource` writes a package onto the
    /// `__MEMSTEAD` ref and reads it back — the read/write surface the
    /// engine resolves authored git-branch schemas through.
    #[test]
    fn git_branch_source_round_trips_a_written_package() {
        use memstead_base::schema_source::SchemaSource;

        let tmp = TempDir::new().unwrap();
        let gitdir = tmp.path().join("vault-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        gix::init_bare(&gitdir).unwrap();

        let source = GitBranchSchemaSource::for_workspace(tmp.path());
        // Empty ref → no authored schemas to overlay.
        assert!(source.read_schemas().unwrap().is_empty());

        let manifest = br#"name: refsrc
version: 0.1.0
description: A git-branch SchemaSource round-trip fixture.
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        let doc = br#"name: doc
description: t
when_to_use: here
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: _default
propagating_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;
        source
            .write_schema(
                "refsrc",
                "0.1.0",
                &[
                    ("schema.yaml".to_string(), manifest.to_vec()),
                    ("types/doc.yaml".to_string(), doc.to_vec()),
                ],
            )
            .unwrap();

        let schemas = source.read_schemas().unwrap();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].manifest.name, "refsrc");
    }

    /// `vault-repo-git:__SCHEMAS:software/` loads as a single
    /// `Schema` whose manifest name + version match the embedded
    /// YAML. Pins the gix-tree-walk path against the smallest valid
    /// schema shape — one type, the required `_default` relationship.
    #[test]
    fn schema_registry_loads_software_schema_from_schemas_ref() {
        let manifest = r#"name: software
version: 1.0.0
description: Minimal software schema for the vault-repo gix-loader test.
when_to_use: In the vault_repo_schemas loader test only.
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: Hierarchical containment
      default_weight: 3.0
    - name: REFERENCES
      description: Soft reference
      default_weight: 0.5
    - name: _default
      description: Fallback weight for unknown relationships
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        let sample_type = r#"name: sample
description: Sample type for tests
when_to_use: Whenever a minimal type is needed
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules:
      - One sentence describing the body.
metadata_fields:
  - key: status
    description: Lifecycle state
    field_type: string
    default_value: active
    enum_values:
      - active
      - closed
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
propagating_relationships: []
updatable_fields:
  - title
  - body
  - status
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules:
  - Keep it short.
"#;
        let tmp = seed_vault_repo_with_schemas(&[(
            "software",
            manifest,
            &[("sample", sample_type)],
        )]);

        let outcome = load_schemas_from_ref(tmp.path()).expect("loader must succeed");
        let schemas = match outcome {
            LoadOutcome::Schemas(s) => s,
            other => panic!(
                "expected Schemas outcome, got: {}",
                match other {
                    LoadOutcome::NoVaultRepo => "NoVaultRepo",
                    LoadOutcome::NoSchemas => "NoSchemas",
                    LoadOutcome::Schemas(_) => unreachable!(),
                }
            ),
        };
        assert_eq!(schemas.len(), 1, "expected one schema, got {}", schemas.len());
        let schema = &schemas[0];
        assert_eq!(schema.manifest.name, "software");
        assert_eq!(schema.version, semver::Version::new(1, 0, 0));
    }

    /// A workspace without `vault-repo/.git/` returns `NoVaultRepo` so
    /// the caller can fall back to its disk-based schema source.
    #[test]
    fn no_vault_repo_returns_no_vault_repo() {
        let tmp = TempDir::new().unwrap();
        let outcome = load_schemas_from_ref(tmp.path()).expect("loader must not error");
        assert!(matches!(outcome, LoadOutcome::NoVaultRepo));
    }

    /// An empty stub vault-repo (created by `init_vault_repo_stub` —
    /// `gix::init_bare` with no commits) carries no `__SYSTEM` ref and
    /// returns `NoVaultRepo`. Pins the legacy-fixture compatibility
    /// also relied on by the engine's "is real vault-repo" gate.
    #[test]
    fn empty_stub_vault_repo_returns_no_vault_repo() {
        let tmp = TempDir::new().unwrap();
        let gitdir = tmp.path().join("vault-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        gix::init_bare(&gitdir).unwrap();
        let outcome = load_schemas_from_ref(tmp.path()).expect("loader must not error");
        assert!(matches!(outcome, LoadOutcome::NoVaultRepo));
    }

    /// A real vault-repo whose `__SYSTEM` ref exists but `__SCHEMAS` is
    /// absent returns `NoSchemas` — the caller may still consult its
    /// disk-based source. Pins the LoadOutcome variant the
    /// orchestrator branches on.
    ///
    /// Post-s139 cutover: the loader reads `__MEMSTEAD` only. A vault-repo
    /// with no `__MEMSTEAD` ref surfaces as `NoVaultRepo`, not `NoSchemas`
    /// (the latter is reserved for `__MEMSTEAD` exists but its `schemas/`
    /// subtree is empty). The orchestrator branches both variants the
    /// same way (fall back to disk overlay), so the runtime contract
    /// is preserved even though the outcome variant changed.
    #[test]
    fn vault_repo_without_memstead_ref_returns_no_vault_repo() {
        let tmp = TempDir::new().unwrap();
        let gitdir = tmp.path().join("vault-repo").join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        let repo = gix::init_bare(&gitdir).unwrap();

        let actor = gix::actor::Signature {
            name: "test".into(),
            email: "test@example.com".into(),
            time: gix::date::Time { seconds: 0, offset: 0 },
        };
        let mut buf = gix::date::parse::TimeBuf::default();
        let actor_ref = actor.to_ref(&mut buf);
        repo.commit_as(
            actor_ref,
            actor_ref,
            "refs/heads/__SYSTEM",
            "seed __SYSTEM",
            repo.empty_tree().id().detach(),
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();

        let outcome = load_schemas_from_ref(tmp.path()).expect("loader must not error");
        assert!(matches!(outcome, LoadOutcome::NoVaultRepo));
    }

    // The earlier `loader_prefers_memstead_when_present` and
    // `loader_falls_back_to_legacy_when_memstead_absent` tests (and their
    // `minimal_manifest` / `minimal_type_yaml` helpers) were retired
    // in s139 alongside the legacy `__SCHEMAS` body — there is no
    // longer a fallback path to verify, and the fixture's migrator
    // call (s139) means every fixture-built workspace serves schemas
    // from `__MEMSTEAD` by construction. The
    // `schema_registry_loads_software_schema_from_schemas_ref` test
    // above already pins that path.

    /// Helper for the missing LoadOutcome::Schemas Debug — we want
    /// pattern matching to print the variant name on failure.
    impl std::fmt::Debug for LoadOutcome {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                LoadOutcome::NoVaultRepo => f.write_str("NoVaultRepo"),
                LoadOutcome::NoSchemas => f.write_str("NoSchemas"),
                LoadOutcome::Schemas(s) => write!(f, "Schemas({})", s.len()),
            }
        }
    }
}
