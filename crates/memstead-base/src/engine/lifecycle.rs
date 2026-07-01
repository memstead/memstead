//! Engine lifecycle — settings/workspace-root setters, runtime
//! vault add/remove, reload, and export.
//!
//! `register_writable_vault` / `unregister_writable_vault` are the
//! engine-level primitives the `memstead_vault_create` / `memstead_vault_delete`
//! handlers build on. `reload_one_vault*` re-reads a mount's backend
//! and refreshes the in-memory store; `reload_each_writable_vault*`
//! sweeps every writable mount. `export_markdown` regenerates entity
//! markdown for folder mounts; `export_vault` produces a portable
//! `.mem` archive via the backend-aware dispatch in
//! [`crate::ops::export`].

use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::backend::{BackendError, VaultBackend};
use crate::engine_fallback_type;
use crate::entity::generator::generate_markdown;
use crate::entity::loader::parse_entries;
use crate::entity::store_builder::push_entities_into_store;
use crate::entity::EntityId;
use crate::ops::WarningHint;
use crate::vault::VaultOrigin;
use crate::workspace::{Mount, MountStorage, WorkspaceSettings};

use super::boot::collect_source_entries;
use super::{BackendFactory, Engine, EngineError, GitBranchOps, MountedBackend};

impl Engine {

    /// Replace the workspace-level settings. Called by
    /// [`Self::from_workspace_root`] (and the pro counterpart) after
    /// reading `.memstead/workspace.toml`. Tests / direct callers leave
    /// the default empty value in place. Cheap clone — settings
    /// carry only data shapes (raw rule lists, link policy map),
    /// no compiled matchers. Invalidates the lazy
    /// `create_rule_set_memo` so the next synthesis call rebuilds
    /// from the new policy.
    pub fn set_settings(&mut self, settings: WorkspaceSettings) {
        self.settings = settings;
        self.create_rule_set_memo = OnceCell::new();
    }

    /// Replace the backend factory. Pro consumers call this once at boot
    /// (`engine_from_workspace_root`) to install
    /// `memstead_git_branch::storage::instantiate_pro_backend` so the engine
    /// can materialise git-branch backends on top of folder + archive.
    /// Basis consumers leave the default in place.
    pub fn set_backend_factory(&mut self, factory: BackendFactory) {
        self.backend_factory = factory;
    }

    /// Install the git-branch ops bundle. Pro boot
    /// (`memstead_git_branch::engine_from_workspace_root`) calls this once
    /// at construction. Basis consumers leave it unset and the
    /// git-branch dispatch branches collapse to typed errors / empty
    /// reports — basis has no git-branch mounts.
    pub fn set_git_branch_ops(&mut self, ops: GitBranchOps) {
        self.git_branch_ops = Some(ops);
    }

    /// Install a schema package onto the workspace's git-branch backend —
    /// the unified `__MEMSTEAD:schemas/<name>@<version>/` ref. `files`
    /// are `(relative-path, bytes)` pairs (`schema.yaml`,
    /// `types/<t>.yaml`, optional `vault-template.json`). Returns the
    /// resulting commit sha; idempotent at the storage layer (an
    /// identical re-install produces no new commit).
    ///
    /// Folder workspaces install schemas by writing under
    /// `<workspace>/.memstead/schemas/` directly; this is the git-branch
    /// path, where the engine owns the vault-repo and the write must
    /// route through it. Errors when no git-branch ops are wired (basis
    /// flavour) or no git-branch mount exists to resolve the shared
    /// vault-repo gitdir from. The caller reloads (or restarts) to pick
    /// the new schema into the resolution catalogue.
    pub fn install_schema(
        &self,
        name: &str,
        version: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<String, EngineError> {
        // Resolve the shared vault-repo gitdir: prefer a live git-branch
        // mount's gitdir (authoritative — that is where the engine reads
        // schemas from), falling back to the workspace's `vault-repo/.git`
        // so a schema can be installed into an empty vault-repo *before*
        // any vault pins it.
        let gitdir = self
            .mounts
            .iter()
            .find_map(|m| match &m.mount.storage {
                crate::workspace::MountStorage::GitBranch { gitdir, .. } => Some(gitdir.clone()),
                _ => None,
            })
            .or_else(|| self.workspace_root().map(|r| r.join("vault-repo").join(".git")))
            .ok_or_else(|| {
                EngineError::Vault(
                    "schema install requires a vault-repo workspace (no git-branch mount and \
                     no workspace root to resolve the vault-repo gitdir)"
                        .to_string(),
                )
            })?;
        let ops = self.git_branch_ops.as_ref().ok_or_else(|| {
            EngineError::Vault("git-branch ops are not wired on this engine".to_string())
        })?;
        (ops.write_schema)(&gitdir, name, version, files).map_err(EngineError::Backend)
    }
    /// Unregister a writable vault at runtime. Engine-level
    /// primitive that `memstead_vault_delete` builds on.
    ///
    /// Removes the named mount from [`Self::mounts`], drops the
    /// vault's entities from the store, refreshes the
    /// [`VaultRouterSnapshot`] via `Arc::make_mut` (COW swap so
    /// readers holding a pre-swap snapshot see the pre-state for
    /// their lifetime), and invalidates the community + search
    /// memos. Does NOT touch the backend's on-disk state — the
    /// caller (`delete_vault` orchestrator) decides whether to
    /// remove the directory / gitdir after this returns.
    ///
    /// Returns `Ok(Some(backend))` when the vault was present and
    /// unregistered — the caller can drive any backend-specific
    /// follow-up cleanup (`backend.delete_artifacts()` for the
    /// vault-repo branch + `__MEMSTEAD` config when `delete_files=true`).
    /// Returns `Ok(None)` when no mount named the vault (idempotent —
    /// repeated calls are safe).
    pub fn unregister_writable_vault(
        &mut self,
        vault_name: &str,
    ) -> Result<Option<Box<dyn VaultBackend>>, EngineError> {
        let pos = self
            .mounts
            .iter()
            .position(|m| m.mount.vault == vault_name);
        let Some(idx) = pos else {
            return Ok(None);
        };

        // Drop the mount first — releases all engine-side state that
        // referenced the backend. The `Box<dyn VaultBackend>` itself
        // travels back to the caller so backend-side cleanup
        // (`delete_artifacts`) can run after the engine snapshot
        // settled.
        let mount = self.mounts.remove(idx);

        // Drop the schema entry for this vault (kept in lockstep
        // with `self.mounts`).
        self.schemas.remove(&mount.mount.vault);

        // Drop entities. The store's vault index is the
        // authoritative count; the return value (number of
        // entities removed) is informational only — the caller
        // already knows the vault and doesn't need the count.
        let _removed = self.store.remove_entities_by_vault(vault_name);

        // COW snapshot swap on the vault_router. `Arc::make_mut`
        // clones the inner snapshot when other Arcs exist; if this
        // is the only handle (typical for the engine's lifetime),
        // it returns the existing inner directly without cloning.
        // Readers that captured an `Arc` before this call observe
        // the pre-swap state — the in-flight handler's
        // `vault_router()` borrow is unaffected by this mutation.
        Arc::make_mut(&mut self.vault_router).remove_writable(vault_name);

        // Invalidate dependent memos — community detection + search
        // indexes were computed over the pre-removal store and are
        // now stale. Mutation paths already invalidate; this
        // matches the contract.
        self.invalidate_communities();
        self.invalidate_search_indexes();

        Ok(Some(mount.backend))
    }

    /// Register a writable vault at runtime. Engine-level primitive
    /// that `memstead_vault_create` builds on.
    ///
    /// Steps:
    /// 1. Name collision probe against the current `vault_router`
    ///    snapshot. Writable AND read-only entries collide; the
    ///    error surfaces the colliding source so the orchestrator
    ///    can render a recovery hint.
    /// 2. Schema resolution via the built-in catalogue (mirrors
    ///    [`Self::from_mounts`]; workspace-authored schema
    ///    resolution lifts later).
    /// 3. Per-vault config load (folder backends only; git-branch /
    ///    archive return None — same contract as
    ///    [`Self::from_mounts`]).
    /// 4. Entity load via the backend, parse, push into the engine's
    ///    store with a `LoadCollector` so drift warnings forward to
    ///    `self.load_warnings`.
    /// 5. Insert schema into [`Self::schemas`].
    /// 6. Push the [`MountedBackend`] into [`Self::mounts`].
    /// 7. COW snapshot swap on [`Self::vault_router`] via
    ///    `Arc::make_mut` + `add_writable(name, dir, origin, vault_path)`.
    ///    Folder mounts surface their on-disk path; other backends
    ///    register with `dir: None` (matches pro's contract).
    ///    `vault_path` carries the create-time organisational `path`
    ///    component (mirrors `VaultCreateParams.path`) — the
    ///    delete-side lifecycle composer reads it back to rebuild the
    ///    `<vault_path>/<name>` candidate the create-side composer
    ///    matched against. Caller threads `None` for flat-layout
    ///    registrations and `Some(p)` for hierarchical ones.
    /// 8. Invalidate community + search memos.
    ///
    /// Returns `Err(EngineError::VaultNameCollision)` when the name
    /// is already registered. Other failures (schema-not-found,
    /// backend read errors) propagate as their typed variants. On
    /// failure no engine mutation happens: every potentially-
    /// mutating step runs only after the collision probe succeeds,
    /// and intermediate failures propagate before the mount /
    /// router are touched.
    pub fn register_writable_vault(
        &mut self,
        mount: Mount,
        backend: Box<dyn VaultBackend>,
        origin: VaultOrigin,
    ) -> Result<(), EngineError> {
        // Step 1: name collision probe.
        if let Some(existing) = self.vault_router.origin_for_vault(&mount.vault) {
            return Err(EngineError::VaultNameCollision {
                name: mount.vault.clone(),
                source_origin: existing.render_source(),
            });
        }
        if self
            .vault_router
            .archive_path_for_vault(&mount.vault)
            .is_some()
        {
            return Err(EngineError::VaultNameCollision {
                name: mount.vault.clone(),
                source_origin: "attached read vault".to_string(),
            });
        }

        // Step 2: per-vault config load via the backend trait. Read
        // before resolving the schema — the vault's own config carries
        // the authoritative pin (mirrors the boot path), so a vault
        // re-registered or mounted from another machine resolves from
        // its own backend, not this workspace's mount expectation.
        let vault_config = backend
            .read_vault_config()
            .ok()
            .flatten()
            .and_then(|bytes| {
                let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
                memstead_schema::config::parse_vault_config(&value).ok()
            });

        // Step 3: schema resolution. `VaultConfig.schema` is the
        // authoritative settled pin; `Mount.schema` is the fallback when
        // the config carries none, and an expectation assertion when it
        // does — a disagreement surfaces `SchemaPinMismatch` (config
        // wins, neither silently dropped). Mirrors `from_mounts_inner`.
        // Resolve against the engine's full loaded catalogue (already-
        // loaded workspace/local-storage schemas layered over built-ins)
        // so a vault registered against a backend-installed (e.g.
        // git-branch `__MEMSTEAD:schemas/` ref) schema resolves.
        let mut builtin_schemas: Vec<std::sync::Arc<memstead_schema::Schema>> =
            self.workspace_schemas().to_vec();
        builtin_schemas.extend(
            memstead_schema::builtins::load_builtin_schemas()
                .map_err(|e| EngineError::SchemaResolverInit(e.to_string()))?,
        );
        let config_pin = vault_config.as_ref().and_then(|c| c.schema.as_ref());
        let mount_pin = mount.schema.as_ref();
        if let (Some(cfg), Some(mp)) = (config_pin, mount_pin)
            && cfg != mp
        {
            self.load_warnings
                .push(crate::ops::WarningHint::SchemaPinMismatch {
                    vault: mount.vault.clone(),
                    config_pin: cfg.as_display(),
                    mount_pin: mp.as_display(),
                });
        }
        let settled_pin = config_pin.or(mount_pin);
        let effective_pin = mount
            .migration_target
            .as_ref()
            .or(settled_pin)
            .ok_or_else(|| EngineError::VaultConfigIncomplete {
                vault: mount.vault.clone(),
                missing_fields: vec!["schema".to_string()],
            })?
            .clone();
        let schema = crate::engine::SchemaResolver::new(&builtin_schemas)
            .resolve(&effective_pin)
            .map_err(|sources| EngineError::SchemaNotFound {
                vault: mount.vault.clone(),
                pin: effective_pin.as_display(),
                sources,
            })?;

        // Step 4: load entities via the backend, push into the
        // engine's store with a LoadCollector so drift warnings
        // forward into `self.load_warnings`. Derive the vault
        // roster + last-segment suffixes from the POST-registration
        // view (new vault included) so cross-vault references
        // targeting the new vault resolve correctly during this
        // load.
        let (entries, read_errors) = collect_source_entries(backend.as_ref())?;
        let load_result = parse_entries(entries, read_errors, &mount.vault, schema.as_ref());

        let mut vault_names: Vec<String> = self
            .mounts
            .iter()
            .map(|m| m.mount.vault.clone())
            .collect();
        vault_names.push(mount.vault.clone());
        let known_suffixes: Vec<String> = vault_names
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
                vault_names: &vault_names,
            }),
        );
        self.load_errors.extend(load_result.errors);

        // Step 5: insert schema (kept in lockstep with `self.mounts`).
        self.schemas.insert(mount.vault.clone(), schema);

        // Re-run the parse-time relation validator now that the new
        // vault's schema is in `self.schemas`. Mirrors the boot path
        // (`Engine::from_mounts_inner`) — hand-edited or externally-
        // generated markdown in the newly-attached mount goes through
        // the same gauntlet (grammar / unknown_rel_type / shape /
        // cycle) and offending relations are dropped with typed
        // `PARSED_RELATION_INVALID` warnings on `self.load_warnings`.
        // The newly-pushed mount isn't in `self.mounts` yet (that's
        // Step 6 below), so build the map from `self.mounts` plus the
        // about-to-be-attached mount we're still holding.
        let mut mount_caps: std::collections::HashMap<String, crate::workspace::MountCapability> =
            self.mounts
                .iter()
                .map(|m| (m.mount.vault.clone(), m.mount.capability))
                .collect();
        mount_caps.insert(mount.vault.clone(), mount.capability);
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

        // Step 6: push the MountedBackend.
        let last_known_head = backend.current_head().ok().flatten();
        let vault_name_for_router = mount.vault.clone();
        let storage_for_router = mount.storage.clone();
        self.mounts.push(MountedBackend {
            mount,
            backend,
            last_known_head,
            vault_config,
            // A runtime-created vault is authored live, not installed from
            // an archive — it carries no archive-borne provenance payload.
            archive_provenance: None,
        });

        // Step 7: COW snapshot swap on vault_router. Folder mounts
        // surface their on-disk path; other backends register with
        // `dir: None` (vault-repo-backed mounts have no working tree).
        let dir: Option<PathBuf> = match &storage_for_router {
            MountStorage::Folder { path } => Some(path.clone()),
            MountStorage::GitBranch { .. }
            | MountStorage::Archive { .. }
            | MountStorage::InMemory => None,
        };
        Arc::make_mut(&mut self.vault_router).add_writable(
            vault_name_for_router,
            dir,
            origin,
        );

        // Step 8: invalidate dependent memos.
        self.invalidate_communities();
        self.invalidate_search_indexes();

        Ok(())
    }

    /// Override the workspace root after construction. The pro
    /// boot helper `memstead_git_branch::engine_from_workspace_root`
    /// calls this so the engine knows the path even when the boot
    /// route runs through the pro adapter rather than
    /// [`Self::from_workspace_root`].
    pub fn set_workspace_root(&mut self, root: PathBuf) {
        self.workspace_root = Some(root);
    }

    /// Persist the engine's current mount list to the workspace
    /// store so a freshly-booted sibling process observes the same
    /// vault membership. Called by
    /// [`crate::vault_management::create_vault`] /
    /// [`crate::vault_management::delete_vault`] after the in-memory
    /// router mutation lands — without this, the per-vault content
    /// (branch + `__MEMSTEAD` config blob, or folder + `.memstead/config.json`)
    /// is already on disk, but the next process boot reads an empty
    /// `.memstead/state/mounts.json` and the engine starts with zero
    /// writable vaults.
    ///
    /// No-op when `workspace_root` is unset (tests / ad-hoc
    /// consumers that build the engine directly from a mount list).
    /// Production boot paths (`Engine::from_workspace_root` and the
    /// pro counterpart) always set the root, so the engine-side
    /// fix covers every caller — including the future UniFFI binding
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
        let workspace = crate::workspace::Workspace {
            mounts: self.mounts.iter().map(|m| m.mount.clone()).collect(),
            settings: self.settings.clone(),
        };
        let store = crate::FileWorkspaceStore::new();
        crate::workspace_store::WorkspaceStoreAdapter::save_state(&store, root, &workspace)
            .map_err(|e| EngineError::Vault(format!("persist workspace state: {e}")))
    }
    /// Set a vault's schema pin — the conformance-gated schema-migration
    /// trigger. Behaviour per the pinned contract:
    ///
    /// - requested == current pin → `Noop`, no state change.
    /// - requested != pin, vault integral against the target →
    ///   atomic switch (`schema_pin = target`, migration state
    ///   cleared) in one workspace-store write → `Switched`.
    /// - requested != pin, vault NOT integral → enter (or stay in)
    ///   dual-pin: `migration_target = target`, writes validate
    ///   against the target from this call on, `findings` carries
    ///   the non-integral entities → `MigrationStarted`
    ///   (first call) / `MigrationPending` (same target re-issued).
    /// - re-issued with the in-flight target once every entity is
    ///   integral → atomic switch → `Switched`.
    ///
    /// The trigger is a label change gated by the conformance check —
    /// no content hashing. The response hands the agent findings and
    /// nothing else (no migration scripts, no hints); each repair
    /// write is validated strictly against the target.
    pub fn set_vault_schema(
        &mut self,
        vault: &str,
        target: &memstead_schema::SchemaRef,
    ) -> Result<crate::engine::SetSchemaOutcome, EngineError> {
        use crate::engine::{SetSchemaOutcome, SetSchemaResult};
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.vault == vault)
            .ok_or_else(|| EngineError::UnknownVault(vault.to_string()))?;

        // The requested target must resolve before anything else —
        // an unknown ref is an error, not a migration into nowhere.
        let target_schema = self.resolve_schema_by_ref(target).ok_or_else(|| {
            // The migration resolver consulted workspace-authored
            // schemas layered over the built-ins (`resolve_schema_by_ref`).
            let consulted: Vec<_> = self
                .workspace_schemas
                .iter()
                .chain(self.builtin_schemas.iter())
                .cloned()
                .collect();
            EngineError::SchemaNotFound {
                vault: vault.to_string(),
                pin: target.as_display(),
                sources: crate::engine::error::SchemaSourceDiagnostic::for_failed_pin(
                    &target.name,
                    &target.version,
                    &consulted,
                ),
            }
        })?;

        // `Mount.schema` is now the optional assertion; for a vault the
        // operator is actively re-pinning it is normally `Some` (and kept
        // in sync with the config by the switch below). `<unset>` covers a
        // mount that carried no assertion.
        let current_pin = self.mounts[mount_idx].mount.schema.clone();
        let current_pin_display = current_pin
            .as_ref()
            .map(|p| p.as_display())
            .unwrap_or_else(|| "<unset>".to_string());
        let in_flight = self.mounts[mount_idx].mount.migration_target.clone();

        if current_pin.as_ref() == Some(target) {
            return Ok(SetSchemaOutcome {
                vault: vault.to_string(),
                schema_pin: current_pin_display,
                migration_target: in_flight.map(|t| t.as_display()),
                outcome: SetSchemaResult::Noop,
                findings: Vec::new(),
            });
        }

        // Conformance gate against the requested target. The full
        // integrity definition includes the consistency axis, but the
        // schema-switch gate is conformance: consistency breaks are
        // schema-independent (they neither block nor are caused by a
        // pin change) and keep their always-available repair paths.
        let findings = crate::ops::integrity::conformance_findings(
            &self.store,
            vault,
            target_schema.as_ref(),
            &self.schemas,
        );

        if findings.is_empty() {
            // Atomic switch. The pin's authoritative home is the backend
            // config (boot resolution prefers it over `Mount.schema`), so
            // persist there FIRST — if that write fails, every other piece
            // of state stays untouched and the switch is a clean no-op.
            // Without this the new pin landed only in `mounts.json` and was
            // silently reverted on the next process boot for any
            // config-present vault.
            self.persist_vault_schema_pin(mount_idx, target)?;
            self.mounts[mount_idx].mount.schema = Some(target.clone());
            self.mounts[mount_idx].mount.migration_target = None;
            self.schemas.insert(vault.to_string(), target_schema);
            self.invalidate_communities();
            self.persist_state()?;
            return Ok(SetSchemaOutcome {
                vault: vault.to_string(),
                schema_pin: target.as_display(),
                migration_target: None,
                outcome: SetSchemaResult::Switched,
                findings: Vec::new(),
            });
        }

        let outcome = if in_flight.as_ref() == Some(target) {
            SetSchemaResult::MigrationPending
        } else {
            SetSchemaResult::MigrationStarted
        };
        self.mounts[mount_idx].mount.migration_target = Some(target.clone());
        // Writes validate against the target from this point on —
        // the load-bearing dual-pin semantic.
        self.schemas.insert(vault.to_string(), target_schema);
        self.invalidate_communities();
        self.persist_state()?;
        Ok(SetSchemaOutcome {
            vault: vault.to_string(),
            schema_pin: current_pin_display,
            migration_target: Some(target.as_display()),
            outcome,
            findings,
        })
    }

    /// Persist a vault's new schema pin into the authoritative backend
    /// config (`.memstead/config.json` for folder, the `__MEMSTEAD`
    /// vault-config blob for git-branch).
    ///
    /// Boot resolution treats the backend config as the authoritative
    /// settled pin and `Mount.schema` (the `mounts.json` copy) as a
    /// cross-checked assertion. A schema switch that updated only
    /// `mounts.json` would therefore be silently reverted on the next
    /// process boot — the config still names the old pin. This keeps the
    /// authoritative home in sync at switch time.
    ///
    /// Value-level field bump: only the `"schema"` string is rewritten;
    /// every other config field (`readVaults`, write guidance, …) is
    /// preserved verbatim. Config-absent vaults (no `config.json`) keep
    /// `Mount.schema` as their settled pin, so there is nothing to update
    /// — a clean no-op.
    fn persist_vault_schema_pin(
        &mut self,
        mount_idx: usize,
        target: &memstead_schema::SchemaRef,
    ) -> Result<(), EngineError> {
        let Some(bytes) = self.mounts[mount_idx]
            .backend
            .read_vault_config()
            .map_err(|e| EngineError::Vault(format!("read vault config for pin update: {e}")))?
        else {
            return Ok(());
        };
        let mut value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| EngineError::Vault(format!("parse vault config for pin update: {e}")))?;
        value["schema"] = serde_json::Value::String(target.as_display());
        let new_bytes = serde_json::to_vec_pretty(&value).map_err(|e| {
            EngineError::Vault(format!("serialize vault config for pin update: {e}"))
        })?;
        self.mounts[mount_idx]
            .backend
            .write_vault_config(&new_bytes)
            .map_err(|e| EngineError::Vault(format!("write vault config for pin update: {e}")))?;
        // Refresh the cached parsed config so in-session reads observe the
        // new pin without a reload.
        if let Ok(cfg) = memstead_schema::config::parse_vault_config(&value) {
            self.mounts[mount_idx].vault_config = Some(cfg);
        }
        Ok(())
    }

    /// Regenerate entity markdown files from the in-memory store.
    ///
    /// Dispatch:
    /// - When `vault_filter` is `Some(name)`, only that vault's mount
    ///   is considered. If its active backend doesn't support markdown
    ///   regeneration in place (today: anything other than
    ///   `MountStorage::Folder`), the call refuses with
    ///   [`EngineError::MarkdownExportUnsupportedBackend`] carrying
    ///   the active backend's id and the supported-backend list.
    /// - When `vault_filter` is `None`, every mount is iterated.
    ///   Folder mounts regenerate as today; non-folder mounts are
    ///   recorded in [`crate::ops::ExportResult::skipped_mounts`] so
    ///   the caller can surface the partial-success shape.
    ///
    /// Per-folder-mount behaviour: iterate the store, regenerate each
    /// non-stub entity belonging to the mount's vault, compare to the
    /// on-disk file, write if changed.
    ///
    /// `schema_filter` narrows the per-entity-type subset: when
    /// `Some(name)`, only entities whose `entity_type` matches are
    /// regenerated. `None` exports every type.
    ///
    /// Pre-fix this returned
    /// `ExportResult { written: 0, unchanged: 0 }` for git-branch /
    /// archive mounts — a successful-looking no-op that masked the
    /// backend-incompatibility. The typed refusal (per-vault) and the
    /// `skipped_mounts` channel (workspace-wide) give the caller an
    /// agent-actionable signal in one round-trip.
    pub fn export_markdown(
        &self,
        vault_filter: Option<&str>,
        schema_filter: Option<&str>,
    ) -> Result<crate::ops::ExportResult, EngineError> {
        use crate::workspace::MountStorage;
        let fallback = engine_fallback_type();
        let supported_backends = vec!["folder".to_string()];

        if let Some(name) = vault_filter {
            let mount = self
                .mounts
                .iter()
                .find(|m| m.mount.vault == name)
                .ok_or_else(|| EngineError::UnknownVault(name.to_string()))?;
            if !matches!(mount.mount.storage, MountStorage::Folder { .. }) {
                return Err(EngineError::MarkdownExportUnsupportedBackend {
                    vault: name.to_string(),
                    active_backend: mount.mount.storage.backend_id().to_string(),
                    supported_backends,
                });
            }
        }

        let mut total_written = 0;
        let mut total_unchanged = 0;
        let mut skipped_mounts: Vec<crate::ops::SkippedMount> = Vec::new();

        for mount in &self.mounts {
            let vault_name = mount.mount.vault.as_str();
            if let Some(filter) = vault_filter
                && vault_name != filter
            {
                continue;
            }
            let MountStorage::Folder { path: vault_dir } = &mount.mount.storage else {
                skipped_mounts.push(crate::ops::SkippedMount {
                    vault: vault_name.to_string(),
                    active_backend: mount.mount.storage.backend_id().to_string(),
                    reason: "backend_does_not_support_markdown_export".to_string(),
                });
                continue;
            };
            let schema = match self.schemas.get(vault_name) {
                Some(s) => s,
                None => continue,
            };

            for entity in self.store.all_entities() {
                if entity.stub || entity.file_path.is_empty() {
                    continue;
                }
                if entity.id.vault() != vault_name {
                    continue;
                }
                if let Some(filter) = schema_filter
                    && entity.entity_type != filter
                {
                    continue;
                }
                let type_def = schema
                    .get_type(&entity.entity_type)
                    .unwrap_or_else(|| fallback.clone());
                let generated = generate_markdown(entity, type_def.as_ref());

                let full_path = vault_dir.join(&entity.file_path);
                let needs_write = match std::fs::read_to_string(&full_path) {
                    Ok(existing) => existing != generated,
                    Err(_) => true,
                };
                if needs_write {
                    let _ = crate::entity::writer::write_entity(entity, vault_dir, type_def.as_ref());
                    total_written += 1;
                } else {
                    total_unchanged += 1;
                }
            }
        }

        Ok(crate::ops::ExportResult {
            written: total_written,
            unchanged: total_unchanged,
            skipped_mounts,
        })
    }

    /// Export a vault as a portable `.mem` archive.
    ///
    /// Dispatch is internal: the engine looks up the mount whose vault
    /// name matches and branches on its `MountStorage`. Folder mounts
    /// produce a snapshot archive (current `.md` files + config);
    /// git-branch mounts invoke the registered [`GitBranchOps::export`]
    /// hook to produce a history archive (the per-vault branch tip's
    /// tree); archive mounts reject with `BackendError::Sealed`
    /// (already-an-archive — no meaningful re-export).
    ///
    /// The vault's `VaultConfig` is looked up via
    /// [`Self::vault_config_for`]; unloaded configs (folder mounts
    /// without a `.memstead/config.json`, git-branch mounts without a
    /// `__MEMSTEAD:vaults/<vault>/config.json`) surface as
    /// `EngineError::InvalidInput`. Workspace-level schema dir is
    /// threaded from `self.settings.schemas_dir` for the
    /// schema-source resolution chain.
    pub fn export_vault(
        &self,
        vault_name: &str,
        output_path: &std::path::Path,
    ) -> Result<crate::ops::VaultExportResult, EngineError> {
        let mount = self
            .mounts
            .iter()
            .find(|m| m.mount.vault == vault_name)
            .ok_or_else(|| EngineError::UnknownVault(vault_name.to_string()))?;
        let config = self.vault_config_for(vault_name).ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "vault '{vault_name}' has no loaded VaultConfig — cannot export"
            ))
        })?;
        // F1: surface the missing-version case as a typed
        // `VAULT_CONFIG_INCOMPLETE` envelope with structured recovery
        // details, rather than letting it bubble through as the
        // backend's `INTERNAL` collapse pointing at the wrong path
        // (`.memstead/config.json` is the folder-backend layout — the
        // vault-repo backend keeps the blob under `__MEMSTEAD:vaults/`).
        // The check fires for both backends symmetrically.
        if config.version.is_none() {
            return Err(EngineError::VaultConfigIncomplete {
                vault: vault_name.to_string(),
                missing_fields: vec!["version".to_string()],
            });
        }
        let workspace_root = self.workspace_root.as_deref();
        // Authored schemas live at the fixed `<workspace>/.memstead/schemas/`
        // location (the `schemas_dir` key is retired). Absent dir → the
        // schema-source chain falls through to cache/built-in, as before.
        let fixed_schemas_dir =
            workspace_root.map(|r| r.join(".memstead").join("schemas"));
        let workspace_schemas_dir = fixed_schemas_dir.as_deref();
        match &mount.mount.storage {
            MountStorage::Folder { path } => crate::ops::export::export_vault(
                path,
                config,
                output_path,
                workspace_root,
                workspace_schemas_dir,
            )
            .map_err(|e| {
                EngineError::Backend(BackendError::Other(format!("export_vault: {e}")))
            }),
            MountStorage::GitBranch { gitdir, branch } => {
                let hook = self.git_branch_ops.as_ref().ok_or_else(|| {
                    EngineError::Backend(BackendError::Other(
                        "git-branch export hook not installed (pro flavour not loaded)"
                            .to_string(),
                    ))
                })?;
                // Source per-entity provenance from the git-branch mutation
                // log (commit trailers) and hand the serialised payload to
                // the export hook to embed — symmetric with the bytes path.
                let provenance_bytes = mount
                    .backend
                    .read_provenance(None)
                    .ok()
                    .and_then(|records| crate::ops::export::build_archive_provenance(&records))
                    .and_then(|prov| prov.to_archive_bytes().ok());
                (hook.export)(
                    gitdir,
                    branch,
                    vault_name,
                    config,
                    output_path,
                    workspace_root,
                    workspace_schemas_dir,
                    provenance_bytes.as_deref(),
                )
                .map_err(EngineError::Backend)
            }
            MountStorage::Archive { .. } => {
                Err(EngineError::Backend(BackendError::Sealed))
            }
            // `.mem` export from an in-memory vault lands with the
            // writable-session-server plan (it needs a backend-level
            // archive builder); this plan adds the backend, not the
            // export path, so refuse explicitly rather than silently.
            MountStorage::InMemory => Err(EngineError::Backend(BackendError::Other(
                "export not yet supported for in-memory backend".to_string(),
            ))),
        }
    }

    /// Update a vault's `version` field in its per-vault config and
    /// persist it through the backend. Backend-symmetric: folder
    /// backends rewrite `.memstead/config.json`; git-branch backends
    /// commit `__MEMSTEAD:vaults/<vault>/config.json`. Archive mounts
    /// reject with `BackendError::Sealed`.
    ///
    /// Returns the (vault, old_version, new_version) triple so
    /// callers can surface the change without an extra read. Reads
    /// the current value from the in-memory `VaultConfig` and
    /// updates it on success, keeping the next call free of a
    /// stale-version read.
    ///
    /// `EngineError::UnknownVault` when the name resolves to no
    /// mount; `EngineError::ReadOnlyMount` when the mount is sealed
    /// for writes; `EngineError::InvalidInput` when the mount has no
    /// loaded `VaultConfig` (folder mount with no
    /// `.memstead/config.json`; the residual missing-config path is
    /// distinct from the missing-version path). F1.
    pub fn set_vault_version(
        &mut self,
        vault_name: &str,
        new_version: semver::Version,
        note: Option<&str>,
    ) -> Result<crate::ops::SetVaultVersionOutcome, EngineError> {
        // Resolve the mount up-front so an unknown-vault name refuses
        // before any drift-probe side effect lands.
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.vault == vault_name)
            .ok_or_else(|| EngineError::UnknownVault(vault_name.to_string()))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(vault_name.to_string()));
        }

        // Probe for concurrent-drift before the write — a sibling
        // engine that committed between our last snapshot and now
        // surfaces `VAULT_RELOADED` on the response so callers see
        // the drift without a separate read round-trip. Drift
        // warnings ride alongside the success outcome; an
        // unreachable-backend probe collapses to no warnings (the
        // existing accessor warn-logs internally and skips).
        let mut warnings = self.reload_if_stale(Some(vault_name));
        // Provenance nudge — same posture as every other commit-
        // producing mutation: when `require_notes` is set and no note
        // was supplied, ride a non-blocking `NOTE_MISSING` warning.
        // The version bump still commits.
        if let Some(w) = self.note_missing_warning("set_vault_version", note) {
            warnings.push(w);
        }

        let mounted = &mut self.mounts[mount_idx];
        let mut config = mounted
            .vault_config
            .clone()
            .ok_or_else(|| {
                EngineError::InvalidInput(format!(
                    "vault '{vault_name}' has no loaded VaultConfig — \
                     cannot set version (initialize the vault via `memstead init` \
                     or `memstead vault create` first)"
                ))
            })?;
        let old_version = config.version.clone();
        config.version = Some(new_version.clone());

        let mut bytes = serde_json::to_vec_pretty(&config).map_err(|e| {
            EngineError::InvalidInput(format!(
                "could not serialize vault config: {e}"
            ))
        })?;
        bytes.push(b'\n');
        mounted.backend.write_vault_config_with_note(&bytes, note)?;
        mounted.vault_config = Some(config);

        // Refresh the head cursor so the next drift probe doesn't
        // surface VAULT_RELOADED for the commit we just produced
        // (the git-branch backend's `write_vault_config` writes a
        // commit on `__MEMSTEAD`; folder backends carry no head and the
        // refresh is a no-op).
        let new_head = mounted.backend.current_head().ok().flatten();
        if let Some(sha) = new_head {
            mounted.last_known_head = Some(sha);
        }

        Ok(crate::ops::SetVaultVersionOutcome {
            vault: vault_name.to_string(),
            old_version,
            new_version,
            warnings,
        })
    }

    /// Set (or clear) one opaque sync-state token in a vault's per-vault
    /// config and persist it through the backend. The ingest layer calls
    /// this after a successful pass over a source's changed slice to
    /// record "the source state the graph was last synced against".
    ///
    /// `key` and `token` are both opaque to the engine: the key is
    /// conventionally `"<ingest>/<facet>"` but the engine treats it as an
    /// arbitrary string; the token's meaning belongs to the medium-type
    /// layer (git → commit id, graph → snapshot token, filesystem → a
    /// JSON-stringified stat digest). The engine never parses either.
    /// An **empty** `token` removes the key — the surface for clearing a
    /// baseline (which the next ingest pass re-seeds at the current
    /// source state).
    ///
    /// Backend-symmetric like [`Self::set_vault_version`]: folder backends
    /// rewrite `.memstead/config.json`; git-branch backends commit
    /// `__MEMSTEAD:vaults/<vault>/config.json`. Archive mounts reject with
    /// `BackendError::Sealed`.
    ///
    /// Returns the (vault, key, previous-token) triple so callers can
    /// surface the change without an extra read. `EngineError::UnknownVault`
    /// when the name resolves to no mount; `EngineError::ReadOnlyMount`
    /// when the mount is sealed for writes; `EngineError::InvalidInput`
    /// when the mount has no loaded `VaultConfig`.
    pub fn set_vault_sync_state(
        &mut self,
        vault_name: &str,
        key: &str,
        token: &str,
        note: Option<&str>,
    ) -> Result<crate::ops::SetVaultSyncStateOutcome, EngineError> {
        // Resolve the mount up-front so an unknown-vault name refuses
        // before any drift-probe side effect lands.
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.vault == vault_name)
            .ok_or_else(|| EngineError::UnknownVault(vault_name.to_string()))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(vault_name.to_string()));
        }

        // Probe for concurrent-drift before the write — same posture as
        // every other commit-producing mutation; a sibling engine that
        // committed since our last snapshot surfaces `VAULT_RELOADED`.
        let mut warnings = self.reload_if_stale(Some(vault_name));
        if let Some(w) = self.note_missing_warning("set_vault_sync_state", note) {
            warnings.push(w);
        }

        let mounted = &mut self.mounts[mount_idx];
        let mut config = mounted.vault_config.clone().ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "vault '{vault_name}' has no loaded VaultConfig — \
                 cannot set sync state (initialize the vault via `memstead init` \
                 or `memstead vault create` first)"
            ))
        })?;

        // Empty token clears the baseline; otherwise insert/overwrite.
        // `removed` distinguishes a no-op clear (key absent) from a real
        // one so the outcome is honest.
        let removed;
        let previous;
        if token.is_empty() {
            previous = config.sync_state.remove(key);
            removed = previous.is_some();
        } else {
            previous = config
                .sync_state
                .insert(key.to_string(), token.to_string());
            removed = false;
        }

        let mut bytes = serde_json::to_vec_pretty(&config).map_err(|e| {
            EngineError::InvalidInput(format!("could not serialize vault config: {e}"))
        })?;
        bytes.push(b'\n');
        mounted.backend.write_vault_config_with_note(&bytes, note)?;
        mounted.vault_config = Some(config);

        // Refresh the head cursor so the next drift probe doesn't surface
        // VAULT_RELOADED for the commit we just produced.
        let new_head = mounted.backend.current_head().ok().flatten();
        if let Some(sha) = new_head {
            mounted.last_known_head = Some(sha);
        }

        Ok(crate::ops::SetVaultSyncStateOutcome {
            vault: vault_name.to_string(),
            key: key.to_string(),
            previous,
            removed,
            warnings,
        })
    }

    /// Re-read the named mount's backend entities and refresh the
    /// in-memory store for that vault. Returns the diff against the
    /// pre-reload snapshot — `added` (ids newly present), `removed`
    /// (ids no longer present), `changed` (same id, different
    /// `content_hash`).
    ///
    /// Operator-triggered: useful when an external writer modified
    /// disk while this engine instance was alive (the basis flavour
    /// assumes single-writer; this primitive is the escape hatch when
    /// that assumption breaks). On the happy path the diff is empty.
    ///
    /// Drift detection (whether disk *did* change) is not part of this
    /// surface — callers that want to short-circuit on "nothing
    /// changed" must compare `added.is_empty() && changed.is_empty()
    /// && removed.is_empty()` against the result. Backend-specific
    /// drift signals (git HEAD comparison, mtime check) live in the
    /// pro-flavour engine where they have meaning.
    ///
    /// Invalidates community + search-index memos on success.
    pub fn reload_one_vault(
        &mut self,
        vault: &str,
    ) -> Result<crate::ops::ReloadResult, EngineError> {
        // Per-vault reload is intentionally silent on the engine-
        // wide `load_warnings` accumulator — matches pro's
        // `reload_one_vault`. A LOCAL sink absorbs any warnings
        // the parser emits during this reload and is discarded.
        // Drift events still surface as `VaultReloaded` warnings
        // via `reload_if_stale`.
        let mut sink: Vec<WarningHint> = Vec::new();
        self.reload_one_vault_inner(vault, &mut sink)
    }

    /// Inner per-vault body shared by [`Self::reload_one_vault`]
    /// and [`Self::reload_each_writable_vault`]. The caller passes
    /// a warning sink so the workspace-wide reload can forward
    /// warnings into `self.load_warnings` while the single-vault
    /// path keeps the accumulator pristine.
    fn reload_one_vault_inner(
        &mut self,
        vault: &str,
        warnings_sink: &mut Vec<WarningHint>,
    ) -> Result<crate::ops::ReloadResult, EngineError> {
        // Locate the target mount + schema. Unknown vault short-
        // circuits before any store mutation.
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.vault == vault)
            .ok_or_else(|| EngineError::UnknownVault(vault.to_string()))?;
        let schema = self
            .schemas
            .get(vault)
            .cloned()
            .ok_or_else(|| EngineError::UnknownVault(vault.to_string()))?;

        // Snapshot pre-reload (id, content_hash) for this vault.
        let pre: HashMap<EntityId, String> = self
            .store
            .all_entities()
            .filter(|e| !e.stub && e.vault == vault)
            .map(|e| (e.id.clone(), e.content_hash.clone()))
            .collect();
        let pre_ids: std::collections::HashSet<EntityId> =
            pre.keys().cloned().collect();

        // Walk the backend; surface read-time errors instead of
        // mutating the store on a failed reload.
        let backend = self.mounts[mount_idx].backend.as_ref();
        let (entries, read_errors) = collect_source_entries(backend)?;
        let load_result = parse_entries(entries, read_errors, vault, schema.as_ref());

        // Build the LoadCollector inputs — vault roster + last-
        // segment suffixes — so the parser pipeline can emit
        // typed drift warnings into the caller's sink.
        let vault_names: Vec<String> = self
            .mounts
            .iter()
            .map(|m| m.mount.vault.clone())
            .collect();
        let known_suffixes: Vec<String> = vault_names
            .iter()
            .map(|n| crate::entity::store_builder::last_segment_suffix(n).to_string())
            .collect();

        // Failure fence above; below this point the store is mutated.
        self.store.remove_entities_by_vault(vault);
        let fallback = engine_fallback_type();
        push_entities_into_store(
            &mut self.store,
            load_result.entities,
            fallback.as_ref(),
            Some(crate::entity::store_builder::LoadCollector {
                warnings: warnings_sink,
                known_suffixes: &known_suffixes,
                vault_names: &vault_names,
            }),
        );
        // Re-run parse-time relation validation across the workspace.
        // A reload re-parses one vault but the validator's cycle pass
        // is global (acyclic-rel-type subgraphs span vaults), so the
        // scan runs against the whole store. Hand-edits arriving via
        // sibling-writer commits get the same gauntlet boot enforces
        // (grammar / unknown_rel_type / shape / cycle).
        let mount_caps: std::collections::HashMap<String, crate::workspace::MountCapability> =
            self.mounts
                .iter()
                .map(|m| (m.mount.vault.clone(), m.mount.capability))
                .collect();
        // Restore cross-vault edges that point INTO this vault. The
        // removal cascade above dropped their incoming mirrors and the
        // re-push only rebuilt edges authored by this vault's own
        // entities, so a cross-vault `A→B` would silently vanish from the
        // index until a workspace-wide reload. Reconstruct from the
        // authoritative source records (in-memory only — no other vault is
        // re-read), then let the remap pass below reclassify alias sources.
        crate::entity::store_builder::reconstruct_incoming_cross_vault_edges(
            &mut self.store,
            vault,
        );
        crate::entity::store_builder::validate_loaded_relations(
            &mut self.store,
            &self.schemas,
            &mount_caps,
            warnings_sink,
        );
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );
        // Surface load errors back through the engine's accumulator
        // so subsequent `load_errors()` calls reflect the latest read.
        // We don't clear pre-existing errors from other vaults — only
        // append; an external operator that wants a clean slate runs
        // a full re-init.
        self.load_errors.extend(load_result.errors);

        // Diff post-reload against the snapshot.
        let mut added: Vec<EntityId> = Vec::new();
        let mut changed: Vec<EntityId> = Vec::new();
        for entity in self.store.all_entities() {
            if entity.stub || entity.vault != vault {
                continue;
            }
            match pre.get(&entity.id) {
                None => added.push(entity.id.clone()),
                Some(prev_hash) if prev_hash != &entity.content_hash => {
                    changed.push(entity.id.clone());
                }
                Some(_) => {}
            }
        }
        let post_ids: std::collections::HashSet<EntityId> = self
            .store
            .all_entities()
            .filter(|e| !e.stub && e.vault == vault)
            .map(|e| e.id.clone())
            .collect();
        let mut removed: Vec<EntityId> = pre_ids.difference(&post_ids).cloned().collect();
        added.sort_by(|a, b| a.0.cmp(&b.0));
        changed.sort_by(|a, b| a.0.cmp(&b.0));
        removed.sort_by(|a, b| a.0.cmp(&b.0));

        self.invalidate_communities();
        self.invalidate_search_indexes();

        Ok(crate::ops::ReloadResult {
            added,
            changed,
            removed,
        })
    }

    /// Rich-shape variant of [`Self::reload_one_vault`] that returns
    /// a [`crate::ops::ReloadReport`] (vault + head_before + head_after
    /// + entities_loaded + changed_entity_ids) instead of the slim
    /// [`crate::ops::ReloadResult`]. Handler-facing wrapper consumed
    /// by the `memstead_reload` MCP tool — the rich shape is the wire
    /// contract MCP callers depend on; the slim form stays for
    /// programmatic consumers that just want the diff lists.
    ///
    /// `head_before` is the engine's **prior cursor** for this vault
    /// (its cached `last_known_head`), *not* the current on-disk tip:
    /// when a sibling has committed since, the tip has already advanced,
    /// so reporting it would make the advertised
    /// `changes_since(since=head_before)` recipe span an empty range.
    /// `head_after` is the freshly-peeled tip from
    /// [`crate::backend::VaultBackend::current_head`]; the reload also
    /// advances the cursor to it, so a follow-up staleness probe does
    /// not re-reload the same window. Backends without history (folder,
    /// archive) carry no cursor and return `Ok(None)`; both fields fall
    /// back to [`crate::ops::EMPTY_TREE_SHA`] for wire-shape stability.
    ///
    /// `entities_loaded` is the post-reload non-stub count for the
    /// vault — same semantic as pro's report.
    ///
    /// `changed_entity_ids` is the union of `added ∪ changed ∪
    /// removed` from the underlying [`crate::ops::ReloadResult`]
    /// so callers don't have to merge three lists themselves —
    /// matches pro's bundled wire shape.
    pub fn reload_one_vault_report(
        &mut self,
        vault: &str,
    ) -> Result<crate::ops::ReloadReport, EngineError> {
        // `head_before` is the engine's PRIOR cursor — the SHA it last
        // knew for this vault — not the current (possibly already
        // drifted) on-disk tip. Reporting the tip would collapse the
        // `changes_since(since=head_before)` range to empty in exactly
        // the sibling-drift case the recipe targets. Only history-backed
        // mounts (git-branch) carry a git cursor: folder / archive
        // backends have no `current_head`, so their `head_before` stays
        // the empty-tree sentinel that pairs with the equally-empty
        // `head_after` below.
        let tracks_head = self
            .mounts
            .iter()
            .find(|m| m.mount.vault == vault)
            .and_then(|m| m.backend.current_head().ok().flatten())
            .is_some();
        let head_before = if tracks_head {
            self.mounts
                .iter()
                .find(|m| m.mount.vault == vault)
                .and_then(|m| m.last_known_head.clone())
                .unwrap_or_else(|| crate::ops::EMPTY_TREE_SHA.to_string())
        } else {
            crate::ops::EMPTY_TREE_SHA.to_string()
        };

        let result = self.reload_one_vault(vault)?;

        // Capture head_after = the freshly-peeled tip, and advance the
        // engine's cursor to it. Without this advance the next
        // operation's `reload_if_stale` would compare the stale cursor
        // against the same tip and re-reload the identical window,
        // re-emitting a spurious `VAULT_RELOADED`. Only history-backed
        // mounts (current_head → Some) carry a cursor to advance.
        let head_after_raw = self
            .mounts
            .iter()
            .find(|m| m.mount.vault == vault)
            .and_then(|m| m.backend.current_head().ok().flatten());
        if let Some(new_head) = head_after_raw.clone()
            && let Some(m) = self.mounts.iter_mut().find(|m| m.mount.vault == vault)
        {
            m.last_known_head = Some(new_head);
        }
        let head_after = head_after_raw.unwrap_or_else(|| crate::ops::EMPTY_TREE_SHA.to_string());

        let entities_loaded = self
            .store
            .all_entities()
            .filter(|e| !e.stub && e.vault == vault)
            .count();

        // Union of added + changed + removed, sorted lexicographically
        // for deterministic wire output. Matches pro's "single
        // changed_entity_ids list" contract — saves callers from
        // merging three slices themselves.
        let mut changed_entity_ids: Vec<EntityId> = result
            .added
            .into_iter()
            .chain(result.changed.into_iter())
            .chain(result.removed.into_iter())
            .collect();
        changed_entity_ids.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(crate::ops::ReloadReport {
            vault: vault.to_string(),
            head_before,
            head_after,
            entities_loaded,
            changed_entity_ids,
        })
    }

    /// Batched rich-shape variant — returns one
    /// [`crate::ops::ReloadReport`] per mounted vault in declaration
    /// order. Counterpart to [`Self::reload_each_writable_vault`]
    /// (slim) that the `memstead_reload` MCP tool's no-vault path
    /// consumes.
    ///
    /// Also re-reads `.memstead/workspace.toml` and refreshes
    /// [`crate::workspace::WorkspaceSettings`] before sweeping the
    /// vaults — this is the pairing with the CLI's
    /// `memstead workspace allow-create / grant-cross-link / set-mutations`
    /// family. Without this re-read, a CLI write would land on disk but
    /// the running MCP would still serve the engine's boot-time policy
    /// snapshot; every subsequent `memstead_vault_create` against the new
    /// allowlist would fail with `VAULT_PATH_NOT_ALLOWED` until process
    /// restart. The workspace-wide form runs the heavier path; the
    /// per-vault form (`reload_one_vault_report`) intentionally skips
    /// the workspace re-read — content drift doesn't imply policy
    /// drift.
    ///
    /// Reload of `workspace.toml` is best-effort: a missing or
    /// unparseable file leaves the existing settings untouched. The
    /// per-vault sweep is the primary contract — settings refresh is
    /// the additive bonus.
    ///
    /// First-error-aborts: if any vault's reload fails, the loop
    /// stops and the error propagates. Vaults reloaded before the
    /// failing one are already mutated in the store; the returned
    /// error has no rollback. Operators run the per-vault form to
    /// retry the failing vault explicitly.
    pub fn reload_each_writable_vault_reports(
        &mut self,
    ) -> Result<Vec<crate::ops::ReloadReport>, EngineError> {
        self.refresh_workspace_settings_if_possible();
        let names: Vec<String> = self
            .mounts
            .iter()
            .map(|m| m.mount.vault.clone())
            .collect();
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let report = self.reload_one_vault_report(&name)?;
            out.push(report);
        }
        Ok(out)
    }

    /// Best-effort refresh of [`crate::workspace::WorkspaceSettings`]
    /// from the workspace's `.memstead/workspace.toml`. Called by the
    /// workspace-wide reload sweep so CLI-driven policy edits become
    /// visible to a live engine without process restart.
    ///
    /// Silent no-op when the engine has no `workspace_root` (legacy
    /// in-memory constructions) or when the on-disk file is missing /
    /// unparseable. The per-vault reload contract stays the canonical
    /// failure surface; settings refresh failures are intentionally
    /// non-fatal so a malformed workspace.toml doesn't break content
    /// drift detection.
    fn refresh_workspace_settings_if_possible(&mut self) {
        let Some(root) = self.workspace_root.clone() else {
            return;
        };
        let store = crate::workspace_store::FileWorkspaceStore::new();
        let workspace = match crate::workspace_store::WorkspaceStoreAdapter::load(&store, &root) {
            Ok(w) => w,
            Err(_) => return,
        };
        self.set_settings(workspace.settings);
    }

    /// Reload every mounted vault in declaration order; returns one
    /// `(vault, ReloadResult)` per mount.
    ///
    /// Failure model is **first-error-aborts**: if any vault's reload
    /// fails, the loop stops and the error propagates. Vaults reloaded
    /// before the failing one are already mutated in the store; the
    /// returned error has no rollback. Operators run the per-vault
    /// form to retry the failing vault explicitly.
    ///
    /// Caller-friendly batching wrapper around [`Self::reload_one_vault`];
    /// internal cache invalidation happens once per vault (the inner
    /// call invalidates) so an N-vault batch invalidates the memos
    /// N times. That's wasteful for large workspaces; once the
    /// `memstead_reload` MCP handler migrates we can tighten this to one
    /// invalidation at the end.
    pub fn reload_each_writable_vault(
        &mut self,
    ) -> Result<Vec<(String, crate::ops::ReloadResult)>, EngineError> {
        let names: Vec<String> = self
            .mounts
            .iter()
            .map(|m| m.mount.vault.clone())
            .collect();
        // Workspace-wide reload semantics: take the engine-wide
        // sink, clear it, route per-vault inner reloads through it,
        // put it back. The result is `self.load_warnings` carries
        // every typed drift warning the reload sweep produced (so
        // the next `engine.health()` call surfaces them).
        let mut sink = std::mem::take(&mut self.load_warnings);
        sink.clear();
        let mut out = Vec::with_capacity(names.len());
        let mut loop_err = None;
        for name in names {
            match self.reload_one_vault_inner(&name, &mut sink) {
                Ok(result) => out.push((name, result)),
                Err(e) => {
                    loop_err = Some(e);
                    break;
                }
            }
        }
        self.load_warnings = sink;
        if let Some(e) = loop_err {
            return Err(e);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    
    

    
    use tempfile::TempDir;

    use crate::backend::{BackendError, VaultBackend};
    use crate::engine::test_helpers::*;
    use crate::engine::{Engine, EngineError};
    use crate::ops::WarningHint;
    use crate::storage::{ArchiveBackend, FilesystemVaultWriter};
    use crate::vault::VaultOrigin;
    
    

    #[test]
    fn reload_each_writable_vault_repopulates_load_warnings() {
        // Boot with a clean vault, then mid-flight write a file
        // with a duplicate heading, then call reload_each_writable_vault.
        // The accumulator should pick up the new typed warning.
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir.clone()),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        assert!(engine.load_warnings().is_empty(), "clean boot has no warnings");

        // Drop a markdown file with two `## Identity` headings.
        let body = "---\ntype: spec\n---\n# Dup\n\n## Identity\n\nfirst.\n\n## Identity\n\nsecond.\n";
        std::fs::write(vault_dir.join("dup.md"), body).unwrap();

        engine.reload_each_writable_vault().unwrap();
        let warnings = engine.load_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, WarningHint::DuplicateSectionHeading { .. })),
            "workspace-wide reload must repopulate load_warnings: {warnings:?}",
        );
    }

    /// `validate_loaded_relations` runs on the reload path too — a
    /// sibling-writer commit that injects a markdown file carrying a
    /// schema-undeclared rel-type must surface as a typed
    /// `PARSED_RELATION_INVALID` warning after `reload_each_writable_vault`.
    /// Without the reload-path wiring this drift would slip past the
    /// validator (boot only catches what existed at startup).
    #[test]
    fn reload_picks_up_parse_time_relation_drift_from_sibling_writer() {
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();
        // Seed a clean target entity at boot.
        let target_body =
            "---\ntype: spec\n---\n# Target\n\n## Identity\n\nThe target.\n";
        std::fs::write(vault_dir.join("target.md"), target_body).unwrap();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir.clone()),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        // Clean boot — no parse-time relation warnings yet.
        assert!(
            !engine
                .load_warnings()
                .iter()
                .any(|w| matches!(w, WarningHint::ParsedRelationInvalid { .. })),
            "clean boot must not emit ParsedRelationInvalid; got: {:?}",
            engine.load_warnings()
        );

        // Sibling-writer drops a new file with an unknown rel-type.
        let drift_body =
            "---\ntype: spec\n---\n# Source\n\n## Identity\n\nThe source.\n\n## Relationships\n\n- **MADE_UP_TYPE**: [[specs--target]]\n";
        std::fs::write(vault_dir.join("source.md"), drift_body).unwrap();

        engine.reload_each_writable_vault().unwrap();

        let invalid: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter_map(|w| match w {
                WarningHint::ParsedRelationInvalid {
                    rel_type,
                    reason,
                    origin,
                    ..
                } => Some((rel_type.clone(), reason.clone(), origin.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            invalid.len(),
            1,
            "reload must surface the parse-time drift, got: {invalid:?}",
        );
        assert_eq!(invalid[0].0, "MADE_UP_TYPE");
        assert_eq!(invalid[0].1, "unknown_rel_type");
        assert_eq!(invalid[0].2, "writable");
    }

    #[test]
    fn reload_one_vault_keeps_engine_load_warnings_pristine() {
        // Boot with a duplicate-heading file so the accumulator
        // starts non-empty. Single-vault reload should NOT clear
        // or repopulate the engine-wide accumulator (mirrors pro's
        // contract: per-vault reload is silent on the engine-wide
        // sink). The engine field stays as the boot-time snapshot.
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();
        let body = "---\ntype: spec\n---\n# Dup\n\n## Identity\n\na.\n\n## Identity\n\nb.\n";
        std::fs::write(vault_dir.join("dup.md"), body).unwrap();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir.clone()),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let pre = engine.load_warnings().to_vec();
        assert!(!pre.is_empty(), "boot must populate load_warnings");

        engine.reload_one_vault("specs").unwrap();
        let post = engine.load_warnings();
        // Pristine: same as boot snapshot.
        assert_eq!(post.len(), pre.len(), "single-vault reload must not touch sink");
    }

    /// A cross-vault edge `A→B` must survive a
    /// per-vault reload of the TARGET vault B. The removal cascade drops
    /// B's incoming mirrors (including the cross-vault one sourced from A)
    /// and the re-push only rebuilds edges authored by B, so without the
    /// reconstruction pass the edge silently vanishes from the in-memory
    /// index while staying intact in A's record and on disk — under-
    /// reporting topology until a workspace-wide reload heals it.
    #[test]
    fn per_vault_reload_of_target_preserves_incoming_cross_vault_edge() {
        let tmp = TempDir::new().unwrap();
        let a_dir = tmp.path().join("a");
        let b_dir = tmp.path().join("b");
        std::fs::create_dir_all(&a_dir).unwrap();
        std::fs::create_dir_all(&b_dir).unwrap();
        let a_writer = FilesystemVaultWriter::new(a_dir.clone());
        let b_writer = FilesystemVaultWriter::new(b_dir.clone());
        let mut engine = Engine::from_mounts(vec![
            (
                folder_mount("specs", a_dir),
                Box::new(a_writer) as Box<dyn VaultBackend>,
            ),
            (
                folder_mount("memos", b_dir),
                Box::new(b_writer) as Box<dyn VaultBackend>,
            ),
        ])
        .unwrap();

        // Grant the cross-vault link specs → memos so the relate lands.
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_vault_links.insert(
            "specs".to_string(),
            memstead_schema::workspace_config::CrossLinkValue::Wildcard,
        );
        engine.set_settings(settings);

        let (actor, client) = cli_actor();
        let source = engine
            .create_entity(empty_create_args("specs", "Source"), actor, Some(&client), None)
            .unwrap();
        let target = engine
            .create_entity(empty_create_args("memos", "Target"), actor, Some(&client), None)
            .unwrap();
        engine
            .relate_entity(
                crate::engine::RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // (outgoing-present, incoming-present) for the A→B edge.
        let has_edge = |e: &Engine| {
            let out = e
                .store()
                .outgoing(&source.id)
                .iter()
                .any(|edge| edge.target == target.id);
            let inc = e
                .store()
                .incoming(&target.id)
                .iter()
                .any(|edge| edge.from == source.id);
            (out, inc)
        };

        assert_eq!(
            has_edge(&engine),
            (true, true),
            "edge must be indexed in both directions after relate",
        );

        // Per-vault reload of the TARGET vault — the bug trigger.
        engine.reload_one_vault("memos").unwrap();
        assert_eq!(
            has_edge(&engine),
            (true, true),
            "cross-vault edge into B must survive a per-vault reload of B",
        );

        // Convergence: a workspace-wide reload yields the same incoming
        // adjacency for the target — no path-dependent difference.
        engine.reload_each_writable_vault().unwrap();
        assert_eq!(
            has_edge(&engine),
            (true, true),
            "per-vault and workspace reload converge on the same edge",
        );

        // Complement: the edge stayed in the source record throughout —
        // the bug and the fix are about the index, not the records.
        assert!(
            engine
                .store()
                .get(&source.id)
                .unwrap()
                .relationships
                .iter()
                .any(|r| r.target == target.id),
            "source record must retain the relationship throughout",
        );
    }

    /// A per-vault reload of the SOURCE
    /// vault leaves the cross-vault edge intact too — the source's own
    /// outgoing edges are rebuilt by the re-push, and the reconstruction
    /// pass for the OTHER vault is not needed here. Guards against a fix
    /// that fixates on the target case and perturbs the source case.
    #[test]
    fn per_vault_reload_of_source_preserves_outgoing_cross_vault_edge() {
        let tmp = TempDir::new().unwrap();
        let a_dir = tmp.path().join("a");
        let b_dir = tmp.path().join("b");
        std::fs::create_dir_all(&a_dir).unwrap();
        std::fs::create_dir_all(&b_dir).unwrap();
        let a_writer = FilesystemVaultWriter::new(a_dir.clone());
        let b_writer = FilesystemVaultWriter::new(b_dir.clone());
        let mut engine = Engine::from_mounts(vec![
            (
                folder_mount("specs", a_dir),
                Box::new(a_writer) as Box<dyn VaultBackend>,
            ),
            (
                folder_mount("memos", b_dir),
                Box::new(b_writer) as Box<dyn VaultBackend>,
            ),
        ])
        .unwrap();
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_vault_links.insert(
            "specs".to_string(),
            memstead_schema::workspace_config::CrossLinkValue::Wildcard,
        );
        engine.set_settings(settings);

        let (actor, client) = cli_actor();
        let source = engine
            .create_entity(empty_create_args("specs", "Source"), actor, Some(&client), None)
            .unwrap();
        let target = engine
            .create_entity(empty_create_args("memos", "Target"), actor, Some(&client), None)
            .unwrap();
        engine
            .relate_entity(
                crate::engine::RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        engine.reload_one_vault("specs").unwrap();

        let out = engine
            .store()
            .outgoing(&source.id)
            .iter()
            .any(|edge| edge.target == target.id);
        let inc = engine
            .store()
            .incoming(&target.id)
            .iter()
            .any(|edge| edge.from == source.id);
        assert!(out && inc, "outgoing cross-vault edge must survive a source-vault reload");
    }

    #[test]
    fn workspace_root_setter_round_trips() {
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let root = tmp.path().to_path_buf();
        engine.set_workspace_root(root.clone());
        assert_eq!(engine.workspace_root(), Some(root.as_path()));
    }

    #[test]
    fn export_vault_folder_backend_produces_archive() {
        // Folder-backed vault with config + one entity. The
        // export_vault dispatcher routes to the folder backend's
        // override which produces a deterministic .memstead archive.
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().join("specs");
        std::fs::create_dir_all(vault_dir.join(".memstead")).unwrap();
        let config_body = r#"{
            "format": 1,
            "schema": "default@1.0.0",
            "version": "1.0.0"
        }"#;
        std::fs::write(vault_dir.join(".memstead").join("config.json"), config_body).unwrap();

        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir.clone()),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();

        let archive_path = tmp.path().join("specs.mem");
        let result = engine.export_vault("specs", &archive_path).unwrap();
        assert!(archive_path.exists(), "archive must exist on disk");
        assert!(result.size_bytes > 0);
        // entity_count is 0 here (no .md files seeded); the function
        // still produces an archive carrying the config + schema.
        assert_eq!(result.entity_count, 0);
    }

    #[test]
    fn export_vault_unknown_vault_returns_unknown_vault() {
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let output = tmp.path().join("out.mem");
        let err = engine.export_vault("missing", &output).unwrap_err();
        assert!(matches!(err, EngineError::UnknownVault(v) if v == "missing"));
    }

    #[test]
    fn export_vault_missing_config_returns_invalid_input() {
        // Folder mount with no .memstead/config.json — `vault_config_for`
        // returns None and `export_vault` surfaces InvalidInput
        // rather than reaching the backend.
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let output = tmp.path().join("out.mem");
        let err = engine.export_vault("specs", &output).unwrap_err();
        assert!(matches!(err, EngineError::InvalidInput(_)));
    }

    #[test]
    fn export_vault_archive_backend_returns_sealed() {
        // Archive backends are already-an-archive — re-export is
        // intentionally rejected via BackendError::Sealed.
        let tmp = TempDir::new().unwrap();
        let archive_path = build_archive(
            tmp.path(),
            "ext",
            &[(".memstead/config.json", b"{\"format\":1,\"schema\":\"default@1.0.0\",\"version\":\"1.0.0\"}")],
        );
        let engine = Engine::from_mounts(vec![(
            archive_mount("ext", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let output = tmp.path().join("out.mem");
        let err = engine.export_vault("ext", &output).unwrap_err();
        assert!(matches!(err, EngineError::Backend(BackendError::Sealed)));
    }

    #[test]
    fn export_markdown_writes_unchanged_files_zero_writes() {
        // Seed a folder-backed vault with one entity, then call
        // export_markdown. The entity's file already matches the
        // generated content (engine wrote it via create_entity), so
        // export reports `unchanged: 1, written: 0`.
        let tmp = TempDir::new().unwrap();
        let (engine, _seeded) = engine_with_seed(&tmp, "Sample");
        let result = engine.export_markdown(None, None).unwrap();
        assert_eq!(
            result.written, 0,
            "freshly-created entity's file already matches generated markdown"
        );
        assert_eq!(result.unchanged, 1, "the one seeded entity counts as unchanged");
        assert!(result.skipped_mounts.is_empty(), "folder-only workspace has no skipped mounts");
    }

    #[test]
    fn export_markdown_skips_non_folder_mounts() {
        // Archive-mounted vault has no working tree — workspace-wide
        // export records it under skipped_mounts and reports zero
        // writes / zero unchanged for the rest.
        let tmp = TempDir::new().unwrap();
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"# Title: Foo\n")]);
        let engine = Engine::from_mounts(vec![(
            archive_mount("ext", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let result = engine.export_markdown(None, None).unwrap();
        assert_eq!(result.written, 0);
        assert_eq!(result.unchanged, 0);
        assert_eq!(result.skipped_mounts.len(), 1, "archive mount is in the skipped list");
        let entry = &result.skipped_mounts[0];
        assert_eq!(entry.vault, "ext");
        assert_eq!(entry.active_backend, "archive");
        assert_eq!(entry.reason, "backend_does_not_support_markdown_export");
    }

    #[test]
    fn export_markdown_per_vault_refuses_on_incompatible_backend() {
        // Per-vault export against an archive-backed vault returns
        // the typed `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` refusal
        // naming the active backend and the supported-backend list.
        let tmp = TempDir::new().unwrap();
        let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"# Title: Foo\n")]);
        let engine = Engine::from_mounts(vec![(
            archive_mount("ext", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let err = engine.export_markdown(Some("ext"), None).unwrap_err();
        assert_eq!(err.code(), "MARKDOWN_EXPORT_UNSUPPORTED_BACKEND");
        let details = err.details();
        assert_eq!(details["vault"], "ext");
        assert_eq!(details["active_backend"], "archive");
        assert_eq!(details["supported_backends"], serde_json::json!(["folder"]));
    }


    #[test]
    fn register_writable_vault_adds_mount_and_router_entry() {
        // Start with one vault; register a second at runtime. Both
        // should be visible afterwards.
        let tmp = TempDir::new().unwrap();
        let vault_a = tmp.path().join("a");
        std::fs::create_dir_all(&vault_a).unwrap();
        let writer_a = FilesystemVaultWriter::new(vault_a.clone());

        let mut engine = Engine::from_mounts(vec![(
            folder_mount("alpha", vault_a),
            Box::new(writer_a) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        assert!(engine.vault_router().is_writable("alpha"));

        let vault_b = tmp.path().join("b");
        std::fs::create_dir_all(&vault_b).unwrap();
        let writer_b = FilesystemVaultWriter::new(vault_b.clone());

        engine
            .register_writable_vault(
                folder_mount("beta", vault_b.clone()),
                Box::new(writer_b) as Box<dyn VaultBackend>,
                VaultOrigin::ExplicitToml,
            )
            .unwrap();

        // Both vaults are now writable + visible.
        assert!(engine.vault_router().is_writable("alpha"));
        assert!(engine.vault_router().is_writable("beta"));
        assert!(engine.vault_router().is_visible("beta"));

        // Mount + schema lookups resolve.
        assert!(engine.mount("beta").is_some());
        assert!(engine.schemas().contains_key("beta"));

        // Folder path surfaces via vault_router.
        assert_eq!(
            engine.vault_router().dir_for_vault("beta"),
            Some(vault_b.as_path()),
        );
    }

    /// Schema-pin authority on the runtime-register path (symmetric with
    /// the boot path): a vault registered at runtime resolves its schema
    /// from its own config (`software@0.1.0`) even though the mount
    /// expects an unresolvable pin — register succeeds, and the
    /// disagreement surfaces a `SchemaPinMismatch` warning.
    #[test]
    fn register_writable_vault_resolves_schema_from_vault_config() {
        let tmp = TempDir::new().unwrap();
        let vault_a = tmp.path().join("a");
        std::fs::create_dir_all(&vault_a).unwrap();
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("alpha", vault_a.clone()),
            Box::new(FilesystemVaultWriter::new(vault_a)) as Box<dyn VaultBackend>,
        )])
        .unwrap();

        let vault_b = tmp.path().join("b");
        std::fs::create_dir_all(vault_b.join(".memstead")).unwrap();
        std::fs::write(
            vault_b.join(".memstead").join("config.json"),
            r#"{"schema":"software@0.1.0"}"#,
        )
        .unwrap();
        let mount_b = crate::workspace::Mount {
            vault: "beta".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "totally-not-a-schema",
                semver::Version::new(9, 9, 9),
            )),
            storage: crate::workspace::MountStorage::Folder { path: vault_b.clone() },
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        engine
            .register_writable_vault(
                mount_b,
                Box::new(FilesystemVaultWriter::new(vault_b)) as Box<dyn VaultBackend>,
                VaultOrigin::ExplicitToml,
            )
            .expect("config pin software@0.1.0 is authoritative — register must succeed despite the unresolvable mount pin");

        assert!(engine.schemas().contains_key("beta"));
        let surfaced = engine.load_warnings().iter().any(|w| {
            matches!(
                w,
                WarningHint::SchemaPinMismatch { vault, config_pin, mount_pin }
                    if vault == "beta"
                        && config_pin == "software@0.1.0"
                        && mount_pin == "totally-not-a-schema@9.9.9"
            )
        });
        assert!(
            surfaced,
            "SchemaPinMismatch must surface for beta: {:?}",
            engine.load_warnings(),
        );
    }

    #[test]
    fn register_writable_vault_rejects_existing_name() {
        // Re-registering an already-writable vault must fail with
        // VaultNameCollision and not mutate the engine.
        let tmp = TempDir::new().unwrap();
        let vault_a = tmp.path().join("a");
        std::fs::create_dir_all(&vault_a).unwrap();
        let writer_a = FilesystemVaultWriter::new(vault_a.clone());

        let mut engine = Engine::from_mounts(vec![(
            folder_mount("alpha", vault_a),
            Box::new(writer_a) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let mount_count_pre = engine.mounts().len();

        let vault_collide = tmp.path().join("alpha-2");
        std::fs::create_dir_all(&vault_collide).unwrap();
        let writer_collide = FilesystemVaultWriter::new(vault_collide.clone());

        let err = engine
            .register_writable_vault(
                folder_mount("alpha", vault_collide),
                Box::new(writer_collide) as Box<dyn VaultBackend>,
                VaultOrigin::ExplicitToml,
            )
            .unwrap_err();
        match err {
            EngineError::VaultNameCollision { name, source_origin } => {
                assert_eq!(name, "alpha");
                // post-restructure source_origin references
                // `.memstead/workspace.toml`; the assertion stays
                // permissive (substring OR non-empty) so the test
                // doesn't lock the exact wording.
                assert!(
                    source_origin.contains(".memstead/workspace.toml")
                        || !source_origin.is_empty()
                );
            }
            other => panic!("expected VaultNameCollision, got {other:?}"),
        }

        // Engine state unchanged.
        assert_eq!(engine.mounts().len(), mount_count_pre);
    }

    #[test]
    fn register_writable_vault_loads_entities_into_store() {
        // The newly-registered vault's entities should surface in
        // the engine's store after registration.
        let tmp = TempDir::new().unwrap();
        let vault_a = tmp.path().join("a");
        std::fs::create_dir_all(&vault_a).unwrap();
        let writer_a = FilesystemVaultWriter::new(vault_a.clone());

        let mut engine = Engine::from_mounts(vec![(
            folder_mount("alpha", vault_a),
            Box::new(writer_a) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let pre_count = engine.store().all_entities().count();

        // Build vault_b with a markdown entity on disk.
        let vault_b = tmp.path().join("b");
        std::fs::create_dir_all(&vault_b).unwrap();
        std::fs::write(
            vault_b.join("b1.md"),
            "---\ntype: spec\n---\n# B1\n\n## Identity\n\nseed.\n",
        )
        .unwrap();
        let writer_b = FilesystemVaultWriter::new(vault_b.clone());

        engine
            .register_writable_vault(
                folder_mount("beta", vault_b),
                Box::new(writer_b) as Box<dyn VaultBackend>,
                VaultOrigin::ExplicitToml,
            )
            .unwrap();

        let post_count = engine.store().all_entities().count();
        assert!(post_count > pre_count, "register must load entities");
        let beta_count = engine
            .store()
            .all_entities()
            .filter(|e| e.vault == "beta")
            .count();
        assert_eq!(beta_count, 1);
    }

    #[test]
    fn register_then_unregister_round_trips() {
        // End-to-end check: register a vault, then unregister it,
        // and confirm the engine returns to the pre-registration
        // state.
        let tmp = TempDir::new().unwrap();
        let vault_a = tmp.path().join("a");
        std::fs::create_dir_all(&vault_a).unwrap();
        let writer_a = FilesystemVaultWriter::new(vault_a.clone());

        let mut engine = Engine::from_mounts(vec![(
            folder_mount("alpha", vault_a),
            Box::new(writer_a) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let pre_mounts = engine.mounts().len();

        let vault_b = tmp.path().join("b");
        std::fs::create_dir_all(&vault_b).unwrap();
        let writer_b = FilesystemVaultWriter::new(vault_b);

        engine
            .register_writable_vault(
                folder_mount("beta", tmp.path().join("b")),
                Box::new(writer_b) as Box<dyn VaultBackend>,
                VaultOrigin::ExplicitToml,
            )
            .unwrap();
        assert_eq!(engine.mounts().len(), pre_mounts + 1);

        let removed = engine.unregister_writable_vault("beta").unwrap();
        assert!(removed.is_some());
        assert_eq!(engine.mounts().len(), pre_mounts);
        assert!(!engine.vault_router().is_writable("beta"));
    }

    #[test]
    fn unregister_writable_vault_returns_false_for_unknown_name() {
        // Idempotent contract: repeated calls / unknown names are
        // not errors — return false so callers can branch without
        // a typed error envelope for the common "already gone" case.
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().to_path_buf();
        let writer = FilesystemVaultWriter::new(vault_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", vault_dir),
            Box::new(writer) as Box<dyn VaultBackend>,
        )])
        .unwrap();
        let removed = engine.unregister_writable_vault("missing").unwrap();
        assert!(removed.is_none(), "unknown vault returns Ok(None)");
        // The original vault is still present and readable.
        assert!(engine.vault_router().is_writable("specs"));
    }

    #[test]
    fn unregister_writable_vault_drops_mount_and_router_entry() {
        // Heterogeneous engine: two mounts. Unregister one and
        // assert (a) it's gone from the mount list, (b) gone from
        // the vault_router's writable set, (c) the OTHER mount is
        // untouched.
        let tmp = TempDir::new().unwrap();
        let vault_a = tmp.path().join("a");
        std::fs::create_dir_all(&vault_a).unwrap();
        let writer_a = FilesystemVaultWriter::new(vault_a.clone());
        let vault_b = tmp.path().join("b");
        std::fs::create_dir_all(&vault_b).unwrap();
        let writer_b = FilesystemVaultWriter::new(vault_b.clone());

        let mut engine = Engine::from_mounts(vec![
            (
                folder_mount("alpha", vault_a),
                Box::new(writer_a) as Box<dyn VaultBackend>,
            ),
            (
                folder_mount("beta", vault_b),
                Box::new(writer_b) as Box<dyn VaultBackend>,
            ),
        ])
        .unwrap();

        let removed = engine.unregister_writable_vault("alpha").unwrap();
        assert!(removed.is_some());

        // alpha is gone from every surface.
        assert!(!engine.vault_router().is_writable("alpha"));
        assert!(!engine.vault_router().is_visible("alpha"));
        assert!(engine.mount("alpha").is_none());

        // beta survives unchanged.
        assert!(engine.vault_router().is_writable("beta"));
        assert!(engine.mount("beta").is_some());
    }

    #[test]
    fn unregister_writable_vault_drops_entities_for_that_vault_only() {
        // Build an engine with two vaults, write one entity to each
        // backend, build the engine (loads both), unregister one,
        // assert the store still has the other vault's entity.
        let tmp = TempDir::new().unwrap();
        let vault_a = tmp.path().join("a");
        std::fs::create_dir_all(&vault_a).unwrap();
        std::fs::write(
            vault_a.join("a1.md"),
            "---\ntype: spec\n---\n# A1\n\n## Identity\n\nseed.\n",
        )
        .unwrap();
        let writer_a = FilesystemVaultWriter::new(vault_a.clone());

        let vault_b = tmp.path().join("b");
        std::fs::create_dir_all(&vault_b).unwrap();
        std::fs::write(
            vault_b.join("b1.md"),
            "---\ntype: spec\n---\n# B1\n\n## Identity\n\nseed.\n",
        )
        .unwrap();
        let writer_b = FilesystemVaultWriter::new(vault_b.clone());

        let mut engine = Engine::from_mounts(vec![
            (
                folder_mount("alpha", vault_a),
                Box::new(writer_a) as Box<dyn VaultBackend>,
            ),
            (
                folder_mount("beta", vault_b),
                Box::new(writer_b) as Box<dyn VaultBackend>,
            ),
        ])
        .unwrap();

        let pre_total = engine.store().all_entities().count();
        assert!(pre_total >= 2, "both vaults must load entities");

        engine.unregister_writable_vault("alpha").unwrap();

        // alpha's entities are gone.
        let alpha_remaining = engine
            .store()
            .all_entities()
            .filter(|e| e.vault == "alpha")
            .count();
        assert_eq!(alpha_remaining, 0);

        // beta's entities survive.
        let beta_remaining = engine
            .store()
            .all_entities()
            .filter(|e| e.vault == "beta")
            .count();
        assert!(beta_remaining > 0, "beta entities must survive");
    }
    #[test]
    fn reload_one_vault_returns_empty_diff_when_disk_is_unchanged() {
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let result = engine
            .reload_one_vault("specs")
            .expect("reload on stable disk must succeed");
        assert!(result.added.is_empty(), "added: {:?}", result.added);
        assert!(result.changed.is_empty(), "changed: {:?}", result.changed);
        assert!(result.removed.is_empty(), "removed: {:?}", result.removed);
    }

    #[test]
    fn reload_one_vault_picks_up_external_addition() {
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        // Simulate an external writer dropping a new entity on disk
        // without going through the engine.
        std::fs::write(
            tmp.path().join("external.md"),
            "---\ntype: spec\n---\n# External\n\n## Identity\n\nE.\n",
        )
        .unwrap();
        let result = engine.reload_one_vault("specs").unwrap();
        assert_eq!(
            result.added.iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
            vec!["specs--external"]
        );
        assert!(result.changed.is_empty());
        assert!(result.removed.is_empty());
        // The new entity is now reachable through the engine.
        assert!(
            engine
                .get_entity(&crate::EntityId::new("specs", "external"))
                .is_some()
        );
    }

    #[test]
    fn reload_one_vault_picks_up_external_removal() {
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        // Lonely Three exists from the demo fixture; remove it
        // off-engine and reload.
        std::fs::remove_file(tmp.path().join("lonely-three.md")).unwrap();
        let result = engine.reload_one_vault("specs").unwrap();
        assert!(result.added.is_empty());
        assert!(result.changed.is_empty());
        assert_eq!(
            result.removed.iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
            vec!["specs--lonely-three"]
        );
    }

    #[test]
    fn reload_one_vault_picks_up_external_change() {
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        // Overwrite an existing entity's content; the new
        // `content_hash` must surface in the `changed` diff.
        std::fs::write(
            tmp.path().join("source-one.md"),
            "---\ntype: spec\n---\n# Source One Edited\n\n## Identity\n\nNew body.\n",
        )
        .unwrap();
        let result = engine.reload_one_vault("specs").unwrap();
        assert!(result.added.is_empty());
        assert_eq!(
            result.changed.iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
            vec!["specs--source-one"]
        );
        assert!(result.removed.is_empty());
    }

    #[test]
    fn reload_one_vault_rejects_unknown_vault() {
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let err = engine.reload_one_vault("nope").unwrap_err();
        match err {
            EngineError::UnknownVault(name) => assert_eq!(name, "nope"),
            other => panic!("expected UnknownVault, got {other:?}"),
        }
    }

    #[test]
    fn reload_each_writable_vault_returns_one_entry_per_mount() {
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let reports = engine
            .reload_each_writable_vault()
            .expect("batch reload on stable disk must succeed");
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].0, "specs");
        assert!(reports[0].1.added.is_empty());
        assert!(reports[0].1.changed.is_empty());
        assert!(reports[0].1.removed.is_empty());
    }

    // ---- Engine::settings -------------------------------------------

    #[test]
    fn settings_default_to_empty_on_fresh_engine() {
        let tmp = TempDir::new().unwrap();
        let engine = build_demo_engine(&tmp);
        let s = engine.settings();
        assert!(s.vault_create_rules.is_empty());
        assert!(s.vault_delete_rules.is_empty());
        assert!(s.cross_vault_links.is_empty());
    }

    #[test]
    fn set_settings_replaces_workspace_policy() {
        use crate::workspace::{CreateRuleSetting, DeleteRuleSetting, WorkspaceSettings};
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let mut settings = WorkspaceSettings::default();
        settings.vault_create_rules.push(CreateRuleSetting {
            pattern: "exec-*".to_string(),
            schemas: vec!["default@1.0.0".to_string()],
            default_cross_links: None,
        });
        settings.vault_delete_rules.push(DeleteRuleSetting {
            pattern: "exec-*".to_string(),
        });
        engine.set_settings(settings);
        assert_eq!(engine.settings().vault_create_rules.len(), 1);
        assert_eq!(engine.settings().vault_create_rules[0].pattern, "exec-*");
        assert_eq!(engine.settings().vault_delete_rules.len(), 1);
        assert_eq!(engine.settings().vault_delete_rules[0].pattern, "exec-*");
    }

    // ---- Engine::reload_each_writable_vault (continued) -------------

    #[test]
    fn reload_each_writable_vault_picks_up_external_changes_per_vault() {
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        // Mutate disk: add one entity, remove another, change a third.
        std::fs::write(
            tmp.path().join("new-via-disk.md"),
            "---\ntype: spec\n---\n# New Via Disk\n\n## Identity\n\nN.\n",
        )
        .unwrap();
        std::fs::remove_file(tmp.path().join("lonely-three.md")).unwrap();
        std::fs::write(
            tmp.path().join("source-one.md"),
            "---\ntype: spec\n---\n# Source One\n\n## Identity\n\nDifferent body.\n",
        )
        .unwrap();

        let reports = engine.reload_each_writable_vault().unwrap();
        assert_eq!(reports.len(), 1);
        let (vault, result) = &reports[0];
        assert_eq!(vault, "specs");
        assert_eq!(
            result.added.iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
            vec!["specs--new-via-disk"]
        );
        assert_eq!(
            result.removed.iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
            vec!["specs--lonely-three"]
        );
        assert_eq!(
            result.changed.iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
            vec!["specs--source-one"]
        );
    }

    // ---- Engine::reload_one_vault_report (rich-shape wrapper) -------

    #[test]
    fn reload_one_vault_report_returns_rich_shape_for_folder_default() {
        // Folder backend has no current_head (Ok(None)); the wrapper
        // falls back to EMPTY_TREE_SHA for both head_before and
        // head_after. entities_loaded reflects the post-reload count;
        // changed_entity_ids is empty when the disk is unchanged.
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let report = engine.reload_one_vault_report("specs").unwrap();
        assert_eq!(report.vault, "specs");
        assert_eq!(report.head_before, crate::ops::EMPTY_TREE_SHA);
        assert_eq!(report.head_after, crate::ops::EMPTY_TREE_SHA);
        // build_demo_engine seeds 3 entities (Source One, Target Two,
        // Lonely Three) — all real, no stubs from those creates.
        assert_eq!(report.entities_loaded, 3);
        // No external disk changes between init and reload → empty diff.
        assert!(report.changed_entity_ids.is_empty());
    }

    #[test]
    fn reload_one_vault_report_unions_added_changed_removed_into_one_list() {
        // Mutate disk: add one, remove one, change one. The report's
        // changed_entity_ids unions the slim ReloadResult's three
        // diff lists into a single sorted vec — matches pro's
        // wire contract.
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        std::fs::write(
            tmp.path().join("new-via-disk.md"),
            "---\ntype: spec\n---\n# New Via Disk\n\n## Identity\n\nN.\n",
        )
        .unwrap();
        std::fs::remove_file(tmp.path().join("lonely-three.md")).unwrap();
        std::fs::write(
            tmp.path().join("source-one.md"),
            "---\ntype: spec\n---\n# Source One\n\n## Identity\n\nDifferent body.\n",
        )
        .unwrap();

        let report = engine.reload_one_vault_report("specs").unwrap();
        assert_eq!(report.vault, "specs");
        let ids: Vec<&str> = report
            .changed_entity_ids
            .iter()
            .map(|id| id.as_ref())
            .collect();
        // Sorted lexicographically: lonely-three < new-via-disk < source-one
        assert_eq!(
            ids,
            vec![
                "specs--lonely-three",
                "specs--new-via-disk",
                "specs--source-one",
            ]
        );
    }

    #[test]
    fn reload_one_vault_report_rejects_unknown_vault() {
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let err = engine.reload_one_vault_report("missing").unwrap_err();
        assert!(matches!(err, EngineError::UnknownVault(_)));
    }

    #[test]
    fn reload_each_writable_vault_reports_returns_one_entry_per_mount() {
        let tmp = TempDir::new().unwrap();
        let mut engine = build_demo_engine(&tmp);
        let reports = engine.reload_each_writable_vault_reports().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].vault, "specs");
        assert_eq!(reports[0].entities_loaded, 3);
    }

    /// Workspace-wide reload re-reads `.memstead/workspace.toml` and
    /// refreshes [`WorkspaceSettings`]. This is the pairing with the
    /// CLI's `memstead workspace allow-create / grant-cross-link /
    /// set-mutations` family — without it, a CLI write lands on disk
    /// but the running engine keeps serving the boot-time policy
    /// snapshot until process restart.
    #[test]
    fn reload_each_writable_vault_reports_refreshes_workspace_settings() {
        let tmp = TempDir::new().unwrap();

        // Minimum-viable workspace.toml (no rules) + one writable
        // folder-backed vault.
        let memstead_dir = tmp.path().join(".memstead");
        std::fs::create_dir_all(&memstead_dir).unwrap();
        let workspace_toml = memstead_dir.join("workspace.toml");
        std::fs::write(
            &workspace_toml,
            "format = \"memstead-git-branch-1\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        let mounts_json = memstead_dir.join("state").join("mounts.json");
        std::fs::create_dir_all(mounts_json.parent().unwrap()).unwrap();
        let vault_dir = tmp.path().join("specs");
        std::fs::create_dir_all(&vault_dir).unwrap();
        let mounts_body = format!(
            r#"{{ "format": "memstead-mounts-1", "mounts": [{{ "vault": "specs", "schema": "default@1.0.0", "storage": {{ "type": "folder", "path": "{}" }}, "capability": "write", "lifecycle": "eager", "cross_linkable": true }}] }}"#,
            vault_dir.display(),
        );
        std::fs::write(&mounts_json, mounts_body).unwrap();

        let mut engine = Engine::from_workspace_root(tmp.path()).unwrap();
        assert!(
            engine.settings().vault_create_rules.is_empty(),
            "boot-time settings carry no create rules"
        );

        // Simulate an out-of-band CLI write to workspace.toml.
        std::fs::write(
            &workspace_toml,
            "format = \"memstead-git-branch-1\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n\n[[vault_management.create]]\npattern = \"exec-*\"\nschemas = [\"default@1.0.0\"]\n",
        )
        .unwrap();

        engine.reload_each_writable_vault_reports().unwrap();

        let rules = &engine.settings().vault_create_rules;
        assert_eq!(rules.len(), 1, "workspace-wide reload must refresh the policy");
        assert_eq!(rules[0].pattern, "exec-*");
    }

    // ---- Engine::reload_if_stale ------------------------------


    // ---- set_vault_schema / dual-pin migration ----

    const MIG_TYPE_TAIL: &str = r#"sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: _default
propagating_relationships: []
updatable_fields: []
health_required_fields: []
staleness_threshold_days: 90
write_rules: []
"#;

    /// Schema manifest for the migration tests: `name@version` with a
    /// `doc` type. `with_status = true` adds a required, no-default
    /// enum field `status` — entities created without it are
    /// non-conformant against that schema.
    fn mig_manifest(name: &str, version: &str) -> String {
        format!(
            r#"name: {name}
version: {version}
description: migration test schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: USES
      description: link
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#
        )
    }

    fn mig_type_yaml(with_status: bool) -> String {
        let metadata = if with_status {
            "metadata_fields:\n  - key: status\n    description: Lifecycle state\n    field_type: string\n    enum_values:\n      - open\n      - closed\n"
        } else {
            "metadata_fields: []\n"
        };
        format!("name: doc\ndescription: t\nwhen_to_use: tests\n{metadata}{MIG_TYPE_TAIL}")
    }

    fn write_mig_schema(root: &std::path::Path, dir: &str, name: &str, version: &str, with_status: bool) {
        let d = root.join(dir);
        std::fs::create_dir_all(d.join("types")).unwrap();
        std::fs::write(d.join("schema.yaml"), mig_manifest(name, version)).unwrap();
        std::fs::write(d.join("types").join("doc.yaml"), mig_type_yaml(with_status)).unwrap();
    }

    /// Engine with one vault pinned `mig-a@0.1.0` (no required
    /// metadata) plus loadable `mig-a@0.2.0` (identical shape) and
    /// `mig-b@0.1.0` (required enum `status`) in the workspace
    /// schemas dir. Two conformant-under-A entities are created.
    fn migration_engine() -> (tempfile::TempDir, Engine) {
        let tmp = tempfile::TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        write_mig_schema(&schemas_dir, "mig-a-1", "mig-a", "0.1.0", false);
        write_mig_schema(&schemas_dir, "mig-a-2", "mig-a", "0.2.0", false);
        write_mig_schema(&schemas_dir, "mig-b-1", "mig-b", "0.1.0", true);
        let vault_dir = tmp.path().join("vault");
        std::fs::create_dir_all(&vault_dir).unwrap();
        let writer = crate::storage::FilesystemVaultWriter::new(vault_dir.clone());
        let mut mount = folder_mount("specs", vault_dir);
        mount.schema = Some("mig-a@0.1.0".parse().unwrap());
        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn crate::backend::VaultBackend>)],
            Some(&schemas_dir),
        )
        .unwrap();
        for title in ["One", "Two"] {
            let mut args = empty_create_args("specs", title);
            args.entity_type = "doc".to_string();
            args.sections = indexmap::IndexMap::from_iter([(
                "body".to_string(),
                "content".to_string(),
            )]);
            engine
                .create_entity(args, crate::vcs::Actor::Cli, None, None)
                .expect("conformant create under mig-a");
        }
        (tmp, engine)
    }

    fn sref(s: &str) -> memstead_schema::SchemaRef {
        s.parse().unwrap()
    }

    #[test]
    fn set_schema_noop_on_current_pin() {
        let (_tmp, mut engine) = migration_engine();
        let out = engine
            .set_vault_schema("specs", &sref("mig-a@0.1.0"))
            .unwrap();
        assert_eq!(out.outcome, crate::engine::SetSchemaResult::Noop);
        assert_eq!(out.schema_pin, "mig-a@0.1.0");
        assert_eq!(out.migration_target, None);
        assert!(out.findings.is_empty());
    }

    #[test]
    fn set_schema_switches_immediately_when_integral() {
        // Version bump within the same domain; entities conform to
        // the identical-shape 0.2.0, so the switch is immediate.
        let (_tmp, mut engine) = migration_engine();
        let out = engine
            .set_vault_schema("specs", &sref("mig-a@0.2.0"))
            .unwrap();
        assert_eq!(out.outcome, crate::engine::SetSchemaResult::Switched);
        assert_eq!(out.schema_pin, "mig-a@0.2.0");
        assert_eq!(out.migration_target, None);
        assert!(out.findings.is_empty());
        assert_eq!(
            engine.schema_pin("specs").unwrap().as_display(),
            "mig-a@0.2.0"
        );
        assert!(engine.migration_target("specs").is_none());
    }

    /// Regression: an atomic switch must persist the new pin into the
    /// **authoritative** backend config, not just `mounts.json`. Boot
    /// resolution prefers the backend config's pin over `Mount.schema`,
    /// so before this fix the switch evaporated on the next process boot
    /// for any config-present vault (every `create_vault`-made vault).
    #[test]
    fn set_schema_switch_persists_pin_into_backend_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        write_mig_schema(&schemas_dir, "mig-a-1", "mig-a", "0.1.0", false);
        write_mig_schema(&schemas_dir, "mig-a-2", "mig-a", "0.2.0", false);
        let vault_dir = tmp.path().join("vault");
        std::fs::create_dir_all(vault_dir.join(".memstead")).unwrap();
        // Config-present vault: the authoritative pin lives here.
        std::fs::write(
            vault_dir.join(".memstead").join("config.json"),
            br#"{"schema":"mig-a@0.1.0"}"#,
        )
        .unwrap();
        let writer = crate::storage::FilesystemVaultWriter::new(vault_dir.clone());
        let mut mount = folder_mount("specs", vault_dir.clone());
        mount.schema = Some("mig-a@0.1.0".parse().unwrap());
        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![(
                mount,
                Box::new(writer) as Box<dyn crate::backend::VaultBackend>,
            )],
            Some(&schemas_dir),
        )
        .unwrap();

        let out = engine
            .set_vault_schema("specs", &sref("mig-a@0.2.0"))
            .unwrap();
        assert_eq!(out.outcome, crate::engine::SetSchemaResult::Switched);

        // The authoritative backend config now carries the new pin —
        // otherwise the switch would evaporate on reboot.
        let cfg_bytes = std::fs::read(vault_dir.join(".memstead").join("config.json")).unwrap();
        let cfg: serde_json::Value = serde_json::from_slice(&cfg_bytes).unwrap();
        assert_eq!(
            cfg["schema"], "mig-a@0.2.0",
            "atomic switch must update the authoritative backend config"
        );
    }

    #[test]
    fn set_schema_unknown_target_refuses_schema_not_found() {
        let (_tmp, mut engine) = migration_engine();
        let err = engine
            .set_vault_schema("specs", &sref("nope@9.9.9"))
            .unwrap_err();
        assert_eq!(err.code(), "SCHEMA_NOT_FOUND");
        // No state change.
        assert!(engine.migration_target("specs").is_none());
    }

    #[test]
    fn set_schema_migration_lifecycle_end_to_end() {
        let (_tmp, mut engine) = migration_engine();
        let target = sref("mig-b@0.1.0");

        // 1. Non-integral target → migration starts; pin unchanged.
        let out = engine.set_vault_schema("specs", &target).unwrap();
        assert_eq!(out.outcome, crate::engine::SetSchemaResult::MigrationStarted);
        assert_eq!(out.schema_pin, "mig-a@0.1.0");
        assert_eq!(out.migration_target.as_deref(), Some("mig-b@0.1.0"));
        assert_eq!(out.findings.len(), 2, "both entities lack `status`");
        assert!(out
            .findings
            .iter()
            .all(|f| f.code == "REQUIRED_FIELD_UNSET"));

        // 2. Reads of not-yet-repaired entities stay permissive.
        let one = crate::entity::EntityId::new("specs", "one");
        assert!(engine.store().get(&one).is_some());

        // 3. Re-issue while unrepaired → pending, full remaining set.
        let out = engine.set_vault_schema("specs", &target).unwrap();
        assert_eq!(out.outcome, crate::engine::SetSchemaResult::MigrationPending);
        assert_eq!(out.findings.len(), 2);

        // 4. Writes validate against the TARGET: `status` is unknown
        //    to the pinned mig-a but declared by mig-b — setting it
        //    must commit; an invalid enum value must refuse.
        let mut bad = crate::engine::UpdateEntityArgs {
            id: one.clone(),
            expected_hash: None,
            sections: indexmap::IndexMap::new(),
            append_sections: indexmap::IndexMap::new(),
            patch_sections: indexmap::IndexMap::new(),
            metadata: indexmap::IndexMap::from_iter([(
                "status".to_string(),
                "banana".to_string(),
            )]),
            metadata_unset: Vec::new(),
            declare_relations: Vec::new(),
            dry_run: false,
            relations_unset: Vec::new(),
        };
        let err = engine
            .update_entity(bad.clone(), crate::vcs::Actor::Cli, None, None)
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_ENUM_VALUE", "strict against target");
        bad.metadata =
            indexmap::IndexMap::from_iter([("status".to_string(), "open".to_string())]);
        engine
            .update_entity(bad, crate::vcs::Actor::Cli, None, None)
            .expect("repair write validated against the migration target");

        // 5. One entity repaired → still pending, findings shrink.
        let out = engine.set_vault_schema("specs", &target).unwrap();
        assert_eq!(out.outcome, crate::engine::SetSchemaResult::MigrationPending);
        assert_eq!(out.findings.len(), 1, "only `two` remains non-integral");

        // 6. Repair the second entity, re-issue → atomic switch.
        let two = crate::entity::EntityId::new("specs", "two");
        let repair = crate::engine::UpdateEntityArgs {
            id: two.clone(),
            expected_hash: None,
            sections: indexmap::IndexMap::new(),
            append_sections: indexmap::IndexMap::new(),
            patch_sections: indexmap::IndexMap::new(),
            metadata: indexmap::IndexMap::from_iter([(
                "status".to_string(),
                "closed".to_string(),
            )]),
            metadata_unset: Vec::new(),
            declare_relations: Vec::new(),
            dry_run: false,
            relations_unset: Vec::new(),
        };
        engine
            .update_entity(repair, crate::vcs::Actor::Cli, None, None)
            .unwrap();
        let out = engine.set_vault_schema("specs", &target).unwrap();
        assert_eq!(out.outcome, crate::engine::SetSchemaResult::Switched);
        assert_eq!(out.schema_pin, "mig-b@0.1.0");
        assert_eq!(out.migration_target, None);
        assert!(out.findings.is_empty());
        assert_eq!(
            engine.schema_pin("specs").unwrap().as_display(),
            "mig-b@0.1.0"
        );
        assert!(engine.migration_target("specs").is_none());
    }

    /// During migration every not-yet-repaired entity is
    /// non-conformant against the target, so `relations_unset` works
    /// on exactly those entities with no mode flag — and the same
    /// update can complete the entity's repair.
    #[test]
    fn relations_unset_works_during_migration_without_mode_flag() {
        let (_tmp, mut engine) = migration_engine();
        let one = crate::entity::EntityId::new("specs", "one");
        let two = crate::entity::EntityId::new("specs", "two");
        engine
            .relate_entity(
                crate::engine::RelateEntityArgs {
                    source: one.clone(),
                    expected_hash: None,
                    rel_type: "USES".to_string(),
                    target: two.clone(),
                    remove: false,
                    description: None,
                },
                crate::vcs::Actor::Cli,
                None,
                None,
            )
            .unwrap();
        // Conformant under the pin → the repair gate is shut.
        let shut = engine
            .update_entity(
                crate::engine::UpdateEntityArgs {
                    id: one.clone(),
                    expected_hash: None,
                    sections: indexmap::IndexMap::new(),
                    append_sections: indexmap::IndexMap::new(),
                    patch_sections: indexmap::IndexMap::new(),
                    metadata: indexmap::IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: vec![crate::ops::RelationUnsetArg {
                        rel_type: "USES".to_string(),
                        target: two.clone(),
                    }],
                },
                crate::vcs::Actor::Cli,
                None,
                None,
            )
            .unwrap_err();
        assert_eq!(shut.code(), "REPAIR_NOT_NEEDED");

        // Enter migration → `one` is now non-conformant against the
        // target; the same call opens, removes the relation, and the
        // bundled `status` set makes the entity integral-against-target.
        engine
            .set_vault_schema("specs", &sref("mig-b@0.1.0"))
            .unwrap();
        engine
            .update_entity(
                crate::engine::UpdateEntityArgs {
                    id: one.clone(),
                    expected_hash: None,
                    sections: indexmap::IndexMap::new(),
                    append_sections: indexmap::IndexMap::new(),
                    patch_sections: indexmap::IndexMap::new(),
                    metadata: indexmap::IndexMap::from_iter([(
                        "status".to_string(),
                        "open".to_string(),
                    )]),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: vec![crate::ops::RelationUnsetArg {
                        rel_type: "USES".to_string(),
                        target: two.clone(),
                    }],
                },
                crate::vcs::Actor::Cli,
                None,
                None,
            )
            .expect("repair-shaped update lands during migration without a flag");
        let entity = engine.store().get(&one).unwrap();
        assert!(entity.relationships.is_empty());
    }

    /// Boot honors a persisted in-flight migration: a mount carrying
    /// `migration_target` validates writes against the target from
    /// the first call of the new process — the resumability half of
    /// the dual-pin contract.
    #[test]
    fn boot_resumes_dual_pin_validation_against_target() {
        let (tmp, engine) = migration_engine();
        drop(engine);
        let schemas_dir = tmp.path().join("schemas");
        let vault_dir = tmp.path().join("vault");
        let writer = crate::storage::FilesystemVaultWriter::new(vault_dir.clone());
        let mut mount = folder_mount("specs", vault_dir);
        mount.schema = Some("mig-a@0.1.0".parse().unwrap());
        mount.migration_target = Some("mig-b@0.1.0".parse().unwrap());
        let engine = Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn crate::backend::VaultBackend>)],
            Some(&schemas_dir),
        )
        .unwrap();
        // Effective validation schema is the target...
        let (name, version) = {
            let s = engine.schema_for("specs").unwrap();
            let (n, v) = s.id();
            (n.to_string(), v.to_string())
        };
        assert_eq!((name.as_str(), version.as_str()), ("mig-b", "0.1.0"));
        // ...while the settled pin and the in-flight target read back
        // distinctly.
        assert_eq!(engine.schema_pin("specs").unwrap().as_display(), "mig-a@0.1.0");
        assert_eq!(
            engine.migration_target("specs").unwrap().as_display(),
            "mig-b@0.1.0"
        );
    }
}

