//! Mount registration: registering and unregistering writable mems and read mounts, the load-warning channel, the full refresh and the state persist.

use super::*;

impl Engine {
    /// Unregister a writable mem at runtime. Engine-level
    /// primitive that `memstead_mem_delete` builds on.
    ///
    /// Removes the named mount from [`Self::mounts`], drops the
    /// mem's entities from the store, refreshes the
    /// [`MemRouterSnapshot`] via `Arc::make_mut` (COW swap so
    /// readers holding a pre-swap snapshot see the pre-state for
    /// their lifetime), and invalidates the community + search
    /// memos. Does NOT touch the backend's on-disk state — the
    /// caller (`delete_mem` orchestrator) decides whether to
    /// remove the directory / gitdir after this returns.
    ///
    /// Returns `Ok(Some(backend))` when the mem was present and
    /// unregistered — the caller can drive any backend-specific
    /// follow-up cleanup (`backend.delete_artifacts()` for the
    /// mem-repo branch + `__MEMSTEAD` config when `delete_files=true`).
    /// Returns `Ok(None)` when no mount named the mem (idempotent —
    /// repeated calls are safe).
    pub fn unregister_writable_mem(
        &mut self,
        mem_name: &str,
    ) -> Result<Option<Box<dyn MemBackend>>, EngineError> {
        let pos = self.mounts.iter().position(|m| m.mount.mem == mem_name);
        let Some(idx) = pos else {
            return Ok(None);
        };

        // Drop the mount first — releases all engine-side state that
        // referenced the backend. The `Box<dyn MemBackend>` itself
        // travels back to the caller so backend-side cleanup
        // (`delete_artifacts`) can run after the engine snapshot
        // settled.
        let mount = self.mounts.remove(idx);

        // Drop the schema entry for this mem (kept in lockstep
        // with `self.mounts`).
        self.schemas_remove(&mount.mount.mem);

        // Drop entities. The store's mem index is the
        // authoritative count; the return value (number of
        // entities removed) is informational only — the caller
        // already knows the mem and doesn't need the count.
        let _removed = self.store.remove_entities_by_mem(mem_name);

        // Purge load-time warnings attributed to the removed mem.
        // `health()` merges `self.load_warnings` unconditionally, so
        // a skipped purge leaves phantom warnings citing entities the
        // store no longer holds — for the whole engine lifetime, since
        // nothing else clears the accumulator on the MCP path.
        // Attribution is by SOURCE mem only (`WarningHint::source_mem`):
        // a warning whose source entity lives in a surviving mem stays
        // even when its target pointed into the deleted mem — the
        // invalid row still exists in that survivor's markdown and
        // remains visible drift (recover-worthy), not stale state.
        self.load_warnings
            .retain(|w| w.source_mem() != Some(mem_name));

        // COW snapshot swap on the mem_router. `Arc::make_mut`
        // clones the inner snapshot when other Arcs exist; if this
        // is the only handle (typical for the engine's lifetime),
        // it returns the existing inner directly without cloning.
        // Readers that captured an `Arc` before this call observe
        // the pre-swap state — the in-flight handler's
        // `mem_router()` borrow is unaffected by this mutation.
        Arc::make_mut(&mut self.mem_router).remove_writable(mem_name);

        // Invalidate dependent memos — community detection + search
        // indexes were computed over the pre-removal store and are
        // now stale. Mutation paths already invalidate; this
        // matches the contract.
        self.invalidate_communities();
        self.invalidate_search_indexes();

        Ok(Some(mount.backend))
    }

    /// Register a read-only mount at runtime — the install path's
    /// engine primitive. Same registration pipeline as
    /// [`Self::register_writable_mem`] (collision probe, config read,
    /// schema resolution, entity load, router swap); the router branch
    /// lands the mount in the read-only slot for
    /// `capability: ReadOnly` + `Archive` storage, so `is_writable`
    /// stays false and `archive_path_for_mem` resolves.
    pub fn register_read_mount(
        &mut self,
        mount: Mount,
        backend: Box<dyn MemBackend>,
        origin: MemOrigin,
    ) -> Result<(), EngineError> {
        self.register_writable_mem_inner(mount, backend, origin, true)
    }

    /// Unregister a read-only mount at runtime — the uninstall path's
    /// engine primitive, mirroring [`Self::unregister_writable_mem`]
    /// for the read-only slot. Returns `Ok(None)` when the name is
    /// not a registered read-only mount (writable mems are the
    /// delete/unregister verbs' business, deliberately not this
    /// one's). Registration removal only — the backing archive file
    /// (global cache) is never touched.
    pub fn unregister_read_mount(
        &mut self,
        mem_name: &str,
    ) -> Result<Option<Box<dyn MemBackend>>, EngineError> {
        let pos = self.mounts.iter().position(|m| {
            m.mount.mem == mem_name
                && m.mount.capability == crate::workspace::MountCapability::ReadOnly
        });
        let Some(idx) = pos else {
            return Ok(None);
        };
        let mount = self.mounts.remove(idx);
        self.schemas_remove(&mount.mount.mem);
        let _removed = self.store.remove_entities_by_mem(mem_name);
        self.load_warnings
            .retain(|w| w.source_mem() != Some(mem_name));
        Arc::make_mut(&mut self.mem_router).remove_read_only(mem_name);
        self.invalidate_communities();
        self.invalidate_search_indexes();
        Ok(Some(mount.backend))
    }

    /// Append a typed load-time warning from outside the engine's own
    /// load pipeline — the boot orchestrators (which live in the full
    /// crate) use this to surface one-time migrations they perform
    /// around engine construction.
    pub fn push_load_warning(&mut self, warning: crate::ops::WarningHint) {
        self.load_warnings.push(warning);
    }

    /// Register a writable mem at runtime. Engine-level primitive
    /// that `memstead_mem_create` builds on.
    ///
    /// Steps:
    /// 1. Name collision probe against the current `mem_router`
    ///    snapshot. Writable AND read-only entries collide; the
    ///    error surfaces the colliding source so the orchestrator
    ///    can render a recovery hint.
    /// 2. Schema resolution via the built-in catalogue (mirrors
    ///    [`Self::from_mounts`]; workspace-authored schema
    ///    resolution lifts later).
    /// 3. Per-mem config load (folder backends only; git-branch /
    ///    archive return None — same contract as
    ///    [`Self::from_mounts`]).
    /// 4. Entity load via the backend, parse, push into the engine's
    ///    store with a `LoadCollector` so drift warnings forward to
    ///    `self.load_warnings`.
    /// 5. Insert schema into [`Self::schemas`].
    /// 6. Push the [`MountedBackend`] into [`Self::mounts`].
    /// 7. COW snapshot swap on [`Self::mem_router`] via
    ///    `Arc::make_mut` + `add_writable(name, dir, origin, mem_path)`.
    ///    Folder mounts surface their on-disk path; other backends
    ///    register with `dir: None` (matches full's contract).
    ///    `mem_path` carries the create-time organisational `path`
    ///    component (mirrors `MemCreateParams.path`) — the
    ///    delete-side lifecycle composer reads it back to rebuild the
    ///    `<mem_path>/<name>` candidate the create-side composer
    ///    matched against. Caller threads `None` for flat-layout
    ///    registrations and `Some(p)` for hierarchical ones.
    /// 8. Invalidate community + search memos.
    ///
    /// Returns `Err(EngineError::MemNameCollision)` when the name
    /// is already registered. Other failures (schema-not-found,
    /// backend read errors) propagate as their typed variants. On
    /// failure no engine mutation happens: every potentially-
    /// mutating step runs only after the collision probe succeeds,
    /// and intermediate failures propagate before the mount /
    /// router are touched.
    pub fn register_writable_mem(
        &mut self,
        mount: Mount,
        backend: Box<dyn MemBackend>,
        origin: MemOrigin,
    ) -> Result<(), EngineError> {
        self.register_writable_mem_inner(mount, backend, origin, true)
    }

    /// [`Self::register_writable_mem`] with the workspace-global
    /// passes (relation validation, alias remap, memo invalidation)
    /// made optional: `run_global_passes: false` lets a batch caller
    /// ([`Self::full_refresh`]) attach N mounts and run the global
    /// passes ONCE afterwards instead of N times under the engine
    /// lock. A `false` caller MUST run
    /// [`Self::finish_batched_registrations`] after its loop, or
    /// loaded relations skip validation and alias edges stay
    /// unmapped.
    fn register_writable_mem_inner(
        &mut self,
        mount: Mount,
        backend: Box<dyn MemBackend>,
        origin: MemOrigin,
        run_global_passes: bool,
    ) -> Result<(), EngineError> {
        // Step 1: name collision probe.
        if let Some(existing) = self.mem_router.origin_for_mem(&mount.mem) {
            return Err(EngineError::MemNameCollision {
                name: mount.mem.clone(),
                source_origin: existing.render_source(),
            });
        }
        if self.mem_router.archive_path_for_mem(&mount.mem).is_some() {
            return Err(EngineError::MemNameCollision {
                name: mount.mem.clone(),
                source_origin: "attached read mem".to_string(),
            });
        }

        // Step 2: per-mem config load via the backend trait. Read
        // before resolving the schema — the mem's own config carries
        // the authoritative pin (mirrors the boot path), so a mem
        // re-registered or mounted from another machine resolves from
        // its own backend, not this workspace's mount expectation.
        let mem_config = backend.read_mem_config().ok().flatten().and_then(|bytes| {
            let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            memstead_schema::config::parse_mem_config(&value).ok()
        });

        // Step 3: schema resolution. `MemConfig.schema` is the
        // authoritative settled pin; `Mount.schema` is the fallback when
        // the config carries none, and an expectation assertion when it
        // does — a disagreement surfaces `SchemaPinMismatch` (config
        // wins, neither silently dropped). Mirrors `from_mounts_inner`.
        // Resolve against the engine's full loaded catalogue (already-
        // loaded workspace/local-storage schemas layered over built-ins)
        // so a mem registered against a backend-installed (e.g.
        // git-branch `__MEMSTEAD:schemas/` ref) schema resolves.
        let mut builtin_schemas: Vec<std::sync::Arc<memstead_schema::Schema>> =
            self.workspace_schemas().to_vec();
        // An archive-backed mount's own sealed vocabulary sits between
        // the workspace tier and the built-ins, for this resolution
        // only — see `from_mounts_inner` for why it never joins the
        // shared catalogue.
        builtin_schemas.extend(crate::engine::boot::embedded_archive_schemas(&mount));
        builtin_schemas.extend(
            memstead_schema::builtins::load_builtin_schemas()
                .map_err(|e| EngineError::SchemaResolverInit(e.to_string()))?,
        );
        let config_pin = mem_config.as_ref().and_then(|c| c.schema.as_ref());
        let mount_pin = mount.schema.as_ref();
        if let (Some(cfg), Some(mp)) = (config_pin, mount_pin)
            && cfg != mp
        {
            self.load_warnings
                .push(crate::ops::WarningHint::SchemaPinMismatch {
                    mem: mount.mem.clone(),
                    config_pin: cfg.as_display(),
                    mount_pin: mp.as_display(),
                });
        }
        let settled_pin = config_pin.or(mount_pin);
        let effective_pin = mount
            .migration_target
            .as_ref()
            .or(settled_pin)
            .ok_or_else(|| EngineError::MemConfigIncomplete {
                mem: mount.mem.clone(),
                missing_fields: vec!["schema".to_string()],
            })?
            .clone();
        let schema = crate::engine::SchemaResolver::new(&builtin_schemas)
            .resolve(&effective_pin)
            .map_err(|sources| {
                EngineError::SchemaNotFound {
                    mem: mount.mem.clone(),
                    pin: effective_pin.as_display(),
                    sources,
                    install_hint: None,
                }
                .with_schema_install_probe(self.workspace_root())
            })?;

        // Step 4: load entities via the backend, push into the
        // engine's store with a LoadCollector so drift warnings
        // forward into `self.load_warnings`. Derive the mem
        // roster + last-segment suffixes from the POST-registration
        // view (new mem included) so cross-mem references
        // targeting the new mem resolve correctly during this
        // load.
        let (entries, read_errors) = collect_source_entries(backend.as_ref())?;
        if let Some(w) = crate::engine::boot::unbacked_mount_warning(
            &mount,
            backend.as_ref(),
            Some(entries.len()),
        ) {
            self.load_warnings.push(w);
        }
        let load_result = parse_entries(entries, read_errors, &mount.mem, schema.as_ref());

        let mut mem_names: Vec<String> = self.mounts.iter().map(|m| m.mount.mem.clone()).collect();
        mem_names.push(mount.mem.clone());
        let known_suffixes: Vec<String> = mem_names
            .iter()
            .map(|n| crate::entity::store_builder::last_segment_suffix(n).to_string())
            .collect();
        let fallback = engine_fallback_type();
        push_entities_into_store(
            &mut self.store,
            load_result.entities,
            fallback.as_ref(),
            Some(crate::entity::store_builder::LoadCollector {
                warnings: &mut self.load_warnings,
                known_suffixes: &known_suffixes,
                mem_names: &mem_names,
            }),
        );
        self.load_errors.extend(load_result.errors);

        // Step 5: insert schema (kept in lockstep with `self.mounts`).
        self.schemas_insert(mount.mem.clone(), schema);

        // Re-run the parse-time relation validator now that the new
        // mem's schema is in `self.schemas`. Mirrors the boot path
        // (`Engine::from_mounts_inner`) — hand-edited or externally-
        // generated markdown in the newly-attached mount goes through
        // the same gauntlet (grammar / unknown_rel_type / shape /
        // cycle) and offending relations are dropped with typed
        // `PARSED_RELATION_INVALID` warnings on `self.load_warnings`.
        // The newly-pushed mount isn't in `self.mounts` yet (that's
        // Step 6 below), so build the map from `self.mounts` plus the
        // about-to-be-attached mount we're still holding.
        if run_global_passes {
            let mut mount_caps: std::collections::HashMap<
                String,
                crate::workspace::MountCapability,
            > = self
                .mounts
                .iter()
                .map(|m| (m.mount.mem.clone(), m.mount.capability))
                .collect();
            mount_caps.insert(mount.mem.clone(), mount.capability);
            crate::entity::store_builder::validate_loaded_relations(
                &mut self.store,
                &self.schemas,
                &mount_caps,
                &mut self.load_warnings,
            );
            crate::entity::store_builder::remap_alias_target_edge_sources(
                &mut self.store,
                &self.schemas,
            );
        }

        // Step 6: push the MountedBackend.
        let last_known_head = backend.current_head().ok().flatten();
        let mem_name_for_router = mount.mem.clone();
        let storage_for_router = mount.storage.clone();
        let mount_capability_for_router = mount.capability;
        self.mounts.push(MountedBackend {
            mount,
            backend,
            last_known_head,
            mem_config,
            // A runtime-created mem is authored live, not installed from
            // an archive — it carries no archive-borne provenance payload.
            archive_provenance: None,
            // Registered live and loaded in this call — never deferred.
            deferred: false,
        });

        // Step 7: COW snapshot swap on mem_router, branched on the
        // mount's capability — a read-only archive mount registers in
        // the router's read-only slot (so `is_writable` stays false
        // and `archive_path_for_mem` resolves), everything else in the
        // writable slot. Folder mounts surface their on-disk path;
        // other backends register with `dir: None` (mem-repo-backed
        // mounts have no working tree).
        match (&mount_capability_for_router, &storage_for_router) {
            (crate::workspace::MountCapability::ReadOnly, MountStorage::Archive { path }) => {
                Arc::make_mut(&mut self.mem_router)
                    .add_read_only(mem_name_for_router, path.clone());
            }
            _ => {
                let dir: Option<PathBuf> = match &storage_for_router {
                    MountStorage::Folder { path } => Some(path.clone()),
                    MountStorage::GitBranch { .. }
                    | MountStorage::Archive { .. }
                    | MountStorage::InMemory => None,
                };
                Arc::make_mut(&mut self.mem_router).add_writable(mem_name_for_router, dir, origin);
            }
        }

        // Step 8: invalidate dependent memos.
        if run_global_passes {
            self.invalidate_communities();
            self.invalidate_search_indexes();
        }

        Ok(())
    }

    /// The batched tail of `register_writable_mem_inner(...,
    /// run_global_passes: false)`: one workspace-global relation
    /// validation, one alias remap, one memo invalidation for the
    /// whole batch of registrations.
    /// [`Self::register_writable_mem`] without the workspace-global
    /// passes — the batch form the roster reconciliation uses; the
    /// caller runs [`Self::finish_batched_registrations`] once after.
    pub(crate) fn register_writable_mem_batched(
        &mut self,
        mount: Mount,
        backend: Box<dyn MemBackend>,
        origin: MemOrigin,
    ) -> Result<(), EngineError> {
        self.register_writable_mem_inner(mount, backend, origin, false)
    }

    pub(crate) fn finish_batched_registrations(&mut self) {
        let mount_caps: std::collections::HashMap<String, crate::workspace::MountCapability> = self
            .mounts
            .iter()
            .map(|m| (m.mount.mem.clone(), m.mount.capability))
            .collect();
        crate::entity::store_builder::validate_loaded_relations(
            &mut self.store,
            &self.schemas,
            &mount_caps,
            &mut self.load_warnings,
        );
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );
        self.invalidate_communities();
        self.invalidate_search_indexes();
    }

    /// Additive full refresh — the warm-server half of "restart the
    /// process": re-scan the schema sources and the mount manifest,
    /// making newly installed schema versions resolvable and newly
    /// registered mems usable, WITHOUT applying removals. The
    /// asymmetry is deliberate and is the whole safety argument:
    /// adding extends what the in-memory store can answer, while
    /// removing can strand entities, in-flight handles, and cached
    /// hashes the process is still serving. Removals are skipped and
    /// reported; a restart applies them.
    ///
    /// Failure model is per-item: a schema source or a mount that
    /// fails to refresh lands in `failures` and never surfaces as
    /// newly available; the others proceed. Each mount registration
    /// is all-or-nothing (every fallible step runs before the store
    /// is touched), so a failed item leaves no half-updated state.
    /// The workspace-global passes (relation validation, alias remap,
    /// memo invalidation) run ONCE per refresh regardless of how many
    /// mounts attached.
    ///
    /// A newly mounted mem starts cold and loads like any other
    /// mount. Content reload of pre-existing mems is NOT part of this
    /// method — callers that want both (the `memstead_reload
    /// full=true` surface) run the existing content-reload sweep
    /// alongside.
    pub fn full_refresh(&mut self) -> crate::ops::FullRefreshReport {
        let started = std::time::Instant::now();
        let mut report = crate::ops::FullRefreshReport::default();

        let Some(_root) = self.workspace_root.clone() else {
            report.failures.push(crate::ops::RefreshFailure {
                item: "workspace".to_string(),
                error: "engine has no workspace root (ad-hoc mount-list construction) — \
                        nothing to re-scan"
                    .to_string(),
            });
            report.elapsed_ms = started.elapsed().as_millis() as u64;
            return report;
        };

        // Workspace policy — same best-effort refresh the
        // workspace-wide content reload performs.
        self.refresh_workspace_settings_if_possible();

        // --- Schema sources, additively. ---
        self.refresh_schema_sources(&mut report);

        // --- Mount roster: the same reconciliation every operation runs
        // (roster.rs), forced here even when the fingerprint says
        // unchanged so the report is authoritative. Removals APPLY. ---
        match self.reconcile_roster_forced() {
            Ok(change) => {
                report.mems_mounted = change.added;
                report.mems_unmounted = change.removed;
                report.mems_quarantined = change.quarantined;
                report.failures.extend(change.failures);
            }
            Err(e) => report.failures.push(crate::ops::RefreshFailure {
                item: "mount-manifest".to_string(),
                error: e.to_string(),
            }),
        }
        report.mems_mounted.sort();
        report.mems_unmounted.sort();
        report.mems_quarantined.sort();

        report.elapsed_ms = started.elapsed().as_millis() as u64;
        report
    }

    /// Re-scan the schema sources additively into `report`
    /// (`schemas_added`, `schema_removals_skipped`, per-source failures).
    pub(crate) fn refresh_schema_sources(&mut self, report: &mut crate::ops::FullRefreshReport) {
        let Some(root) = self.workspace_root.clone() else {
            return;
        };
        use crate::schema_source::SchemaSource as _;
        let mut fresh: Vec<std::sync::Arc<memstead_schema::Schema>> = Vec::new();
        let mut sources_complete = true;
        match crate::schema_source::FolderSchemaSource::for_workspace(&root).read_schemas() {
            Ok(mut s) => fresh.append(&mut s),
            Err(e) => {
                sources_complete = false;
                report.failures.push(crate::ops::RefreshFailure {
                    item: "schema-source:folder".to_string(),
                    error: e.to_string(),
                });
            }
        }
        if let Some(ops) = self.git_branch_ops() {
            match (ops.read_ref_schemas)(&root) {
                Ok(mut s) => fresh.append(&mut s),
                Err(e) => {
                    sources_complete = false;
                    report.failures.push(crate::ops::RefreshFailure {
                        item: "schema-source:memstead-ref".to_string(),
                        error: e.to_string(),
                    });
                }
            }
        }
        let key = |s: &memstead_schema::Schema| {
            let (name, version) = s.id();
            format!("{name}@{version}")
        };
        let existing: std::collections::HashSet<String> =
            self.workspace_schemas.iter().map(|s| key(s)).collect();
        let fresh_keys: std::collections::HashSet<String> = fresh.iter().map(|s| key(s)).collect();
        for schema in fresh {
            let k = key(&schema);
            if !existing.contains(&k) && !report.schemas_added.contains(&k) {
                report.schemas_added.push(k);
                self.workspace_schemas.push(schema);
            }
        }
        report.schemas_added.sort();
        // Removal detection is only meaningful when every source was
        // actually readable — otherwise an unreadable source would
        // masquerade as a mass removal.
        if sources_complete {
            report.schema_removals_skipped = existing
                .difference(&fresh_keys)
                .cloned()
                .collect::<Vec<_>>();
            report.schema_removals_skipped.sort();
        }
    }

    /// Override the workspace root after construction. The full
    /// boot helper `memstead_git_branch::engine_from_workspace_root`
    /// calls this so the engine knows the path even when the boot
    /// route runs through the full adapter rather than
    /// [`Self::from_workspace_root`].
    pub fn set_workspace_root(&mut self, root: PathBuf) {
        self.workspace_root = Some(root);
        self.capture_roster_fingerprint();
    }

    /// Persist the engine's current mount list to the workspace
    /// store so a freshly-booted sibling process observes the same
    /// mem membership. Called by
    /// [`crate::mem_management::create_mem`] /
    /// [`crate::mem_management::delete_mem`] after the in-memory
    /// router mutation lands — without this, the per-mem content
    /// (branch + `__MEMSTEAD` config blob, or folder + `.memstead/config.json`)
    /// is already on disk, but the next process boot reads an empty
    /// `.memstead/state/mounts.json` and the engine starts with zero
    /// writable mems.
    ///
    /// No-op when `workspace_root` is unset (tests / ad-hoc
    /// consumers that build the engine directly from a mount list).
    /// Production boot paths (`Engine::from_workspace_root` and the
    /// full counterpart) always set the root, so the engine-side
    /// fix covers every caller — in-process embedders included
    /// — by construction.
    ///
    /// Hardcoded against [`crate::FileWorkspaceStore`] because that
    /// is the only V1 adapter; a future SQLite or remote adapter
    /// would install through a setter mirroring
    /// [`Self::set_backend_factory`].
    pub fn persist_state(&self) -> Result<(), EngineError> {
        let Some(root) = self.workspace_root.as_ref() else {
            return Ok(());
        };
        use crate::workspace_store::WorkspaceStoreAdapter as _;
        let store = crate::FileWorkspaceStore::new();
        let map = |e: crate::workspace_store::StoreError| {
            EngineError::Mem(format!("persist workspace state: {e}"))
        };

        // Publish THIS engine's changes, not its whole cached view. An
        // earlier version serialized `self.mounts` wholesale, so a
        // long-lived process silently dropped every mount a sibling
        // process had registered since the cache was taken — the same
        // condition the mem-config writers close by re-reading, reaching
        // the workspace roster through the one writer they did not cover.
        // The delta is computed against `mounts_baseline` (what this
        // engine last read or wrote) rather than against a clock: a
        // single-writer workspace has an identical on-disk roster, so
        // the merge is a no-op there.
        // The roster this engine speaks for is the attached mounts PLUS
        // the quarantined ones: a quarantined mem's retained Mount is
        // what lets `reload` re-attempt the attach after a repair, and
        // dropping it from the file is how "degrade, never disappear"
        // turns into "disappear". It is also the record the
        // quarantine-repair path repins before asking for a state write.
        let ours: Vec<crate::workspace::Mount> = self
            .mounts
            .iter()
            .map(|m| m.mount.clone())
            .chain(self.quarantined.iter().map(|q| q.mount.clone()))
            .collect();

        for attempt in 0..8 {
            let expected = store.read_state_bytes(root).map_err(map)?;
            let on_disk: Vec<crate::workspace::Mount> = match expected.as_deref() {
                Some(bytes) => store.parse_state_bytes(root, bytes).map_err(map)?,
                None => Vec::new(),
            };

            let merged = {
                let baseline = self.mounts_baseline.borrow();
                merge_mount_rosters(&baseline, &ours, on_disk)
            };
            let workspace = crate::workspace::Workspace {
                mounts: merged,
                settings: self.settings.clone(),
            };

            if store
                .save_state_cas(root, &workspace, expected.as_deref())
                .map_err(map)?
            {
                *self.mounts_baseline.borrow_mut() = ours;
                return Ok(());
            }
            if attempt == 7 {
                return Err(EngineError::Mem(
                    "workspace state is being written concurrently: eight compare-and-set \
                     attempts all lost the race. Retry, or find the writer that is not \
                     backing off."
                        .to_string(),
                ));
            }
        }
        unreachable!("the loop returns on success and on exhaustion")
    }
}

/// Three-way merge of a mount roster for a state write.
///
/// `baseline` is what the writing engine last read or wrote, `ours` is
/// its roster now, `on_disk` is what the file holds at this instant.
/// The result keeps every on-disk mount the writer did not touch (so a
/// sibling's registration survives), drops the ones the writer removed
/// since its baseline, and applies the ones it added or changed.
///
/// A mount present in `ours` unchanged since the baseline does NOT
/// overwrite the on-disk record of the same name: if a sibling edited
/// it and we did not, the sibling's edit is the newer statement about
/// it, and republishing our stale copy is exactly the loss this merge
/// exists to prevent.
fn merge_mount_rosters(
    baseline: &[crate::workspace::Mount],
    ours: &[crate::workspace::Mount],
    on_disk: Vec<crate::workspace::Mount>,
) -> Vec<crate::workspace::Mount> {
    use std::collections::{HashMap, HashSet};

    let ours_names: HashSet<&str> = ours.iter().map(|m| m.mem.as_str()).collect();
    let removed_by_us: HashSet<&str> = baseline
        .iter()
        .map(|m| m.mem.as_str())
        .filter(|n| !ours_names.contains(n))
        .collect();
    let baseline_by_name: HashMap<&str, &crate::workspace::Mount> =
        baseline.iter().map(|m| (m.mem.as_str(), m)).collect();

    let mut merged: Vec<crate::workspace::Mount> = on_disk
        .into_iter()
        .filter(|m| !removed_by_us.contains(m.mem.as_str()))
        .collect();

    for mount in ours {
        let untouched_by_us = baseline_by_name
            .get(mount.mem.as_str())
            .is_some_and(|b| *b == mount);
        match merged.iter_mut().find(|d| d.mem == mount.mem) {
            Some(slot) => {
                if !untouched_by_us {
                    *slot = mount.clone();
                }
            }
            None => merged.push(mount.clone()),
        }
    }
    merged
}
