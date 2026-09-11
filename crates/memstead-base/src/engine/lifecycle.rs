//! Engine lifecycle — settings/workspace-root setters, runtime
//! mem add/remove, reload, and export.
//!
//! `register_writable_mem` / `unregister_writable_mem` are the
//! engine-level primitives the `memstead_mem_create` / `memstead_mem_delete`
//! handlers build on. `reload_one_mem*` re-reads a mount's backend
//! and refreshes the in-memory store; `reload_each_writable_mem*`
//! sweeps every writable mount. `export_markdown` regenerates entity
//! markdown for folder mounts; `export_mem` produces a portable
//! `.mem` archive via the backend-aware dispatch in
//! [`crate::ops::export`].

use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::backend::{BackendError, MemBackend};
use crate::engine_fallback_type;
use crate::entity::EntityId;
use crate::entity::generator::generate_markdown;
use crate::entity::loader::parse_entries;
use crate::entity::store_builder::push_entities_into_store;
use crate::mem::MemOrigin;
use crate::ops::WarningHint;
use crate::workspace::{Mount, MountStorage, WorkspaceSettings};

use super::boot::collect_source_entries;
use super::{BackendFactory, Engine, EngineError, GitBranchOps, MountedBackend};

/// What [`Engine::stage_sealed_schema`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaStaging {
    /// The pin already resolves in this workspace — nothing written.
    /// Re-installing a mem, and installing a second mem that pins the
    /// same schema, both land here.
    AlreadyResolvable,
    /// The package was written into the workspace's local schema
    /// storage and is resolvable in this process from now on.
    Staged,
    /// The archive carries no embedded schema tree. Nothing to stage;
    /// the pin must resolve on its own or the mount refuses.
    NoEmbeddedSchema,
    /// The workspace has no sealed-package storage (a folder-shaped
    /// workspace: its `.memstead/schemas/` is the authoring tier and
    /// never holds a third party's sealed bytes). The package was read
    /// and checked against the pin, nothing was written, and the mount
    /// resolves its vocabulary from the archive itself — on this shape
    /// the archive IS the schema storage.
    CarriedByArchive,
}

impl Engine {
    /// Replace the workspace-level settings. Called by
    /// [`Self::from_workspace_root`] (and the full counterpart) after
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

    /// Replace the backend factory. Full consumers call this once at boot
    /// (`engine_from_workspace_root`) to install
    /// `memstead_git_branch::storage::instantiate_full_backend` so the engine
    /// can materialise git-branch backends on top of folder + archive.
    /// Consumers without the git-branch crate leave the default in place.
    pub fn set_backend_factory(&mut self, factory: BackendFactory) {
        self.backend_factory = factory;
    }

    /// Insert into the engine's schema map, bumping the schemas epoch
    /// (the second half of the derived-memo key — flywheel W8/01):
    /// derived structures depend on schemas, so any change here must
    /// be visible to the memo-invalidation hooks even when the store
    /// generation did not move.
    pub(crate) fn schemas_insert(
        &mut self,
        mem: String,
        schema: std::sync::Arc<memstead_schema::Schema>,
    ) {
        self.schemas_epoch += 1;
        self.schemas.insert(mem, schema);
    }

    /// Remove from the engine's schema map, bumping the schemas epoch
    /// — see [`Self::schemas_insert`].
    pub(crate) fn schemas_remove(&mut self, mem: &str) {
        self.schemas_epoch += 1;
        self.schemas.remove(mem);
    }

    /// Install the unmounted-mem storage discovery hook (flywheel
    /// W7/02). Full boot sets it; without one, writes referencing
    /// unmounted mems keep the forward-reference mechanic unchanged.
    pub fn set_unmounted_storage_prober(&mut self, prober: super::UnmountedStorageProber) {
        self.unmounted_storage_prober = Some(prober);
    }

    /// Replace the mutation-timestamp clock — the source every
    /// engine-stamped metadata field (`init_timestamp` /
    /// `auto_timestamp` schema flags: `created_date`, `last_modified`)
    /// reads. A testing affordance for suites that assert over
    /// canonical entity bytes (e.g. cross-surface hash parity):
    /// pin both engines to the same constant and byte-level
    /// nondeterminism from wall-clock seconds disappears. Production
    /// code never calls this — the default installed at construction
    /// is the system clock, and the stamped format is unchanged
    /// either way.
    pub fn set_mutation_clock(&mut self, clock: crate::engine::MutationClock) {
        self.mutation_clock = clock;
    }

    /// Set the caller-declared role for subsequent mutations
    ///. The surface calls this before every
    /// mutation with the per-call parameter resolved against its
    /// session default (per-call wins); `Role::Unspecified` records
    /// as absence.
    pub fn set_role(&mut self, role: crate::vcs::Role) {
        self.current_role = role;
    }

    /// The currently declared role — what the next mutation records.
    pub fn current_role(&self) -> crate::vcs::Role {
        self.current_role
    }

    /// Set the caller-declared identity for subsequent mutations and
    /// checks. The surface calls this before
    /// every operation with the per-call parameter resolved against
    /// its session default (per-call wins); `None` records as
    /// absence. Callers pass an already-normalised value
    /// ([`crate::vcs::normalise_identity`]).
    pub fn set_identity(&mut self, identity: Option<String>) {
        self.current_identity = identity;
    }

    /// Set the transport's actor for the commits this session causes
    /// without a per-call actor (config writes, sync-state stamps, the
    /// anchor writers). The CLI sets `Cli` at boot; the MCP server
    /// sets `Agent`.
    pub fn set_actor(&mut self, actor: crate::vcs::Actor) {
        self.current_actor = actor;
    }

    /// The transport's actor (see [`Self::set_actor`]).
    pub fn current_actor(&self) -> crate::vcs::Actor {
        self.current_actor
    }

    /// Set the transport's client id, when it has one (the MCP client
    /// after `initialize`; the CLI's own id).
    pub fn set_client(&mut self, client: Option<crate::vcs::ClientId>) {
        self.current_client = client;
    }

    /// The transport's client id (see [`Self::set_client`]).
    pub fn current_client(&self) -> Option<&crate::vcs::ClientId> {
        self.current_client.as_ref()
    }

    /// The commit context of one mutation: its tool, actor, client and
    /// note, with the session's declared role and identity. Every
    /// commit the engine writes is built through here (or through
    /// [`Self::session_commit_context`]), so no path can drop a trailer.
    pub fn commit_context<'a>(
        &self,
        tool: Option<&'a str>,
        actor: crate::vcs::Actor,
        client: Option<crate::vcs::ClientId>,
        note: Option<String>,
    ) -> crate::vcs::CommitContext<'a> {
        crate::vcs::CommitContext::new(
            tool,
            actor,
            client,
            note,
            self.current_role,
            self.current_identity.clone(),
        )
    }

    /// The commit context of a mutation the session causes as itself:
    /// the transport's actor and client (see [`Self::set_actor`] and
    /// [`Self::set_client`]) with the session's role and identity.
    pub fn session_commit_context<'a>(
        &self,
        tool: Option<&'a str>,
        note: Option<String>,
    ) -> crate::vcs::CommitContext<'a> {
        self.commit_context(tool, self.current_actor, self.current_client.clone(), note)
    }

    /// The currently declared identity — what the next mutation or
    /// check records.
    pub fn current_identity(&self) -> Option<&str> {
        self.current_identity.as_deref()
    }

    /// Current mutation timestamp as the second-granularity ISO form
    /// the stamping paths write. Reads [`Self::mutation_clock`] — the
    /// system clock unless a test pinned it.
    pub(crate) fn now_iso(&self) -> String {
        crate::engine::mutation::iso_from_system_time((self.mutation_clock)())
    }

    /// Install the git-branch ops bundle. Full boot
    /// (`memstead_git_branch::engine_from_workspace_root`) calls this once
    /// at construction. Consumers without the git-branch crate leave it unset
    /// and the git-branch dispatch branches collapse to typed errors / empty
    /// reports; such an engine serves no git-branch mounts.
    pub fn set_git_branch_ops(&mut self, ops: GitBranchOps) {
        self.git_branch_ops = Some(ops);
    }

    /// Install a schema package onto the workspace's git-branch backend —
    /// the unified `__MEMSTEAD:schemas/<name>@<version>/` ref. `files`
    /// are `(relative-path, bytes)` pairs (`schema.yaml`,
    /// `types/<t>.yaml`, optional `mem-template.json`). Returns the
    /// resulting commit sha; idempotent at the storage layer (an
    /// identical re-install produces no new commit).
    ///
    /// Folder workspaces install schemas by writing under
    /// `<workspace>/.memstead/schemas/` directly; this is the git-branch
    /// path, where the engine owns the mem-repo and the write must
    /// route through it. Errors when no git-branch ops are wired
    /// or no git-branch mount exists to resolve the shared
    /// mem-repo gitdir from. The caller reloads (or restarts) to pick
    /// the new schema into the resolution catalogue.
    pub fn install_schema(
        &self,
        name: &str,
        version: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<String, EngineError> {
        // Validation gate: the engine refuses to seal a package that the
        // loader would reject or whose section headings cannot round-trip
        // to their keys. Install time is the last moment the author can
        // act — sealed schemas keep loading even when a later rule would
        // refuse them, so nothing invalid may pass this point.
        Self::validate_schema_package(name, version, files)?;
        // Resolve the shared mem-repo gitdir: prefer a live git-branch
        // mount's gitdir (authoritative — that is where the engine reads
        // schemas from), falling back to the workspace's `mem-repo/.git`
        // so a schema can be installed into an empty mem-repo *before*
        // any mem pins it.
        let gitdir = self
            .mounts
            .iter()
            .find_map(|m| match &m.mount.storage {
                crate::workspace::MountStorage::GitBranch { gitdir, .. } => Some(gitdir.clone()),
                _ => None,
            })
            .or_else(|| {
                self.workspace_root()
                    .map(|r| r.join("mem-repo").join(".git"))
            })
            .ok_or_else(|| {
                EngineError::Mem(
                    "schema install requires a mem-repo workspace (no git-branch mount and \
                     no workspace root to resolve the mem-repo gitdir)"
                        .to_string(),
                )
            })?;
        let ops = self.git_branch_ops.as_ref().ok_or_else(|| {
            EngineError::Mem("git-branch ops are not wired on this engine".to_string())
        })?;
        // Seal the package AS-GIVEN: the format marker is a generation
        // claim only the resolver can make (an authored directory source
        // arrives already stamped; a legacy builtin or sealed source
        // arrives unmarked, and absence IS its legacy claim). Stamping
        // here mis-labelled legacy content as current and flipped every
        // bare field's meaning in the sealed copy — the manufacturing
        // defect an earlier plan removed.
        (ops.write_schema)(&gitdir, name, version, files).map_err(EngineError::Backend)
    }

    /// Make a **sealed third-party** schema package resolvable in this
    /// workspace — the install-time half of "a published mem installs
    /// on the strength of the schema it carries".
    ///
    /// The package (package-relative files: `schema.yaml`,
    /// `types/<t>.yaml`, `schema-format.json`) is written into the
    /// workspace's local schema storage, the same storage
    /// [`Self::install_schema`] writes to and the pin resolver reads —
    /// so the mount that follows resolves without a fourth mechanism
    /// and a second mem pinning the same schema finds it already
    /// there. The loaded schema also lands in this engine's live
    /// catalogue, so the caller mounts in the same process without a
    /// reload.
    ///
    /// Two things it deliberately does NOT do. It does not run the
    /// authoring gate ([`Self::validate_schema_package`]): these are a
    /// third party's sealed bytes, the installing user cannot fix
    /// them, and the archive validator has already admitted them under
    /// the sealed reading — applying the authoring gate here would
    /// make an archive that is valid to publish invalid to install.
    /// And it does not append the format marker: presence of the
    /// marker IS the package's metadata-polarity generation, so
    /// injecting one would rewrite the meaning of bytes the publisher
    /// sealed.
    ///
    /// Idempotent: a pin the engine can already resolve returns
    /// [`SchemaStaging::AlreadyResolvable`] with nothing written.
    ///
    /// Shape-agnostic: on a workspace with no sealed-package storage (the
    /// folder shape) the package is still read and checked against the
    /// pin, so a broken embedded schema refuses before any mount side
    /// effect, but nothing is written — the archive-backed mount that
    /// follows resolves the vocabulary from the archive itself
    /// ([`SchemaStaging::CarriedByArchive`]). What that shape forgoes is
    /// the staged copy's extras: `memstead schema <pin>` rendering the
    /// installed package, and a second, writable mem pinning it.
    pub fn stage_sealed_schema(
        &mut self,
        mem: &str,
        pin: &memstead_schema::SchemaRef,
        files: &[(String, Vec<u8>)],
    ) -> Result<SchemaStaging, EngineError> {
        let catalogue: Vec<std::sync::Arc<memstead_schema::Schema>> = self
            .workspace_schemas
            .iter()
            .chain(self.builtin_schemas.iter())
            .cloned()
            .collect();
        if crate::engine::SchemaResolver::new(&catalogue)
            .resolve(pin)
            .is_ok()
        {
            return Ok(SchemaStaging::AlreadyResolvable);
        }
        if files.is_empty() {
            // Nothing embedded to stage. The caller's mount attempt
            // reports the unresolved pin with its own source trail —
            // that IS the actionable error here.
            return Ok(SchemaStaging::NoEmbeddedSchema);
        }

        let unloadable = |reason: String| EngineError::EmbeddedSchemaInvalid {
            mem: mem.to_string(),
            pin: pin.as_display(),
            reason,
        };
        // Read it exactly as the local schema source will read it back
        // — one sealed reader, so admission and re-read cannot diverge.
        let schema =
            memstead_schema::load_sealed_package(files).map_err(|e| unloadable(e.to_string()))?;
        let (name, version) = schema.id();
        if name != pin.name || version != pin.version {
            return Err(unloadable(format!(
                "the package declares '{name}@{version}' but the mem pins {}",
                pin.as_display()
            )));
        }

        if self.sealed_schema_gitdir().is_none() {
            return Ok(SchemaStaging::CarriedByArchive);
        }
        self.write_to_local_schema_source(&name, &version.to_string(), files)?;
        self.workspace_schemas.push(std::sync::Arc::new(schema));
        Ok(SchemaStaging::Staged)
    }

    /// The gitdir that holds sealed third-party packages (the
    /// `__MEMSTEAD:schemas/` ref): the first git-branch mount's, else the
    /// workspace's `mem-repo/.git`. `None` on a folder-shaped workspace,
    /// whose `.memstead/schemas/` is the authoring tier and never holds
    /// sealed bytes.
    fn sealed_schema_gitdir(&self) -> Option<PathBuf> {
        self.mounts
            .iter()
            .find_map(|m| match &m.mount.storage {
                crate::workspace::MountStorage::GitBranch { gitdir, .. } => Some(gitdir.clone()),
                _ => None,
            })
            .or_else(|| {
                self.workspace_root()
                    .map(|r| r.join("mem-repo").join(".git"))
            })
            .filter(|g| g.is_dir())
    }

    /// Write a schema package into the workspace's local schema
    /// storage — the git-branch `__MEMSTEAD:schemas/` ref when the
    /// workspace is mem-repo-shaped. Shares
    /// [`Self::install_schema`]'s gitdir resolution so authored and
    /// staged packages land in one place.
    fn write_to_local_schema_source(
        &self,
        name: &str,
        version: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), EngineError> {
        let gitdir = self.sealed_schema_gitdir().ok_or_else(|| {
            EngineError::Mem(
                "staging a schema requires a mem-repo workspace — the folder workspace's \
                     `.memstead/schemas/` is the authoring tier and never holds sealed \
                     third-party packages"
                    .to_string(),
            )
        })?;
        let ops = self.git_branch_ops.as_ref().ok_or_else(|| {
            EngineError::Mem("git-branch ops are not wired on this engine".to_string())
        })?;
        (ops.write_schema)(&gitdir, name, version, files).map_err(EngineError::Backend)?;
        Ok(())
    }

    /// Validate a schema package's files before they are sealed. Runs
    /// the full loader (structural + semantic) plus the section-heading
    /// round-trip gate, and checks the manifest's declared identity
    /// matches the `(name, version)` the package is being installed
    /// under — a mismatch would seal the schema under a ref its own
    /// manifest contradicts.
    ///
    /// `pub` so the below-boot install path (memstead-git-branch's
    /// repair surface) runs the SAME gate as this booted path — the
    /// two must never fork into separate validation regimes.
    pub fn validate_schema_package(
        name: &str,
        version: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), EngineError> {
        let invalid = |message: String| EngineError::SchemaPackageInvalid {
            name: name.to_string(),
            version: version.to_string(),
            message,
        };
        let manifest_yaml = files
            .iter()
            .find(|(rel, _)| rel == "schema.yaml")
            .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
            .ok_or_else(|| invalid("package has no schema.yaml".to_string()))?;
        let types: Vec<(String, String)> = files
            .iter()
            .filter_map(|(rel, bytes)| {
                rel.strip_prefix("types/")
                    .and_then(|f| f.strip_suffix(".yaml"))
                    .map(|stem| {
                        (
                            stem.to_string(),
                            String::from_utf8_lossy(bytes).into_owned(),
                        )
                    })
            })
            .collect();
        // Install validates the CURRENT language (the author acts
        // now) — the retired `optional:` key refuses here, and absent
        // required keys mean optional.
        let schema = memstead_schema::load_schema_from_memory_with_format(
            &manifest_yaml,
            &types,
            memstead_schema::loader::MetadataPolarityFormat::RequiredOptIn,
        )
        .map_err(|e| invalid(e.to_string()))?;
        memstead_schema::check_section_heading_roundtrip(&schema)
            .map_err(|e| invalid(e.to_string()))?;
        memstead_schema::check_reserved_metadata_keys(&schema)
            .map_err(|e| invalid(e.to_string()))?;
        memstead_schema::check_section_formats(&schema).map_err(|e| invalid(e.to_string()))?;
        let (declared_name, declared_version) =
            (schema.manifest.name.as_str(), schema.version.to_string());
        if declared_name != name || declared_version != version {
            return Err(invalid(format!(
                "manifest declares '{declared_name}@{declared_version}' but the package is \
                 being installed as '{name}@{version}'"
            )));
        }
        Self::validate_schema_exemplars(&std::sync::Arc::new(schema)).map_err(invalid)?;
        Ok(())
    }

    /// Validate every type exemplar a schema carries by running it
    /// through the REAL create validation stage:
    /// an in-memory engine is booted with the candidate schema pinned
    /// on a virtual mem, and each exemplar is submitted as a
    /// `dry_run` create — the same gates a real write runs (sections,
    /// metadata + enums, rel-type vocabulary, edge shape, description
    /// posture), commit-free by construction. Placeholder relation
    /// targets are bare slugs scoped to the virtual mem, so target
    /// existence is never checked (an absent target is the legal
    /// would-be-stub path).
    ///
    /// Returns the defect as a message naming the type — the caller
    /// wraps it in its own typed envelope (`SchemaPackageInvalid` on
    /// the install path). There is deliberately no warn-and-carry
    /// mode: a non-conformant exemplar refuses, because the whole
    /// value of an exemplar is the impossibility of drift.
    ///
    /// `pub` so the built-in suite gates every shipped exemplar
    /// through the SAME validator (a broken built-in exemplar fails
    /// CI), and the below-boot install path shares the gate via
    /// [`Self::validate_schema_package`].
    pub fn validate_schema_exemplars(
        schema: &std::sync::Arc<memstead_schema::Schema>,
    ) -> Result<(), String> {
        let with_exemplars: Vec<&str> = schema
            .manifest
            .types
            .iter()
            .filter(|t| {
                schema
                    .types
                    .get(t.as_str())
                    .is_some_and(|td| td.exemplar.is_some())
            })
            .map(String::as_str)
            .collect();
        if with_exemplars.is_empty() {
            return Ok(());
        }

        let (name, version) = schema.id();
        let mem = "exemplar";
        let mount = crate::workspace::Mount {
            mem: mem.to_string(),
            schema: Some(memstead_schema::SchemaRef::new(name, version)),
            storage: crate::workspace::MountStorage::InMemory,
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let backend = Box::new(crate::storage::InMemoryBackend::new()) as Box<dyn MemBackend>;
        let mut engine = Engine::from_mounts_with_schemas_dir_and_extra(
            vec![(mount, backend)],
            None,
            vec![schema.clone()],
        )
        .map_err(|e| format!("exemplar validation could not boot: {e}"))?;

        for type_name in with_exemplars {
            let td = schema
                .types
                .get(type_name)
                .expect("filtered on presence above");
            let ex = td.exemplar.as_ref().expect("filtered on presence above");
            let mut relations = Vec::with_capacity(ex.relations.len());
            for r in &ex.relations {
                let target = r.target_slug();
                if target.contains("--") || target.trim().is_empty() {
                    return Err(format!(
                        "type '{type_name}' exemplar relation target '{target}' must be a bare \
                         placeholder slug (no `--`, non-empty) — exemplars live outside \
                         any mem",
                    ));
                }
                relations.push(crate::ops::RelateArg {
                    target: crate::entity::EntityId::new(mem, target),
                    rel_type: r.rel_type_name().to_string(),
                    description: r.description.clone(),
                });
            }
            let args = crate::engine::CreateEntityArgs {
                anchors: Vec::new(),
                mem: mem.to_string(),
                title: ex.title.clone(),
                entity_type: type_name.to_string(),
                sections: ex.sections.clone(),
                metadata: ex.metadata.clone(),
                relations,
                dry_run: true,
            };
            if let Err(e) = engine.create_entity(args, crate::vcs::Actor::Cli, None, None) {
                return Err(format!(
                    "type '{type_name}' exemplar does not conform: [{}] {e}",
                    e.code()
                ));
            }
        }
        Ok(())
    }
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
        builtin_schemas.extend(super::boot::embedded_archive_schemas(&mount));
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
        if let Some(w) =
            super::boot::unbacked_mount_warning(&mount, backend.as_ref(), Some(entries.len()))
        {
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
    /// Set a mem's schema pin — the conformance-gated schema-migration
    /// trigger. Behaviour per the pinned contract:
    ///
    /// - requested == current pin → `Noop`, no state change.
    /// - requested != pin, mem integral against the target →
    ///   atomic switch (`schema_pin = target`, migration state
    ///   cleared) in one workspace-store write → `Switched`.
    /// - requested != pin, mem NOT integral → enter (or stay in)
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
    pub fn set_mem_schema(
        &mut self,
        mem: &str,
        target: &memstead_schema::SchemaRef,
    ) -> Result<crate::engine::SetSchemaOutcome, EngineError> {
        use crate::engine::{SetSchemaOutcome, SetSchemaResult};
        // A quarantined mem's set-schema IS the repair path (an
        // unresolvable pin is the commonest quarantine cause): repin
        // the retained mount, then re-attempt the attach — the same
        // in-process recovery `reload` performs after an external
        // repair. Ordinary mounts proceed below unchanged.
        if self.quarantine_reason(mem).is_some() {
            return self.set_schema_on_quarantined(mem, target);
        }
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| self.unknown_mem_error(mem))?;
        // Capability gate, identical in shape and position to the six
        // sibling setters (`set_mem_version` … `set_mem_sync_state`):
        // a schema-pin change starts a migration — the one lifecycle
        // mutation a read-only mount must be able to refuse like any
        // other.
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem.to_string()));
        }

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
                mem: mem.to_string(),
                pin: target.as_display(),
                sources: crate::engine::error::SchemaSourceDiagnostic::for_failed_pin(
                    &target.name,
                    &target.version,
                    &consulted,
                ),
                install_hint: None,
            }
            .with_schema_install_probe(self.workspace_root())
        })?;

        // `Mount.schema` is now the optional assertion; for a mem the
        // operator is actively re-pinning it is normally `Some` (and kept
        // in sync with the config by the switch below). `<unset>` covers a
        // mount that carried no assertion.
        let current_pin = self.mounts[mount_idx].mount.schema.clone();
        let current_pin_display = current_pin
            .as_ref()
            .map(|p| p.as_display())
            .unwrap_or_else(|| "<unset>".to_string());
        let in_flight = self.mounts[mount_idx].mount.migration_target.clone();

        // The noop question is asked of the pin the engine actually
        // serves (the loaded schema, resolved from the authoritative
        // backend config at boot), never of the mount's expectation
        // alone. With a `SCHEMA_PIN_MISMATCH` (mount says 0.4.0, config
        // says 0.1.0) the old comparison against the mount answered
        // "noop" for a target of 0.4.0 and the config stayed at 0.1.0
        // forever: the one command meant to repair the mismatch could
        // not. When the served pin already equals the target and only
        // the mount expectation lags, the expectation is aligned in
        // place and reported as switched.
        let served_pin: Option<memstead_schema::SchemaRef> = self.schemas.get(mem).map(|s| {
            let (name, version) = s.id();
            memstead_schema::SchemaRef::new(name, version)
        });
        // During a dual-pin migration the served schema IS the target
        // (writes validate against it) while the config still pins the
        // old generation, so the shortcut applies only with no migration
        // in flight; an in-flight target falls through to the
        // conformance gate that completes it.
        if served_pin.as_ref() == Some(target) && in_flight.is_none() {
            if current_pin.as_ref() == Some(target) {
                return Ok(SetSchemaOutcome {
                    mem: mem.to_string(),
                    schema_pin: current_pin_display,
                    migration_target: in_flight.map(|t| t.as_display()),
                    outcome: SetSchemaResult::Noop,
                    findings: Vec::new(),
                    stamped_schema: self.stamped_schema_of(mount_idx),
                });
            }
            self.mounts[mount_idx].mount.schema = Some(target.clone());
            self.mounts[mount_idx].mount.migration_target = None;
            self.persist_state()?;
            return Ok(SetSchemaOutcome {
                mem: mem.to_string(),
                schema_pin: target.as_display(),
                migration_target: None,
                outcome: SetSchemaResult::Switched,
                findings: Vec::new(),
                stamped_schema: self.stamped_schema_of(mount_idx),
            });
        }

        // Conformance gate against the requested target. The full
        // integrity definition includes the consistency axis, but the
        // schema-switch gate is conformance: consistency breaks are
        // schema-independent (they neither block nor are caused by a
        // pin change) and keep their always-available repair paths.
        let findings = crate::ops::integrity::conformance_findings(
            &self.store,
            mem,
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
            // config-present mem.
            self.persist_mem_schema_pin(mount_idx, target)?;
            self.mounts[mount_idx].mount.schema = Some(target.clone());
            self.mounts[mount_idx].mount.migration_target = None;
            self.schemas_insert(mem.to_string(), target_schema);
            self.invalidate_communities();
            // The index field set derives from the pinned schema — the
            // schema-switch staleness the whole-map drop used to mask:
            // this mem's index must be
            // rebuilt against the new field set.
            self.invalidate_search_indexes();
            self.persist_state()?;
            // The completed migration re-stamps the mutation stamp (the
            // marker `ENGINE_VERSION_SKEW` reads) with the generation
            // the mem now sits on. Without this the marker kept naming
            // the old pin until the next entity write, and an agent
            // reading it after the migration re-derived from a stale
            // generation (B5 grader finding, 2026-09-02). The stamp
            // writer's equality guard keeps a no-move write from
            // happening; its warnings ride entity mutations and have no
            // channel on this outcome, the same standing as the sweep.
            let _stamp_warnings_have_no_channel_here = self.stamp_mutation_versions(mount_idx);
            return Ok(SetSchemaOutcome {
                mem: mem.to_string(),
                schema_pin: target.as_display(),
                migration_target: None,
                outcome: SetSchemaResult::Switched,
                findings: Vec::new(),
                stamped_schema: self.stamped_schema_of(mount_idx),
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
        self.schemas_insert(mem.to_string(), target_schema);
        self.invalidate_communities();
        // Same field-set dependency as the atomic-switch branch above.
        self.invalidate_search_indexes();
        self.persist_state()?;
        // A dual-pin entry never moves the marker: the mem still sits
        // on the old generation until every entity is integral, and
        // the outcome says which generation the marker carries.
        Ok(SetSchemaOutcome {
            mem: mem.to_string(),
            schema_pin: current_pin_display,
            migration_target: Some(target.as_display()),
            outcome,
            findings,
            stamped_schema: self.stamped_schema_of(mount_idx),
        })
    }

    /// The resolved schema the mem's mutation stamp names, from the
    /// engine's cached config (kept current by the shared config
    /// writer), or `None` when the mem carries no stamp.
    fn stamped_schema_of(&self, mount_idx: usize) -> Option<String> {
        self.mounts
            .get(mount_idx)
            .and_then(|m| m.mem_config.as_ref())
            .and_then(|c| c.mutation_stamp.as_ref())
            .map(|st| st.schema.clone())
    }

    /// Persist a mem's new schema pin into the authoritative backend
    /// config (`.memstead/config.json` for folder, the `__MEMSTEAD`
    /// mem-config blob for git-branch).
    ///
    /// Boot resolution treats the backend config as the authoritative
    /// settled pin and `Mount.schema` (the `mounts.json` copy) as a
    /// cross-checked assertion. A schema switch that updated only
    /// `mounts.json` would therefore be silently reverted on the next
    /// process boot — the config still names the old pin. This keeps the
    /// authoritative home in sync at switch time.
    ///
    /// Value-level field bump: only the `"schema"` string is rewritten;
    /// every other config field (`readMems`, write guidance, …) is
    /// preserved verbatim. Config-absent mems (no `config.json`) keep
    /// `Mount.schema` as their settled pin, so there is nothing to update
    /// — a clean no-op.
    /// **The one way this engine writes a mem config.** Every config-writing
    /// operation goes through it; none serializes its cached struct
    /// (consistency-sweep 04/03, criteria 1 and 2).
    ///
    /// The defect it removes: a long-lived MCP server reads each mem's config
    /// once at boot and holds it for days. Eight operations used to clone that
    /// cached struct, set one field, and write the whole thing back, so
    /// anything a sibling process changed in between was gone. The
    /// reload-before-operation invariant did not save them, because the
    /// staleness probe watches the entity branch (git-branch) or the change
    /// log (folder) and a config-only write advances neither.
    ///
    /// What this does instead is what the schema-pin writer next door already
    /// did: read the config the backend HAS, mutate the one field on that
    /// JSON, write it back. A field this call did not set cannot be reverted,
    /// because this call never had an opinion about it.
    ///
    /// `apply` receives the freshly-read config, PARSED. It deliberately does
    /// not receive raw JSON: an earlier draft handed out the `Value` and each
    /// closure wrote its own key name, which put `review_mark` in the file
    /// where the struct reads `reviewMark` and lost the field on the next
    /// read. Serde owns the wire names; the closures must not restate them.
    /// Unknown fields survive because `MemConfig` flattens them into `extra`.
    ///
    /// `apply` runs again on a re-read if the file moved between the read and
    /// the write, so it must be a pure function of the config it is handed,
    /// never of the engine's cache.
    ///
    /// Returns the parsed new config plus, when the stored config had moved on
    /// from what this engine last observed, the fields the intervening writer
    /// had changed. The caller rides that on its own response as
    /// `CONFIG_WRITE_INTERVENED`.
    pub(crate) fn write_mem_config_merged(
        &mut self,
        mount_idx: usize,
        mem_name: &str,
        tool: &'static str,
        note: Option<&str>,
        apply: &dyn Fn(&mut memstead_schema::config::MemConfig),
    ) -> Result<(memstead_schema::config::MemConfig, Vec<String>), EngineError> {
        // The config-write commit carries the operation's provenance
        // like every other commit: the tool that caused it, the
        // session's actor, client, role and identity, and the note.
        let ctx = self.session_commit_context(Some(tool), note.map(String::from));
        let backend = self.mounts[mount_idx].backend.as_ref();
        let read = |b: &dyn crate::backend::MemBackend| -> Result<Vec<u8>, EngineError> {
            b.read_mem_config()
                .map_err(|e| EngineError::Mem(format!("read mem config for update: {e}")))?
                .ok_or_else(|| {
                    EngineError::InvalidInput(format!(
                        "mem '{mem_name}' has no stored MemConfig (initialize the mem via \
                         `memstead init` or `memstead mem create` first)"
                    ))
                })
        };

        let stored = read(backend)?;
        // What the intervening writer changed, if anyone did. Compared against
        // the engine's cached copy, never against a clock: the rule forbids
        // reacting to cache age, and a single-writer workspace has an
        // identical cache, so this is empty there.
        let intervened = match self.mounts[mount_idx].mem_config.as_ref() {
            Some(cached) => changed_config_fields(cached, &stored),
            None => Vec::new(),
        };

        let render =
            |raw: &[u8]| -> Result<(memstead_schema::config::MemConfig, Vec<u8>), EngineError> {
                let value: serde_json::Value = serde_json::from_slice(raw)
                    .map_err(|e| EngineError::Mem(format!("parse mem config for update: {e}")))?;
                let mut cfg = memstead_schema::config::parse_mem_config(&value)
                    .map_err(|e| EngineError::Mem(format!("parse mem config for update: {e}")))?;
                apply(&mut cfg);
                let mut bytes = serde_json::to_vec_pretty(&cfg)
                    .map_err(|e| EngineError::Mem(format!("serialize mem config: {e}")))?;
                bytes.push(b'\n');
                Ok((cfg, bytes))
            };
        let (mut parsed, mut bytes) = render(&stored)?;

        // Compare-and-set, done by the backend so the check and the write are
        // one step. An earlier draft re-read here and then wrote, which is
        // check-then-write and leaves exactly the lost-update window
        // open. On a mismatch the loop re-reads, re-applies onto what is
        // there, and retries: the intervening writer's change is merged, never
        // overwritten. Bounded, because an unbounded retry against a hot
        // writer is a hang, and reaching the bound is a real contention
        // problem the caller should hear about rather than a state to spin in.
        let mut expected = stored;
        for attempt in 0..8 {
            let wrote = self.mounts[mount_idx].backend.write_mem_config_cas(
                Some(&expected),
                &bytes,
                &ctx,
            )?;
            if wrote {
                break;
            }
            if attempt == 7 {
                return Err(EngineError::Mem(format!(
                    "mem '{mem_name}' config is being written concurrently: eight \
                     compare-and-set attempts all lost the race. Retry, or find the \
                     writer that is not backing off."
                )));
            }
            expected = read(self.mounts[mount_idx].backend.as_ref())?;
            let rendered = render(&expected)?;
            parsed = rendered.0;
            bytes = rendered.1;
        }

        let mounted = &mut self.mounts[mount_idx];
        mounted.mem_config = Some(parsed.clone());
        // Refresh the head cursor so the next drift probe does not surface
        // MEM_RELOADED for the commit this call just produced.
        if let Some(sha) = mounted.backend.current_head().ok().flatten() {
            mounted.last_known_head = Some(sha);
        }
        Ok((parsed, intervened))
    }

    fn persist_mem_schema_pin(
        &mut self,
        mount_idx: usize,
        target: &memstead_schema::SchemaRef,
    ) -> Result<(), EngineError> {
        let ctx = self.session_commit_context(Some("set_mem_schema"), None);
        let value = bump_backend_schema_pin(self.mounts[mount_idx].backend.as_ref(), target, &ctx)?;
        // Refresh the cached parsed config so in-session reads observe the
        // new pin without a reload.
        if let Some(value) = value
            && let Ok(cfg) = memstead_schema::config::parse_mem_config(&value)
        {
            self.mounts[mount_idx].mem_config = Some(cfg);
        }
        Ok(())
    }

    /// Regenerate entity markdown files from the in-memory store.
    ///
    /// Dispatch:
    /// - When `mem_filter` is `Some(name)`, only that mem's mount
    ///   is considered. If its active backend doesn't support markdown
    ///   regeneration in place (today: anything other than
    ///   `MountStorage::Folder`), the call refuses with
    ///   [`EngineError::MarkdownExportUnsupportedBackend`] carrying
    ///   the active backend's id and the supported-backend list.
    /// - When `mem_filter` is `None`, every mount is iterated.
    ///   Folder mounts regenerate as today; non-folder mounts are
    ///   recorded in [`crate::ops::ExportResult::skipped_mounts`] so
    ///   the caller can surface the partial-success shape.
    ///
    /// Per-folder-mount behaviour: iterate the store, regenerate each
    /// non-stub entity belonging to the mount's mem, compare to the
    /// on-disk file, write if changed.
    ///
    /// `schema_filter` narrows the per-entity-type subset: when
    /// `Some(name)`, only entities whose `entity_type` matches are
    /// regenerated. `None` exports every type.
    ///
    /// Pre-fix this returned
    /// `ExportResult { written: 0, unchanged: 0 }` for git-branch /
    /// archive mounts — a successful-looking no-op that masked the
    /// backend-incompatibility. The typed refusal (per-mem) and the
    /// `skipped_mounts` channel (workspace-wide) give the caller an
    /// agent-actionable signal in one round-trip.
    pub fn export_markdown(
        &self,
        mem_filter: Option<&str>,
        schema_filter: Option<&str>,
    ) -> Result<crate::ops::ExportResult, EngineError> {
        use crate::workspace::MountStorage;
        let fallback = engine_fallback_type();
        let supported_backends = vec!["folder".to_string()];

        if let Some(name) = mem_filter {
            let mount = self
                .mounts
                .iter()
                .find(|m| m.mount.mem == name)
                .ok_or_else(|| self.unknown_mem_error(name))?;
            if !matches!(mount.mount.storage, MountStorage::Folder { .. }) {
                return Err(EngineError::MarkdownExportUnsupportedBackend {
                    mem: name.to_string(),
                    active_backend: mount.mount.storage.backend_id().to_string(),
                    supported_backends,
                });
            }
        }

        let mut total_written = 0;
        let mut refused: Vec<crate::ops::RefusedEntity> = Vec::new();
        let mut total_unchanged = 0;
        let mut skipped_mounts: Vec<crate::ops::SkippedMount> = Vec::new();

        for mount in &self.mounts {
            let mem_name = mount.mount.mem.as_str();
            if let Some(filter) = mem_filter
                && mem_name != filter
            {
                continue;
            }
            let MountStorage::Folder { path: mem_dir } = &mount.mount.storage else {
                skipped_mounts.push(crate::ops::SkippedMount {
                    mem: mem_name.to_string(),
                    active_backend: mount.mount.storage.backend_id().to_string(),
                    reason: "backend_does_not_support_markdown_export".to_string(),
                });
                continue;
            };
            let schema = match self.schemas.get(mem_name) {
                Some(s) => s,
                None => continue,
            };

            for entity in self.store.all_entities() {
                if entity.stub || entity.file_path.is_empty() {
                    continue;
                }
                if entity.id.mem() != mem_name {
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

                let full_path = mem_dir.join(&entity.file_path);
                let needs_write = match std::fs::read_to_string(&full_path) {
                    Ok(existing) => existing != generated,
                    Err(_) => true,
                };
                if needs_write {
                    match crate::entity::writer::write_entity(entity, mem_dir, type_def.as_ref()) {
                        Ok(_) => total_written += 1,
                        // The one refusal the export must not swallow: writing
                        // this entity would bury the sections its open fence
                        // absorbed. Name it and carry on — an export that
                        // aborted here would strand every other entity.
                        Err(e @ crate::entity::writer::WriteError::UnterminatedFence { .. }) => {
                            refused.push(crate::ops::RefusedEntity {
                                id: entity.id.to_string(),
                                reason: "UNTERMINATED_FENCE_IN_STORED_BODY".to_string(),
                                detail: e.to_string(),
                            });
                        }
                        Err(_) => {}
                    }
                } else {
                    total_unchanged += 1;
                }
            }
        }

        Ok(crate::ops::ExportResult {
            refused_entities: refused,
            written: total_written,
            unchanged: total_unchanged,
            skipped_mounts,
        })
    }

    /// Export a mem as a portable `.mem` archive.
    ///
    /// Dispatch is internal: the engine looks up the mount whose mem
    /// name matches and branches on its `MountStorage`. Folder mounts
    /// produce a snapshot archive (current `.md` files + config);
    /// git-branch mounts invoke the registered [`GitBranchOps::export`]
    /// hook to produce a history archive (the per-mem branch tip's
    /// tree); archive mounts reject with `BackendError::Sealed`
    /// (already-an-archive — no meaningful re-export).
    ///
    /// The mem's `MemConfig` is looked up via
    /// [`Self::mem_config_for`]; unloaded configs (folder mounts
    /// without a `.memstead/config.json`, git-branch mounts without a
    /// `__MEMSTEAD:mems/<mem>/config.json`) surface as
    /// `EngineError::InvalidInput`. Workspace-level schema dir is
    /// threaded from `self.settings.schemas_dir` for the
    /// schema-source resolution chain.
    /// Resolve the mem's pinned schema from the workspace's
    /// `__MEMSTEAD:schemas/` ref (git-branch schema store) for the
    /// export paths of NON-git-branch mounts. `None` when the git-branch
    /// ops are not wired, the workspace has no mem-repo, or the
    /// package is not on the ref — callers then fall through to the
    /// disk/builtin chain unchanged. Without this, a folder mem whose
    /// schema `memstead schema install` sealed on the ref LOADS but
    /// cannot EXPORT: the loader and the archive assembler must read
    /// the same store.
    pub(crate) fn ref_schema_source_for(
        &self,
        config: &memstead_schema::MemConfig,
    ) -> Option<Vec<memstead_schema::SchemaSourceFile>> {
        let ops = self.git_branch_ops.as_ref()?;
        let root = self.workspace_root.as_deref()?;
        let pin = config.schema.as_ref()?;
        (ops.collect_ref_schema_source)(root, pin).ok().flatten()
    }

    pub fn export_mem(
        &self,
        mem_name: &str,
        output_path: &std::path::Path,
    ) -> Result<crate::ops::MemExportResult, EngineError> {
        let mount = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        let config = self.mem_config_for(mem_name).ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "mem '{mem_name}' has no loaded MemConfig — cannot export"
            ))
        })?;
        // F1: surface the missing-version case as a typed
        // `MEM_CONFIG_INCOMPLETE` envelope with structured recovery
        // details, rather than letting it bubble through as the
        // backend's `INTERNAL` collapse pointing at the wrong path
        // (`.memstead/config.json` is the folder-backend layout — the
        // mem-repo backend keeps the blob under `__MEMSTEAD:mems/`).
        // The check fires for both backends symmetrically.
        if config.version.is_none() {
            return Err(EngineError::MemConfigIncomplete {
                mem: mem_name.to_string(),
                missing_fields: vec!["version".to_string()],
            });
        }
        // Collected before the backend split so both storage kinds report
        // it. `install` refuses the archive for each of these; naming them at
        // export time is the same courtesy the dangling cross-mem edges get,
        // and for the same reason: an operator who learns at install time has
        // already shared the archive.
        let fenced: Vec<String> = self
            .store
            .all_entities()
            .filter(|e| e.mem == mem_name && !e.stub)
            .filter(|e| {
                e.sections
                    .values()
                    .any(|v| crate::markdown::closing_fence_if_unterminated(v.trim()).is_some())
            })
            .map(|e| e.id.to_string())
            .collect();
        let workspace_root = self.workspace_root.as_deref();
        // Authored schemas live at the fixed `<workspace>/.memstead/schemas/`
        // location (the `schemas_dir` key is retired). Absent dir → the
        // schema-source chain falls through to cache/built-in, as before.
        let fixed_schemas_dir = workspace_root.map(|r| r.join(".memstead").join("schemas"));
        let workspace_schemas_dir = fixed_schemas_dir.as_deref();
        let exported = match &mount.mount.storage {
            MountStorage::Folder { path } => crate::ops::export::export_mem(
                path,
                config,
                output_path,
                workspace_root,
                workspace_schemas_dir,
                self.ref_schema_source_for(config),
            )
            .map_err(|e| EngineError::Backend(BackendError::Other(format!("export_mem: {e}")))),
            MountStorage::GitBranch { gitdir, branch } => {
                let hook = self.git_branch_ops.as_ref().ok_or_else(|| {
                    EngineError::Backend(BackendError::Other(
                        "git-branch export hook not installed (git-branch ops not wired)"
                            .to_string(),
                    ))
                })?;
                // Source per-entity provenance from the git-branch mutation
                // log (commit trailers) and hand the serialised payload to
                // the export hook to embed — symmetric with the bytes path.
                let entity_paths = crate::ops::export::entity_paths_of(
                    &mount
                        .backend
                        .list_entities()
                        .map_err(EngineError::Backend)?,
                );
                let records = mount.backend.read_provenance(None).unwrap_or_default();
                let (provenance, redactions) =
                    crate::ops::export::build_redacted_archive_provenance(&records, &entity_paths);
                let provenance_bytes = provenance.to_archive_bytes().ok();
                // Source the anchors sidecar from the branch tip — symmetric
                // with the bytes-export path so the disk `.mem` carries anchors.
                let anchors_bytes = mount.backend.read_anchors_sidecar().ok().flatten();
                (hook.export)(
                    gitdir,
                    branch,
                    mem_name,
                    config,
                    output_path,
                    workspace_root,
                    workspace_schemas_dir,
                    provenance_bytes.as_deref(),
                    anchors_bytes.as_deref(),
                )
                .map(|mut r| {
                    r.redactions = redactions;
                    r
                })
                .map_err(EngineError::Backend)
            }
            MountStorage::Archive { .. } => Err(EngineError::Backend(BackendError::Sealed)),
            // `.mem` export from an in-memory mem lands with the
            // writable-session-server plan (it needs a backend-level
            // archive builder); this plan adds the backend, not the
            // export path, so refuse explicitly rather than silently.
            MountStorage::InMemory => Err(EngineError::Backend(BackendError::Other(
                "export not yet supported for in-memory backend".to_string(),
            ))),
        };
        exported.map(|mut r| {
            r.unterminated_fence_entities = fenced;
            r
        })
    }

    /// Update a mem's `version` field in its per-mem config and
    /// persist it through the backend. Backend-symmetric: folder
    /// backends rewrite `.memstead/config.json`; git-branch backends
    /// commit `__MEMSTEAD:mems/<mem>/config.json`. Archive mounts
    /// reject with `BackendError::Sealed`.
    ///
    /// Returns the (mem, old_version, new_version) triple so
    /// callers can surface the change without an extra read. Reads
    /// the current value from the in-memory `MemConfig` and
    /// updates it on success, keeping the next call free of a
    /// stale-version read.
    ///
    /// `EngineError::UnknownMem` when the name resolves to no
    /// mount; `EngineError::ReadOnlyMount` when the mount is sealed
    /// for writes; `EngineError::InvalidInput` when the mount has no
    /// loaded `MemConfig` (folder mount with no
    /// `.memstead/config.json`; the residual missing-config path is
    /// distinct from the missing-version path). F1.
    /// Record pipeline-edit provenance through `mem`'s backend — the
    /// bridge the pipeline-edit block (outside the engine module) uses
    /// to reach a mount's backend. A mem that isn't currently mounted
    /// is a successful no-op: pipeline configs may reference unmounted
    /// mems, and provenance is recorded against the mounted set.
    pub fn record_pipeline_edit_provenance(
        &self,
        mem: &str,
        kind: &str,
        edits: &[(String, Option<Vec<u8>>)],
        note: Option<&str>,
        verb: &str,
    ) -> Result<(), crate::backend::BackendError> {
        let ctx =
            self.session_commit_context(Some("memstead_pipeline_edit"), note.map(String::from));
        match self.mounts.iter().find(|m| m.mount.mem == mem) {
            Some(m) => m.backend.record_pipeline_edit(kind, edits, &ctx, verb),
            None => Ok(()),
        }
    }

    pub fn set_mem_version(
        &mut self,
        mem_name: &str,
        new_version: semver::Version,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemVersionOutcome, EngineError> {
        // Resolve the mount up-front so an unknown-mem name refuses
        // before any drift-probe side effect lands.
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        // Probe for concurrent-drift before the write — a sibling
        // engine that committed between our last snapshot and now
        // surfaces `MEM_RELOADED` on the response so callers see
        // the drift without a separate read round-trip. Drift
        // warnings ride alongside the success outcome; an
        // unreachable-backend probe collapses to no warnings (the
        // existing accessor warn-logs internally and skips).
        let mut warnings = self.reload_if_stale(Some(mem_name));
        // Provenance nudge — same posture as every other commit-
        // producing mutation: when `require_notes` is set and no note
        // was supplied, ride a non-blocking `NOTE_MISSING` warning.
        // The version bump still commits.
        if let Some(w) = self.note_missing_warning("set_mem_version", note) {
            warnings.push(w);
        }

        // The old value is read from the STORED config, not the cache: the
        // cache may be days stale, and reporting a version the file has not
        // held since boot would be its own small lie.
        let old_version = self.mounts[mount_idx]
            .mem_config
            .as_ref()
            .and_then(|c| c.version.clone());
        let target = new_version.clone();
        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_version",
            note,
            &move |c: &mut memstead_schema::config::MemConfig| {
                c.version = Some(target.clone());
            },
        )?;
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemVersionOutcome {
            mem: mem_name.to_string(),
            old_version,
            new_version,
            warnings,
        })
    }

    /// Update a mem's `description` field in its per-mem config and
    /// persist it through the backend — the one-line text mem-archive
    /// export embeds and the registry card surfaces. `None` clears the
    /// field. Same backend symmetry, drift probe, and provenance-note
    /// posture as [`Self::set_mem_version`]; archive mounts reject with
    /// `BackendError::Sealed`.
    pub fn set_mem_description(
        &mut self,
        mem_name: &str,
        new_description: Option<String>,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemDescriptionOutcome, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        let mut warnings = self.reload_if_stale(Some(mem_name));
        if let Some(w) = self.note_missing_warning("set_mem_description", note) {
            warnings.push(w);
        }

        let old_description = self.mounts[mount_idx]
            .mem_config
            .as_ref()
            .and_then(|c| c.description.clone());
        let target = new_description.clone();
        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_description",
            note,
            &move |c: &mut memstead_schema::config::MemConfig| {
                c.description = target.clone();
            },
        )?;
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemDescriptionOutcome {
            mem: mem_name.to_string(),
            old_description,
            new_description,
            warnings,
        })
    }

    /// Update a mem's display `title` — free text, NOT identity: the
    /// mem name stays the sole handle everywhere. `None` clears it.
    /// Mirrors [`Self::set_mem_description`] in backend symmetry,
    /// drift probe, and provenance-note posture.
    pub fn set_mem_title(
        &mut self,
        mem_name: &str,
        new_title: Option<String>,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemTitleOutcome, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        let mut warnings = self.reload_if_stale(Some(mem_name));
        if let Some(w) = self.note_missing_warning("set_mem_title", note) {
            warnings.push(w);
        }

        let old_title = self.mounts[mount_idx]
            .mem_config
            .as_ref()
            .and_then(|c| c.title.clone());
        let target = new_title.clone();
        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_title",
            note,
            &move |c: &mut memstead_schema::config::MemConfig| {
                c.title = target.clone();
            },
        )?;
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemTitleOutcome {
            mem: mem_name.to_string(),
            old_title,
            new_title,
            warnings,
        })
    }

    /// Update a mem's `subject` block — scope, method, deliberate
    /// exclusions, published verbatim. `None` clears the block AS A
    /// UNIT. Mirrors [`Self::set_mem_description`].
    pub fn set_mem_subject(
        &mut self,
        mem_name: &str,
        new_subject: Option<memstead_schema::MemSubject>,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemSubjectOutcome, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        let mut warnings = self.reload_if_stale(Some(mem_name));
        if let Some(w) = self.note_missing_warning("set_mem_subject", note) {
            warnings.push(w);
        }

        let old_subject = self.mounts[mount_idx]
            .mem_config
            .as_ref()
            .and_then(|c| c.subject.clone());
        let target = new_subject.clone();
        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_subject",
            note,
            &move |c: &mut memstead_schema::config::MemConfig| {
                c.subject = target.clone();
            },
        )?;
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemSubjectOutcome {
            mem: mem_name.to_string(),
            old_subject,
            new_subject,
            warnings,
        })
    }

    /// Mark (or unmark) a mem as **internal** — hidden from the default
    /// `memstead_overview` roster and public projections, while remaining a
    /// real, schema-validated, diffable mem (inspectable when explicitly
    /// scoped by name, and deletable). The ingest process-state redesign
    /// (candidate (b)) flags each `ingest/<name>` process mem this way so it
    /// does not clutter the roster alongside real content.
    ///
    /// Stored as the top-level `internal` config field (captured by the
    /// flattened `extra` map). Backend-symmetric like
    /// [`Self::set_mem_description`]; `EngineError::UnknownMem` /
    /// `ReadOnlyMount` / `InvalidInput` on the usual failures.
    pub fn set_mem_internal(
        &mut self,
        mem_name: &str,
        internal: bool,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemInternalOutcome, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        let _ = self.reload_if_stale(Some(mem_name));

        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_internal",
            note,
            &move |c: &mut memstead_schema::config::MemConfig| {
                if internal {
                    c.extra
                        .insert("internal".to_string(), serde_json::Value::Bool(true));
                } else {
                    c.extra.remove("internal");
                }
            },
        )?;
        // This one used to return a bare `bool` and so had nowhere to put the
        // intervention report: it was dropped, not even logged.
        // A writer whose signature
        // cannot carry a warning is a writer that silently will not.
        let mut warnings = Vec::new();
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemInternalOutcome {
            mem: mem_name.to_string(),
            internal,
            warnings,
        })
    }

    /// Set (or clear) one opaque sync-state token in a mem's per-mem
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
    /// Backend-symmetric like [`Self::set_mem_version`]: folder backends
    /// rewrite `.memstead/config.json`; git-branch backends commit
    /// `__MEMSTEAD:mems/<mem>/config.json`. Archive mounts reject with
    /// `BackendError::Sealed`.
    ///
    /// Returns the (mem, key, previous-token) triple so callers can
    /// surface the change without an extra read. `EngineError::UnknownMem`
    /// when the name resolves to no mount; `EngineError::ReadOnlyMount`
    /// when the mount is sealed for writes; `EngineError::InvalidInput`
    /// when the mount has no loaded `MemConfig`.
    pub fn set_mem_sync_state(
        &mut self,
        mem_name: &str,
        key: &str,
        token: &str,
        note: Option<&str>,
    ) -> Result<crate::ops::SetMemSyncStateOutcome, EngineError> {
        // Resolve the mount up-front so an unknown-mem name refuses
        // before any drift-probe side effect lands.
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        // Probe for concurrent-drift before the write — same posture as
        // every other commit-producing mutation; a sibling engine that
        // committed since our last snapshot surfaces `MEM_RELOADED`.
        let mut warnings = self.reload_if_stale(Some(mem_name));
        if let Some(w) = self.note_missing_warning("set_mem_sync_state", note) {
            warnings.push(w);
        }

        // `previous` has to come from the config this write actually lands
        // on, not from the cache: a sibling ingest pass may have moved the
        // very token this call is replacing, and reporting the cached value
        // would name a baseline that has not been current since boot. The
        // cell is written by each `apply` pass, so after a compare-and-set
        // retry it holds what was really overwritten.
        let seen: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
        let key_owned = key.to_string();
        let token_owned = token.to_string();
        let (_, intervened) = self.write_mem_config_merged(
            mount_idx,
            mem_name,
            "set_mem_sync_state",
            note,
            &|c: &mut memstead_schema::config::MemConfig| {
                *seen.borrow_mut() = if token_owned.is_empty() {
                    c.sync_state.remove(&key_owned)
                } else {
                    c.sync_state.insert(key_owned.clone(), token_owned.clone())
                };
            },
        )?;
        let previous = seen.into_inner();
        // `removed` distinguishes a no-op clear (key absent) from a real one
        // so the outcome is honest.
        let removed = token.is_empty() && previous.is_some();
        if !intervened.is_empty() {
            warnings.push(crate::ops::WarningHint::ConfigWriteIntervened {
                mem: mem_name.to_string(),
                fields: intervened,
            });
        }

        Ok(crate::ops::SetMemSyncStateOutcome {
            mem: mem_name.to_string(),
            key: key.to_string(),
            previous,
            removed,
            warnings,
        })
    }

    /// Re-read the named mount's backend entities and refresh the
    /// in-memory store for that mem. Returns the diff against the
    /// pre-reload snapshot — `added` (ids newly present), `removed`
    /// (ids no longer present), `changed` (same id, different
    /// `content_hash`).
    ///
    /// Operator-triggered: useful when an external writer modified
    /// disk while this engine instance was alive (the folder backend
    /// assumes single-writer; this primitive is the escape hatch when
    /// that assumption breaks). On the happy path the diff is empty.
    ///
    /// Drift detection (whether disk *did* change) is not part of this
    /// surface — callers that want to short-circuit on "nothing
    /// changed" must compare `added.is_empty() && changed.is_empty()
    /// && removed.is_empty()` against the result. Backend-specific
    /// drift signals (git HEAD comparison, mtime check) live in the
    /// backends where they have meaning.
    ///
    /// Invalidates community + search-index memos on success.
    /// Load every DEFERRED (lazy, not-yet-loaded) mem matching `mem`
    /// (`None` = all) into the store — the first-read trigger of the
    /// lazy-mount lifecycle. Runs before the staleness probes on every
    /// operation ([`Self::reload_if_stale`] calls it first), so an
    /// operation scoped to one mem loads exactly that mem, and a
    /// workspace-scoped operation (search, overview, health) loads
    /// whatever it needs to answer over a complete store — a count
    /// computed over a partial store is never presented as truth.
    ///
    /// Loading rides the ordinary per-mem reload (entity walk, store
    /// push, the workspace-global validation passes, memo
    /// invalidation), so a lazily-loaded mem gets the same refusals and
    /// warnings an eager boot produces for the same content — deferral
    /// changes WHEN the gauntlet runs, never whether. After each load,
    /// pending `SuspiciousNestedPrefix` warnings whose target arrived
    /// with this mem are dropped — the same legitimate-cross-mem
    /// exemption the eager boot applies once every mount is loaded.
    ///
    /// A deferred load that FAILS quarantines the mem with the same
    /// typed reporting an eager boot failure produces, at this first
    /// read: the mount leaves the serving roster, the quarantine roster
    /// gains the typed reason, and the existing reattach contract
    /// applies. Deferral never converts a load failure into an
    /// empty-mem impression.
    ///
    /// No-op for workspaces without lazy mounts, for already-loaded
    /// mems, and for a filter naming no deferred mem.
    pub fn ensure_mems_loaded(&mut self, mem: Option<&str>) {
        let pending: Vec<String> = self
            .mounts
            .iter()
            .filter(|m| m.deferred && mem.is_none_or(|v| m.mount.mem == v))
            .map(|m| m.mount.mem.clone())
            .collect();
        for name in pending {
            match self.reload_one_mem(&name) {
                Ok(_) => {
                    if let Some(state) = self.mounts.iter_mut().find(|m| m.mount.mem == name) {
                        state.deferred = false;
                    }
                    // Cross-mem links INTO this mem were scanned by the
                    // nested-prefix detector against a store that did
                    // not yet carry it; now that its entities exist,
                    // drop any hit the arrival resolves (mirrors the
                    // boot-time retain over the complete store).
                    let store = &self.store;
                    self.load_warnings.retain(|w| match w {
                        crate::ops::WarningHint::SuspiciousNestedPrefix { resolved_id, .. } => {
                            store.get(resolved_id).is_none_or(|e| e.stub)
                        }
                        _ => true,
                    });
                }
                Err(e) => {
                    // Quarantine at the moment of first read — mirror
                    // the eager boot failure path byte-for-byte in
                    // consequence: out of the serving roster, onto the
                    // quarantine roster with the typed reason.
                    let Some(idx) = self.mounts.iter().position(|m| m.mount.mem == name) else {
                        continue;
                    };
                    let removed = self.mounts.remove(idx);
                    self.schemas_remove(&name);
                    self.quarantined.push(crate::engine::QuarantinedMem {
                        mount: removed.mount,
                        reason_code: e.code().to_string(),
                        reason_message: e.to_string(),
                    });
                    self.mem_router = std::sync::Arc::new(
                        crate::engine::boot::build_mem_router_from_mounts(&self.mounts),
                    );
                    self.invalidate_communities();
                    // The schemas epoch just moved (`schemas_remove`),
                    // so a filled search memo is stale-keyed — clear it
                    // here or the next search trips the memo-key
                    // assert (the first W8/01 grade demonstrated
                    // exactly that on this branch).
                    self.invalidate_search_indexes();
                }
            }
        }
    }

    pub fn reload_one_mem(&mut self, mem: &str) -> Result<crate::ops::ReloadResult, EngineError> {
        // Per-mem reload refreshes THIS mem's slice of the engine-wide
        // `load_warnings` accumulator: stale boot-time warnings for the
        // mem drop, fresh re-parse warnings take their place, other
        // mems' entries stay untouched. (The earlier "intentionally
        // silent" contract let a reload heal drift on disk while
        // `health()` kept reporting the healed warning forever — the
        // same class of stale-state lie the mem-delete purge closes.)
        //
        // The sink is filtered by source-mem attribution before it
        // merges: `validate_loaded_relations` scans the whole store, so
        // in principle the sink can carry other mems' warnings. In the
        // common case those mems' invalid rows were already dropped
        // from the in-memory store at their own load, so the filter is
        // a no-op guard against cross-mem duplicates, not a routine
        // trim. Failure leaves the accumulator untouched (`?` fires
        // before the merge), matching the inner fn's no-mutation-on-
        // failed-read fence. Drift events still surface as
        // `MemReloaded` warnings via `reload_if_stale`.
        // A quarantined mem's reload is the way back into service:
        // re-attempt the whole attach (backend, schema resolution,
        // entity load). On success the roster entry disappears; on
        // failure the mem stays quarantined with a refreshed reason.
        if self.quarantine_reason(mem).is_some() {
            return self.reattach_quarantined_mem(mem);
        }
        let mut sink: Vec<WarningHint> = Vec::new();
        let result = self.reload_one_mem_inner(mem, &mut sink)?;
        self.load_warnings.retain(|w| w.source_mem() != Some(mem));
        self.load_warnings
            .extend(sink.into_iter().filter(|w| w.source_mem() == Some(mem)));
        Ok(result)
    }

    /// The quarantine branch of [`Self::set_mem_schema`]: repin the
    /// retained mount (target must resolve — the same booted resolver;
    /// repair never force-writes a pin that resolves nowhere), bump
    /// the backend config through the shared value-level writer,
    /// persist the mount state, then re-attempt the attach. A reattach
    /// that still fails (some second cause) leaves the mem quarantined
    /// with its refreshed reason — the pin switch itself is durable
    /// either way. The booted path's conformance gate cannot run over
    /// an unloaded mem; findings surface on the post-reattach health.
    fn set_schema_on_quarantined(
        &mut self,
        mem: &str,
        target: &memstead_schema::SchemaRef,
    ) -> Result<crate::engine::SetSchemaOutcome, EngineError> {
        use crate::engine::{SetSchemaOutcome, SetSchemaResult};
        // Same target-ref validation as the ordinary branch.
        if self.resolve_schema_by_ref(target).is_none() {
            let consulted: Vec<_> = self
                .workspace_schemas
                .iter()
                .chain(self.builtin_schemas.iter())
                .cloned()
                .collect();
            return Err(EngineError::SchemaNotFound {
                mem: mem.to_string(),
                pin: target.as_display(),
                sources: crate::engine::error::SchemaSourceDiagnostic::for_failed_pin(
                    &target.name,
                    &target.version,
                    &consulted,
                ),
                install_hint: None,
            }
            .with_schema_install_probe(self.workspace_root()));
        }
        let Some(q_idx) = self.quarantined.iter().position(|q| q.mount.mem == mem) else {
            return Err(self.unknown_mem_error(mem));
        };
        // Repin the retained mount and, where a backend can be
        // instantiated, the authoritative backend config (shared
        // value-level bump — same writer as the ordinary branch).
        self.quarantined[q_idx].mount.schema = Some(target.clone());
        self.quarantined[q_idx].mount.migration_target = None;
        if let Ok(backend) = (self.backend_factory)(&self.quarantined[q_idx].mount) {
            let ctx = self.session_commit_context(Some("set_mem_schema"), None);
            let _ = bump_backend_schema_pin(backend.as_ref(), target, &ctx);
        }
        self.persist_state()?;
        // Re-attempt the attach. Failure keeps the quarantine (fresh
        // reason on the roster) but the pin switch stands — the
        // outcome reports the switch, the roster reports any
        // remaining cause.
        let _ = self.reattach_quarantined_mem(mem);
        // The below-gate repair path validated nothing, so it stamps
        // nothing: the marker reports what the last validated mutation
        // stamped (read from the re-attached mount when the repair
        // succeeded), and the next entity write re-stamps it.
        let stamped_schema = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .and_then(|idx| self.stamped_schema_of(idx));
        Ok(SetSchemaOutcome {
            mem: mem.to_string(),
            schema_pin: target.as_display(),
            migration_target: None,
            outcome: SetSchemaResult::Switched,
            findings: Vec::new(),
            stamped_schema,
        })
    }

    /// Re-attempt the boot-time attach of a quarantined mem —
    /// backend instantiation, schema resolution (same resolver and
    /// catalogue layering as boot: workspace-authored schemas over
    /// built-ins), then a per-mem entity load. Success removes the
    /// roster entry and the mem serves again in the same process;
    /// any failure keeps (re-)quarantining with the fresh typed
    /// reason, so the roster never goes stale against the live state.
    fn reattach_quarantined_mem(
        &mut self,
        mem: &str,
    ) -> Result<crate::ops::ReloadResult, EngineError> {
        let Some(q_idx) = self.quarantined.iter().position(|q| q.mount.mem == mem) else {
            return Err(self.unknown_mem_error(mem));
        };
        let mount = self.quarantined[q_idx].mount.clone();

        let requarantine = |this: &mut Self, e: &EngineError| {
            this.quarantined[q_idx].reason_code = e.code().to_string();
            this.quarantined[q_idx].reason_message = e.to_string();
        };

        let backend = match (self.backend_factory)(&mount) {
            Ok(b) => b,
            Err(e) => {
                let err = EngineError::Mem(e.to_string());
                requarantine(self, &err);
                return Err(self.unknown_mem_error(mem));
            }
        };

        // Same config / pin-authority reads as the boot loop.
        let last_known_head = backend.current_head().ok().flatten();
        let mem_config = backend.read_mem_config().ok().flatten().and_then(|bytes| {
            let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            memstead_schema::config::parse_mem_config(&value).ok()
        });
        let archive_provenance = backend
            .read_archive_provenance()
            .ok()
            .flatten()
            .and_then(|bytes| memstead_schema::ArchiveProvenance::from_archive_bytes(&bytes).ok());
        let config_pin = mem_config.as_ref().and_then(|c| c.schema.clone());
        let effective_pin = mount
            .migration_target
            .clone()
            .or(config_pin)
            .or(mount.schema.clone());
        let Some(effective_pin) = effective_pin else {
            let err = EngineError::MemConfigIncomplete {
                mem: mem.to_string(),
                missing_fields: vec!["schema".to_string()],
            };
            requarantine(self, &err);
            return Err(self.unknown_mem_error(mem));
        };
        // Same catalogue order as boot: workspace tier, then the mount's
        // own embedded vocabulary when it is archive-backed, then the
        // built-ins.
        let catalogue: Vec<std::sync::Arc<memstead_schema::Schema>> = self
            .workspace_schemas
            .iter()
            .cloned()
            .chain(super::boot::embedded_archive_schemas(&mount))
            .chain(self.builtin_schemas.iter().cloned())
            .collect();
        let schema = match crate::engine::SchemaResolver::new(&catalogue).resolve(&effective_pin) {
            Ok(s) => s,
            Err(sources) => {
                let err = EngineError::SchemaNotFound {
                    mem: mem.to_string(),
                    pin: effective_pin.as_display(),
                    sources,
                    install_hint: None,
                }
                .with_schema_install_probe(self.workspace_root());
                requarantine(self, &err);
                return Err(self.unknown_mem_error(mem));
            }
        };

        // Attach, then load entities through the ordinary per-mem
        // reload. An entity-load failure re-quarantines (the mount is
        // detached again) — quarantine is not tolerance.
        self.quarantined.remove(q_idx);
        self.schemas_insert(mem.to_string(), schema);
        self.mounts.push(crate::engine::MountedBackend {
            mount,
            backend,
            last_known_head,
            mem_config,
            archive_provenance,
            // The reattach loads entities immediately below — a
            // quarantine return-to-service is never deferred.
            deferred: false,
        });
        self.mem_router = std::sync::Arc::new(crate::engine::boot::build_mem_router_from_mounts(
            &self.mounts,
        ));
        let mut sink: Vec<WarningHint> = Vec::new();
        match self.reload_one_mem_inner(mem, &mut sink) {
            Ok(result) => {
                self.load_warnings.retain(|w| w.source_mem() != Some(mem));
                self.load_warnings
                    .extend(sink.into_iter().filter(|w| w.source_mem() == Some(mem)));
                self.invalidate_communities();
                Ok(result)
            }
            Err(e) => {
                let mount_idx = self.mounts.len() - 1;
                let mounted = self.mounts.remove(mount_idx);
                self.schemas_remove(mem);
                self.mem_router = std::sync::Arc::new(
                    crate::engine::boot::build_mem_router_from_mounts(&self.mounts),
                );
                self.quarantined.push(crate::engine::QuarantinedMem {
                    mount: mounted.mount,
                    reason_code: e.code().to_string(),
                    reason_message: e.to_string(),
                });
                // Same epoch-moved staleness as the deferred-load
                // quarantine branch: `schemas_remove` bumped the
                // epoch, so both memos must clear.
                self.invalidate_communities();
                self.invalidate_search_indexes();
                Err(e)
            }
        }
    }

    /// Inner per-mem body shared by [`Self::reload_one_mem`]
    /// and [`Self::reload_each_writable_mem`]. The caller passes
    /// a warning sink so the workspace-wide reload can forward
    /// warnings into `self.load_warnings` while the single-mem
    /// path keeps the accumulator pristine.
    fn reload_one_mem_inner(
        &mut self,
        mem: &str,
        warnings_sink: &mut Vec<WarningHint>,
    ) -> Result<crate::ops::ReloadResult, EngineError> {
        // Locate the target mount + schema. Unknown mem short-
        // circuits before any store mutation.
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| self.unknown_mem_error(mem))?;
        let schema = self
            .schemas
            .get(mem)
            .cloned()
            .ok_or_else(|| self.unknown_mem_error(mem))?;

        // Snapshot pre-reload (id, content_hash) for this mem.
        let pre: HashMap<EntityId, String> = self
            .store
            .all_entities()
            .filter(|e| !e.stub && e.mem == mem)
            .map(|e| (e.id.clone(), e.content_hash.clone()))
            .collect();
        let pre_ids: std::collections::HashSet<EntityId> = pre.keys().cloned().collect();

        // Walk the backend; surface read-time errors instead of
        // mutating the store on a failed reload.
        let backend = self.mounts[mount_idx].backend.as_ref();
        let (entries, read_errors) = collect_source_entries(backend)?;
        // The unbacked-mount probe rides the reload like the other
        // load-time warnings: a branch that appeared (or vanished) since
        // boot changes the answer, and the sink's per-mem replace below
        // drops the boot-time one.
        let unbacked = super::boot::unbacked_mount_warning(
            &self.mounts[mount_idx].mount,
            backend,
            Some(entries.len()),
        );
        let load_result = parse_entries(entries, read_errors, mem, schema.as_ref());

        // Build the LoadCollector inputs — mem roster + last-
        // segment suffixes — so the parser pipeline can emit
        // typed drift warnings into the caller's sink.
        let mem_names: Vec<String> = self.mounts.iter().map(|m| m.mount.mem.clone()).collect();
        let known_suffixes: Vec<String> = mem_names
            .iter()
            .map(|n| crate::entity::store_builder::last_segment_suffix(n).to_string())
            .collect();

        // Failure fence above; below this point the store is mutated.
        self.store.remove_entities_by_mem(mem);
        if let Some(w) = unbacked {
            warnings_sink.push(w);
        }
        let fallback = engine_fallback_type();
        push_entities_into_store(
            &mut self.store,
            load_result.entities,
            fallback.as_ref(),
            Some(crate::entity::store_builder::LoadCollector {
                warnings: warnings_sink,
                known_suffixes: &known_suffixes,
                mem_names: &mem_names,
            }),
        );
        // Re-run parse-time relation validation across the workspace.
        // A reload re-parses one mem but the validator's cycle pass
        // is global (acyclic-rel-type subgraphs span mems), so the
        // scan runs against the whole store. Hand-edits arriving via
        // sibling-writer commits get the same gauntlet boot enforces
        // (grammar / unknown_rel_type / shape / cycle).
        let mount_caps: std::collections::HashMap<String, crate::workspace::MountCapability> = self
            .mounts
            .iter()
            .map(|m| (m.mount.mem.clone(), m.mount.capability))
            .collect();
        // Restore cross-mem edges that point INTO this mem. The
        // removal cascade above dropped their incoming mirrors and the
        // re-push only rebuilt edges authored by this mem's own
        // entities, so a cross-mem `A→B` would silently vanish from the
        // index until a workspace-wide reload. Reconstruct from the
        // authoritative source records (in-memory only — no other mem is
        // re-read), then let the remap pass below reclassify alias sources.
        crate::entity::store_builder::reconstruct_incoming_cross_mem_edges(&mut self.store, mem);
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
        // THIS mem's stale entries are replaced, not accumulated — a
        // repaired file (e.g. a resolved merge conflict) must stop
        // reporting its old refusal, and a re-read of a still-broken
        // file must not duplicate its entry. Other mems' entries are
        // untouched (their reloads own them). Folder mounts key their
        // entries by absolute source path under the mem root; other
        // backends have no path-attributable entries to replace.
        if let crate::workspace::MountStorage::Folder { path } =
            &self.mounts[mount_idx].mount.storage
        {
            let root = path.clone();
            self.load_errors.retain(|(p, _)| !p.starts_with(&root));
            // The backend walk yields mem-relative paths while the boot
            // loader yields absolute ones — normalize to absolute so the
            // replace-on-reload key stays uniform across both origins.
            self.load_errors
                .extend(load_result.errors.into_iter().map(|(p, m)| {
                    let abs = if p.is_relative() { root.join(&p) } else { p };
                    (abs, m)
                }));
        } else {
            self.load_errors.extend(load_result.errors);
        }

        // Refresh the mem's config from the backend too (D13). `sync_state`
        // (the projection baselines) and the schema pin / write guidance are
        // mem-scoped state that rides the mem branch, so an out-of-band write
        // — a sibling `projection advance` / `mem set-sync-state` — must become
        // visible after a per-mem reload, not only entity changes. A missing or
        // unparseable config leaves the cached value untouched (best-effort:
        // the reload never fails on a config read hiccup).
        if let Ok(Some(bytes)) = self.mounts[mount_idx].backend.read_mem_config()
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && let Ok(cfg) = memstead_schema::config::parse_mem_config(&value)
        {
            self.mounts[mount_idx].mem_config = Some(cfg);
        }

        // Diff post-reload against the snapshot.
        let mut added: Vec<EntityId> = Vec::new();
        let mut changed: Vec<EntityId> = Vec::new();
        for entity in self.store.all_entities() {
            if entity.stub || entity.mem != mem {
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
            .filter(|e| !e.stub && e.mem == mem)
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

    /// Rich-shape variant of [`Self::reload_one_mem`] that returns a
    /// [`crate::ops::ReloadReport`] (mem + head_before + head_after +
    /// entities_loaded + changed_entity_ids) instead of the slim
    /// [`crate::ops::ReloadResult`]. Handler-facing wrapper consumed
    /// by the `memstead_reload` MCP tool — the rich shape is the wire
    /// contract MCP callers depend on; the slim form stays for
    /// programmatic consumers that just want the diff lists.
    ///
    /// `head_before` is the engine's **prior cursor** for this mem
    /// (its cached `last_known_head`), *not* the current on-disk tip:
    /// when a sibling has committed since, the tip has already advanced,
    /// so reporting it would make the advertised
    /// `changes_since(since=head_before)` recipe span an empty range.
    /// `head_after` is the freshly-peeled tip from
    /// [`crate::backend::MemBackend::current_head`]; the reload also
    /// advances the cursor to it, so a follow-up staleness probe does
    /// not re-reload the same window. Backends without history (folder,
    /// archive) carry no cursor and return `Ok(None)`; both fields fall
    /// back to [`crate::ops::EMPTY_TREE_SHA`] for wire-shape stability.
    ///
    /// `entities_loaded` is the post-reload non-stub count for the
    /// mem — same semantic as full's report.
    ///
    /// `changed_entity_ids` is the union of `added ∪ changed ∪
    /// removed` from the underlying [`crate::ops::ReloadResult`]
    /// so callers don't have to merge three lists themselves —
    /// matches full's bundled wire shape.
    pub fn reload_one_mem_report(
        &mut self,
        mem: &str,
    ) -> Result<crate::ops::ReloadReport, EngineError> {
        // `head_before` is the engine's PRIOR cursor — the SHA it last
        // knew for this mem — not the current (possibly already
        // drifted) on-disk tip. Reporting the tip would collapse the
        // `changes_since(since=head_before)` range to empty in exactly
        // the sibling-drift case the recipe targets. `current_head` is
        // backend-defined: git-branch serves a git cursor, the folder
        // backend serves its change-ledger watermark, and a backend
        // without a head signal returns None — for those, `head_before`
        // stays the empty-tree sentinel that pairs with the
        // equally-empty `head_after` below.
        let tracks_head = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| m.backend.current_head().ok().flatten())
            .is_some();
        let head_before = if tracks_head {
            self.mounts
                .iter()
                .find(|m| m.mount.mem == mem)
                .and_then(|m| m.last_known_head.clone())
                .unwrap_or_else(|| crate::ops::EMPTY_TREE_SHA.to_string())
        } else {
            crate::ops::EMPTY_TREE_SHA.to_string()
        };

        let result = self.reload_one_mem(mem)?;

        // Capture head_after = the freshly-peeled tip, and advance the
        // engine's cursor to it. Without this advance the next
        // operation's `reload_if_stale` would compare the stale cursor
        // against the same tip and re-reload the identical window,
        // re-emitting a spurious `MEM_RELOADED`. Only history-backed
        // mounts (current_head → Some) carry a cursor to advance.
        let head_after_raw = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| m.backend.current_head().ok().flatten());
        if let Some(new_head) = head_after_raw.clone()
            && let Some(m) = self.mounts.iter_mut().find(|m| m.mount.mem == mem)
        {
            m.last_known_head = Some(new_head);
        }
        let head_after = head_after_raw.unwrap_or_else(|| crate::ops::EMPTY_TREE_SHA.to_string());

        let entities_loaded = self
            .store
            .all_entities()
            .filter(|e| !e.stub && e.mem == mem)
            .count();

        // Union of added + changed + removed, sorted lexicographically
        // for deterministic wire output. Matches full's "single
        // changed_entity_ids list" contract — saves callers from
        // merging three slices themselves.
        let mut changed_entity_ids: Vec<EntityId> = result
            .added
            .into_iter()
            .chain(result.changed)
            .chain(result.removed)
            .collect();
        changed_entity_ids.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(crate::ops::ReloadReport {
            mem: mem.to_string(),
            head_before,
            head_after,
            entities_loaded,
            changed_entity_ids,
        })
    }

    /// Batched rich-shape variant — returns one
    /// [`crate::ops::ReloadReport`] per mounted mem in declaration
    /// order. Counterpart to [`Self::reload_each_writable_mem`]
    /// (slim) that the `memstead_reload` MCP tool's no-mem path
    /// consumes.
    ///
    /// `load_warnings` semantics ride on the per-mem contract: each
    /// [`Self::reload_one_mem`] in the sweep refreshes its own mem's
    /// slice of the engine-wide accumulator, so a full sweep leaves
    /// the accumulator equivalent to a fresh boot. (Earlier this
    /// variant discarded every reload warning while its slim
    /// counterpart repopulated — the MCP workspace-wide reload could
    /// never clear a stale warning.) On first-error-abort, mems
    /// reloaded before the failure carry refreshed slices and the
    /// rest keep their boot-time entries — no slice is lost.
    ///
    /// Also re-reads `.memstead/workspace.toml` and refreshes
    /// [`crate::workspace::WorkspaceSettings`] before sweeping the
    /// mems — this is the pairing with the CLI's
    /// `memstead workspace allow-create / grant-cross-link / set-mutations`
    /// family. Without this re-read, a CLI write would land on disk but
    /// the running MCP would still serve the engine's boot-time policy
    /// snapshot; every subsequent `memstead_mem_create` against the new
    /// allowlist would fail with `MEM_PATH_NOT_ALLOWED` until process
    /// restart. The workspace-wide form runs the heavier path; the
    /// per-mem form (`reload_one_mem_report`) intentionally skips
    /// the workspace re-read — content drift doesn't imply policy
    /// drift.
    ///
    /// Reload of `workspace.toml` is best-effort: a missing or
    /// unparseable file leaves the existing settings untouched. The
    /// per-mem sweep is the primary contract — settings refresh is
    /// the additive bonus.
    ///
    /// First-error-aborts: if any mem's reload fails, the loop
    /// stops and the error propagates. Mems reloaded before the
    /// failing one are already mutated in the store; the returned
    /// error has no rollback. Operators run the per-mem form to
    /// retry the failing mem explicitly.
    pub fn reload_each_writable_mem_reports(
        &mut self,
    ) -> Result<Vec<crate::ops::ReloadReport>, EngineError> {
        self.refresh_workspace_settings_if_possible();
        let names: Vec<String> = self.mounts.iter().map(|m| m.mount.mem.clone()).collect();
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let report = self.reload_one_mem_report(&name)?;
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
    /// unparseable. The per-mem reload contract stays the canonical
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

    /// Reload every mounted mem in declaration order; returns one
    /// `(mem, ReloadResult)` per mount.
    ///
    /// Failure model is **first-error-aborts**: if any mem's reload
    /// fails, the loop stops and the error propagates. Mems reloaded
    /// before the failing one are already mutated in the store; the
    /// returned error has no rollback. Operators run the per-mem
    /// form to retry the failing mem explicitly.
    ///
    /// Caller-friendly batching wrapper around [`Self::reload_one_mem`];
    /// internal cache invalidation happens once per mem (the inner
    /// call invalidates) so an N-mem batch invalidates the memos
    /// N times. That's wasteful for large workspaces; once the
    /// `memstead_reload` MCP handler migrates we can tighten this to one
    /// invalidation at the end.
    pub fn reload_each_writable_mem(
        &mut self,
    ) -> Result<Vec<(String, crate::ops::ReloadResult)>, EngineError> {
        let names: Vec<String> = self.mounts.iter().map(|m| m.mount.mem.clone()).collect();
        // Workspace-wide reload semantics: take the engine-wide
        // sink, clear it, route per-mem inner reloads through it,
        // put it back. The result is `self.load_warnings` carries
        // every typed drift warning the reload sweep produced (so
        // the next `engine.health()` call surfaces them).
        let mut sink = std::mem::take(&mut self.load_warnings);
        sink.clear();
        let mut out = Vec::with_capacity(names.len());
        let mut loop_err = None;
        for name in names {
            match self.reload_one_mem_inner(&name, &mut sink) {
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

/// Value-level schema-pin bump on a backend's mem config: read the
/// config blob, rewrite ONLY the `"schema"` string, write it back —
/// every other field (`readMems`, write guidance, sync state, …) is
/// preserved verbatim. Config-absent backends (no `config.json`) are a
/// clean no-op returning `None`; the caller's `Mount.schema` then
/// stays the settled pin. Returns the updated JSON value on a write so
/// callers can refresh caches.
///
/// One shared implementation for the booted path
/// (`Engine::persist_mem_schema_pin`) and the below-boot repair path
/// (memstead-git-branch) — the two must never fork: a pin written by
/// repair must be byte-shaped exactly as one written by the engine.
/// Field names on which the stored config differs from `cached`.
///
/// Compared as JSON so the comparison covers every field the struct models
/// without a hand-written field list that a new field would silently escape.
/// A parse failure yields no fields rather than a false report: the write
/// itself will surface the malformed config.
fn changed_config_fields(
    cached: &memstead_schema::config::MemConfig,
    stored_bytes: &[u8],
) -> Vec<String> {
    let (Ok(a), Ok(b)) = (
        serde_json::to_value(cached),
        serde_json::from_slice::<serde_json::Value>(stored_bytes),
    ) else {
        return Vec::new();
    };
    let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else {
        return Vec::new();
    };
    let mut keys: std::collections::BTreeSet<&String> = a.keys().collect();
    keys.extend(b.keys());
    keys.into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .map(|k| k.to_string())
        .collect()
}

pub fn bump_backend_schema_pin(
    backend: &dyn crate::backend::MemBackend,
    target: &memstead_schema::SchemaRef,
    ctx: &crate::vcs::CommitContext<'_>,
) -> Result<Option<serde_json::Value>, EngineError> {
    let Some(bytes) = backend
        .read_mem_config()
        .map_err(|e| EngineError::Mem(format!("read mem config for pin update: {e}")))?
    else {
        return Ok(None);
    };
    let mut value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| EngineError::Mem(format!("parse mem config for pin update: {e}")))?;
    value["schema"] = serde_json::Value::String(target.as_display());
    let new_bytes = serde_json::to_vec_pretty(&value)
        .map_err(|e| EngineError::Mem(format!("serialize mem config for pin update: {e}")))?;
    backend
        .write_mem_config(&new_bytes, ctx)
        .map_err(|e| EngineError::Mem(format!("write mem config for pin update: {e}")))?;
    Ok(Some(value))
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

#[cfg(test)]
mod tests;
