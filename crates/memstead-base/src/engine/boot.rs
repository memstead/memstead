//! Engine construction — `from_mounts*` and `from_workspace_root`.
//!
//! `from_mounts` is the in-process constructor every test, in-process
//! embedder, and the MCP filesystem server reach through.
//! `from_workspace_root` is the folder boot path that produces the
//! same engine from a workspace root; the git-branch counterpart lives in
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
            // Compared as SEMVER, not as full strings.
            // The old rule fired on any difference including the `+g<sha>`
            // build metadata, so every rebuild between releases read as
            // skew — noise on any workspace whose binary is built from
            // source, which is every one of this project's own workspaces. Semver ordering
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
            // An archive-backed mount carries its own sealed vocabulary
            // and resolves against it on every workspace shape: a
            // folder workspace has no sealed-package storage to stage a
            // third party's schema into, and a mem published under such
            // a schema would otherwise quarantine there. The embedded
            // package is layered per mount, between the workspace tier
            // and the built-ins, and never into the shared catalogue —
            // a sealed vocabulary is this mount's, not something a
            // writable mem may pin.
            let embedded = embedded_archive_schemas(&m.mount);
            let resolved = if embedded.is_empty() {
                SchemaResolver::new(&builtin_schemas).resolve(effective_pin)
            } else {
                let mut per_mount: Vec<Arc<Schema>> = Vec::with_capacity(
                    workspace_schemas.len() + embedded.len() + builtin_schemas_only.len(),
                );
                per_mount.extend(workspace_schemas.iter().cloned());
                per_mount.extend(embedded);
                per_mount.extend(builtin_schemas_only.iter().cloned());
                SchemaResolver::new(&per_mount).resolve(effective_pin)
            };
            let schema = match resolved {
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
            // SCOPED TO PATH-BACKED STORAGE, and the exception is a deliberate
            // departure from backend parity rather than an oversight. For a
            // git-branch mount, "the ref does not exist" is ALSO the normal
            // state of a mem never pushed or never cloned, and quarantine
            // removes the mount from the serving set. Parity was attempted
            // twice. Each attempt cleared one blocker and uncovered the next:
            // the mount lookup (a line), the pre-push gate's schema-before-ref
            // ordering (fixed here, and worth fixing on its own), and finally
            // `pull`'s pre-fast-forward validation, which needs a resolved
            // schema a quarantined mem does not have. That last one is a
            // semantic decision about what validation means for a mem being
            // cloned, not plumbing. Recorded in the session log.
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
            // unbacked or quarantined.
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

        // The baseline a later `persist_state` diffs against: the roster
        // as handed to this constructor. A mount quarantined before this
        // point is deliberately absent from it, so the merge treats its
        // on-disk record as "not ours to remove" and leaves it standing —
        // degrade, never disappear.
        let mounts_baseline =
            std::cell::RefCell::new(mounted.iter().map(|m| m.mount.clone()).collect::<Vec<_>>());

        Ok(Self {
            mounts: mounted,
            mounts_baseline,
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
            backend_factory: crate::workspace_store::instantiate_local_backend,
            unmounted_storage_prober: None,
            schemas_epoch: 0,
            git_branch_ops: None,
            event_subscribers: Arc::new(std::sync::Mutex::new(
                crate::engine::events::SubscriberRegistry::new(),
            )),
            pending_mem_changed: Vec::new(),
            roster_fingerprint: None,
            roster_subscribers: Arc::new(std::sync::Mutex::new((0, Vec::new()))),
            recently_unmounted: std::collections::HashSet::new(),
            #[cfg(test)]
            inject_unmount_failure: None,
            mutation_clock: Arc::new(crate::engine::mutation::wall_clock_now),
            current_role: crate::vcs::Role::Unspecified,
            current_identity: None,
            current_actor: crate::vcs::Actor::Agent,
            current_client: None,
        })
    }

    /// Boot an engine from a workspace root using only the local
    /// backends (folder + archive): the folder boot path. The CLI's
    /// folder-workspace path reaches the engine through this entry
    /// point; git-branch mounts quarantine unless a backend factory is set.
    ///
    /// Loads the workspace through [`crate::FileWorkspaceStore`],
    /// instantiates each mount's backend via
    /// [`crate::instantiate_local_backend`], and constructs the
    /// engine via [`Engine::from_mounts`].
    ///
    /// Errors:
    /// - [`Layout::Empty`](crate::Layout) → [`BootError::NotInitialised`]
    /// - any mount declaring [`crate::workspace::MountStorage::GitBranch`]
    ///   → [`BootError::Instantiate`] wrapping
    ///   [`crate::InstantiateError::GitBranchBackendUnavailable`]
    /// - underlying store / engine failures lift through the
    ///   `#[from]` conversions
    pub fn from_workspace_root(workspace_root: &Path) -> Result<Self, BootError> {
        use crate::workspace_store::{
            FileWorkspaceStore, Layout, WorkspaceStoreAdapter, detect_layout,
            instantiate_local_backend,
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
            match instantiate_local_backend(&mount) {
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
        // built-ins only). This is the folder boot path's schema-authoring
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
        engine.capture_roster_fingerprint();
        // Load the workspace store's pipeline configs — the v2 single-record
        // binding store — and expose them read-only. A malformed config
        // surfaces a typed `StoreError::Parse` naming the file (early
        // validation of operator-edited configs); an absent `projections/`
        // directory resolves to empty. A file in a retired binding format
        // quarantines its binding with `StoreError::LegacyProjectionStore`
        // (the engine never reads a prior generation and no longer converts
        // one; the binding is re-authored with `memstead projection init`).
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

/// Public re-export of [`resolve_builtin_schema_pin`] for the lifecycle
/// orchestrators in `crate::mem_management`. Mirrors the orchestrators'
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

/// The sealed schema package an archive-backed mount carries inside its
/// `.mem` (`.memstead/schema/`), read through the same sealed reader
/// the install validator uses, so the mount can resolve its pin from
/// the archive itself. Empty for every other storage kind, for an
/// archive mount with no on-disk path (the from-bytes boot already
/// threads its embedded schemas through `extra_schemas`), and for an
/// archive that cannot be read: the pin then fails to resolve and the
/// mem quarantines with the ordinary not-found reason, which names the
/// sources consulted.
pub(crate) fn embedded_archive_schemas(mount: &Mount) -> Vec<Arc<Schema>> {
    let crate::workspace::MountStorage::Archive { path } = &mount.storage else {
        return Vec::new();
    };
    if path.as_os_str().is_empty() || !path.is_file() {
        return Vec::new();
    }
    use crate::schema_source::SchemaSource as _;
    crate::schema_source::ArchiveSchemaSource::from_path(path)
        .ok()
        .and_then(|source| source.read_schemas().ok())
        .unwrap_or_default()
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
    // One backend pass for the whole mem: the git-branch backend
    // answers with a single tree walk, where the per-path route
    // re-inflated the root tree once per entity (quadratic in the
    // mem size — the boot cost the sizing curve reported as
    // super-linear).
    let reads = backend.read_all_entities()?;
    let mut entries: Vec<SourceEntry> = Vec::with_capacity(reads.len());
    let mut errors: Vec<SourceReadError> = Vec::new();
    for (path, read) in reads {
        match read {
            Ok(bytes) => match String::from_utf8(bytes) {
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
            Err(e) => errors.push(SourceReadError {
                source_path: path,
                error: std::io::Error::other(e.to_string()),
            }),
        }
    }
    Ok((entries, errors))
}

#[cfg(test)]
mod tests;
