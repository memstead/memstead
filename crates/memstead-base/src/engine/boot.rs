//! Engine construction — `from_mounts*` and `from_workspace_root`.
//!
//! `from_mounts` is the in-process constructor every test, the macOS
//! UniFFI consumer, and the MCP filesystem server reach through.
//! `from_workspace_root` is the lean boot helper that produces the
//! same engine from a workspace root; the full counterpart lives in
//! `memstead_git_branch::engine_from_workspace_root` and follows the same
//! shape with the git-branch backend added to the factory.
//!
//! Free helpers in this module materialise the workspace schemas
//! catalogue, walk each mount's backend at load-time, and synthesise
//! the [`MemRouterSnapshot`] from the resolved mount list — pieces
//! the two entry points share.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use memstead_schema::Schema;

use crate::backend::MemBackend;
use crate::engine_fallback_type;
use crate::entity::loader::parse_entries;
use crate::entity::source::{SourceEntry, SourceReadError};
use crate::entity::store_builder::push_entities_into_store;
use crate::mem::{MemOrigin, MemRouterSnapshot};
use crate::ops::WarningHint;
use crate::store::Store;
use crate::workspace::{Mount, MountCapability, MountStorage, WorkspaceSettings};

use super::{BootError, Engine, EngineError, MountedBackend};

impl Engine {
    /// Build an engine from `(mount, backend)` pairs. The backend
    /// is the implementor that will serve reads / writes for that
    /// mount's mem.
    ///
    /// Returns [`EngineError::DuplicateMem`] when two mounts name
    /// the same mem; that's a configuration error the caller must
    /// fix before the engine can route deterministically. An empty
    /// mount list is allowed (returns an engine that errors
    /// `UnknownMem` on every read) — useful for tests; production
    /// callers will reject empty inputs at the persistence-adapter
    /// layer.
    pub fn from_mounts(mounts: Vec<(Mount, Box<dyn MemBackend>)>) -> Result<Self, EngineError> {
        Self::from_mounts_inner(mounts, Vec::new())
    }

    /// Construct an engine from mounts plus an optional workspace
    /// schemas directory. Loads every subdirectory of `schemas_dir`
    /// as a workspace-authored schema and combines with the builtin
    /// catalogue for per-mem schema-pin resolution. Workspace
    /// schemas take precedence on (name, version) collision —
    /// matches full's behaviour.
    ///
    /// `schemas_dir = None` is equivalent to [`Self::from_mounts`].
    /// Used by `engine_from_workspace_root` to thread the
    /// `[schemas_dir]` workspace-toml entry into schema resolution.
    pub fn from_mounts_with_schemas_dir(
        mounts: Vec<(Mount, Box<dyn MemBackend>)>,
        schemas_dir: Option<&Path>,
    ) -> Result<Self, EngineError> {
        let extra_schemas = load_workspace_schemas(schemas_dir)?;
        Self::from_mounts_inner(mounts, extra_schemas)
    }

    /// Like [`Self::from_mounts_with_schemas_dir`] but layers additional,
    /// pre-loaded local-storage schemas (e.g. those a git-branch backend
    /// reads from its `__MEMSTEAD:schemas/` ref via `SchemaSource`) on
    /// top of the folder `schemas_dir` set. Both are local-storage
    /// schemas — they override built-ins on `(name, version)` collision.
    /// The git-branch boot path uses this to make ref-installed schemas
    /// resolvable, which `from_mounts_with_schemas_dir` (folder only)
    /// does not.
    pub fn from_mounts_with_schemas_dir_and_extra(
        mounts: Vec<(Mount, Box<dyn MemBackend>)>,
        schemas_dir: Option<&Path>,
        mut extra: Vec<Arc<memstead_schema::Schema>>,
    ) -> Result<Self, EngineError> {
        let mut local = load_workspace_schemas(schemas_dir)?;
        local.append(&mut extra);
        Self::from_mounts_inner(mounts, local)
    }

    pub(crate) fn from_mounts_inner(
        mounts: Vec<(Mount, Box<dyn MemBackend>)>,
        extra_schemas: Vec<Arc<memstead_schema::Schema>>,
    ) -> Result<Self, EngineError> {
        let mut seen: std::collections::HashSet<String> =
            std::collections::HashSet::with_capacity(mounts.len());
        let mut mounted: Vec<MountedBackend> = Vec::with_capacity(mounts.len());
        for (mount, backend) in mounts {
            if !seen.insert(mount.mem.clone()) {
                return Err(EngineError::DuplicateMem(mount.mem));
            }
            // Seed the per-mount drift baseline. A backend that
            // doesn't track HEAD (folder, archive) returns Ok(None)
            // — drift detection is then a no-op for the mount. A
            // probe failure during init falls back to None so a
            // later successful probe can establish the baseline.
            let last_known_head = backend.current_head().ok().flatten();
            // Load the per-mem `.memstead/config.json` via the
            // backend trait. Each backend resolves its own
            // canonical location (folder: `<root>/.memstead/config.json`;
            // archive: inside the zip; git-branch:
            // `__MEMSTEAD:mems/<leaf>/config.json`). Read failures
            // or missing files surface as
            // `None` — `memstead_health` accommodates the missing-config
            // case (handler emits empty `writeGuidance` + `extra`).
            let mem_config = backend.read_mem_config().ok().flatten().and_then(|bytes| {
                let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
                memstead_schema::config::parse_mem_config(&value).ok()
            });
            // Read the optional authoring-provenance payload the archive
            // carries (`.memstead/provenance.json`). A malformed payload is
            // downgraded to `None` (the member is additive — a parse
            // failure means "provenance absent", not "mount failed").
            let archive_provenance =
                backend
                    .read_archive_provenance()
                    .ok()
                    .flatten()
                    .and_then(|bytes| {
                        memstead_schema::ArchiveProvenance::from_archive_bytes(&bytes).ok()
                    });
            mounted.push(MountedBackend {
                mount,
                backend,
                last_known_head,
                mem_config,
                archive_provenance,
            });
        }

        // Walk each backend, parse entries, populate one shared Store.
        // Resolve each mount's schema pin against the built-in schema
        // catalogue. The schema-registry resolver (which would also
        // honor workspace-authored schemas living inside the storage
        // backend) lands as a separate plan; this resolution closes
        // the gap for the built-in catalogue so a workspace pinning
        // a non-default built-in (e.g. `software`, `memory`) surfaces
        // the right schema rather than silently downgrading to
        // `default`.
        let builtin_schemas_only = memstead_schema::builtins::load_builtin_schemas()
            .map_err(|e| EngineError::SchemaResolverInit(e.to_string()))?;
        // Workspace-authored schemas resolve first (override builtins
        // on (name, version) collision); builtins fill the rest.
        let workspace_schemas = extra_schemas.clone();
        let mut catalogue: Vec<Arc<memstead_schema::Schema>> =
            Vec::with_capacity(extra_schemas.len() + builtin_schemas_only.len());
        catalogue.extend(extra_schemas);
        catalogue.extend(builtin_schemas_only.clone());
        let builtin_schemas = catalogue;
        let mut store = Store::new();
        let mut load_errors: Vec<(PathBuf, String)> = Vec::new();
        let mut schemas: HashMap<String, Arc<Schema>> = HashMap::with_capacity(mounted.len());
        let fallback = engine_fallback_type();

        // Derive the mem roster + last-segment suffixes ONCE so the
        // per-mount load loop hands the same view to every
        // `LoadCollector`. `known_suffixes` is the input the
        // nested-prefix detector compares against; the full
        // `mem_names` list feeds the two-pass cross-mem resolver
        // in `push_entities_into_store`.
        let mem_names: Vec<String> = mounted.iter().map(|m| m.mount.mem.clone()).collect();
        let known_suffixes: Vec<String> = mem_names
            .iter()
            .map(|n| crate::entity::store_builder::last_segment_suffix(n).to_string())
            .collect();
        let mut load_warnings: Vec<WarningHint> = Vec::new();

        for m in &mounted {
            // Schema-pin authority: the mem's own per-mem config is
            // the authoritative settled pin, so a copied or cloned mem
            // resolves its schema from its own backend without consulting
            // this workspace's `mounts.json`. `Mount.schema` (the mount
            // record's pin) is the fallback when the config carries no
            // schema, and an expectation assertion when it does — a
            // disagreement surfaces a `SchemaPinMismatch` warning rather
            // than silently preferring either.
            let config_pin = m.mem_config.as_ref().and_then(|c| c.schema.as_ref());
            let mount_pin = m.mount.schema.as_ref();
            // `Mount.schema` is an optional expectation assertion: warn
            // only when it is set *and* disagrees with the authoritative
            // config pin.
            if let (Some(cfg), Some(mp)) = (config_pin, mount_pin)
                && cfg != mp
            {
                load_warnings.push(WarningHint::SchemaPinMismatch {
                    mem: m.mount.mem.clone(),
                    config_pin: cfg.as_display(),
                    mount_pin: mp.as_display(),
                });
            }
            // Authoritative pin first (the backend config), then the
            // mount assertion as fallback when the config carries none.
            let settled_pin = config_pin.or(mount_pin);
            // Dual-pin: a mem mid-migration validates against the
            // migration target, not the settled pin.
            let effective_pin = m
                .mount
                .migration_target
                .as_ref()
                .or(settled_pin)
                .ok_or_else(|| EngineError::MemConfigIncomplete {
                    mem: m.mount.mem.clone(),
                    missing_fields: vec!["schema".to_string()],
                })?;
            let schema = SchemaResolver::new(&builtin_schemas)
                .resolve(effective_pin)
                .map_err(|sources| EngineError::SchemaNotFound {
                    mem: m.mount.mem.clone(),
                    pin: effective_pin.as_display(),
                    sources,
                })?;
            schemas.insert(m.mount.mem.clone(), schema.clone());

            let (entries, read_errors) = collect_source_entries(m.backend.as_ref())?;
            let load_result = parse_entries(entries, read_errors, &m.mount.mem, schema.as_ref());
            // Wire the LoadCollector so the parser/store-builder
            // pipeline forwards typed drift warnings
            // (`SuspiciousNestedPrefix`, `DuplicateSectionHeading`,
            // `InlineWikiLinkAutoStubbed`) into `load_warnings`.
            // Mutation paths still pass `None` to stay silent.
            push_entities_into_store(
                &mut store,
                load_result.entities,
                fallback.as_ref(),
                Some(crate::entity::store_builder::LoadCollector {
                    warnings: &mut load_warnings,
                    known_suffixes: &known_suffixes,
                    mem_names: &mem_names,
                }),
            );
            load_errors.extend(load_result.errors);
        }

        // Parse-time relation validation runs after every mount's
        // entities are loaded so cross-mem target types are
        // resolvable. Hand-edits, external tooling, and the macOS
        // app's editor surface can inject relations that bypass
        // `memstead_relate`; this is the only place those get caught.
        // Mutation paths pre-validate before writing, so they
        // never trip the warning post-load.
        let mount_caps: std::collections::HashMap<String, crate::workspace::MountCapability> =
            mounted
                .iter()
                .map(|m| (m.mount.mem.clone(), m.mount.capability))
                .collect();
        crate::entity::store_builder::validate_loaded_relations(
            &mut store,
            &schemas,
            &mount_caps,
            &mut load_warnings,
        );

        // Stamp `EdgeSource::BodyLink` on edges whose rel-type matches
        // the source mem's `alias_target_rel_type` pointer. Runs
        // after `validate_loaded_relations` so the surviving relation
        // set is schema-clean before the labeling pass.
        crate::entity::store_builder::remap_alias_target_edge_sources(&mut store, &schemas);

        // Derive the runtime mem router from the mount list.
        // Mirrors full's `Engine::from_init` step that registers every
        // mount with `MemRouterSnapshot` so handlers reach a
        // consistent writable/visible roster regardless of which
        // backend serves the mem.
        let mem_router = build_mem_router_from_mounts(&mounted);

        Ok(Self {
            mounts: mounted,
            store,
            schemas,
            workspace_schemas,
            builtin_schemas: builtin_schemas_only,
            load_errors,
            community_memo: OnceCell::new(),
            #[cfg(not(target_arch = "wasm32"))]
            search_indexes_memo: OnceCell::new(),
            settings: WorkspaceSettings::default(),
            create_rule_set_memo: OnceCell::new(),
            declared_origins: HashMap::new(),
            workspace_root: None,
            load_warnings,
            pipeline_configs: crate::pipeline_store::PipelineConfigs::default(),
            mem_router: Arc::new(mem_router),
            backend_factory: crate::workspace_store::instantiate_lean_backend,
            git_branch_ops: None,
            event_subscribers: Arc::new(std::sync::Mutex::new(
                crate::engine::events::SubscriberRegistry::new(),
            )),
            pending_mem_changed: Vec::new(),
        })
    }

    /// Boot an engine from a workspace root using only lean-flavour
    /// backends (folder + archive). The MCP filesystem server, the
    /// CLI's lean dispatcher, and the macOS UniFFI consumer all reach
    /// the new engine through this entry point — replacing per-flavour
    /// init code with one call.
    ///
    /// Loads the workspace through [`crate::FileWorkspaceStore`],
    /// instantiates each mount's backend via
    /// [`crate::instantiate_lean_backend`], and constructs the
    /// engine via [`Engine::from_mounts`].
    ///
    /// Errors:
    /// - [`Layout::Empty`](crate::Layout) → [`BootError::NotInitialised`]
    /// - any mount declaring [`crate::workspace::MountStorage::GitBranch`]
    ///   → [`BootError::Instantiate`] wrapping
    ///   [`crate::InstantiateError::GitBranchRequiresMemRepoFeature`]
    /// - underlying store / engine failures lift through the
    ///   `#[from]` conversions
    pub fn from_workspace_root(workspace_root: &Path) -> Result<Self, BootError> {
        use crate::workspace_store::{
            FileWorkspaceStore, Layout, WorkspaceStoreAdapter, detect_layout,
            instantiate_lean_backend,
        };

        let workspace = match detect_layout(workspace_root) {
            // Standalone collapse: a bare folder mem (`.memstead/config.json`,
            // no `workspace.toml`) roots as a one-mount workspace rather than
            // refusing — the lone-mem boot path is the unified one.
            Layout::Empty => match crate::workspace_store::standalone_workspace(workspace_root) {
                Some(ws) => ws,
                None => {
                    return Err(BootError::NotInitialised(workspace_root.to_path_buf()));
                }
            },
            Layout::New => FileWorkspaceStore::new().load(workspace_root)?,
        };

        let settings = workspace.settings.clone();
        let mut mounts: Vec<(Mount, Box<dyn MemBackend>)> =
            Vec::with_capacity(workspace.mounts.len());
        for mount in workspace.mounts {
            let backend = instantiate_lean_backend(&mount)?;
            mounts.push((mount, backend));
        }
        // Folder-backend authoring path: authored schema packages live
        // at the fixed `<workspace>/.memstead/schemas/<name>@<version>/`
        // location — the folder analogue of the git-branch backend's
        // `__MEMSTEAD:schemas/` ref. Read them through the folder
        // `SchemaSource` (which no-ops when the directory is absent, so a
        // workspace that authored no schemas resolves exactly as before —
        // built-ins only). This is the lean flavour's schema-authoring
        // path, which it lacked.
        use crate::schema_source::SchemaSource as _;
        let local = crate::schema_source::FolderSchemaSource::for_workspace(workspace_root)
            .read_schemas()
            .map_err(|e| EngineError::SchemaResolverInit(e.to_string()))?;
        let mut engine = Engine::from_mounts_inner(mounts, local)?;
        engine.set_settings(settings);
        engine.workspace_root = Some(workspace_root.to_path_buf());
        // Load the workspace store's pipeline configs (Medium / Facet /
        // Projection / Ingest) and expose them read-only. A malformed
        // config surfaces a typed `StoreError::Parse` naming the file —
        // early validation of operator-edited configs (the loader's stated
        // value). Absent primitive directories resolve to empty.
        // Authored pipeline configs (Medium / Facet / Projection / Ingest)
        // from the workspace store. The legacy `scopes|projections|ingests/`
        // JSON folders are no longer read at boot — the migration-window
        // compatibility shim retired once the bundled pipelines migrated
        // (2026-06-14). `memstead pipeline migrate` is the only path from
        // old-shape configs into the store. A malformed config surfaces a
        // typed `StoreError::Parse` naming the file.
        engine.set_pipeline_configs(crate::pipeline_store::load_pipeline_configs(
            workspace_root,
        )?);
        // Publish the authoring meta-schemas into `.memstead/meta-schemas/`
        // so an editor validates authored schema YAML against them
        // (resolved by each package's `# yaml-language-server:` directive).
        // Best-effort — a read-only workspace still boots.
        let _ = memstead_schema::meta_schema::publish_meta_schemas(workspace_root);
        Ok(engine)
    }
}

/// Derive a [`MemRouterSnapshot`] from the engine's resolved mount
/// list. Mirrors full's `Engine::from_init` mount-register loop so the
/// runtime router carries the same writable/visible roster regardless
/// of which backend serves each mem.
///
/// One pass over the mounts:
/// - Writable mounts ([`MountCapability::Write`]) register via
///   `add_writable` with the storage's worktree path. Folder mounts
///   surface `MountStorage::Folder.path`; git-branch mounts surface
///   `None` (the mem content lives only inside the gitdir).
///   Archive mounts should never be writable; if one slips through,
///   it registers with `dir: None`.
/// - Read-only folder / git-branch mounts also register via
///   `add_writable` with `dir: None`, then are *visible-only* —
///   `is_writable` returns `false` because we follow up with a
///   `remove_writable` (no-op for archives because archives are
///   registered as `add_read_only`).
///
/// Actually we keep it simple: writable mounts go through
/// `add_writable`; read-only mounts go through `add_read_only` with
/// a synthesized archive-style path. For folder/git-branch read-only
/// mounts we use the path the storage offers as the archive_path
/// argument — semantically wrong but the router treats
/// `add_read_only` data as opaque for visibility tracking. The two
/// callers that care (`archive_path_for_mem`, `dir_for_mem`)
/// branch on backend type at the handler level rather than reading
/// these synthesized paths.
///
/// Origin is `MemOrigin::ExplicitToml` for every mount built from
/// `Workspace.mounts` — the file-adapter case. `RuntimeCreated`
/// origins land when `memstead_mem_create` migrates onto the unified
/// engine and produces fresh runtime registrations.
fn build_mem_router_from_mounts(mounts: &[MountedBackend]) -> MemRouterSnapshot {
    let mut router = MemRouterSnapshot::new();
    for m in mounts {
        match m.mount.capability {
            MountCapability::Write => {
                let dir: Option<PathBuf> = match &m.mount.storage {
                    MountStorage::Folder { path } => Some(path.clone()),
                    MountStorage::GitBranch { .. } => None,
                    MountStorage::Archive { .. } => None,
                    // In-memory mounts have no on-disk working dir —
                    // they register writable with `dir: None`, the same
                    // shape mem-repo-backed mounts use.
                    MountStorage::InMemory => None,
                };
                router.add_writable(m.mount.mem.clone(), dir, MemOrigin::ExplicitToml);
            }
            MountCapability::ReadOnly => match &m.mount.storage {
                MountStorage::Archive { path } => {
                    router.add_read_only(m.mount.mem.clone(), path.clone());
                }
                MountStorage::Folder { path } => {
                    router.add_read_only(m.mount.mem.clone(), path.clone());
                }
                MountStorage::GitBranch { gitdir, .. } => {
                    router.add_read_only(m.mount.mem.clone(), gitdir.clone());
                }
                // A read-only in-memory mount has no on-disk read
                // source to register. The engine never produces this
                // configuration (in-memory mounts are created writable
                // for ephemeral sessions); handled here only to keep
                // the match total.
                MountStorage::InMemory => {}
            },
        }
    }
    router
}

/// Public re-export of [`resolve_builtin_schema_pin`] for lifecycle
/// orchestrators in `memstead-engine`. Mirrors full's
/// `resolve_mem_schema` against the built-in catalogue;
/// workspace-schema-registry resolution lifts later.
pub fn resolve_builtin_schema_pin_pub(
    pin: &memstead_schema::SchemaRef,
    catalogue: &[Arc<memstead_schema::Schema>],
) -> Option<Arc<memstead_schema::Schema>> {
    resolve_builtin_schema_pin(pin, catalogue)
}

/// The engine's schema-pin resolver — the single named entry point a
/// load path resolves a `name@version` pin through. Consults schema
/// sources in a fixed order: **local storage** (the mem's own storage
/// backend — folder `.memstead/schemas/` or the git-branch
/// `__MEMSTEAD:schemas/` ref, layered first into the catalogue so it
/// wins on `(name, version)` collision), **built-in** (compiled into the
/// binary), **remote** (memstead.io, reserved, not implemented). The
/// order is fixed in code — local-over-built-in by the catalogue's
/// insertion precedence, remote always last. On a miss it yields the
/// per-source [`SchemaSourceDiagnostic`] trail the `SCHEMA_NOT_FOUND`
/// envelope carries.
///
/// Holds a borrowed view of the merged catalogue (`local ⧺ built-in`)
/// the boot / register paths assemble, so resolution allocates nothing.
pub struct SchemaResolver<'a> {
    catalogue: &'a [Arc<memstead_schema::Schema>],
}

impl<'a> SchemaResolver<'a> {
    /// Wrap the merged resolution catalogue (workspace-authored schemas
    /// layered over the built-in set, local winning on collision).
    pub fn new(catalogue: &'a [Arc<memstead_schema::Schema>]) -> Self {
        Self { catalogue }
    }

    /// Resolve a pin to its schema, or the fixed-order source
    /// diagnostics on a miss (fed straight into
    /// `EngineError::SchemaNotFound`'s `sources`).
    pub fn resolve(
        &self,
        pin: &memstead_schema::SchemaRef,
    ) -> Result<Arc<memstead_schema::Schema>, Vec<crate::engine::error::SchemaSourceDiagnostic>>
    {
        resolve_builtin_schema_pin(pin, self.catalogue).ok_or_else(|| {
            crate::engine::error::SchemaSourceDiagnostic::for_failed_pin(
                &pin.name,
                &pin.version,
                self.catalogue,
            )
        })
    }
}

/// Walk `schemas_dir` and load every immediate subdirectory as a
/// workspace-authored schema. Each subdirectory must contain a
/// `schema.yaml` manifest (and optional `types/*.yaml`) — silently
/// skips entries that don't carry the manifest. `pub(crate)` so the
/// folder `SchemaSource` reads through the same walker the boot path uses.
pub(crate) fn load_workspace_schemas(
    schemas_dir: Option<&Path>,
) -> Result<Vec<Arc<memstead_schema::Schema>>, EngineError> {
    let Some(dir) = schemas_dir else {
        return Ok(Vec::new());
    };
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };
    let mut schemas: Vec<Arc<memstead_schema::Schema>> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !path.join("schema.yaml").is_file() {
            continue;
        }
        let schema = memstead_schema::load_schema_from_dir(&path)
            .map_err(|e| EngineError::SchemaResolverInit(e.to_string()))?;
        schemas.push(Arc::new(schema));
    }
    Ok(schemas)
}

pub(super) fn resolve_builtin_schema_pin(
    pin: &memstead_schema::SchemaRef,
    catalogue: &[Arc<memstead_schema::Schema>],
) -> Option<Arc<memstead_schema::Schema>> {
    catalogue
        .iter()
        .find(|s| {
            let id = s.id();
            id.0 == pin.name && id.1 == pin.version
        })
        .cloned()
}

pub(super) fn collect_source_entries(
    backend: &dyn MemBackend,
) -> Result<(Vec<SourceEntry>, Vec<SourceReadError>), EngineError> {
    let paths = backend.list_entities()?;
    let mut entries: Vec<SourceEntry> = Vec::with_capacity(paths.len());
    let mut errors: Vec<SourceReadError> = Vec::new();
    for path in paths {
        match backend.read_entity(&path) {
            Ok(Some(bytes)) => match String::from_utf8(bytes) {
                Ok(content) => entries.push(SourceEntry {
                    relative_path: path.to_string_lossy().into_owned(),
                    source_path: path.clone(),
                    content,
                }),
                Err(e) => errors.push(SourceReadError {
                    source_path: path,
                    error: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
                }),
            },
            Ok(None) => {
                // Listed-but-absent: list/read race. Skip silently.
            }
            Err(e) => errors.push(SourceReadError {
                source_path: path,
                error: std::io::Error::other(e.to_string()),
            }),
        }
    }
    Ok((entries, errors))
}

#[cfg(test)]
mod tests {

    use std::path::Path;

    use memstead_schema::SchemaRef;
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::test_helpers::*;
    use crate::engine::{Engine, EngineError};
    use crate::ops::WarningHint;
    use crate::storage::{ArchiveBackend, FilesystemMemWriter, MemWriter};
    use crate::vcs::CommitContext;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    /// The `SchemaResolver` resolves a pin against the catalogue and, on
    /// a miss, yields the fixed-order (`local_storage` → `builtin` →
    /// `remote`) source diagnostics the `SCHEMA_NOT_FOUND` envelope carries.
    #[test]
    fn schema_resolver_resolves_builtin_and_yields_ordered_diagnostics_on_miss() {
        let catalogue = memstead_schema::builtins::load_builtin_schemas().unwrap();
        let resolver = super::SchemaResolver::new(&catalogue);

        let ok: SchemaRef = "default@1.0.0".parse().unwrap();
        assert!(resolver.resolve(&ok).is_ok(), "shipped built-in resolves");

        let miss: SchemaRef = "nope@9.9.9".parse().unwrap();
        let sources = resolver.resolve(&miss).unwrap_err();
        let labels: Vec<&str> = sources.iter().map(|s| s.source).collect();
        assert_eq!(labels, ["local_storage", "builtin", "remote"]);
        assert!(sources.iter().all(|s| !s.pinned_version_match));
    }

    #[test]
    fn empty_mount_list_constructs_and_errors_unknown_mem_on_read() {
        let engine = Engine::from_mounts(Vec::new()).unwrap();
        assert!(engine.mem_names().is_empty());
        match engine.list_entities("missing") {
            Err(EngineError::UnknownMem(v)) => assert_eq!(v, "missing"),
            other => panic!("expected UnknownMem, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_mem_names_rejected_at_construction() {
        let tmp = TempDir::new().unwrap();
        let writer1: Box<dyn MemBackend> =
            Box::new(FilesystemMemWriter::new(tmp.path().to_path_buf()));
        let writer2: Box<dyn MemBackend> =
            Box::new(FilesystemMemWriter::new(tmp.path().to_path_buf()));
        let err = Engine::from_mounts(vec![
            (folder_mount("specs", tmp.path().to_path_buf()), writer1),
            (folder_mount("specs", tmp.path().to_path_buf()), writer2),
        ])
        .unwrap_err();
        assert!(matches!(err, EngineError::DuplicateMem(v) if v == "specs"));
    }

    #[test]
    fn from_mounts_populates_load_warnings_from_duplicate_section_heading() {
        // A markdown file with the same `## Identity` heading twice
        // should cause the parser to emit a typed
        // `DuplicateSectionHeading` warning. With the
        // LoadCollector wiring, that warning lands on
        // `engine.load_warnings()`.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let body =
            "---\ntype: spec\n---\n# Dup\n\n## Identity\n\nfirst.\n\n## Identity\n\nsecond.\n";
        std::fs::write(mem_dir.join("dup.md"), body).unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let warnings = engine.load_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, WarningHint::DuplicateSectionHeading { .. })),
            "load_warnings must surface DuplicateSectionHeading: {warnings:?}",
        );
    }

    /// Parse-time relation validation drops relations whose `rel_type`
    /// is not declared in the source mem's strict-mode schema and
    /// emits `PARSED_RELATION_INVALID { reason: "unknown_rel_type" }`.
    /// The entity itself loads normally; only the bad relation goes
    /// missing from the in-memory store.
    #[test]
    fn from_mounts_drops_unknown_rel_type_from_hand_edit_with_warning() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        // Hand-authored markdown with a `## Relationships` entry whose
        // type isn't declared in the default schema (strict mode).
        let target = "---\ntype: spec\n---\n# Target\n\n## Identity\n\nThe target.\n";
        let source = "---\ntype: spec\n---\n# Source\n\n## Identity\n\nThe source.\n\n## Relationships\n\n- **MADE_UP_TYPE**: [[specs--target]]\n";
        std::fs::write(mem_dir.join("target.md"), target).unwrap();
        std::fs::write(mem_dir.join("source.md"), source).unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let source_id = crate::entity::EntityId::new("specs", "source");
        let target_id = crate::entity::EntityId::new("specs", "target");
        let source_entity = engine.get_entity(&source_id).expect("source loaded");
        // The offending relation does not survive into the entity's
        // in-memory relationships list.
        assert!(
            source_entity.relationships.is_empty(),
            "MADE_UP_TYPE relation must be dropped from entity.relationships, got: {:?}",
            source_entity.relationships,
        );
        // Nor into the store's edge index.
        let outgoing: Vec<_> = engine
            .store()
            .outgoing(&source_id)
            .iter()
            .filter(|e| e.rel_type == "MADE_UP_TYPE")
            .collect();
        assert!(
            outgoing.is_empty(),
            "MADE_UP_TYPE edge must be dropped from the store"
        );
        // The warning surfaces with the correct payload.
        let parsed_invalid: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter_map(|w| match w {
                WarningHint::ParsedRelationInvalid {
                    entity_id,
                    rel_type,
                    target,
                    reason,
                    origin,
                    recovery,
                } => Some((
                    entity_id.clone(),
                    rel_type.clone(),
                    target.clone(),
                    reason.clone(),
                    origin.clone(),
                    recovery.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            parsed_invalid.len(),
            1,
            "expected one warning, got {parsed_invalid:?}"
        );
        assert_eq!(parsed_invalid[0].0, source_id);
        assert_eq!(parsed_invalid[0].1, "MADE_UP_TYPE");
        assert_eq!(parsed_invalid[0].2, target_id);
        assert_eq!(parsed_invalid[0].3, "unknown_rel_type");
        assert_eq!(parsed_invalid[0].4, "writable");
        // Writable-origin warnings carry the abstract recovery action.
        let recovery = parsed_invalid[0]
            .5
            .as_ref()
            .expect("writable-origin warning must carry recovery");
        assert_eq!(
            recovery.kind,
            crate::ops::ParsedRelationRecovery::KIND_REMOVE_EXPLICIT_RELATION
        );
        assert_eq!(recovery.source_id, parsed_invalid[0].0);
        assert_eq!(recovery.target_id, parsed_invalid[0].2);
        assert_eq!(recovery.rel_type, parsed_invalid[0].1);
    }

    /// Hand-edited markdown can inject a cycle in an `acyclic: true`
    /// rel-type's subgraph — the mutation surface's `would_cycle`
    /// guard never fires for that path. The boot validator's
    /// second pass finds the back-edge and drops it with
    /// `reason: "cycle"`. The entity itself loads normally; one of
    /// the two cycle-closing edges goes missing from the in-memory
    /// store; the other survives.
    #[test]
    fn from_mounts_drops_cycle_closing_edge_in_acyclic_subgraph() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        // Mutual PART_OF — acyclic in the default schema. The
        // wiki-link grammar admits both as well-formed cross-
        // references, so only the cycle pass can catch this.
        let alpha = "---\ntype: spec\n---\n# Alpha\n\n## Identity\n\nfirst.\n\n## Relationships\n\n- **PART_OF**: [[specs--beta]]\n";
        let beta = "---\ntype: spec\n---\n# Beta\n\n## Identity\n\nsecond.\n\n## Relationships\n\n- **PART_OF**: [[specs--alpha]]\n";
        std::fs::write(mem_dir.join("alpha.md"), alpha).unwrap();
        std::fs::write(mem_dir.join("beta.md"), beta).unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let alpha_id = crate::entity::EntityId::new("specs", "alpha");
        let beta_id = crate::entity::EntityId::new("specs", "beta");

        // Both entities are real — only the relation in the cycle
        // gets dropped.
        assert!(engine.get_entity(&alpha_id).is_some_and(|e| !e.stub));
        assert!(engine.get_entity(&beta_id).is_some_and(|e| !e.stub));

        // Exactly one of the two PART_OF edges survives — the cycle
        // is broken by dropping a single back-edge.
        let surviving: Vec<_> = engine
            .store()
            .all_entities()
            .flat_map(|e| {
                engine
                    .store()
                    .outgoing(&e.id)
                    .iter()
                    .filter(|edge| edge.rel_type == "PART_OF")
                    .map(|edge| (e.id.clone(), edge.target.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(
            surviving.len(),
            1,
            "exactly one PART_OF edge must survive the cycle break, got {surviving:?}",
        );

        // The warning surfaces with `reason: "cycle"` and names the
        // dropped pair.
        let cycle_drops: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter_map(|w| match w {
                WarningHint::ParsedRelationInvalid {
                    entity_id,
                    rel_type,
                    target,
                    reason,
                    ..
                } if reason == "cycle" => {
                    Some((entity_id.clone(), rel_type.clone(), target.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            cycle_drops.len(),
            1,
            "exactly one cycle warning must fire, got {cycle_drops:?}",
        );
        // The dropped edge is one of the two PART_OF entries.
        let (dropped_from, dropped_rel_type, dropped_to) = &cycle_drops[0];
        assert_eq!(dropped_rel_type, "PART_OF");
        let is_alpha_to_beta = dropped_from == &alpha_id && dropped_to == &beta_id;
        let is_beta_to_alpha = dropped_from == &beta_id && dropped_to == &alpha_id;
        assert!(
            is_alpha_to_beta || is_beta_to_alpha,
            "dropped edge must be one of the mutual PART_OF pair, got ({dropped_from} -> {dropped_to})",
        );
        // And the surviving edge isn't the same as the dropped one.
        assert_ne!(
            (&surviving[0].0, &surviving[0].1),
            (dropped_from, dropped_to),
            "surviving edge must differ from the dropped one",
        );
    }

    /// Shape-invalid relations on a writable-origin mount get dropped
    /// with `reason: "shape"`; the warning carries a
    /// `remove_explicit_relation` recovery hint whose ids and rel-type
    /// mirror the warning's top-level fields. Same envelope shape as
    /// the `unknown_rel_type` reason — `reason` discriminates the
    /// cause; `recovery.kind` discriminates the action. Uses a
    /// synthetic schema with `source_types` / `target_types`
    /// constraints because the default schema's rel-types are
    /// unconstrained.
    #[test]
    fn from_mounts_emits_recovery_hint_for_writable_shape_drop() {
        use crate::engine::test_helpers::write_schema_files_with_default_type;

        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();
        // A schema declaring a single rel-type whose shape only
        // admits `actor -> doc`. The source markdown below uses
        // `doc -> doc`, which trips the shape validator.
        let manifest = r#"name: shape-test
version: 0.1.0
description: shape-constraint schema
when_to_use: tests
types:
  - doc
  - actor
relationships:
  mode: strict
  definitions:
    - name: OWNS
      description: actor owns doc
      default_weight: 1.0
      source_types: [actor]
      target_types: [doc]
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        write_schema_files_with_default_type(
            &schemas_dir,
            "shape-test",
            manifest,
            &["doc", "actor"],
        );

        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        // Source is type `doc`; target is also type `doc`. The
        // declared `OWNS` rel-type expects `actor -> doc`, so the
        // shape check rejects this pair at load.
        let target = "---\ntype: doc\n---\n# Target\n\n## Body\n\nthe target\n";
        let source = "---\ntype: doc\n---\n# Source\n\n## Body\n\nthe source\n\n## Relationships\n\n- **OWNS**: [[specs--target]]\n";
        std::fs::write(mem_dir.join("target.md"), target).unwrap();
        std::fs::write(mem_dir.join("source.md"), source).unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let pin = SchemaRef::new("shape-test", semver::Version::new(0, 1, 0));
        let mount = Mount {
            mem: "specs".to_string(),
            schema: Some(pin),
            storage: MountStorage::Folder { path: mem_dir },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let engine = Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
            Some(&schemas_dir),
        )
        .unwrap();

        let source_id = crate::entity::EntityId::new("specs", "source");
        let target_id = crate::entity::EntityId::new("specs", "target");

        let shape_drops: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter_map(|w| match w {
                WarningHint::ParsedRelationInvalid {
                    entity_id,
                    rel_type,
                    target,
                    reason,
                    origin,
                    recovery,
                } if reason == "shape" => Some((
                    entity_id.clone(),
                    rel_type.clone(),
                    target.clone(),
                    origin.clone(),
                    recovery.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            shape_drops.len(),
            1,
            "expected one shape-reason warning, got {shape_drops:?}; all warnings = {:?}",
            engine.load_warnings(),
        );
        let (drop_from, drop_type, drop_to, drop_origin, drop_recovery) =
            shape_drops.into_iter().next().unwrap();
        assert_eq!(drop_from, source_id);
        assert_eq!(drop_type, "OWNS");
        assert_eq!(drop_to, target_id);
        assert_eq!(drop_origin, "writable");
        // Recovery mirrors the warning's top-level fields and names
        // the abstract `remove_explicit_relation` action.
        let recovery = drop_recovery.expect("writable origin must carry recovery");
        assert_eq!(
            recovery.kind,
            crate::ops::ParsedRelationRecovery::KIND_REMOVE_EXPLICIT_RELATION
        );
        assert_eq!(recovery.source_id, source_id);
        assert_eq!(recovery.target_id, target_id);
        assert_eq!(recovery.rel_type, "OWNS");
    }

    /// Read-only-origin warnings omit the recovery hint — the engine
    /// cannot rewrite a read-only mount's markdown, so no abstract
    /// action is available. The message field still names the
    /// operator-level path (uninstall the archive or accept the
    /// drift); structured consumers branch on `recovery.is_none()`.
    #[test]
    fn from_mounts_emits_no_recovery_hint_for_readonly_origin() {
        let tmp = TempDir::new().unwrap();
        // Archive content with a `MADE_UP_TYPE` row that the schema
        // does not declare — parses to a `PARSED_RELATION_INVALID`
        // with `reason: "unknown_rel_type"` on a read-only mount.
        let target = "---\ntype: spec\n---\n# Target\n\n## Identity\n\nThe target.\n";
        let source = "---\ntype: spec\n---\n# Source\n\n## Identity\n\nThe source.\n\n## Relationships\n\n- **MADE_UP_TYPE**: [[external--target]]\n";
        let archive_path = build_archive(
            tmp.path(),
            "ext",
            &[
                ("target.md", target.as_bytes()),
                ("source.md", source.as_bytes()),
            ],
        );

        let engine = Engine::from_mounts(vec![(
            archive_mount("external", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)),
        )])
        .unwrap();

        let invalid: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter_map(|w| match w {
                WarningHint::ParsedRelationInvalid {
                    rel_type,
                    reason,
                    origin,
                    recovery,
                    ..
                } => Some((
                    rel_type.clone(),
                    reason.clone(),
                    origin.clone(),
                    recovery.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            invalid.len(),
            1,
            "expected one parse-time drop on the readonly mount, got {invalid:?}",
        );
        assert_eq!(invalid[0].0, "MADE_UP_TYPE");
        assert_eq!(invalid[0].1, "unknown_rel_type");
        assert_eq!(invalid[0].2, "readonly");
        assert!(
            invalid[0].3.is_none(),
            "readonly-origin warning must omit the recovery hint, got {:?}",
            invalid[0].3,
        );
    }

    #[test]
    fn load_on_init_populates_store_from_folder_mount() {
        // Real markdown content: minimal but parses cleanly against
        // the builtin default schema.
        let body = "---\ntype: spec\n---\n# Hello\n\n## Identity\n\nA test entity.\n";

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        <FilesystemMemWriter as MemWriter>::write_entity(
            &writer,
            Path::new("hello.md"),
            body.as_bytes(),
        )
        .unwrap();
        <FilesystemMemWriter as MemWriter>::commit(&writer, "seed", &CommitContext::internal())
            .unwrap();

        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        // Store is populated.
        assert_eq!(engine.store().len(), 1, "expected one entity in the store");
        let id = crate::EntityId::new("specs", "hello");
        let entity = engine.get_entity(&id).expect("entity must be present");
        assert_eq!(entity.title, "Hello");
        assert_eq!(entity.entity_type, "spec");
        assert!(engine.load_errors().is_empty());
        // Schema map carries one entry per mount.
        assert_eq!(engine.schemas().len(), 1);
        assert!(engine.schemas().contains_key("specs"));
    }

    #[test]
    fn load_on_init_populates_store_from_archive_mount() {
        let body =
            "---\ntype: spec\n---\n# From Archive\n\n## Identity\n\nLives in a .memstead zip.\n";

        let tmp = TempDir::new().unwrap();
        let archive_path =
            build_archive(tmp.path(), "ext", &[("from-archive.md", body.as_bytes())]);

        let engine = Engine::from_mounts(vec![(
            archive_mount("external", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)),
        )])
        .unwrap();

        let id = crate::EntityId::new("external", "from-archive");
        let entity = engine.get_entity(&id).expect("entity must be present");
        assert_eq!(entity.title, "From Archive");
        assert!(engine.load_errors().is_empty());
    }

    #[test]
    fn load_on_init_populates_store_from_heterogeneous_mounts() {
        let folder_body = "---\ntype: spec\n---\n# Local\n\n## Identity\n\nLocal entity.\n";
        let archive_body = "---\ntype: spec\n---\n# External\n\n## Identity\n\nArchive entity.\n";

        let tmp = TempDir::new().unwrap();

        let folder_dir = tmp.path().join("folder-mem");
        std::fs::create_dir_all(&folder_dir).unwrap();
        let folder_writer = FilesystemMemWriter::new(folder_dir.clone());
        <FilesystemMemWriter as MemWriter>::write_entity(
            &folder_writer,
            Path::new("local.md"),
            folder_body.as_bytes(),
        )
        .unwrap();
        <FilesystemMemWriter as MemWriter>::commit(
            &folder_writer,
            "seed",
            &CommitContext::internal(),
        )
        .unwrap();

        let archive_path = build_archive(
            tmp.path(),
            "external",
            &[("external.md", archive_body.as_bytes())],
        );

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("local", folder_dir),
                Box::new(folder_writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("external", archive_path.clone()),
                Box::new(ArchiveBackend::new(archive_path)),
            ),
        ])
        .unwrap();

        // Both mems' entities live in one shared store.
        assert_eq!(engine.store().len(), 2);
        assert!(
            engine
                .get_entity(&crate::EntityId::new("local", "local"))
                .is_some()
        );
        assert!(
            engine
                .get_entity(&crate::EntityId::new("external", "external"))
                .is_some()
        );
    }

    #[test]
    fn load_on_init_collects_per_file_parse_errors_without_failing() {
        // One good file + one with malformed frontmatter — the parser
        // produces an error for the malformed file but the good one
        // still loads.
        let good = "---\ntype: spec\n---\n# Good\n\n## Identity\n\nFine.\n";
        let bad = "---\nthis is not valid yaml: : :\n---\n# Bad\n";

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        <FilesystemMemWriter as MemWriter>::write_entity(
            &writer,
            Path::new("good.md"),
            good.as_bytes(),
        )
        .unwrap();
        <FilesystemMemWriter as MemWriter>::write_entity(
            &writer,
            Path::new("bad.md"),
            bad.as_bytes(),
        )
        .unwrap();
        <FilesystemMemWriter as MemWriter>::commit(&writer, "seed", &CommitContext::internal())
            .unwrap();

        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        // The good entity is in the store; construction did not fail.
        assert!(
            engine
                .get_entity(&crate::EntityId::new("specs", "good"))
                .is_some(),
            "good.md must parse and reach the store"
        );
        // Either bad.md surfaces as a load error, or it parses
        // permissively — both are acceptable outcomes here. The
        // contract under test is "construction does not fail on a
        // single bad file".
        let bad_known_to_engine = engine
            .get_entity(&crate::EntityId::new("specs", "bad"))
            .is_some()
            || !engine.load_errors().is_empty();
        assert!(
            bad_known_to_engine,
            "bad.md must either parse or surface in load_errors"
        );
    }

    #[test]
    fn empty_mount_list_yields_empty_store() {
        let engine = Engine::from_mounts(Vec::new()).unwrap();
        assert!(engine.store().is_empty());
        assert!(engine.schemas().is_empty());
        assert!(engine.load_errors().is_empty());
    }

    // ---- Engine::create_entity --------------------------------------

    #[test]
    fn from_workspace_root_errors_for_empty_layout() {
        let tmp = TempDir::new().unwrap();
        let err = Engine::from_workspace_root(tmp.path()).unwrap_err();
        match err {
            crate::BootError::NotInitialised(p) => {
                assert_eq!(p, tmp.path());
            }
            other => panic!("expected NotInitialised, got {other:?}"),
        }
    }

    #[test]
    fn from_workspace_root_loads_new_two_layer_layout() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(
            mem_dir.join("hello.md"),
            "---\ntype: spec\n---\n# Hello\n\n## Identity\n\nA.\n",
        )
        .unwrap();

        let memstead = tmp.path().join(".memstead");
        std::fs::create_dir_all(&memstead).unwrap();
        std::fs::write(
            memstead.join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        // Save the mount via the file adapter so the JSON shape matches
        // the wire format the loader expects.
        use crate::workspace_store::WorkspaceStoreAdapter;
        let store = crate::FileWorkspaceStore::new();
        store
            .save_state(
                tmp.path(),
                &crate::workspace::Workspace {
                    mounts: vec![folder_mount("specs", mem_dir)],
                    settings: crate::workspace::WorkspaceSettings::default(),
                },
            )
            .unwrap();

        let engine = Engine::from_workspace_root(tmp.path()).unwrap();
        assert_eq!(engine.mem_names(), vec!["specs"]);
        let entity = engine
            .get_entity(&crate::EntityId::new("specs", "hello"))
            .expect("seeded entity must load through from_workspace_root");
        assert_eq!(entity.title, "Hello");
    }

    /// AC ("engine-side pipeline loader is activated"): with a workspace
    /// store carrying one Medium, one Facet, one Projection, one Ingest,
    /// the engine on boot enumerates all four through its read-only
    /// queryable surface in the post-refactor shape.
    #[test]
    fn from_workspace_root_loads_pipeline_configs_into_queryable_surface() {
        use crate::pipeline::{
            Facet, Ingest, IngestMode, IngestTrigger, Medium, MediumType, Projection,
        };
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();

        let memstead = tmp.path().join(".memstead");
        std::fs::create_dir_all(&memstead).unwrap();
        std::fs::write(
            memstead.join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        use crate::workspace_store::WorkspaceStoreAdapter;
        crate::FileWorkspaceStore::new()
            .save_state(
                tmp.path(),
                &crate::workspace::Workspace {
                    mounts: vec![folder_mount("specs", mem_dir)],
                    settings: crate::workspace::WorkspaceSettings::default(),
                },
            )
            .unwrap();

        // One of each primitive in the store.
        crate::pipeline_store::write_medium(
            tmp.path(),
            "specs",
            "src",
            &Medium {
                name: "src".to_string(),
                medium_type: MediumType::Codebase,
                pointer: "..".to_string(),
            },
        )
        .unwrap();
        crate::pipeline_store::write_facet(
            tmp.path(),
            "specs",
            "view",
            &Facet {
                name: "view".to_string(),
                medium: "src".to_string(),
                scope: Vec::new(),
                engagement: None,
                preparation: None,
            },
        )
        .unwrap();
        crate::pipeline_store::write_projection(
            tmp.path(),
            "specs",
            "graph",
            &Projection {
                intent: None,
                source_facets: vec!["view".to_string()],
                reference_mems: Vec::new(),
                destination_mem: "specs".to_string(),
            },
        )
        .unwrap();
        crate::pipeline_store::write_ingest(
            tmp.path(),
            "specs-graph",
            &Ingest {
                projection: "specs/graph".to_string(),
                mode: IngestMode::Discovery,
                trigger: IngestTrigger::Loop,
                batch_size: 10,
                deny_paths: Vec::new(),
            },
        )
        .unwrap();

        let engine = Engine::from_workspace_root(tmp.path()).unwrap();
        let pc = engine.pipeline_configs();
        assert_eq!(pc.mediums.len(), 1, "one medium enumerated");
        assert_eq!(pc.mediums[0].config.medium_type, MediumType::Codebase);
        assert_eq!(pc.facets.len(), 1, "one facet enumerated");
        assert_eq!(pc.facets[0].config.medium, "src");
        assert_eq!(pc.projections.len(), 1, "one projection enumerated");
        assert_eq!(pc.projections[0].config.destination_mem, "specs");
        assert_eq!(pc.ingests.len(), 1, "one ingest enumerated");
        assert_eq!(pc.ingests[0].name, "specs-graph");
        assert_eq!(pc.ingests[0].config.mode, IngestMode::Discovery);
    }

    /// The engine edit surface: a wrapper edit (`add_medium`) routes through
    /// the pipeline-edit layer, writes the store, and refreshes the in-memory
    /// snapshot in place (no `reload()`), and referential integrity (a facet
    /// pinning the medium) is enforced through the engine method.
    #[test]
    fn engine_pipeline_edit_methods_mutate_and_refresh_the_snapshot() {
        use crate::pipeline::{Facet, Medium, MediumType};
        use crate::workspace_store::WorkspaceStoreAdapter;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let memstead = tmp.path().join(".memstead");
        std::fs::create_dir_all(&memstead).unwrap();
        std::fs::write(
            memstead.join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        crate::FileWorkspaceStore::new()
            .save_state(
                tmp.path(),
                &crate::workspace::Workspace {
                    mounts: vec![folder_mount("specs", mem_dir)],
                    settings: crate::workspace::WorkspaceSettings::default(),
                },
            )
            .unwrap();

        let mut engine = Engine::from_workspace_root(tmp.path()).unwrap();
        assert!(engine.pipeline_configs().mediums.is_empty());

        engine
            .add_medium(
                "specs",
                "src",
                &Medium {
                    name: "src".to_string(),
                    medium_type: MediumType::Codebase,
                    pointer: "..".to_string(),
                },
            )
            .unwrap();
        // Snapshot refreshed in place.
        assert_eq!(engine.pipeline_configs().mediums.len(), 1);
        assert_eq!(engine.pipeline_configs().mediums[0].name, "src");

        engine
            .add_facet(
                "specs",
                "view",
                &Facet {
                    name: "view".to_string(),
                    medium: "src".to_string(),
                    scope: Vec::new(),
                    engagement: None,
                    preparation: None,
                },
            )
            .unwrap();
        // Deleting a referenced medium is refused through the engine surface.
        let err = engine.delete_medium("specs", "src").unwrap_err();
        assert!(
            matches!(
                err,
                crate::pipeline_edit::PipelineEditError::Referenced { .. }
            ),
            "got {err:?}"
        );
        assert_eq!(
            engine.pipeline_configs().mediums.len(),
            1,
            "refused delete left the medium in place"
        );

        // The JSON entry point (the FFI-facing shape) deserializes and lands.
        engine
            .add_medium_json(
                "specs",
                "docs",
                r#"{"name":"docs","type":"filesystem","pointer":"./docs"}"#,
            )
            .unwrap();
        assert!(
            engine
                .pipeline_configs()
                .mediums
                .iter()
                .any(|m| m.name == "docs" && m.config.medium_type == MediumType::Filesystem),
            "add_medium_json should land a filesystem medium"
        );
        // A malformed payload is refused without touching the store.
        let err = engine
            .add_medium_json("specs", "bad", "{ not json")
            .unwrap_err();
        assert!(
            matches!(
                err,
                crate::pipeline_edit::PipelineEditError::InvalidJson { .. }
            ),
            "got {err:?}"
        );

        // The JSON read counterpart reflects the live store.
        let json = engine.pipeline_configs_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let mediums = parsed["mediums"].as_array().unwrap();
        assert_eq!(mediums.len(), 2, "src + docs");
        assert!(
            mediums
                .iter()
                .any(|m| m["name"] == "docs" && m["config"]["type"] == "filesystem"),
            "pipeline_configs_json should carry the docs medium: {json}"
        );
    }

    /// The lean folder authoring path: a schema package authored at the
    /// fixed `<workspace>/.memstead/schemas/<name>@<version>/` location
    /// is resolved at boot, so a folder mem can pin a non-built-in
    /// schema. Before this wiring `from_workspace_root` loaded only
    /// built-ins, so the pin would refuse with `SCHEMA_NOT_FOUND`.
    #[test]
    fn from_workspace_root_resolves_authored_schema_from_dot_memstead_schemas() {
        use crate::engine::test_helpers::write_schema_files_with_default_type;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();

        // Author a schema package at the fixed folder location.
        let authored_dir = tmp.path().join(".memstead").join("schemas");
        let manifest = r#"name: authored
version: 0.1.0
description: an authored-in-workspace test schema
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
        write_schema_files_with_default_type(&authored_dir, "authored@0.1.0", manifest, &["doc"]);

        // A folder mem pinning the authored (non-built-in) schema.
        let memstead = tmp.path().join(".memstead");
        std::fs::create_dir_all(&memstead).unwrap();
        std::fs::write(
            memstead.join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        let mount = Mount {
            mem: "specs".to_string(),
            schema: Some(SchemaRef::new("authored", semver::Version::new(0, 1, 0))),
            storage: MountStorage::Folder { path: mem_dir },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        use crate::workspace_store::WorkspaceStoreAdapter;
        crate::FileWorkspaceStore::new()
            .save_state(
                tmp.path(),
                &crate::workspace::Workspace {
                    mounts: vec![mount],
                    settings: crate::workspace::WorkspaceSettings::default(),
                },
            )
            .unwrap();

        // Boots cleanly — the authored pin resolved against the fixed
        // location rather than refusing as an unknown built-in.
        let engine = Engine::from_workspace_root(tmp.path())
            .expect("authored schema at .memstead/schemas/ must resolve at boot");
        assert_eq!(engine.mem_names(), vec!["specs"]);
    }

    #[test]
    fn from_workspace_root_propagates_mem_management_settings() {
        // workspace.toml carries [mem_management] rules; the file
        // adapter parses them into Workspace.settings; from_workspace_root
        // calls Engine::set_settings so the engine surface reflects them.
        // End-to-end check that the carriers, parser, and plumbing connect.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();

        let memstead = tmp.path().join(".memstead");
        std::fs::create_dir_all(&memstead).unwrap();
        std::fs::write(
            memstead.join("workspace.toml"),
            r#"format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[[mem_management.create]]
pattern = "exec-*"
schemas = ["default@1.0.0"]

[[mem_management.delete]]
pattern = "exec-*"
"#,
        )
        .unwrap();
        use crate::workspace_store::WorkspaceStoreAdapter;
        let store = crate::FileWorkspaceStore::new();
        store
            .save_state(
                tmp.path(),
                &crate::workspace::Workspace {
                    mounts: vec![folder_mount("specs", mem_dir)],
                    settings: crate::workspace::WorkspaceSettings::default(),
                },
            )
            .unwrap();

        let engine = Engine::from_workspace_root(tmp.path()).unwrap();
        let s = engine.settings();
        assert_eq!(s.mem_create_rules.len(), 1);
        assert_eq!(s.mem_create_rules[0].pattern, "exec-*");
        assert_eq!(
            s.mem_create_rules[0].schemas,
            vec!["default@1.0.0".to_string()]
        );
        assert_eq!(s.mem_delete_rules.len(), 1);
        assert_eq!(s.mem_delete_rules[0].pattern, "exec-*");
    }

    #[test]
    fn from_mounts_rejects_unknown_schema_pin_with_typed_error() {
        let tmp = TempDir::new().unwrap();
        let writer = FilesystemMemWriter::new(tmp.path().to_path_buf());
        let mount = Mount {
            mem: "specs".to_string(),
            schema: Some(SchemaRef::new(
                "totally-not-a-schema",
                semver::Version::new(1, 0, 0),
            )),
            storage: MountStorage::Folder {
                path: tmp.path().to_path_buf(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let err = Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)])
            .unwrap_err();
        match err {
            EngineError::SchemaNotFound { mem, pin, sources } => {
                assert_eq!(mem, "specs");
                assert_eq!(pin, "totally-not-a-schema@1.0.0");
                // The diagnostics name all three sources in fixed order;
                // none carries the unknown pin, and remote is reserved.
                let labels: Vec<&str> = sources.iter().map(|s| s.source).collect();
                assert_eq!(labels, ["local_storage", "builtin", "remote"]);
                assert!(sources.iter().all(|s| !s.pinned_version_match));
                assert_eq!(
                    sources
                        .iter()
                        .find(|s| s.source == "remote")
                        .unwrap()
                        .status,
                    Some("not_configured"),
                );
            }
            other => panic!("expected SchemaNotFound, got {other:?}"),
        }
    }

    /// Schema-pin authority: the mem's own per-mem config is the
    /// authoritative settled pin. Here the config pins a resolvable
    /// schema (`software@0.1.0`) while the workspace mount expects an
    /// unresolvable one — boot succeeds (proving the config pin won,
    /// not the mount's) and surfaces a `SchemaPinMismatch` warning
    /// naming both pins.
    #[test]
    fn mem_config_schema_is_authoritative_over_mount_pin() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"schema":"software@0.1.0"}"#,
        )
        .unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = Mount {
            mem: "specs".to_string(),
            schema: Some(SchemaRef::new(
                "totally-not-a-schema",
                semver::Version::new(9, 9, 9),
            )),
            storage: MountStorage::Folder { path: mem_dir },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let engine = Engine::from_mounts(vec![(
            mount,
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .expect("config pin software@0.1.0 is authoritative — boot must resolve it despite the unresolvable mount pin");

        let mismatch = engine
            .load_warnings()
            .iter()
            .find_map(|w| match w {
                WarningHint::SchemaPinMismatch {
                    mem,
                    config_pin,
                    mount_pin,
                } => Some((mem.clone(), config_pin.clone(), mount_pin.clone())),
                _ => None,
            })
            .expect("SchemaPinMismatch warning must surface naming both pins");
        assert_eq!(mismatch.0, "specs");
        assert_eq!(mismatch.1, "software@0.1.0");
        assert_eq!(mismatch.2, "totally-not-a-schema@9.9.9");
    }

    #[test]
    fn from_workspace_root_rejects_git_branch_mount_with_typed_error() {
        let tmp = TempDir::new().unwrap();
        let memstead = tmp.path().join(".memstead");
        std::fs::create_dir_all(&memstead).unwrap();
        std::fs::write(
            memstead.join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        // Hand-craft a state/mounts.json carrying a git-branch mount —
        // the lean boot path can't instantiate that backend.
        let state_dir = memstead.join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("mounts.json"),
            r#"{
                "format": "memstead-mounts-3",
                "mounts": [
                    {
                        "mem": "specs",
                        "schema": "default@1.0.0",
                        "storage": { "type": "git-branch", "gitdir": "/tmp/x.git", "branch": "specs" },
                        "capability": "write",
                        "lifecycle": "eager",
                        "cross_linkable": true
                    }
                ]
            }"#,
        )
        .unwrap();
        let err = Engine::from_workspace_root(tmp.path()).unwrap_err();
        match err {
            crate::BootError::Instantiate(
                crate::workspace_store::InstantiateError::GitBranchRequiresMemRepoFeature { mem },
            ) => {
                assert_eq!(mem, "specs");
            }
            other => panic!("expected Instantiate(GitBranchRequiresMemRepoFeature), got {other:?}"),
        }
    }

    #[test]
    fn from_workspace_root_roots_standalone_folder_mem() {
        // Standalone collapse: a bare folder mem — `.memstead/config.json`
        // pinning a schema, no `workspace.toml` — boots as a one-mount
        // workspace instead of refusing with NotInitialised.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("config.json"),
            r#"{"schema":"default@1.0.0"}"#,
        )
        .unwrap();
        // A collapsed single-mem folder keeps its `.md` files at the root.
        std::fs::write(
            root.join("hello.md"),
            "---\ntype: spec\n---\n# Hello\n\n## Identity\n\nStandalone body.\n",
        )
        .unwrap();

        let engine = Engine::from_workspace_root(root)
            .expect("a bare folder mem must root as a one-mount workspace");
        assert_eq!(engine.stats().mem_count, 1, "exactly one mount");
        assert!(
            engine.stats().entity_count >= 1,
            "the standalone mem's entity must load"
        );
    }

    #[test]
    fn from_workspace_root_still_rejects_truly_empty_dir() {
        // Refusal complement: a directory with neither `workspace.toml` nor a
        // `.memstead/config.json` is not a mem — it still refuses, so the
        // standalone path never masks a genuinely uninitialised directory.
        let tmp = TempDir::new().unwrap();
        let err = Engine::from_workspace_root(tmp.path()).unwrap_err();
        assert!(
            matches!(err, crate::BootError::NotInitialised(_)),
            "got {err:?}"
        );
    }

    // ---- Engine::reload_one_mem -----------------------------------
}
