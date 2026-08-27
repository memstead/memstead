//! Engine construction — `from_mounts*` and `from_workspace_root`.
//!
//! `from_mounts` is the in-process constructor every test, in-process
//! embedder, and the MCP filesystem server reach through.
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
        Self::from_mounts_inner(mounts, Vec::new(), Vec::new())
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
        let (extra_schemas, failed) = load_workspace_schemas_with_failures(schemas_dir);
        Self::from_mounts_inner(mounts, extra_schemas, failed)
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
        let (mut local, failed) = load_workspace_schemas_with_failures(schemas_dir);
        local.append(&mut extra);
        Self::from_mounts_inner(mounts, local, failed)
    }

    pub(crate) fn from_mounts_inner(
        mounts: Vec<(Mount, Box<dyn MemBackend>)>,
        extra_schemas: Vec<Arc<memstead_schema::Schema>>,
        failed_schema_packages: Vec<FailedSchemaPackage>,
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
                // Set below, after the schema pin resolves: a lazy mount
                // whose METADATA half fails still quarantines at boot;
                // only the entity load defers.
                deferred: false,
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

        // Mem-level failures quarantine the mem instead of failing the
        // workspace (degrade, never disappear — plenum/expertise
        // 2026-08-06/07, where one broken mem took every healthy
        // sibling offline). Nothing is weakened: everything that
        // failed the boot still fails it, the blast radius shrinks to
        // the one mem, which serves nothing until repaired + reloaded.
        let mut quarantined: Vec<crate::engine::QuarantinedMem> = Vec::new();
        let mut quarantined_idx: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        // Mounts whose entity load is DEFERRED (`lifecycle: lazy`): the
        // metadata half above and the schema resolution below still run
        // at boot — the roster must know the mem exists, with its pin —
        // but the entity walk is skipped until the first operation that
        // needs the mem triggers [`Engine::ensure_mems_loaded`].
        let mut deferred_idx: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for (m_idx, m) in mounted.iter().enumerate() {
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
            // Boot-honesty skew check: a mem whose engine-owned
            // mutation stamp names a different engine version than
            // this binary gets a warn-tier hint — informative, never
            // fatal, and a stamp-less (pre-stamp) mem is silent by
            // construction. Read-only: the stamp is only ever
            // rewritten by the next mutation.
            //
            // Compared as SEMVER, not as full strings (04/04, criterion 8).
            // The old rule fired on any difference including the `+g<sha>`
            // build metadata, so every rebuild between releases read as
            // skew — noise on any workspace whose binary is built from
            // source, which is every dogfood workspace. Semver ordering
            // ignores build metadata, so what survives is a real version
            // difference, and it now carries its direction.
            if let Some(stamp) = m
                .mem_config
                .as_ref()
                .and_then(|c| c.mutation_stamp.as_ref())
                && let Some(direction) = crate::build_info::skew_direction(
                    &stamp.engine_version,
                    crate::build_info::full_version(),
                )
            {
                load_warnings.push(WarningHint::EngineVersionSkew {
                    mem: m.mount.mem.clone(),
                    stamped_engine: stamp.engine_version.clone(),
                    running_engine: crate::build_info::full_version().to_string(),
                    stamped_schema: stamp.schema.clone(),
                    direction,
                });
            }
            // Authoritative pin first (the backend config), then the
            // mount assertion as fallback when the config carries none.
            let settled_pin = config_pin.or(mount_pin);
            // Dual-pin: a mem mid-migration validates against the
            // migration target, not the settled pin.
            let Some(effective_pin) = m.mount.migration_target.as_ref().or(settled_pin) else {
                // Missing pin: quarantine, don't abort the workspace.
                let e = EngineError::MemConfigIncomplete {
                    mem: m.mount.mem.clone(),
                    missing_fields: vec!["schema".to_string()],
                };
                quarantined.push(crate::engine::QuarantinedMem {
                    mount: m.mount.clone(),
                    reason_code: e.code().to_string(),
                    reason_message: e.to_string(),
                });
                quarantined_idx.insert(m_idx);
                continue;
            };
            let schema = match SchemaResolver::new(&builtin_schemas).resolve(effective_pin) {
                Ok(schema) => schema,
                Err(sources) => {
                    // Unresolvable pin: the plenum failure class —
                    // quarantine this mem, serve the rest. When the
                    // pin names a workspace-authored package that
                    // FAILED to load (e.g. one still on the retired
                    // `propagating_relationships` key), that load
                    // failure is the honest reason — not a generic
                    // not-found.
                    let failed = failed_schema_packages.iter().find(|f| {
                        f.name.as_deref() == Some(effective_pin.name.as_str())
                            && f.version
                                .as_deref()
                                .is_none_or(|v| v == effective_pin.version.to_string())
                    });
                    let (reason_code, reason_message) = match failed {
                        Some(f) => (
                            "SCHEMA_LOAD_FAILED".to_string(),
                            format!(
                                "schema package at {} failed to load: {}",
                                f.path.display(),
                                f.error
                            ),
                        ),
                        None => {
                            let e = EngineError::SchemaNotFound {
                                mem: m.mount.mem.clone(),
                                pin: effective_pin.as_display(),
                                sources,
                                install_hint: None,
                            };
                            (e.code().to_string(), e.to_string())
                        }
                    };
                    quarantined.push(crate::engine::QuarantinedMem {
                        mount: m.mount.clone(),
                        reason_code,
                        reason_message,
                    });
                    quarantined_idx.insert(m_idx);
                    continue;
                }
            };
            schemas.insert(m.mount.mem.clone(), schema.clone());

            // Generation-behind hint (warn-tier, ungated, never
            // blocking): the pin resolved from the BUILT-IN catalogue
            // and the catalogue registers at least one strictly-higher
            // version of the same name. Locally-installed
            // (workspace-storage) pins are silent — the engine only
            // knows generations for built-ins, and a local install
            // shadowing a built-in (name, version) counts as local
            // (that is also the resolver's precedence). Real semver
            // ordering via `semver::Version`, never string ordering.
            let locally_installed = workspace_schemas.iter().any(|s| {
                s.manifest.name == effective_pin.name && s.version == effective_pin.version
            });
            let is_builtin = builtin_schemas_only.iter().any(|s| {
                s.manifest.name == effective_pin.name && s.version == effective_pin.version
            });
            if !locally_installed
                && is_builtin
                && let Some(newest) =
                    newest_builtin_version(&effective_pin.name, &builtin_schemas_only)
                && *newest > effective_pin.version
            {
                load_warnings.push(WarningHint::SchemaGenerationsBehind {
                    mem: m.mount.mem.clone(),
                    pinned: effective_pin.as_display(),
                    newest: newest.to_string(),
                });
            }

            // Sealed schemas keep loading even when they violate the
            // heading round-trip rule new installs are refused for —
            // the violation surfaces as a health finding here, never
            // as a boot failure (refusing would brick the workspace).
            if let Err(memstead_schema::SchemaLoadError::SectionHeadingMismatch { violations }) =
                memstead_schema::check_section_heading_roundtrip(&schema)
            {
                let (name, version) = schema.id();
                load_warnings.push(WarningHint::SchemaHeadingRoundtripViolation {
                    mem: m.mount.mem.clone(),
                    schema_ref: format!("{name}@{version}"),
                    violations: violations.iter().map(Into::into).collect(),
                });
            }

            // Lazy lifecycle: everything above (config, provenance, pin
            // resolution, schema warnings — the metadata half) ran; the
            // entity walk is the expensive leg and defers to first read.
            // A lazy mount with a broken pin still quarantined above —
            // deferral never converts a metadata failure into silence.
            if m.mount.lifecycle == crate::workspace::MountLifecycle::Lazy {
                if let Some(w) = unbacked_mount_warning(&m.mount, m.backend.as_ref(), None) {
                    load_warnings.push(w);
                }
                deferred_idx.insert(m_idx);
                continue;
            }

            let (entries, read_errors) = match collect_source_entries(m.backend.as_ref()) {
                Ok(pair) => pair,
                Err(e) => {
                    // Backend read failure: quarantine this mem, serve
                    // the rest.
                    quarantined.push(crate::engine::QuarantinedMem {
                        mount: m.mount.clone(),
                        reason_code: e.code().to_string(),
                        reason_message: e.to_string(),
                    });
                    quarantined_idx.insert(m_idx);
                    schemas.remove(&m.mount.mem);
                    continue;
                }
            };
            // Storage that is GONE quarantines rather than serving an empty
            // graph (04/05). "Configured but cannot serve" is exactly what
            // quarantine already means, its three sibling causes already
            // quarantine, and this ends an incoherence: the same broken mount
            // used to quarantine or serve empty depending on whether the
            // mounts file happened to carry a schema assertion, which is
            // unrelated to the breakage. A mem that answers reads with an
            // empty graph is a worse default than one that refuses by name
            // with a reason.
            //
            // The quarantine roster is rendered on every roster surface, which
            // is what keeps this from trading a mount that looks healthy for a
            // mount that is simply gone.
            //
            // SCOPED TO PATH-BACKED STORAGE, and the exception is a dispute
            // with the plan's own criterion 9 rather than an oversight. For a
            // git-branch mount, "the ref does not exist" is ALSO the normal
            // state of a mem never pushed or never cloned, and quarantining it
            // removes the mount from the serving set. Making the transport
            // verbs quarantine-aware was tried and is not enough: the
            // pre-push schema validation needs the mem's resolved schema and
            // `pull` needs its store index, and quarantine removes both. The
            // recovery paths assume a serving mount at three levels, not one.
            // Recorded in the session log for the operator.
            let path_backed = matches!(
                m.mount.storage,
                crate::workspace::MountStorage::Folder { .. }
                    | crate::workspace::MountStorage::Archive { .. }
            );
            if path_backed && !m.backend.storage_present().unwrap_or(true) {
                let location = match &m.mount.storage {
                    crate::workspace::MountStorage::GitBranch { branch, .. } => branch.clone(),
                    crate::workspace::MountStorage::Folder { path }
                    | crate::workspace::MountStorage::Archive { path } => {
                        path.display().to_string()
                    }
                    crate::workspace::MountStorage::InMemory => String::new(),
                };
                quarantined.push(crate::engine::QuarantinedMem {
                    mount: m.mount.clone(),
                    reason_code: "MOUNT_UNBACKED".to_string(),
                    reason_message: format!(
                        "the mount's storage is gone ({location}); it is configured but cannot \
                         serve, so it is held out of the roster rather than answering reads \
                         with an empty graph"
                    ),
                });
                quarantined_idx.insert(m_idx);
                schemas.remove(&m.mount.mem);
                continue;
            }
            // A mount that is PRESENT but holds nothing still only warns: an
            // empty mem is a legitimate state and must never be reported as
            // unbacked or quarantined (criterion 5).
            if let Some(w) =
                unbacked_mount_warning(&m.mount, m.backend.as_ref(), Some(entries.len()))
            {
                load_warnings.push(w);
            }
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
            // Normalize folder-mount error paths to absolute (the
            // backend walk yields mem-relative ones) so the per-mem
            // reload can later replace exactly this mem's entries —
            // a repaired file must stop reporting its old refusal.
            if let crate::workspace::MountStorage::Folder { path } = &m.mount.storage {
                let root = path.clone();
                load_errors.extend(load_result.errors.into_iter().map(|(p, msg)| {
                    let abs = if p.is_relative() { root.join(&p) } else { p };
                    (abs, msg)
                }));
            } else {
                load_errors.extend(load_result.errors);
            }
        }

        // Stamp the deferred flags before the quarantine retain below
        // renumbers the vector.
        for idx in &deferred_idx {
            mounted[*idx].deferred = true;
        }

        // Drop quarantined mounts from the serving roster: a
        // quarantined mem has no backend in service, no entities in
        // the store, no schema in the per-mem map — it exists only on
        // the quarantine roster until repair + reload re-attach it.
        if !quarantined_idx.is_empty() {
            let mut keep_idx = 0usize;
            mounted.retain(|_| {
                let keep = !quarantined_idx.contains(&keep_idx);
                keep_idx += 1;
                keep
            });
        }

        // Parse-time relation validation runs after every mount's
        // entities are loaded so cross-mem target types are
        // resolvable. Hand-edits, external tooling, and embedder
        // editor surfaces can inject relations that bypass
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

        // The nested-prefix drift scan runs per mount, so a cross-mem
        // link into a mem loaded LATER in the mount order probes an
        // incomplete store and false-positives on a perfectly valid id
        // (e.g. `registry--registry-service` referenced from a mem that
        // mounts before `registry`). Now that every mount is loaded,
        // drop any hit whose resolved target exists as a real entity —
        // the same legitimate-cross-mem-reference exemption the
        // in-batch scan already applies when load order permits.
        load_warnings.retain(|w| match w {
            WarningHint::SuspiciousNestedPrefix { resolved_id, .. } => {
                store.get(resolved_id).is_none_or(|e| e.stub)
            }
            _ => true,
        });

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
            labelling_memo: OnceCell::new(),
            #[cfg(not(target_arch = "wasm32"))]
            search_indexes_memo: OnceCell::new(),
            settings: WorkspaceSettings::default(),
            create_rule_set_memo: OnceCell::new(),
            declared_origins: HashMap::new(),
            workspace_root: None,
            load_warnings,
            quarantined,
            boot_diagnosis: None,
            pipeline_configs: crate::pipeline_store::BindingConfigs::default(),
            mem_router: Arc::new(mem_router),
            backend_factory: crate::workspace_store::instantiate_lean_backend,
            unmounted_storage_prober: None,
            schemas_epoch: 0,
            git_branch_ops: None,
            event_subscribers: Arc::new(std::sync::Mutex::new(
                crate::engine::events::SubscriberRegistry::new(),
            )),
            pending_mem_changed: Vec::new(),
            mutation_clock: Arc::new(std::time::SystemTime::now),
            current_role: crate::vcs::Role::Unspecified,
        })
    }

    /// Boot an engine from a workspace root using only lean-flavour
    /// backends (folder + archive). The MCP filesystem server and the
    /// CLI's lean dispatcher reach the new engine through this entry
    /// point — replacing per-flavour init code with one call.
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
        // Backend-instantiation failures quarantine the mem instead of
        // failing the workspace (degrade, never disappear); the roster
        // entry lands on the engine after construction.
        let mut instantiate_quarantine: Vec<crate::engine::QuarantinedMem> = Vec::new();
        for mount in workspace.mounts {
            match instantiate_lean_backend(&mount) {
                Ok(backend) => mounts.push((mount, backend)),
                Err(e) => instantiate_quarantine.push(crate::engine::QuarantinedMem {
                    reason_code: e.code().to_string(),
                    reason_message: e.to_string(),
                    mount,
                }),
            }
        }
        // Folder-backend authoring path: authored schema packages live
        // at the fixed `<workspace>/.memstead/schemas/<name>@<version>/`
        // location — the folder analogue of the git-branch backend's
        // `__MEMSTEAD:schemas/` ref. Read them through the folder
        // `SchemaSource` (which no-ops when the directory is absent, so a
        // workspace that authored no schemas resolves exactly as before —
        // built-ins only). This is the lean flavour's schema-authoring
        // path, which it lacked.
        let fixed_dir = workspace_root.join(".memstead").join("schemas");
        let (local, failed) = load_workspace_schemas_with_failures(Some(fixed_dir.as_path()));
        // Root is known here, so an unresolved pin can be enriched with
        // the never-installed-package hint before it surfaces.
        let mut engine = Engine::from_mounts_inner(mounts, local, failed)
            .map_err(|e| e.with_schema_install_probe(Some(workspace_root)))?;
        engine.quarantined.extend(instantiate_quarantine);
        engine.set_settings(settings);
        engine.workspace_root = Some(workspace_root.to_path_buf());
        // Load the workspace store's pipeline configs — the v2 single-record
        // binding store — and expose them read-only. A malformed config
        // surfaces a typed `StoreError::Parse` naming the file (early
        // validation of operator-edited configs); an absent `projections/`
        // directory resolves to empty. A pre-v2 store refuses boot with
        // `StoreError::LegacyProjectionStore` naming `memstead projection
        // migrate` — the engine never reads a prior generation (2026-07-18
        // consolidation, no compatibility layer). The migrate command itself
        // operates below engine boot, so an unmigrated workspace can still
        // run it.
        engine.set_pipeline_configs(crate::pipeline_store::load_pipeline_configs(
            workspace_root,
        )?);
        // The authoring meta-schemas are NOT published here. They are an
        // editor convenience for hand-authored schema YAML, and publishing
        // them at boot made every read of a mem write to the directory it
        // read: pointing the binary at a workspace stamped a newer binary's
        // meta-schemas over the ones on disk. That broke read-only mounts,
        // installed third-party mems, and sealed corpora, which cannot be
        // verified without being modified. Publishing now happens in the
        // schema-authoring commands (`memstead schema new` / `validate` /
        // `install`), the only paths that produce YAML an editor validates.
        // Boot is a read; a read does not write.
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
pub(crate) fn build_mem_router_from_mounts(mounts: &[MountedBackend]) -> MemRouterSnapshot {
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

/// The newest version registered in the built-in catalogue under
/// `name` — real `semver::Version` ordering (0.10.0 beats 0.9.0),
/// never string ordering. `None` when no built-in carries the name.
/// Feeds the `SCHEMA_GENERATIONS_BEHIND` boot hint.
fn newest_builtin_version<'a>(
    name: &str,
    builtins: &'a [Arc<memstead_schema::Schema>],
) -> Option<&'a semver::Version> {
    builtins
        .iter()
        .filter(|s| s.manifest.name == name)
        .map(|s| &s.version)
        .max()
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
/// skips entries that don't carry the manifest. `pub` so the folder
/// `SchemaSource` and the below-boot repair path (memstead-git-branch)
/// read through the same walker the boot path uses — one loader, no
/// resolution fork between the booted and below-boot surfaces.
pub fn load_workspace_schemas(
    schemas_dir: Option<&Path>,
) -> Result<Vec<Arc<memstead_schema::Schema>>, EngineError> {
    Ok(load_workspace_schemas_with_failures(schemas_dir).0)
}

/// One workspace-authored schema package that failed to load — the
/// package is SKIPPED (never fails the boot; degrade, never
/// disappear), and a mem pinning it quarantines with this failure as
/// its typed reason. `name`/`version` are best-effort peeks at the
/// package's `schema.yaml` header so the pin match works even though
/// the full load refused.
#[derive(Debug, Clone)]
pub struct FailedSchemaPackage {
    pub path: PathBuf,
    pub name: Option<String>,
    pub version: Option<String>,
    /// The loader's typed failure, rendered.
    pub error: String,
}

/// Tolerant form of [`load_workspace_schemas`]: broken packages are
/// skipped and recorded instead of failing the whole walk (the
/// historical `?` made one refusing package — e.g. a schema still on
/// the retired `propagating_relationships` key after a binary
/// upgrade — take every mem in the workspace down).
pub fn load_workspace_schemas_with_failures(
    schemas_dir: Option<&Path>,
) -> (Vec<Arc<memstead_schema::Schema>>, Vec<FailedSchemaPackage>) {
    let Some(dir) = schemas_dir else {
        return (Vec::new(), Vec::new());
    };
    if !dir.is_dir() {
        return (Vec::new(), Vec::new());
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return (Vec::new(), Vec::new()),
    };
    let mut schemas: Vec<Arc<memstead_schema::Schema>> = Vec::new();
    let mut failures: Vec<FailedSchemaPackage> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !path.join("schema.yaml").is_file() {
            continue;
        }
        match memstead_schema::load_schema_from_dir(&path) {
            Ok(schema) => schemas.push(Arc::new(schema)),
            Err(e) => {
                // Best-effort header peek without a YAML dependency:
                // top-level `name:` / `version:` are single-line
                // scalars in every real package.
                let header = std::fs::read_to_string(path.join("schema.yaml")).unwrap_or_default();
                let peek = |k: &str| {
                    header
                        .lines()
                        .find_map(|l| l.strip_prefix(&format!("{k}:")))
                        .map(|v| v.trim().trim_matches('"').to_string())
                        .filter(|v| !v.is_empty())
                };
                failures.push(FailedSchemaPackage {
                    path: path.clone(),
                    name: peek("name"),
                    version: peek("version"),
                    error: e.to_string(),
                });
            }
        }
    }
    (schemas, failures)
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

/// The `MOUNT_UNBACKED` probe for one mount: `Some(warning)` when the
/// storage the mount names does not exist (`missing_ref` /
/// `missing_path`, from [`MemBackend::storage_present`]) or, when
/// `entity_count` is known and zero, holds no entity (`empty`). A
/// probe failure reads as present — best-effort, never a boot failure.
/// Lazy mounts pass `None` for the count: their walk is deferred, so
/// only the storage half is judged at boot.
pub(super) fn unbacked_mount_warning(
    mount: &crate::workspace::Mount,
    backend: &dyn MemBackend,
    entity_count: Option<usize>,
) -> Option<WarningHint> {
    use crate::ops::MountUnbackedReason;
    use crate::workspace::MountStorage;
    let (location, missing_reason) = match &mount.storage {
        MountStorage::GitBranch { branch, .. } => (branch.clone(), MountUnbackedReason::MissingRef),
        MountStorage::Folder { path } => {
            (path.display().to_string(), MountUnbackedReason::MissingPath)
        }
        MountStorage::Archive { path } => {
            (path.display().to_string(), MountUnbackedReason::MissingPath)
        }
        MountStorage::InMemory => return None,
    };
    if !backend.storage_present().unwrap_or(true) {
        return Some(WarningHint::MountUnbacked {
            mem: mount.mem.clone(),
            reason: missing_reason,
            location,
        });
    }
    if entity_count == Some(0) {
        return Some(WarningHint::MountUnbacked {
            mem: mount.mem.clone(),
            reason: MountUnbackedReason::Empty,
            location,
        });
    }
    None
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

    /// The unbacked-mount probe on the folder backend: a path that does
    /// not exist is `missing_path`, an existing directory with no
    /// entity is `empty`, a directory holding one entity is silent, and
    /// a lazy mount (count unknown) is judged on storage presence only.
    #[test]
    fn unbacked_mount_probe_classes_missing_path_empty_and_present() {
        use crate::engine::boot::unbacked_mount_warning;
        use crate::ops::MountUnbackedReason;
        let tmp = TempDir::new().unwrap();
        let mount = |path: std::path::PathBuf| Mount {
            mem: "probe".into(),
            schema: None,
            storage: MountStorage::Folder { path },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };

        let gone = tmp.path().join("gone");
        let backend = FilesystemMemWriter::new(gone.clone());
        let w = unbacked_mount_warning(&mount(gone.clone()), &backend, Some(0))
            .expect("a missing folder is unbacked");
        match &w {
            WarningHint::MountUnbacked {
                mem,
                reason,
                location,
            } => {
                assert_eq!(mem, "probe");
                assert_eq!(*reason, MountUnbackedReason::MissingPath);
                assert_eq!(location, &gone.display().to_string());
            }
            other => panic!("unexpected variant: {other:?}"),
        }
        assert_eq!(w.code(), "MOUNT_UNBACKED");
        let json = serde_json::to_value(&w).unwrap();
        assert_eq!(json["details"]["reason"], "missing_path");
        // Lazy: storage presence alone decides, and the folder is gone.
        assert!(unbacked_mount_warning(&mount(gone), &backend, None).is_some());

        let hollow = tmp.path().join("hollow");
        std::fs::create_dir_all(&hollow).unwrap();
        let backend = FilesystemMemWriter::new(hollow.clone());
        let w = unbacked_mount_warning(&mount(hollow.clone()), &backend, Some(0))
            .expect("an entity-less folder is unbacked");
        assert_eq!(
            serde_json::to_value(&w).unwrap()["details"]["reason"],
            "empty"
        );
        // Lazy with the folder present: nothing to say at boot.
        assert!(unbacked_mount_warning(&mount(hollow.clone()), &backend, None).is_none());
        // One entity: silent.
        assert!(unbacked_mount_warning(&mount(hollow), &backend, Some(1)).is_none());
    }

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

    /// Generation-behind hint: a mem pinning an OLD built-in
    /// generation (`default@1.0.0`; the catalogue retains up to
    /// 1.2.0) boots with the warn-tier `SCHEMA_GENERATIONS_BEHIND`
    /// naming the pinned ref and the newest version — and the hint
    /// never blocks: the boot serves and mutations succeed. A mem
    /// pinning the NEWEST generation stays silent, so its health
    /// output is unchanged.
    #[test]
    fn generation_behind_hint_fires_for_old_builtin_pin_only() {
        // Old pin → hint, non-blocking.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let behind: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter_map(|w| match w {
                WarningHint::SchemaGenerationsBehind {
                    mem,
                    pinned,
                    newest,
                } => Some((mem.clone(), pinned.clone(), newest.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            behind,
            vec![(
                "specs".to_string(),
                "default@1.0.0".to_string(),
                "1.3.0".to_string()
            )],
            "old built-in pin must surface the generation-behind hint"
        );
        assert!(
            engine
                .health()
                .warnings
                .iter()
                .any(|w| w.code() == "SCHEMA_GENERATIONS_BEHIND"),
            "the hint rides health without an include gate"
        );
        // Never blocking: the warned mem still mutates.
        engine
            .create_entity_with_ctx(
                crate::engine::CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "specs".to_string(),
                    title: "Still writable".to_string(),
                    entity_type: "spec".to_string(),
                    sections: indexmap::IndexMap::from_iter([
                        ("identity".to_string(), "i".to_string()),
                        ("purpose".to_string(), "p".to_string()),
                    ]),
                    metadata: indexmap::IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                &crate::vcs::CommitContext::internal(),
            )
            .expect("generation-behind hint must never block mutations");

        // Newest pin → silent (health output unchanged).
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut mount = folder_mount("specs", mem_dir);
        mount.schema = Some("default@1.3.0".parse().unwrap());
        let engine =
            Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
        assert!(
            !engine
                .load_warnings()
                .iter()
                .any(|w| w.code() == "SCHEMA_GENERATIONS_BEHIND"),
            "newest built-in pin must stay silent: {:?}",
            engine.load_warnings()
        );
        let health_json = serde_json::to_string(&engine.health().warnings).unwrap();
        assert!(
            !health_json.contains("SCHEMA_GENERATIONS_BEHIND"),
            "newest pin: health output carries no generation hint"
        );
    }

    /// The newest-generation lookup uses real semver ordering — a
    /// two-digit minor beats a one-digit one (string ordering would
    /// invert them).
    #[test]
    fn newest_builtin_version_orders_by_semver_not_string() {
        let manifest = |version: &str| {
            format!(
                r#"name: gen-test
version: {version}
description: test
when_to_use: test
types:
  - note
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
            )
        };
        let type_yaml = r#"name: note
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
no_self_loop_relationships: []
updatable_fields: [title, body]
health_required_fields: [body]
staleness_threshold_days: 30
write_rules: []
"#;
        let types = vec![("note".to_string(), type_yaml.to_string())];
        let catalogue: Vec<std::sync::Arc<memstead_schema::Schema>> = ["0.9.0", "0.10.0", "0.2.0"]
            .iter()
            .map(|v| {
                std::sync::Arc::new(
                    memstead_schema::load_schema_from_memory(&manifest(v), &types)
                        .expect("fixture schema loads"),
                )
            })
            .collect();
        let newest = super::newest_builtin_version("gen-test", &catalogue)
            .expect("name present in catalogue");
        assert_eq!(newest.to_string(), "0.10.0", "semver, not string, ordering");
        assert!(super::newest_builtin_version("absent", &catalogue).is_none());
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

    /// The per-mount nested-prefix drift scan probes an incomplete
    /// store: a cross-mem link into a mem loaded LATER in the mount
    /// order can't see the real target yet and would false-positive on
    /// a perfectly valid id whose slug repeats its mem name (the
    /// `registry--registry-service` case). The post-load sweep must
    /// drop that hit — while a genuine drift link (target never
    /// materialises as a real entity) keeps its warning.
    #[test]
    fn nested_prefix_warning_exempts_real_cross_mem_target_loaded_later() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("project");
        let registry_dir = tmp.path().join("registry");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(&registry_dir).unwrap();

        // Mount 1 (loads first) links both a real later-loaded entity
        // and a genuinely missing one.
        let source = "---\ntype: spec\n---\n# Source\n\n## Identity\n\nReal: [[registry--registry-service]]. Drifted: [[registry--never-created]].\n";
        std::fs::write(project_dir.join("source.md"), source).unwrap();

        // Mount 2 (loads second) carries the real target whose slug
        // repeats its mem name — the shape the heuristic suspects.
        let service = "---\ntype: spec\n---\n# Registry Service\n\n## Identity\n\nA real entity.\n";
        std::fs::write(registry_dir.join("registry-service.md"), service).unwrap();

        let engine = Engine::from_mounts(vec![
            (
                folder_mount("project", project_dir.clone()),
                Box::new(FilesystemMemWriter::new(project_dir)) as Box<dyn MemBackend>,
            ),
            (
                folder_mount("registry", registry_dir.clone()),
                Box::new(FilesystemMemWriter::new(registry_dir)) as Box<dyn MemBackend>,
            ),
        ])
        .unwrap();

        let nested: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter_map(|w| match w {
                WarningHint::SuspiciousNestedPrefix { resolved_id, .. } => {
                    Some(resolved_id.to_string())
                }
                _ => None,
            })
            .collect();
        assert!(
            !nested.contains(&"registry--registry-service".to_string()),
            "a valid cross-mem id resolving to a real entity must not warn, got {nested:?}",
        );
        assert!(
            nested.contains(&"registry--never-created".to_string()),
            "a genuinely unresolved nested-prefix link must keep its warning, got {nested:?}",
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

    /// The engine-side pipeline loader: with a workspace store carrying one
    /// v2 binding, the engine on boot enumerates it through its read-only
    /// queryable surface; a pre-v2 store refuses boot with the
    /// migrate-naming error (the loader never reads a prior generation).
    #[test]
    fn from_workspace_root_loads_pipeline_configs_into_queryable_surface() {
        use crate::pipeline::{MediumType, Projection};
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

        // One v2 binding in the store.
        crate::pipeline_store::write_binding(
            tmp.path(),
            "specs",
            "graph",
            &sample_v2_binding("specs"),
        )
        .unwrap();

        let engine = Engine::from_workspace_root(tmp.path()).unwrap();
        let pc = engine.pipeline_configs();
        assert_eq!(pc.bindings.len(), 1, "one binding enumerated");
        assert_eq!(pc.bindings[0].mem, "specs");
        assert_eq!(pc.bindings[0].name, "graph");
        assert_eq!(pc.bindings[0].config.destination_mem, "specs");
        assert_eq!(
            pc.bindings[0].config.sources[0].medium_type,
            MediumType::Codebase
        );

        // QUARANTINE (agent-trust plan 04 re-routing of the historical
        // wholesale refusal): a pre-v2 (version-less gen-2) projection
        // file no longer fails the boot — the affected binding
        // quarantines with the migrate-naming reason, still never
        // read, still never tolerated; the workspace and the healthy
        // binding keep serving.
        crate::pipeline_store::write_projection(
            tmp.path(),
            "specs",
            "legacy",
            &Projection {
                intent: None,
                source_facets: vec!["view".to_string()],
                reference_mems: Vec::new(),
                destination_mem: "specs".to_string(),
                rules: None,
            },
        )
        .unwrap();
        let engine = Engine::from_workspace_root(tmp.path()).unwrap();
        let pc = engine.pipeline_configs();
        assert_eq!(pc.bindings.len(), 1, "the healthy binding still serves");
        assert_eq!(pc.quarantined.len(), 1, "the legacy one quarantines");
        assert_eq!(pc.quarantined[0].name, "legacy");
        assert_eq!(pc.quarantined[0].reason_code, "PROJECTION_STORE_LEGACY");
        assert!(
            pc.quarantined[0]
                .reason_message
                .contains("memstead projection migrate"),
            "quarantine reason names the migrate command, got: {}",
            pc.quarantined[0].reason_message
        );
    }

    /// One v2 binding with a single codebase source under `pointer` — the
    /// shared fixture of the boot tests.
    fn sample_v2_binding(dest: &str) -> crate::binding::Binding {
        v2_binding_with_pointer(dest, "..")
    }

    fn v2_binding_with_pointer(dest: &str, pointer: &str) -> crate::binding::Binding {
        use crate::binding::{BINDING_VERSION, Binding, BuildMode, BuildOperation, Operations};
        use crate::pipeline::{IngestTrigger, MediumType, Source};
        Binding {
            version: BINDING_VERSION,
            intent: None,
            sources: vec![Source {
                name: "src".to_string(),
                medium_type: MediumType::Codebase,
                pointer: pointer.to_string(),
                change_detection: None,
                scope: Vec::new(),
                engagement: None,
                preparation: None,
            }],
            reference_mems: Vec::new(),
            destination_mem: dest.to_string(),
            deny_paths: Vec::new(),
            coverage_semantics: None,
            rules: None,
            prune: None,
            operations: Operations {
                build: Some(BuildOperation {
                    mode: BuildMode::Discovery,
                    trigger: IngestTrigger::Loop,
                    batch_size: 10,
                    post_actions: None,
                }),
                sync: None,
                verify: None,
            },
        }
    }

    /// Live per-anchor state (criteria 1, 9 — path-medium subset): a
    /// single-medium `path` mem observes working-tree existence at the current
    /// HEAD. Absent artifact ⇒ `orphaned`; present + non-hash class ⇒
    /// `resolves`; present + hash-bearing class ⇒ the prepared-content hash
    /// comparison adjudicates deterministically — a recorded hash matching
    /// the observed prepared form `resolves`, a stable-medium mismatch is
    /// `drifted` (a real content drift, no longer deferred to `recheck`).
    #[test]
    fn entity_anchors_resolve_live_state_for_path_medium() {
        use crate::anchor::{AnchorInput, AnchorState};
        use crate::vcs::Actor;
        use crate::workspace_store::WorkspaceStoreAdapter;
        use indexmap::IndexMap;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();

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
                    mounts: vec![folder_mount("specs", mem_dir.clone())],
                    settings: crate::workspace::WorkspaceSettings::default(),
                },
            )
            .unwrap();

        // A single `path` source rooted at `<workspace>/src` (medium context
        // now derives from the mem's binding sources). Anchor artifact ids
        // are workspace-relative (pointer-prefixed) — the dialect
        // enumeration / coverage / advance share — so `src/present.rs`
        // observes present and `src/gone.rs` observes absent.
        crate::pipeline_store::write_binding(
            tmp.path(),
            "specs",
            "graph",
            &v2_binding_with_pointer("specs", "src"),
        )
        .unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src").join("present.rs"), "fn main() {}").unwrap();
        // Exists at write time (the write gate refuses dead references);
        // deleted after the write to produce the orphaned READ state.
        std::fs::write(tmp.path().join("src").join("gone.rs"), "fn gone() {}").unwrap();

        let mut engine = Engine::from_workspace_root(tmp.path()).unwrap();

        let anchor = |artifact: &str, class: &str, hash: Option<&str>| AnchorInput {
            artifact: Some(artifact.to_string()),
            grain: Some("file".to_string()),
            class: Some(class.to_string()),
            hash: hash.map(str::to_string),
            hash_stability: Some("stable".to_string()),
            ..Default::default()
        };
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), "Covers src.".to_string());
        sections.insert("purpose".to_string(), "Track sources.".to_string());
        // The prepared-form hash of the present artifact, as the observation
        // computes it — an anchor recording it must resolve clean.
        let present_hash = crate::anchor::prepared_content_hash(
            &std::fs::read(tmp.path().join("src").join("present.rs")).unwrap(),
        );
        let created = engine
            .create_entity(
                crate::CreateEntityArgs {
                    mem: "specs".to_string(),
                    title: "Covers".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    anchors: vec![
                        anchor("src/present.rs", "anchored", Some(&present_hash)), // hash matches → resolves
                        anchor("src/present.rs", "informed-by", None), // present + non-hash → resolves
                        anchor("src/gone.rs", "anchored", Some("h2")), // absent → orphaned
                        anchor("src/present.rs", "derived", Some("stale")), // hash mismatch, stable → drifted
                    ],
                    dry_run: false,
                },
                Actor::Agent,
                None,
                None,
            )
            .unwrap();

        std::fs::remove_file(tmp.path().join("src").join("gone.rs")).unwrap();
        let resolved = engine.entity_anchors_resolved(&created.id);
        assert_eq!(resolved.len(), 4);
        let state_of = |artifact: &str, class: crate::anchor::AnchorProvenanceClass| {
            resolved
                .iter()
                .find(|r| r.anchor.artifact == artifact && r.anchor.class == class)
                .and_then(|r| r.state)
        };
        assert_eq!(
            state_of(
                "src/present.rs",
                crate::anchor::AnchorProvenanceClass::Anchored
            ),
            Some(AnchorState::Resolves),
            "recorded hash matches the observed prepared form → resolves"
        );
        assert_eq!(
            state_of(
                "src/present.rs",
                crate::anchor::AnchorProvenanceClass::Derived
            ),
            Some(AnchorState::Drifted),
            "recorded hash mismatches the observed prepared form on a stable medium → drifted"
        );
        assert_eq!(
            state_of(
                "src/present.rs",
                crate::anchor::AnchorProvenanceClass::InformedBy
            ),
            Some(AnchorState::Resolves),
            "present non-hash anchor resolves on existence"
        );
        assert_eq!(
            state_of(
                "src/gone.rs",
                crate::anchor::AnchorProvenanceClass::Anchored
            ),
            Some(AnchorState::Orphaned),
            "absent artifact is orphaned"
        );
    }

    /// The engine edit surface: a wrapper edit (`add_projection_json`)
    /// routes through the pipeline-edit layer, writes the store, and
    /// refreshes the in-memory snapshot in place (no `reload()`); the JSON
    /// read counterpart reflects the collapsed `{bindings}`-only shape.
    #[test]
    fn engine_pipeline_edit_methods_mutate_and_refresh_the_snapshot() {
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
        assert!(engine.pipeline_configs().bindings.is_empty());

        // The JSON entry point (the FFI-facing shape) deserializes and lands.
        engine
            .add_projection_json(
                "specs",
                "graph",
                r#"{
                    "sources": [{ "name": "src", "type": "codebase", "pointer": "..",
                                  "scope": [{ "path": "**/*.rs", "mode": "allow" }] }],
                    "destination_mem": "specs"
                }"#,
                None,
            )
            .unwrap();
        // Snapshot refreshed in place.
        assert_eq!(engine.pipeline_configs().bindings.len(), 1);
        assert_eq!(engine.pipeline_configs().bindings[0].name, "graph");
        assert_eq!(
            engine.pipeline_configs().bindings[0].config.sources[0].name,
            "src"
        );

        // A malformed payload is refused without touching the store.
        let err = engine
            .add_projection_json("specs", "bad", "{ not json", None)
            .unwrap_err();
        assert!(
            matches!(
                err,
                crate::pipeline_edit::PipelineEditError::InvalidJson { .. }
            ),
            "got {err:?}"
        );

        // Update patches over the stored record; delete removes and refreshes.
        engine
            .update_projection_json("specs", "graph", r#"{"intent":"i2"}"#, None)
            .unwrap();
        assert_eq!(
            engine.pipeline_configs().bindings[0]
                .config
                .intent
                .as_deref(),
            Some("i2")
        );

        // Rename moves the record and refreshes the snapshot.
        engine
            .rename_projection("specs", "graph", "graph2", None)
            .unwrap();
        assert_eq!(engine.pipeline_configs().bindings[0].name, "graph2");

        // The JSON read counterpart reflects the live store in the
        // `{bindings}`-only shape — no `mediums` / `facets` keys.
        let json = engine.pipeline_configs_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("mediums").is_none(), "no mediums key: {json}");
        assert!(parsed.get("facets").is_none(), "no facets key: {json}");
        let bindings = parsed["bindings"].as_array().unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0]["name"], "graph2");
        assert_eq!(bindings[0]["config"]["sources"][0]["type"], "codebase");

        engine.delete_projection("specs", "graph2", None).unwrap();
        assert!(engine.pipeline_configs().bindings.is_empty());
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

    /// Authoring-drift health axis (plan 10): a STAMPED sealed schema
    /// reports a missing authoring package and (separately) a diverged
    /// one; an unmodified package, a cosmetic-only difference (editor
    /// header comment lines), and an unstamped seal produce NO
    /// finding; and the checks alter neither copy.
    #[test]
    fn health_reports_authoring_drift_for_stamped_schemas_only() {
        use crate::engine::test_helpers::write_schema_files_with_default_type;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
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
        // Sealed copy at the fixed install location; authoring copy in
        // the working tree.
        let sealed_root = tmp.path().join(".memstead").join("schemas");
        write_schema_files_with_default_type(&sealed_root, "authored@0.1.0", manifest, &["doc"]);
        let author_root = tmp.path().join("author");
        write_schema_files_with_default_type(&author_root, "authored@0.1.0", manifest, &["doc"]);
        let authoring_dir = author_root.join("authored@0.1.0");
        let sealed_dir = sealed_root.join("authored@0.1.0");
        // The install-time stamp: the seal records where it came from.
        let stamp_path = sealed_dir.join(memstead_schema::INSTALL_PROVENANCE_FILE);
        std::fs::write(
            &stamp_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "authoring_path": authoring_dir.display().to_string(),
            }))
            .unwrap(),
        )
        .unwrap();

        let memstead = tmp.path().join(".memstead");
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
        let engine = Engine::from_workspace_root(tmp.path()).expect("workspace boots");
        let drift_codes = |e: &Engine| -> Vec<String> {
            e.health()
                .warnings
                .iter()
                .filter(|w| w.code().starts_with("SCHEMA_AUTHORING_SOURCE_"))
                .map(|w| w.code().to_string())
                .collect()
        };

        // Unmodified authoring package: no finding, and the check
        // touched neither copy.
        let sealed_before = std::fs::read(sealed_dir.join("schema.yaml")).unwrap();
        let author_before = std::fs::read(authoring_dir.join("schema.yaml")).unwrap();
        assert_eq!(drift_codes(&engine), Vec::<String>::new());
        assert_eq!(
            std::fs::read(sealed_dir.join("schema.yaml")).unwrap(),
            sealed_before,
            "health must not touch the sealed copy"
        );
        assert_eq!(
            std::fs::read(authoring_dir.join("schema.yaml")).unwrap(),
            author_before,
            "health must not touch the authoring copy"
        );

        // Cosmetic-only difference (the CLI-injected editor-header
        // line + a comment): still no finding — parsed equivalence,
        // never raw bytes.
        std::fs::write(
            authoring_dir.join("schema.yaml"),
            format!(
                "# yaml-language-server: $schema=../../.memstead/meta-schemas/schema-manifest.json\n# cosmetic comment\n{manifest}"
            ),
        )
        .unwrap();
        assert_eq!(drift_codes(&engine), Vec::<String>::new());

        // Semantic change: DIVERGED, naming schema, version, and the
        // pinning mems.
        std::fs::write(
            authoring_dir.join("schema.yaml"),
            manifest.replace(
                "an authored-in-workspace test schema",
                "a semantically different description",
            ),
        )
        .unwrap();
        let warnings = engine.health().warnings;
        let diverged = warnings
            .iter()
            .find(|w| w.code() == "SCHEMA_AUTHORING_SOURCE_DIVERGED")
            .expect("semantic change must surface as DIVERGED");
        let d = serde_json::to_value(diverged).unwrap();
        assert_eq!(d["details"]["schema_ref"], "authored@0.1.0");
        assert_eq!(d["details"]["mems"], serde_json::json!(["specs"]));

        // Authoring package gone: the DIFFERENT finding — MISSING.
        std::fs::remove_dir_all(&authoring_dir).unwrap();
        let warnings = engine.health().warnings;
        let missing = warnings
            .iter()
            .find(|w| w.code() == "SCHEMA_AUTHORING_SOURCE_MISSING")
            .expect("vanished authoring package must surface as MISSING");
        let m = serde_json::to_value(missing).unwrap();
        assert_eq!(m["details"]["schema_ref"], "authored@0.1.0");
        assert_eq!(m["details"]["mems"], serde_json::json!(["specs"]));
        assert_eq!(
            m["details"]["stamped_path"],
            authoring_dir.display().to_string()
        );
        assert!(
            !warnings
                .iter()
                .any(|w| w.code() == "SCHEMA_AUTHORING_SOURCE_DIVERGED"),
            "missing and diverged are distinct findings"
        );

        // No stamp → no finding, even with the package still gone.
        std::fs::remove_file(&stamp_path).unwrap();
        assert_eq!(drift_codes(&engine), Vec::<String>::new());
    }

    /// Plan 12: `full_refresh` makes an out-of-band schema install and
    /// an out-of-band mem registration usable warm — additively.
    /// Removals are skipped and reported; a failed mount is reported
    /// per-item and does not abort the rest.
    #[test]
    fn full_refresh_is_additive_and_reports_skipped_removals() {
        use crate::engine::test_helpers::write_schema_files_with_default_type;
        use crate::workspace_store::WorkspaceStoreAdapter;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let mem_a = root.join("mem-a");
        std::fs::create_dir_all(&mem_a).unwrap();
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        let mount = |mem: &str, dir: &Path, schema: &str, version: semver::Version| Mount {
            mem: mem.to_string(),
            schema: Some(SchemaRef::new(schema, version)),
            storage: MountStorage::Folder {
                path: dir.to_path_buf(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let save = |mounts: Vec<Mount>| {
            crate::FileWorkspaceStore::new()
                .save_state(
                    root,
                    &crate::workspace::Workspace {
                        mounts,
                        settings: crate::workspace::WorkspaceSettings::default(),
                    },
                )
                .unwrap();
        };
        save(vec![mount(
            "specs",
            &mem_a,
            "default",
            semver::Version::new(1, 0, 0),
        )]);
        let mut engine = Engine::from_workspace_root(root).expect("workspace boots");
        assert_eq!(engine.mem_names(), vec!["specs"]);

        // --- Out of band, while the "server" runs: install a schema
        // and register a mem pinned to it. ---
        let manifest = r#"name: authored
version: 0.1.0
description: an out-of-band installed schema
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
        write_schema_files_with_default_type(
            &root.join(".memstead").join("schemas"),
            "authored@0.1.0",
            manifest,
            &["doc"],
        );
        let mem_b = root.join("mem-b");
        std::fs::create_dir_all(&mem_b).unwrap();
        save(vec![
            mount("specs", &mem_a, "default", semver::Version::new(1, 0, 0)),
            mount("notes", &mem_b, "authored", semver::Version::new(0, 1, 0)),
        ]);

        // Refusal complement, pre-refresh: the running engine still
        // refuses — the refresh is what changes the outcome.
        let (actor, client) = cli_actor();
        let mut pre = crate::engine::test_helpers::empty_create_args("notes", "Too Early");
        pre.entity_type = "doc".to_string();
        pre.sections = indexmap::IndexMap::from_iter([("body".to_string(), "body".to_string())]);
        let err = engine
            .create_entity(pre.clone(), actor, Some(&client), None)
            .unwrap_err();
        assert_eq!(err.code(), "UNKNOWN_MEM", "{err:?}");
        assert!(
            !engine
                .workspace_schemas()
                .iter()
                .any(|s| s.id().0 == "authored"),
            "schema catalogue is fixed pre-refresh"
        );

        // --- Full refresh: both become usable, warm. ---
        let report = engine.full_refresh();
        assert_eq!(report.schemas_added, vec!["authored@0.1.0".to_string()]);
        assert_eq!(report.mems_mounted, vec!["notes".to_string()]);
        assert!(report.schema_removals_skipped.is_empty(), "{report:?}");
        assert!(report.mem_removals_skipped.is_empty(), "{report:?}");
        assert!(report.failures.is_empty(), "{report:?}");
        engine
            .create_entity(pre, actor, Some(&client), None)
            .expect("newly mounted mem accepts writes after the refresh");

        // --- Removals do NOT take effect: drop `specs` from the
        // manifest and delete the schema package from its source. ---
        std::fs::remove_dir_all(
            root.join(".memstead")
                .join("schemas")
                .join("authored@0.1.0"),
        )
        .unwrap();
        save(vec![mount(
            "notes",
            &mem_b,
            "authored",
            semver::Version::new(0, 1, 0),
        )]);
        let report = engine.full_refresh();
        assert_eq!(report.mem_removals_skipped, vec!["specs".to_string()]);
        assert_eq!(
            report.schema_removals_skipped,
            vec!["authored@0.1.0".to_string()]
        );
        assert!(report.schemas_added.is_empty());
        assert!(report.mems_mounted.is_empty());
        // Both stay live: the unregistered mem still accepts writes,
        // the removed schema version still resolves for its mem.
        engine
            .create_entity(
                crate::engine::test_helpers::empty_create_args("specs", "Still Here"),
                actor,
                Some(&client),
                None,
            )
            .expect("skipped-removal mem stays writable");
        let mut into_notes =
            crate::engine::test_helpers::empty_create_args("notes", "Still Resolvable");
        into_notes.entity_type = "doc".to_string();
        into_notes.sections =
            indexmap::IndexMap::from_iter([("body".to_string(), "body".to_string())]);
        engine
            .create_entity(into_notes, actor, Some(&client), None)
            .expect("removed-from-source schema stays resolvable");

        // --- Per-item failure: a manifest mount whose path is a FILE
        // fails alone; the rest of the refresh proceeds. ---
        let broken = root.join("broken-mem");
        std::fs::write(&broken, b"not a directory").unwrap();
        save(vec![
            mount("notes", &mem_b, "authored", semver::Version::new(0, 1, 0)),
            mount("broken", &broken, "default", semver::Version::new(1, 0, 0)),
        ]);
        let report = engine.full_refresh();
        assert!(
            report.failures.iter().any(|f| f.item == "mount:broken"),
            "failed mount must be reported per-item: {report:?}"
        );
        assert!(
            !report.mems_mounted.contains(&"broken".to_string()),
            "a failed mount never surfaces as newly available"
        );
        assert!(
            engine
                .get_entity(&crate::EntityId::new("broken", "anything"))
                .is_none()
                && engine.mem_names().contains(&"notes"),
            "other mounts unaffected"
        );
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

    /// Deliberate replacement of the historical wholesale-abort test
    /// (`from_mounts_rejects_unknown_schema_pin_with_typed_error`,
    /// agent-trust plan 04): an unresolvable pin no longer fails the
    /// workspace — the mem is QUARANTINED with the same typed
    /// `SCHEMA_NOT_FOUND` reason (nothing is weakened, the blast
    /// radius shrinks), operations naming it refuse `MEM_QUARANTINED`,
    /// and the roster surfaces on health.
    #[test]
    fn from_mounts_quarantines_unknown_schema_pin() {
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
        let engine = Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)])
            .expect("a broken mem quarantines, never fails the workspace");

        let roster = engine.quarantined_mems();
        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].mount.mem, "specs");
        assert_eq!(roster[0].reason_code, "SCHEMA_NOT_FOUND");
        assert!(
            roster[0].reason_message.contains("totally-not-a-schema"),
            "reason carries the failing pin: {}",
            roster[0].reason_message
        );
        // The mem serves nothing: it is not on the mount roster …
        assert!(engine.mounts().iter().all(|m| m.mem != "specs"));
        // … and lookups refuse with the typed quarantine code, not
        // UNKNOWN_MEM.
        let err = engine.unknown_mem_error("specs");
        assert_eq!(err.code(), "MEM_QUARANTINED");
        assert!(
            err.to_string().contains("SCHEMA_NOT_FOUND"),
            "quarantine refusal carries the underlying reason: {err}"
        );
        // Health carries the roster without an include gate.
        let health = engine.health();
        assert_eq!(health.quarantined.len(), 1);
        assert_eq!(health.quarantined[0].reason_code, "SCHEMA_NOT_FOUND");
    }

    /// Criterion 5 (agent-trust plan 04): quarantine → repair →
    /// reload returns the mem to service in the same engine instance;
    /// the roster entry disappears. The repair here is the same
    /// value-level config-pin rewrite `memstead mem set-schema`
    /// performs below boot (plan 03).
    #[test]
    fn reload_returns_repaired_mem_from_quarantine() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(dir.join(".memstead")).unwrap();
        let config_path = dir.join(".memstead").join("config.json");
        std::fs::write(&config_path, r#"{ "schema": "ghost@1.0.0" }"#).unwrap();
        let writer = FilesystemMemWriter::new(dir.clone());
        let mount = Mount {
            mem: "specs".to_string(),
            schema: None,
            storage: MountStorage::Folder { path: dir },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mut engine =
            Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
        assert_eq!(engine.quarantined_mems().len(), 1);
        // Un-repaired reload keeps the quarantine (refreshed reason,
        // typed refusal).
        let err = engine.reload_one_mem("specs").unwrap_err();
        assert_eq!(err.code(), "MEM_QUARANTINED");
        assert_eq!(engine.quarantined_mems().len(), 1);

        // Repair: repin the config to a resolvable schema (what
        // `mem set-schema` does below boot), then reload.
        std::fs::write(&config_path, r#"{ "schema": "default@1.0.0" }"#).unwrap();
        engine
            .reload_one_mem("specs")
            .expect("repaired mem re-attaches on reload");
        assert!(
            engine.quarantined_mems().is_empty(),
            "roster entry disappears after re-attach"
        );
        // …and the mem serves again in the same process.
        let mut sections = indexmap::IndexMap::new();
        sections.insert("identity".to_string(), "back".to_string());
        sections.insert("purpose".to_string(), "post-repair service".to_string());
        engine
            .create_entity_with_ctx(
                crate::engine::CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "specs".to_string(),
                    title: "Back".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: indexmap::IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                &crate::vcs::CommitContext::internal(),
            )
            .expect("reattached mem serves writes");
    }

    /// Criterion 2 complement (agent-trust plan 04): a healthy mem
    /// whose entity body wiki-links INTO a quarantined mem loads
    /// normally — the link degrades like any dangling cross-mem link
    /// (stub target), no cascade failure.
    #[test]
    fn cross_mem_link_into_quarantined_mem_degrades_without_cascade() {
        let tmp = TempDir::new().unwrap();
        let healthy_dir = tmp.path().join("healthy");
        std::fs::create_dir_all(&healthy_dir).unwrap();
        std::fs::write(
            healthy_dir.join("linker.md"),
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n---\n\
             # Linker\n\n## Identity\n\nsee [[badpin:target]] for detail.\n",
        )
        .unwrap();
        let badpin_dir = tmp.path().join("badpin");
        std::fs::create_dir_all(&badpin_dir).unwrap();
        let mount = |mem: &str, dir: std::path::PathBuf, pin: &str| {
            (
                Mount {
                    mem: mem.to_string(),
                    schema: Some(SchemaRef::new(pin, semver::Version::new(1, 0, 0))),
                    storage: MountStorage::Folder { path: dir.clone() },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: true,
                    migration_target: None,
                },
                Box::new(FilesystemMemWriter::new(dir)) as Box<dyn MemBackend>,
            )
        };
        let engine = Engine::from_mounts(vec![
            mount("healthy", healthy_dir, "default"),
            mount("badpin", badpin_dir, "ghost"),
        ])
        .expect("boot survives the cross-mem link into the quarantined mem");
        assert_eq!(engine.quarantined_mems().len(), 1);
        // The linking entity loaded; its target degrades to a stub /
        // dangling link — no cascade, no partial-truth serving of the
        // quarantined mem.
        let linker = engine
            .get_entity(&crate::EntityId::new("healthy", "linker"))
            .expect("linking entity loads");
        assert_eq!(linker.entity_type, "spec");
        assert!(
            engine
                .get_entity(&crate::EntityId::new("badpin", "target"))
                .is_none_or(|e| e.stub),
            "the quarantined-side target is at most a stub, never real data"
        );
    }

    /// Agent-trust plan 06, criterion 3 complement: a workspace where
    /// one mem pins an authored schema still on the retired
    /// `propagating_relationships` key boots — that mem quarantines
    /// with the rename error as its reason (never workspace-fatal),
    /// while healthy mems load and serve.
    #[test]
    fn old_key_authored_schema_quarantines_pinning_mem_never_workspace() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Workspace marker + two folder mems.
        std::fs::create_dir_all(root.join(".memstead").join("state")).unwrap();
        std::fs::write(
            root.join(".memstead").join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        for m in ["healthy", "oldkey"] {
            std::fs::create_dir_all(root.join(m)).unwrap();
        }
        std::fs::write(
            root.join(".memstead").join("state").join("mounts.json"),
            r#"{ "format": "memstead-mounts-3", "mounts": [
                { "mem": "healthy", "schema": "default@1.1.0", "storage": { "type": "folder", "path": "healthy" }, "capability": "write", "lifecycle": "eager", "cross_linkable": true },
                { "mem": "oldkey", "schema": "fieldschema@0.1.0", "storage": { "type": "folder", "path": "oldkey" }, "capability": "write", "lifecycle": "eager", "cross_linkable": true }
            ] }"#,
        )
        .unwrap();
        // The authored package, still on the retired key.
        let pkg = root
            .join(".memstead")
            .join("schemas")
            .join("fieldschema@0.1.0");
        std::fs::create_dir_all(pkg.join("types")).unwrap();
        std::fs::write(
            pkg.join("schema.yaml"),
            "name: fieldschema\nversion: 0.1.0\ndescription: field schema\nwhen_to_use: tests\ntypes:\n  - thing\nrelationships:\n  mode: strict\n  definitions:\n    - name: PART_OF\n      description: h\n      default_weight: 1.0\n      acyclic: true\n    - name: _default\n      description: f\n      default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n",
        )
        .unwrap();
        std::fs::write(
            pkg.join("types").join("thing.yaml"),
            "name: thing\ndescription: t\nwhen_to_use: h\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\npropagating_relationships: []\nupdatable_fields:\n  - title\nhealth_required_fields: []\nstaleness_threshold_days: 90\nwrite_rules: []\n",
        )
        .unwrap();

        let engine = Engine::from_workspace_root(root)
            .expect("the old-key schema quarantines its mem, never the workspace");
        assert!(engine.mounts().iter().any(|m| m.mem == "healthy"));
        let q = engine
            .quarantine_reason("oldkey")
            .expect("oldkey mem is quarantined");
        assert_eq!(q.reason_code, "SCHEMA_LOAD_FAILED");
        assert!(
            q.reason_message.contains("no_self_loop_relationships"),
            "quarantine reason is the rename error naming the new key: {}",
            q.reason_message
        );
    }

    /// Agent-trust plan 06, criterion 2: a mem pinned to the new
    /// ingest@0.3.0 reports its edge-less entry entities as leaf
    /// population, zero false orphans; the prior version (0.2.0) is
    /// unchanged — the same entity still counts as an orphan there.
    #[test]
    fn ingest_0_3_entries_are_leaves_prior_version_unchanged() {
        let entry_md = "---\ntype: coverage_gap\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nstatus: open\n---\n# Gap\n\n## Area\n\nan uncovered area.\n";
        let boot = |pin: &str| {
            let tmp = TempDir::new().unwrap();
            let dir = tmp.path().to_path_buf();
            std::fs::create_dir_all(dir.join(".memstead")).unwrap();
            std::fs::write(
                dir.join(".memstead").join("config.json"),
                format!("{{ \"schema\": \"{pin}\" }}"),
            )
            .unwrap();
            std::fs::write(dir.join("gap.md"), entry_md).unwrap();
            let writer = FilesystemMemWriter::new(dir.clone());
            let mount = Mount {
                mem: "proc".to_string(),
                schema: None,
                storage: MountStorage::Folder { path: dir },
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            };
            let engine =
                Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)])
                    .unwrap();
            (engine.health(), tmp)
        };

        let (health_new, _t1) = boot("ingest@0.3.0");
        assert_eq!(
            health_new.orphan_count, 0,
            "0.3.0 entry types are leaves — zero false orphans"
        );
        assert_eq!(
            health_new
                .leaf_entities_by_type
                .get("ingest@0.3.0:coverage_gap"),
            Some(&1),
            "the population stays visible: {:?}",
            health_new.leaf_entities_by_type
        );

        let (health_old, _t2) = boot("ingest@0.2.0");
        assert_eq!(
            health_old.orphan_count, 1,
            "the prior version's behaviour is unchanged"
        );
        assert!(health_old.leaf_entities_by_type.is_empty());
    }

    /// A workspace mixing one broken mem with healthy siblings boots,
    /// serves the healthy mems fully, and refuses typed on the
    /// quarantined one — the plenum shape (one bad pin, thirteen
    /// healthy hostages) can no longer occur. Drives the pin-failure
    /// and missing-pin variants in one fixture.
    #[test]
    fn broken_mem_quarantines_while_healthy_siblings_serve() {
        let tmp = TempDir::new().unwrap();
        let make_mount = |mem: &str, pin: Option<SchemaRef>| {
            let dir = tmp.path().join(mem);
            std::fs::create_dir_all(&dir).unwrap();
            let writer = FilesystemMemWriter::new(dir.clone());
            (
                Mount {
                    mem: mem.to_string(),
                    schema: pin,
                    storage: MountStorage::Folder { path: dir },
                    capability: MountCapability::Write,
                    lifecycle: MountLifecycle::Eager,
                    cross_linkable: true,
                    migration_target: None,
                },
                Box::new(writer) as Box<dyn MemBackend>,
            )
        };
        let healthy = make_mount(
            "healthy",
            Some(SchemaRef::new("default", semver::Version::new(1, 0, 0))),
        );
        let bad_pin = make_mount(
            "badpin",
            Some(SchemaRef::new("ghost", semver::Version::new(1, 0, 0))),
        );
        let missing_pin = make_mount("nopin", None);
        // Backend-failure variant: the mount's storage path is a FILE,
        // so the backend's entity walk fails at read time.
        let bad_io_path = tmp.path().join("badio");
        std::fs::write(&bad_io_path, "not a directory").unwrap();
        let bad_io = (
            Mount {
                mem: "badio".to_string(),
                schema: Some(SchemaRef::new("default", semver::Version::new(1, 0, 0))),
                storage: MountStorage::Folder {
                    path: bad_io_path.clone(),
                },
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            },
            Box::new(FilesystemMemWriter::new(bad_io_path)) as Box<dyn MemBackend>,
        );
        let mut engine = Engine::from_mounts(vec![healthy, bad_pin, missing_pin, bad_io])
            .expect("mixed workspace boots");

        // Roster: both broken mems, each with its own typed reason.
        let codes: std::collections::HashMap<String, String> = engine
            .quarantined_mems()
            .iter()
            .map(|q| (q.mount.mem.clone(), q.reason_code.clone()))
            .collect();
        assert_eq!(
            codes.get("badpin").map(String::as_str),
            Some("SCHEMA_NOT_FOUND")
        );
        assert_eq!(
            codes.get("nopin").map(String::as_str),
            Some("MEM_CONFIG_INCOMPLETE")
        );
        assert!(
            codes.contains_key("badio"),
            "backend read failure quarantines too: {codes:?}"
        );

        // The healthy mem is fully writable.
        let mut sections = indexmap::IndexMap::new();
        sections.insert("identity".to_string(), "alive".to_string());
        sections.insert("purpose".to_string(), "proof of service".to_string());
        let created = engine
            .create_entity_with_ctx(
                crate::engine::CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "healthy".to_string(),
                    title: "Alive".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: indexmap::IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                &crate::vcs::CommitContext::internal(),
            )
            .expect("healthy mem serves writes");
        assert_eq!(created.id.to_string(), "healthy--alive");

        // Writes against a quarantined mem refuse with the typed code.
        let mut sections = indexmap::IndexMap::new();
        sections.insert("identity".to_string(), "x".to_string());
        let err = engine
            .create_entity_with_ctx(
                crate::engine::CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "badpin".to_string(),
                    title: "Nope".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: indexmap::IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                &crate::vcs::CommitContext::internal(),
            )
            .unwrap_err();
        assert_eq!(err.code(), "MEM_QUARANTINED");
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
    fn from_workspace_root_quarantines_git_branch_mount_on_lean() {
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
        // Deliberate replacement of the historical wholesale-abort
        // assertion (agent-trust plan 04): the lean binary meeting a
        // git-branch mount QUARANTINES that mem (typed
        // UNSUPPORTED_WORKSPACE_SHAPE reason) instead of refusing the
        // whole workspace — the judgment is unchanged, the blast
        // radius shrinks to the one mount the lean flavour cannot
        // serve.
        let engine = Engine::from_workspace_root(tmp.path())
            .expect("lean boot quarantines the git-branch mount, never fails the workspace");
        let roster = engine.quarantined_mems();
        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].mount.mem, "specs");
        assert_eq!(roster[0].reason_code, "UNSUPPORTED_WORKSPACE_SHAPE");
        assert_eq!(engine.unknown_mem_error("specs").code(), "MEM_QUARANTINED");
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
        assert_eq!(engine.status().mem_count, 1, "exactly one mount");
        assert!(
            engine.status().entity_count >= 1,
            "the standalone mem's entity must load"
        );
    }

    #[test]
    fn booting_a_workspace_writes_nothing_into_it() {
        // A read is a read. Booting an engine over a workspace must not
        // touch a single byte of it: not the config, not the mount state,
        // and not the authoring meta-schemas, which used to be republished
        // on every boot and so stamped a newer binary's copy over whatever
        // was on disk. That made a read-only mount, an installed
        // third-party mem, and a sealed corpus impossible to read without
        // modifying. The meta-schemas now publish from the schema-authoring
        // commands instead. This test fails if any boot-time write returns.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".memstead").join("meta-schemas")).unwrap();
        std::fs::write(
            root.join(".memstead").join("config.json"),
            r#"{"schema":"default@1.0.0"}"#,
        )
        .unwrap();
        // A deliberately stale meta-schema: byte-different from the embedded
        // one, so a returning republish overwrites it and the test sees it.
        let stale = root
            .join(".memstead")
            .join("meta-schemas")
            .join("type-definition.schema.json");
        std::fs::write(&stale, "{\"title\":\"stale, from an older binary\"}").unwrap();
        std::fs::write(
            root.join("hello.md"),
            "---\ntype: spec\n---\n# Hello\n\n## Identity\n\nStandalone body.\n",
        )
        .unwrap();

        fn snapshot(dir: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
            let mut out = std::collections::BTreeMap::new();
            let mut stack = vec![dir.to_path_buf()];
            while let Some(d) = stack.pop() {
                for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if let Ok(bytes) = std::fs::read(&p) {
                        out.insert(p, bytes);
                    }
                }
            }
            out
        }

        let before = snapshot(root);
        let engine = Engine::from_workspace_root(root).expect("the fixture must boot");
        assert_eq!(engine.status().mem_count, 1, "fixture sanity: one mount");
        let after = snapshot(root);

        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>(),
            "boot created or removed a file in the workspace it read"
        );
        for (path, bytes) in &before {
            assert_eq!(
                bytes,
                after.get(path).unwrap(),
                "boot rewrote {} in the workspace it read",
                path.display()
            );
        }
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

    /// A workspace whose installed schema violates the heading
    /// round-trip rule still boots and serves reads; the violation
    /// surfaces as a `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` load warning
    /// (merged into health), never as a boot failure — refusing at
    /// boot would brick every workspace that installed such a schema
    /// before the install gate existed.
    #[test]
    fn boot_keeps_loading_violating_schema_and_surfaces_health_finding() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        let pkg = schemas_dir.join("debate");
        std::fs::create_dir_all(pkg.join("types")).unwrap();
        std::fs::write(
            pkg.join("schema.yaml"),
            r#"name: debate
version: 0.1.0
description: sealed-violator fixture
when_to_use: tests
types:
  - question
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
        )
        .unwrap();
        std::fs::write(
            pkg.join("types").join("question.yaml"),
            r#"name: question
description: t
when_to_use: tests
sections:
  - key: answers
    heading: Answers argued
    required: true
    search_weight: 10.0
    write_rules: []
  - key: notes
    heading: Notes
    required: false
    search_weight: 3.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - answers
  - notes
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - answers
  - notes
health_required_fields:
  - answers
staleness_threshold_days: 90
write_rules: []
"#,
        )
        .unwrap();

        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(
            mem_dir.join("q.md"),
            "---\ntype: question\n---\n# Q\n\n## Answers argued\n\nTwo answers.\n",
        )
        .unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = Mount {
            mem: "debate-mem".to_string(),
            schema: Some(SchemaRef::new("debate", semver::Version::new(0, 1, 0))),
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
        .expect("a violating sealed schema must keep loading, never refuse boot");

        // Reads still serve.
        assert!(
            engine.status().entity_count >= 1,
            "entities load despite the schema violation"
        );

        // The violation is a health finding with the full tuple.
        let hits: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter_map(|w| match w {
                WarningHint::SchemaHeadingRoundtripViolation {
                    mem,
                    schema_ref,
                    violations,
                } => Some((mem.clone(), schema_ref.clone(), violations.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "exactly one schema-level finding; all warnings = {:?}",
            engine.load_warnings()
        );
        let (mem, schema_ref, violations) = &hits[0];
        assert_eq!(mem, "debate-mem");
        assert_eq!(schema_ref, "debate@0.1.0");
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].type_name, "question");
        assert_eq!(violations[0].key, "answers");
        assert_eq!(violations[0].heading, "Answers argued");
        assert_eq!(violations[0].derived_key, "answers_argued");
    }

    /// The other half of "still serves reads AND writes": a mem pinned
    /// to a sealed heading-round-trip-violating schema accepts writes.
    /// The update commits (refusal complement: it is NOT refused), the
    /// schema-level health finding persists after the write, and the
    /// write-path `SECTION_HEADING_DIVERGENCE` warning fires where its
    /// condition holds (the file carries a heading that derives to the
    /// written key while the schema declares a different heading text).
    #[test]
    fn sealed_violator_mem_still_serves_writes() {
        // Same fixture as the read test above.
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        let pkg = schemas_dir.join("debate");
        std::fs::create_dir_all(pkg.join("types")).unwrap();
        std::fs::write(
            pkg.join("schema.yaml"),
            r#"name: debate
version: 0.1.0
description: sealed-violator fixture
when_to_use: tests
types:
  - question
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
        )
        .unwrap();
        std::fs::write(
            pkg.join("types").join("question.yaml"),
            r#"name: question
description: t
when_to_use: tests
sections:
  - key: answers
    heading: Answers argued
    required: true
    search_weight: 10.0
    write_rules: []
  - key: notes
    heading: Notes
    required: false
    search_weight: 3.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - answers
  - notes
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - answers
  - notes
health_required_fields:
  - answers
staleness_threshold_days: 90
write_rules: []
"#,
        )
        .unwrap();

        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        // The file's own heading "Answers" derives to the key
        // `answers`, differing from the schema's declared
        // "Answers argued" — the divergence-warning condition.
        std::fs::write(
            mem_dir.join("q.md"),
            "---\ntype: question\n---\n# Q\n\n## Answers\n\nTwo answers.\n",
        )
        .unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = Mount {
            mem: "debate-mem".to_string(),
            schema: Some(SchemaRef::new("debate", semver::Version::new(0, 1, 0))),
            storage: MountStorage::Folder { path: mem_dir },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
            Some(&schemas_dir),
        )
        .expect("a violating sealed schema must keep loading");

        let id = crate::EntityId::new("debate-mem", "q");
        let hash = engine.get_entity(&id).unwrap().content_hash.clone();
        let mut sections = indexmap::IndexMap::new();
        sections.insert("answers".to_string(), "Updated answers body.".to_string());
        let outcome = engine
            .update_entity(
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    anchors_unset: Vec::new(),
                    id: id.clone(),
                    expected_hash: Some(hash),
                    sections,
                    append_sections: indexmap::IndexMap::new(),
                    patch_sections: indexmap::IndexMap::new(),
                    metadata: indexmap::IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                },
                crate::vcs::Actor::Cli,
                None,
                None,
            )
            .expect("a write against a sealed-violator mem must NOT be refused");
        assert!(!outcome.commit_sha.is_empty(), "the write commits");
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.code() == "SECTION_HEADING_DIVERGENCE"),
            "the write-path divergence warning fires where its condition holds: {:?}",
            outcome.warnings
        );

        // The schema-level finding persists after the write.
        assert!(
            engine.load_warnings().iter().any(|w| matches!(
                w,
                WarningHint::SchemaHeadingRoundtripViolation { mem, .. } if mem == "debate-mem"
            )),
            "the health finding persists across writes"
        );
        // The written content is durably on disk and survives the
        // reparse — under the catch-all, because the violating schema's
        // declared heading cannot round-trip to the written key. That
        // fork is exactly what the divergence warning announced (and
        // what the persisting health finding tells the operator to fix
        // at the schema); the "serves writes" guarantee is that the
        // write lands and nothing refuses, not that a broken schema
        // routes content correctly.
        let entity = engine.get_entity(&id).unwrap();
        assert!(
            entity
                .sections
                .values()
                .any(|s| s.contains("Updated answers body.")),
            "written content survives the round-trip (in the catch-all): {:?}",
            entity.sections
        );
    }

    /// A search `mem` filter naming no visible mem refuses typed
    /// `UNKNOWN_MEM` — matching every other mem-naming surface — while a
    /// quarantined mem keeps its established typed refusal and a VALID
    /// mem with no matches still returns success with 0 hits. Absence of
    /// mem and absence of matches are never the same answer
    /// (backlog-sweep plan 05, decision 4).
    #[test]
    fn search_mem_filter_gates_against_visible_roster() {
        use crate::vcs::Actor;
        use crate::workspace_store::WorkspaceStoreAdapter;
        use indexmap::IndexMap;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead").join("config.json"),
            r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
        )
        .unwrap();
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
                    mounts: vec![folder_mount("specs", mem_dir.clone())],
                    settings: crate::workspace::WorkspaceSettings::default(),
                },
            )
            .unwrap();
        let mut engine = Engine::from_workspace_root(tmp.path()).unwrap();
        let mut sections = IndexMap::new();
        sections.insert(
            "identity".to_string(),
            "Zebra searching fixture.".to_string(),
        );
        sections.insert("purpose".to_string(), "Search gate test.".to_string());
        engine
            .create_entity(
                crate::CreateEntityArgs {
                    mem: "specs".to_string(),
                    title: "Zebra".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    anchors: Vec::new(),
                    dry_run: false,
                },
                Actor::Agent,
                None,
                None,
            )
            .unwrap();

        let scope = |mem: Option<&str>, term: &str| crate::ops::SearchScope {
            query: Some(crate::ops::Query {
                any: vec![term.to_string()],
                ..Default::default()
            }),
            mem: mem.map(str::to_string),
            ..Default::default()
        };

        // Nonexistent mem → typed UNKNOWN_MEM, never success-with-0-hits.
        let err = engine
            .search(&scope(Some("no-such-mem"), "zebra"))
            .expect_err("a nonexistent mem filter must refuse");
        assert_eq!(err.code(), "UNKNOWN_MEM", "got {err:?}");

        // Valid mem, matching query → hits.
        let hit = engine.search(&scope(Some("specs"), "zebra")).unwrap();
        assert!(hit.total >= 1, "the fixture entity matches: {hit:?}");

        // Valid mem, no matches → success with 0 hits (the gate
        // distinguishes absence of mem from absence of matches).
        let none = engine
            .search(&scope(Some("specs"), "quixotic-nonword"))
            .unwrap();
        assert_eq!(none.total, 0, "{none:?}");
    }

    // ---- Engine::reload_one_mem -----------------------------------

    // ---- lazy mounts (flywheel W7/01) -----------------------------

    fn lazy_folder_mount(mem: &str, path: std::path::PathBuf) -> Mount {
        Mount {
            lifecycle: MountLifecycle::Lazy,
            ..folder_mount(mem, path)
        }
    }

    fn write_spec(dir: &Path, slug: &str, title: &str, extra: &str) {
        std::fs::write(
            dir.join(format!("{slug}.md")),
            format!("---\ntype: spec\n---\n# {title}\n\n## Identity\n\nBody.\n{extra}"),
        )
        .unwrap();
    }

    fn two_mem_dirs(tmp: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
        let eager_dir = tmp.path().join("eag");
        let lazy_dir = tmp.path().join("laz");
        std::fs::create_dir_all(&eager_dir).unwrap();
        std::fs::create_dir_all(&lazy_dir).unwrap();
        write_spec(&eager_dir, "alpha", "Alpha", "");
        write_spec(&lazy_dir, "omega", "Omega", "");
        (eager_dir, lazy_dir)
    }

    fn mixed_engine(eager_dir: &Path, lazy_dir: &Path) -> Engine {
        Engine::from_mounts(vec![
            (
                folder_mount("eag", eager_dir.to_path_buf()),
                Box::new(FilesystemMemWriter::new(eager_dir.to_path_buf())) as Box<dyn MemBackend>,
            ),
            (
                lazy_folder_mount("laz", lazy_dir.to_path_buf()),
                Box::new(FilesystemMemWriter::new(lazy_dir.to_path_buf())) as Box<dyn MemBackend>,
            ),
        ])
        .unwrap()
    }

    /// The lazy lifecycle defers exactly the entity load: boot carries
    /// the mem on the roster with its schema resolved but no entities;
    /// the first operation touching it (the `reload_if_stale` funnel)
    /// triggers the load; afterwards the mem behaves identically to an
    /// eager mount and the deferred state is gone for good.
    #[test]
    fn lazy_mount_defers_and_first_read_loads() {
        let tmp = TempDir::new().unwrap();
        let (eager_dir, lazy_dir) = two_mem_dirs(&tmp);
        let mut engine = mixed_engine(&eager_dir, &lazy_dir);

        // Boot: eager loaded, lazy on the roster but deferred.
        assert!(
            engine
                .get_entity(&crate::EntityId::new("eag", "alpha"))
                .is_some()
        );
        assert_eq!(engine.deferred_mems(), vec!["laz"]);
        assert!(engine.mem_is_deferred("laz"));
        assert!(
            engine
                .get_entity(&crate::EntityId::new("laz", "omega"))
                .is_none(),
            "deferred mem's entities are not in the store yet"
        );
        // Never absent: the mount roster and schema map both carry it.
        assert!(engine.schema_for("laz").is_some());

        // First read triggers the load through the operation funnel.
        engine.reload_if_stale(Some("laz"));
        assert!(engine.deferred_mems().is_empty());
        let omega = engine
            .get_entity(&crate::EntityId::new("laz", "omega"))
            .expect("first read loads the mem");
        assert_eq!(omega.title, "Omega");

        // Identical to eager from here on: a write round-trips.
        let (actor, client) = cli_actor();
        engine
            .create_entity(
                empty_create_args("laz", "Later"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(
            engine
                .get_entity(&crate::EntityId::new("laz", "later"))
                .is_some()
        );
    }

    /// An operation scoped to an eager mem never loads a lazy sibling
    /// as a side effect; a workspace-scoped funnel pass loads every
    /// deferred mem so no answer computes over a partial store.
    #[test]
    fn scoped_operation_never_loads_lazy_sibling() {
        let tmp = TempDir::new().unwrap();
        let (eager_dir, lazy_dir) = two_mem_dirs(&tmp);
        let mut engine = mixed_engine(&eager_dir, &lazy_dir);

        engine.reload_if_stale(Some("eag"));
        assert_eq!(
            engine.deferred_mems(),
            vec!["laz"],
            "an eager-scoped operation must not load the lazy sibling"
        );

        engine.reload_if_stale(None);
        assert!(
            engine.deferred_mems().is_empty(),
            "a workspace-scoped pass loads every deferred mem"
        );
    }

    /// A deferred load that fails quarantines the mem at the moment of
    /// first read, with the same typed reporting an eager boot failure
    /// produces — never an empty-mem impression.
    #[test]
    fn lazy_load_failure_quarantines_typed() {
        let tmp = TempDir::new().unwrap();
        let (eager_dir, lazy_dir) = two_mem_dirs(&tmp);
        let mut engine = mixed_engine(&eager_dir, &lazy_dir);

        // The backend's tree is destroyed between boot and first read —
        // a plain file now sits where the mem directory was, so the
        // entity walk errors rather than reading an empty directory.
        std::fs::remove_dir_all(&lazy_dir).unwrap();
        std::fs::write(&lazy_dir, b"not a directory").unwrap();
        engine.reload_if_stale(Some("laz"));

        let q = engine
            .quarantine_reason("laz")
            .expect("failed deferred load quarantines, never serves empty");
        assert!(!q.reason_code.is_empty());
        assert!(
            !engine.mem_names().contains(&"laz"),
            "a quarantined mem leaves the serving roster"
        );
        assert!(engine.deferred_mems().is_empty());
    }

    /// Quarantine-ENTRY invalidation pin (flywheel W8/01, first
    /// grade's refutation): entering quarantine from a failed deferred
    /// load removes the mem's schema (epoch bump) — a search memo
    /// filled beforehand is then stale-keyed and MUST clear, or the
    /// next search trips the memo-key debug_assert (the grade's live
    /// repro). Same rule pinned for the reattach-failure branch by
    /// re-failing the reattach.
    #[test]
    fn quarantine_entry_invalidates_both_memos() {
        let tmp = TempDir::new().unwrap();
        let (eager_dir, lazy_dir) = two_mem_dirs(&tmp);
        let mut engine = mixed_engine(&eager_dir, &lazy_dir);

        // Fill both memos while the lazy mem is still deferred.
        let _ = engine.communities();
        let _ = engine.search_indexes();
        assert!(engine.search_indexes_memo.get().is_some());

        // Destroy the backend; the first read quarantines the mem.
        std::fs::remove_dir_all(&lazy_dir).unwrap();
        std::fs::write(&lazy_dir, b"not a directory").unwrap();
        engine.reload_if_stale(Some("laz"));
        assert!(engine.quarantine_reason("laz").is_some());
        assert!(
            engine.search_indexes_memo.get().is_none(),
            "quarantine entry bumps the schemas epoch — the search memo must clear"
        );
        assert!(
            engine.community_memo.get().is_none(),
            "quarantine entry must clear the community memo too"
        );
        // The next search must not trip the memo-key assert.
        let _ = engine.search_indexes();

        // Reattach FAILURE (backend still broken): same rule.
        let _ = engine.communities();
        let _ = engine.search_indexes();
        let _ = engine.reload_one_mem("laz");
        assert!(engine.quarantine_reason("laz").is_some());
        assert!(
            engine.search_indexes_memo.get().is_none(),
            "a failed reattach bumps the epoch — the search memo must clear"
        );
        let _ = engine.search_indexes();
    }

    /// Quarantine-reattach regression pin (flywheel W8/01, criterion
    /// 2's complement): the reattach path routes through the one-mem
    /// reload, which invalidates BOTH derived memos — pinned so
    /// incremental maintenance can never silently degrade it.
    #[test]
    fn quarantine_reattach_invalidates_both_memos() {
        let tmp = TempDir::new().unwrap();
        let (eager_dir, lazy_dir) = two_mem_dirs(&tmp);
        let mut engine = mixed_engine(&eager_dir, &lazy_dir);

        // Quarantine the lazy mem: destroy its backend, trigger load.
        std::fs::remove_dir_all(&lazy_dir).unwrap();
        std::fs::write(&lazy_dir, b"not a directory").unwrap();
        engine.reload_if_stale(Some("laz"));
        assert!(engine.quarantine_reason("laz").is_some());

        // Fill both memos while the mem sits quarantined.
        let _ = engine.communities();
        let _ = engine.search_indexes();
        assert!(engine.community_memo.get().is_some());
        assert!(engine.search_indexes_memo.get().is_some());

        // Restore the backend and reattach via the reload path.
        std::fs::remove_file(&lazy_dir).unwrap();
        std::fs::create_dir_all(&lazy_dir).unwrap();
        write_spec(&lazy_dir, "omega", "Omega", "");
        engine
            .reload_one_mem("laz")
            .expect("reattach succeeds once the backend is back");
        assert!(engine.quarantine_reason("laz").is_none());

        // Both memos cleared — the reattached mem's entities must be
        // visible to the next partition and the next search.
        assert!(
            engine.community_memo.get().is_none(),
            "reattach must invalidate the community memo"
        );
        assert!(
            engine.search_indexes_memo.get().is_none(),
            "reattach must invalidate the search memo"
        );
    }

    /// The lazy load runs the same validation gauntlet an eager boot
    /// runs: content an eager boot warns about produces the SAME
    /// warning when its mem loads lazily — deferral changes when, not
    /// whether.
    #[test]
    fn lazy_load_runs_the_same_validation_gauntlet() {
        let tmp = TempDir::new().unwrap();
        let (eager_dir, lazy_dir) = two_mem_dirs(&tmp);
        // Hand-authored invalid relation in the LAZY mem.
        std::fs::write(
            lazy_dir.join("bad.md"),
            "---\ntype: spec\n---\n# Bad\n\n## Identity\n\nx.\n\n## Relationships\n\n- **MADE_UP_TYPE**: [[laz--omega]]\n",
        )
        .unwrap();

        let mut engine = mixed_engine(&eager_dir, &lazy_dir);
        let warned_before = engine
            .load_warnings()
            .iter()
            .any(|w| matches!(w, WarningHint::ParsedRelationInvalid { .. }));
        engine.reload_if_stale(Some("laz"));
        let warned_after = engine
            .load_warnings()
            .iter()
            .any(|w| matches!(w, WarningHint::ParsedRelationInvalid { .. }));
        assert!(
            !warned_before && warned_after,
            "the gauntlet fires at load time: before={warned_before} after={warned_after}, \
             warnings: {:?}",
            engine.load_warnings()
        );
        // And the offending relation was dropped, as an eager boot drops it.
        let bad = engine
            .get_entity(&crate::EntityId::new("laz", "bad"))
            .unwrap();
        assert!(bad.relationships.is_empty());
    }

    /// The destructive guard sees referrers living in DEFERRED mems:
    /// deleting an entity whose incoming references originate in a lazy,
    /// not-yet-loaded mem refuses `HAS_INCOMING_REFS` exactly as an
    /// eager boot refuses it. The third final grade demonstrated the
    /// counterexample live — the scoped reload left the referrer's mem
    /// unloaded and the delete destroyed the entity an eager boot
    /// protects. The guard now takes the full load first.
    #[test]
    fn delete_guard_sees_referrers_in_deferred_mems() {
        use crate::engine::{DeleteEntityArgs, RelateEntityArgs};

        let tmp = TempDir::new().unwrap();
        let (eager_dir, lazy_dir) = two_mem_dirs(&tmp);
        let (actor, client) = cli_actor();

        // Author the cross-mem referrer through the mutation surface
        // while both mems are eager (with the cross-link grant), so the
        // persisted relation is exactly what a real workspace carries.
        {
            let mut authoring = Engine::from_mounts(vec![
                (
                    folder_mount("eag", eager_dir.to_path_buf()),
                    Box::new(FilesystemMemWriter::new(eager_dir.to_path_buf()))
                        as Box<dyn MemBackend>,
                ),
                (
                    folder_mount("laz", lazy_dir.to_path_buf()),
                    Box::new(FilesystemMemWriter::new(lazy_dir.to_path_buf()))
                        as Box<dyn MemBackend>,
                ),
            ])
            .unwrap();
            let mut settings = crate::workspace::WorkspaceSettings::default();
            settings.cross_mem_links.insert(
                "laz".to_string(),
                memstead_schema::workspace_config::CrossLinkValue::List(vec!["eag".to_string()]),
            );
            authoring.set_settings(settings);
            authoring
                .relate_entity(
                    RelateEntityArgs {
                        source: crate::EntityId::new("laz", "omega"),
                        expected_hash: None,
                        rel_type: "USES".to_string(),
                        target: crate::EntityId::new("eag", "alpha"),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("cross-mem relate lands under the grant");
        }

        // Fresh boot with the REFERRER's mem lazy: the incoming edge
        // into `eag--alpha` lives in an unloaded mem. The guard must
        // still see it — full load before destructive adjudication.
        let mut engine = mixed_engine(&eager_dir, &lazy_dir);
        assert!(engine.mem_is_deferred("laz"));
        let err = engine
            .delete_entity(
                DeleteEntityArgs {
                    id: crate::EntityId::new("eag", "alpha"),
                    expected_hash: None,
                },
                actor,
                Some(&client),
                None,
            )
            .expect_err("a referenced entity must refuse deletion, lazy referrer or not");
        assert!(
            matches!(err, EngineError::HasIncomingRefs { .. }),
            "expected HAS_INCOMING_REFS, got {err:?}"
        );
        assert!(
            engine
                .get_entity(&crate::EntityId::new("eag", "alpha"))
                .is_some(),
            "the entity survives"
        );
    }

    /// The write-time acyclicity guard sees edges living in DEFERRED
    /// mems: an add that closes a cycle THROUGH a lazy, not-yet-loaded
    /// mem refuses `RELATIONSHIP_CYCLE` exactly as an eager boot
    /// refuses it. The fourth final grade demonstrated the
    /// counterexample on a three-mem chain — the mid-path edge lived on
    /// the lazy mem's entity, the walk over the endpoint mems missed
    /// it, the cycle landed, and the next eager boot dropped an
    /// INNOCENT pre-existing edge to break it. A two-mem fixture would
    /// pass vacuously (relate loads both endpoint mems); three mems
    /// with the middle one lazy is the discriminating shape.
    #[test]
    fn acyclicity_guard_sees_edges_in_deferred_mems() {
        use crate::engine::RelateEntityArgs;
        use memstead_schema::workspace_config::CrossLinkValue;

        let tmp = TempDir::new().unwrap();
        let dirs: Vec<std::path::PathBuf> = ["ma", "mb", "mc"]
            .iter()
            .map(|m| {
                let d = tmp.path().join(m);
                std::fs::create_dir_all(&d).unwrap();
                d
            })
            .collect();
        write_spec(&dirs[0], "node", "Node A", "");
        write_spec(&dirs[1], "node", "Node B", "");
        write_spec(&dirs[2], "node", "Node C", "");

        let mounts = |lazy_mid: bool| -> Vec<(Mount, Box<dyn MemBackend>)> {
            ["ma", "mb", "mc"]
                .iter()
                .zip(dirs.iter())
                .map(|(m, d)| {
                    let mut mount = folder_mount(m, d.clone());
                    if lazy_mid && *m == "ma" {
                        mount.lifecycle = MountLifecycle::Lazy;
                    }
                    (
                        mount,
                        Box::new(FilesystemMemWriter::new(d.clone())) as Box<dyn MemBackend>,
                    )
                })
                .collect()
        };
        let grants = || {
            let mut settings = crate::workspace::WorkspaceSettings::default();
            for (from, to) in [("mb", "ma"), ("ma", "mc"), ("mc", "mb")] {
                settings
                    .cross_mem_links
                    .insert(from.to_string(), CrossLinkValue::List(vec![to.to_string()]));
            }
            settings
        };
        let relate = |engine: &mut Engine, from: &str, to: &str| {
            let (actor, client) = cli_actor();
            engine.relate_entity(
                RelateEntityArgs {
                    source: crate::EntityId::new(from, "node"),
                    expected_hash: None,
                    rel_type: "DEPENDS_ON".to_string(),
                    target: crate::EntityId::new(to, "node"),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
        };

        // Author the chain mb→ma→mc while everything is eager.
        {
            let mut authoring = Engine::from_mounts(mounts(false)).unwrap();
            authoring.set_settings(grants());
            relate(&mut authoring, "mb", "ma").expect("mb→ma lands");
            relate(&mut authoring, "ma", "mc").expect("ma→mc lands");
        }

        // Fresh boot with the MID-PATH mem lazy: closing mc→mb would
        // complete the cycle mb→ma→mc→mb through the unloaded mem.
        let mut engine = Engine::from_mounts(mounts(true)).unwrap();
        engine.set_settings(grants());
        assert!(engine.mem_is_deferred("ma"), "the mid-path mem is deferred");
        let err = relate(&mut engine, "mc", "mb")
            .expect_err("a cycle through a deferred mem must refuse, as eager refuses");
        assert!(
            matches!(err, EngineError::RelationshipCycle { .. }),
            "expected RELATIONSHIP_CYCLE, got {err:?}"
        );
    }

    /// The BATCH relate path runs the same acyclicity guard over the
    /// same full store: a two-entry batch (the shape MCP routes to
    /// `batch_relate`) whose first entry closes a cycle through a
    /// deferred mid-path mem is refused whole, exactly as an eager
    /// boot refuses it. The fifth lazy-mount grade demonstrated the
    /// complement live: without the batch-path full load the cycle
    /// committed silently.
    #[test]
    fn batch_acyclicity_guard_sees_edges_in_deferred_mems() {
        use crate::engine::RelateEntityArgs;
        use memstead_schema::workspace_config::CrossLinkValue;

        let tmp = TempDir::new().unwrap();
        let dirs: Vec<std::path::PathBuf> = ["ma", "mb", "mc"]
            .iter()
            .map(|m| {
                let d = tmp.path().join(m);
                std::fs::create_dir_all(&d).unwrap();
                d
            })
            .collect();
        write_spec(&dirs[0], "node", "Node A", "");
        write_spec(&dirs[1], "node", "Node B", "");
        write_spec(&dirs[2], "node", "Node C", "");
        // A second entity in mc so the batch's second entry can be an
        // intra-mem edge that touches ONLY mc — a target in any other
        // mem would put that mem on the batch's touched-mems reload
        // list and load it by that route, masking the guard under test.
        write_spec(&dirs[2], "node2", "Node C2", "");

        let mounts = |lazy_mid: bool| -> Vec<(Mount, Box<dyn MemBackend>)> {
            ["ma", "mb", "mc"]
                .iter()
                .zip(dirs.iter())
                .map(|(m, d)| {
                    let mut mount = folder_mount(m, d.clone());
                    if lazy_mid && *m == "ma" {
                        mount.lifecycle = MountLifecycle::Lazy;
                    }
                    (
                        mount,
                        Box::new(FilesystemMemWriter::new(d.clone())) as Box<dyn MemBackend>,
                    )
                })
                .collect()
        };
        let grants = || {
            let mut settings = crate::workspace::WorkspaceSettings::default();
            for (from, to) in [("mb", "ma"), ("ma", "mc"), ("mc", "mb")] {
                settings
                    .cross_mem_links
                    .insert(from.to_string(), CrossLinkValue::List(vec![to.to_string()]));
            }
            settings
        };
        let relate_args = |from: &str, to: &str| RelateEntityArgs {
            source: crate::EntityId::new(from, "node"),
            expected_hash: None,
            rel_type: "DEPENDS_ON".to_string(),
            target: crate::EntityId::new(to, "node"),
            remove: false,
            description: None,
            dry_run: false,
        };

        // Author the chain mb→ma→mc while everything is eager.
        {
            let (actor, client) = cli_actor();
            let mut authoring = Engine::from_mounts(mounts(false)).unwrap();
            authoring.set_settings(grants());
            authoring
                .relate_entity(relate_args("mb", "ma"), actor, Some(&client), None)
                .expect("mb→ma lands");
            let (actor2, client2) = cli_actor();
            authoring
                .relate_entity(relate_args("ma", "mc"), actor2, Some(&client2), None)
                .expect("ma→mc lands");
        }

        // Fresh boot with the MID-PATH mem lazy; a two-entry batch
        // whose first edge mc→mb completes the cycle mb→ma→mc→mb
        // through the unloaded mem must refuse whole.
        let mut engine = Engine::from_mounts(mounts(true)).unwrap();
        engine.set_settings(grants());
        assert!(engine.mem_is_deferred("ma"), "the mid-path mem is deferred");
        let (actor, client) = cli_actor();
        let result = engine
            .batch_relate(
                vec![
                    (relate_args("mc", "mb"), None),
                    // The second entry makes the batch two entries —
                    // the shape MCP routes to `batch_relate` — and is
                    // an intra-mem edge inside mc: it touches no other
                    // mem (so it cannot load ma via the touched-mems
                    // reload) and closes no cycle of its own.
                    (
                        RelateEntityArgs {
                            source: crate::EntityId::new("mc", "node2"),
                            expected_hash: None,
                            rel_type: "DEPENDS_ON".to_string(),
                            target: crate::EntityId::new("mc", "node"),
                            remove: false,
                            description: None,
                            dry_run: false,
                        },
                        None,
                    ),
                ],
                actor,
                Some(&client),
                false,
            )
            .expect("the batch call itself returns a report-all envelope");
        assert!(
            !result.applied,
            "a batch closing a cycle through a deferred mem must refuse, as eager refuses; got applied with {} succeeded",
            result.succeeded
        );
    }

    /// Write-time cross-mem target verification (flywheel W7/02): a
    /// relate into a DEFERRED Write mem verifies the target against
    /// storage without loading the mem. A storage-verified target is
    /// admitted with a LoadTime stub and NO auto-stub warning (the
    /// entity exists — it resolves when the mem loads); a genuinely
    /// absent target keeps today's forward-reference mechanic, warning
    /// included. Either way the target mem stays deferred.
    #[test]
    fn relate_into_deferred_mem_verifies_against_storage_without_load() {
        use crate::engine::RelateEntityArgs;
        use memstead_schema::workspace_config::CrossLinkValue;

        let tmp = TempDir::new().unwrap();
        let (eager_dir, lazy_dir) = two_mem_dirs(&tmp);
        let mut engine = mixed_engine(&eager_dir, &lazy_dir);
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            "eag".to_string(),
            CrossLinkValue::List(vec!["laz".to_string()]),
        );
        engine.set_settings(settings);

        let relate = |engine: &mut Engine, to: &str| {
            let (actor, client) = cli_actor();
            engine.relate_entity(
                RelateEntityArgs {
                    source: crate::EntityId::new("eag", "alpha"),
                    expected_hash: None,
                    rel_type: "SUPPORTS".to_string(),
                    target: crate::EntityId::new("laz", to),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
        };

        // Storage-verified target: laz--omega exists on disk.
        let outcome = relate(&mut engine, "omega").expect("verified target admits");
        assert!(
            engine.mem_is_deferred("laz"),
            "verification never loads the mem"
        );
        assert!(
            !outcome
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::AutoStubCreated { .. })),
            "a storage-verified target is not an auto-stub case: {:?}",
            outcome.warnings
        );
        let stub = engine
            .store()
            .get(&crate::EntityId::new("laz", "omega"))
            .expect("until-load stub present");
        assert!(stub.stub);
        assert_eq!(
            stub.stub_kind,
            Some(crate::entity::StubKind::LoadTime),
            "verified-in-storage stub carries the load-time kind"
        );

        // Genuinely absent target: forward-reference mechanic intact.
        let outcome = relate(&mut engine, "missing").expect("absent Write-mem target auto-stubs");
        assert!(engine.mem_is_deferred("laz"), "still no load");
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::AutoStubCreated { .. })),
            "absent target keeps the auto-stub warning: {:?}",
            outcome.warnings
        );
        let stub = engine
            .store()
            .get(&crate::EntityId::new("laz", "missing"))
            .expect("forward-reference stub present");
        assert_eq!(
            stub.stub_kind,
            Some(crate::entity::StubKind::ForwardReference)
        );
    }

    /// The read-only contract, now answerable without load (flywheel
    /// W7/02): an entity PRESENT in a deferred read-only mem's storage
    /// is admitted — the refusal never fires merely because the mem is
    /// unloaded — and an ABSENT one refuses with the existing typed
    /// error. The mem stays deferred through both.
    #[test]
    fn readonly_deferred_target_answers_from_storage() {
        use crate::engine::RelateEntityArgs;
        use memstead_schema::workspace_config::CrossLinkValue;

        let tmp = TempDir::new().unwrap();
        let (eager_dir, lazy_dir) = two_mem_dirs(&tmp);
        let mut ro_mount = lazy_folder_mount("laz", lazy_dir.to_path_buf());
        ro_mount.capability = crate::workspace::MountCapability::ReadOnly;
        let mut engine = Engine::from_mounts(vec![
            (
                folder_mount("eag", eager_dir.to_path_buf()),
                Box::new(FilesystemMemWriter::new(eager_dir.to_path_buf())) as Box<dyn MemBackend>,
            ),
            (
                ro_mount,
                Box::new(FilesystemMemWriter::new(lazy_dir.to_path_buf())) as Box<dyn MemBackend>,
            ),
        ])
        .unwrap();
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            "eag".to_string(),
            CrossLinkValue::List(vec!["laz".to_string()]),
        );
        engine.set_settings(settings);

        let relate = |engine: &mut Engine, to: &str| {
            let (actor, client) = cli_actor();
            engine.relate_entity(
                RelateEntityArgs {
                    source: crate::EntityId::new("eag", "alpha"),
                    expected_hash: None,
                    rel_type: "SUPPORTS".to_string(),
                    target: crate::EntityId::new("laz", to),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
        };

        relate(&mut engine, "omega").expect("present-in-storage RO target admits");
        assert!(
            engine.mem_is_deferred("laz"),
            "the admit never loads the mem"
        );

        let err =
            relate(&mut engine, "missing").expect_err("absent RO target keeps the typed refusal");
        assert!(
            matches!(err, EngineError::CrossMemTargetNotFound { .. }),
            "expected CROSS_MEM_TARGET_NOT_FOUND, got {err:?}"
        );
        assert!(
            engine.mem_is_deferred("laz"),
            "the refusal never loads the mem either"
        );
    }

    /// A cross-mem body link from an eager mem into a lazy one is a
    /// stub until the target mem loads, and resolves to the real entity
    /// afterwards — never silently dropped, never a spurious permanent
    /// warning.
    #[test]
    fn cross_mem_link_into_lazy_mem_resolves_on_load() {
        let tmp = TempDir::new().unwrap();
        let (eager_dir, lazy_dir) = two_mem_dirs(&tmp);
        write_spec(
            &eager_dir,
            "linker",
            "Linker",
            "\nSee [[laz--omega]] for detail.\n",
        );

        let mut engine = mixed_engine(&eager_dir, &lazy_dir);
        let target = crate::EntityId::new("laz", "omega");
        assert!(
            engine.get_entity(&target).is_none_or(|e| e.stub),
            "before the lazy load the cross-mem target is at most a stub, never real"
        );

        engine.reload_if_stale(Some("laz"));
        let resolved = engine.get_entity(&target).expect("target loaded");
        assert!(!resolved.stub, "after the load the target is real");
        assert!(
            !engine.load_warnings().iter().any(
                |w| matches!(w, WarningHint::SuspiciousNestedPrefix { resolved_id, .. } if resolved_id.as_ref() == target.as_ref())
            ),
            "no lingering nested-prefix warning for a resolved cross-mem link: {:?}",
            engine.load_warnings()
        );
    }
}
