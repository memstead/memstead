//! Mounts and routing: which mems exist, where each one lives, which are writable, quarantined or deferred, and how an entity id resolves to one of them.

use super::*;

impl Engine {
    /// Mem names the engine knows about, in declaration order.
    /// Cheap; useful for callers that need to enumerate before
    /// dispatching by mem.
    pub fn mem_names(&self) -> Vec<&str> {
        self.mounts.iter().map(|m| m.mount.mem.as_str()).collect()
    }

    /// Derivation-staleness report for one mem:
    /// every EXPLICIT edge whose rel-type the mem's schema declares
    /// `derivation: true`, compared against its recorded baseline.
    /// Baseline differs from the target's current hash → `stale`;
    /// no baseline recorded (edge predates the declaration, or was
    /// load-derived) → `unbaselined`, distinctly — never fabricated
    /// as fresh or stale. Fresh edges are not reported. A mem whose
    /// schema declares no derivation rel-types returns the empty
    /// report; an unreadable sidecar reads as empty (every edge
    /// unbaselined) rather than an error.
    pub fn derivation_report(
        &self,
        mem: &str,
    ) -> Result<Vec<crate::ops::health::DerivationFinding>, EngineError> {
        if !self.mem_router.is_visible(mem) {
            return Err(self.unknown_mem_error(mem));
        }
        let Some(schema) = self.schemas.get(mem) else {
            return Ok(Vec::new());
        };
        let declared: std::collections::HashSet<&str> = schema
            .manifest
            .relationships
            .definitions
            .iter()
            .filter(|d| d.derivation)
            .map(|d| d.name.as_str())
            .collect();
        if declared.is_empty() {
            return Ok(Vec::new());
        }
        let sidecar = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| {
                m.backend
                    .read_entity(Path::new(crate::derivation::DERIVATION_SIDECAR_PATH))
                    .ok()
                    .flatten()
            })
            .and_then(|bytes| crate::derivation::DerivationSidecar::from_bytes(&bytes).ok())
            .unwrap_or_default();

        let mut out = Vec::new();
        let mut sources: Vec<&crate::entity::Entity> = self
            .store
            .all_entities()
            .filter(|e| !e.stub && e.id.mem() == mem)
            .collect();
        sources.sort_by(|a, b| a.id.as_ref().cmp(b.id.as_ref()));
        for entity in sources {
            for edge in self.store.outgoing(&entity.id) {
                if !declared.contains(edge.rel_type.as_str())
                    || edge.source != crate::store::EdgeSource::Explicit
                {
                    continue;
                }
                let current = self
                    .store
                    .get(&edge.target)
                    .map(|t| t.content_hash.clone())
                    .unwrap_or_default();
                match sidecar.get(entity.id.as_ref(), &edge.rel_type, edge.target.as_ref()) {
                    None => out.push(crate::ops::health::DerivationFinding {
                        source: entity.id.clone(),
                        rel_type: edge.rel_type.clone(),
                        target: edge.target.clone(),
                        state: "unbaselined".to_string(),
                        baseline: None,
                        current,
                    }),
                    Some(baseline) if baseline != current => {
                        out.push(crate::ops::health::DerivationFinding {
                            source: entity.id.clone(),
                            rel_type: edge.rel_type.clone(),
                            target: edge.target.clone(),
                            state: "stale".to_string(),
                            baseline: Some(baseline.to_string()),
                            current,
                        })
                    }
                    Some(_) => {}
                }
            }
        }
        Ok(out)
    }

    /// Public-shape mount record for `mem`, or `None` for an unknown
    /// mem.
    ///
    /// Surfaces the operator-facing
    /// [`crate::workspace::Mount`] (mem name, schema pin, storage
    /// reference, capability, lifecycle, cross_linkable) so MCP / CLI
    /// handlers can branch on backend-specific shapes via
    /// [`crate::workspace::MountStorage`] when they need accessors
    /// that don't make sense on every backend (e.g. gitdir / branch
    /// for `memstead_health { include_config: true }`'s git-class
    /// payload). Backends that want the equivalent of full's
    /// `engine.gitdir_for(mem)` match
    /// `engine.mount(mem).map(|m| &m.storage)` against
    /// `MountStorage::GitBranch { gitdir, branch }` and walk
    /// directly — keeps the engine surface backend-neutral.
    ///
    /// Counterpart to [`Self::mem_names`] which lists every mount.
    pub fn mount(&self, mem: &str) -> Option<&crate::workspace::Mount> {
        self.mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .map(|m| &m.mount)
    }

    /// Orphan count attributed to each mem's pinned schema, over the
    /// given `orphan_ids` (the caller pre-filters them by any mem scope).
    /// Lets a health surface show that ingest-mem isolates (orphans by
    /// design) and code-mem debt land in different schema buckets rather
    /// than one blended, misleading total. Mems with no settled pin
    /// bucket under the empty string.
    pub fn orphans_by_schema(
        &self,
        orphan_ids: &[EntityId],
    ) -> std::collections::BTreeMap<String, usize> {
        let mut by_schema = std::collections::BTreeMap::new();
        for id in orphan_ids {
            let schema = self
                .store()
                .get(id)
                .and_then(|e| self.mount(&e.mem))
                .and_then(|m| m.schema.as_ref().map(|s| s.as_display()))
                .unwrap_or_default();
            *by_schema.entry(schema).or_insert(0) += 1;
        }
        by_schema
    }

    /// Community count attributed to each schema across `mems`: a cluster
    /// counts toward every schema whose mems it touches, so these figures
    /// can sum above the global community count — the same "touches"
    /// semantic as the mem-scoped count. Per-schema dedup keeps a cluster
    /// touching two mems of one schema from being counted twice.
    pub fn communities_by_schema(
        &self,
        mems: &[String],
    ) -> std::collections::BTreeMap<String, usize> {
        let louvain = self.communities();
        let mut buckets: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
            std::collections::BTreeMap::new();
        for name in mems {
            let schema = self
                .mount(name)
                .and_then(|m| m.schema.as_ref().map(|s| s.as_display()))
                .unwrap_or_default();
            let clusters = crate::graph::community::clusters_in_mem(self.store(), louvain, name);
            buckets.entry(schema).or_default().extend(clusters);
        }
        buckets
            .into_iter()
            .map(|(schema, set)| (schema, set.len()))
            .collect()
    }

    /// All mounts the engine knows about, in declaration order.
    /// Counterpart to [`Self::mem_names`] when the caller needs
    /// the full mount shape (e.g. to enumerate by storage variant).
    pub fn mounts(&self) -> Vec<&crate::workspace::Mount> {
        self.mounts.iter().map(|m| &m.mount).collect()
    }

    /// Names of mems whose mount declares
    /// [`crate::workspace::MountCapability::Write`], in declaration
    /// order. Convenience over `mounts().iter().filter(...).map(...)`
    /// for handlers that gate by writable status (`memstead_health`,
    /// `memstead_overview`'s mem roster, the lifecycle tools'
    /// candidate list). Read-only mounts (archive backends) are
    /// excluded.
    pub fn writable_mem_names(&self) -> Vec<&str> {
        self.mounts
            .iter()
            .filter(|m| m.mount.capability == MountCapability::Write)
            .map(|m| m.mount.mem.as_str())
            .collect()
    }

    /// The default writable mem — the target a mutation lands in when
    /// it omits `mem`. `None` when no writable mem is mounted.
    ///
    /// Defined as the **first writable mount in declaration order**, i.e.
    /// the seed / earliest-created writable mem. This is a *stable*
    /// designation, not a function of the current name set: new mems
    /// register via `register_writable_mem`, which pushes onto the end
    /// of the mount list (and `mounts.json` preserves that order across
    /// reboots), so creating an additional mem never moves the default
    /// — even one whose name sorts ahead alphabetically. Deleting the
    /// current default promotes the next-earliest writable mem; that is
    /// the only thing that shifts it. Both the MCP `resolve_mem` and the
    /// CLI's omitted-`--mem` path resolve through here so the two
    /// surfaces always agree (the
    /// pre-fix MCP path read `writable_mems().iter().next()` off an
    /// unordered `HashSet`, which silently retargeted writes when a second
    /// mem appeared).
    pub fn default_writable_mem(&self) -> Option<&str> {
        self.mounts
            .iter()
            .find(|m| m.mount.capability == MountCapability::Write)
            .map(|m| m.mount.mem.as_str())
    }

    /// On-disk folder path for a folder-backed mount, or `None` for
    /// any other backend (git-branch, archive) or unknown mem.
    /// Convenience over `engine.mount(mem).map(|m| &m.storage)` +
    /// matching on `MountStorage::Folder { path }`. Used by
    /// handlers that need a filesystem path for a folder mem
    /// (e.g. `memstead_health { include_config: true }`'s
    /// `mems[].vcs.worktree` field for folder mounts).
    pub fn folder_path_for_mem(&self, mem: &str) -> Option<&Path> {
        match self.mount(mem).map(|m| &m.storage) {
            Some(crate::workspace::MountStorage::Folder { path }) => Some(path.as_path()),
            _ => None,
        }
    }

    /// Runtime snapshot of writable / visible mems. Handlers that
    /// need the writable roster (`memstead_health`'s `writable_mems` /
    /// `read_mems`), per-mem origin tag (`include_config:
    /// true`'s `mems[].origin`), or visibility check
    /// (`memstead_overview`'s mem list, the lifecycle tools' collision
    /// guard) consume the router here. Returned by reference — the
    /// `Arc` is held on the engine; callers that need a clonable
    /// handle can `Arc::clone` the engine's field directly when that
    /// surface arrives.
    pub fn mem_router(&self) -> &MemRouterSnapshot {
        &self.mem_router
    }

    /// Resolve the gitdir for a writable mem. Used by `memstead_health
    /// { include_config: true }` to surface per-mem `vcs.gitdir`
    /// so outer-repo bookkeeping clients can `git -C <gitdir>` per
    /// mem without hardcoding the layout.
    ///
    /// - `EngineError::UnknownMem` when the name does not resolve.
    /// - `EngineError::Mem` when the mount's storage is not
    ///   git-branch-backed (folder, archive — they have no gitdir).
    pub fn gitdir_for(&self, mem_name: &str) -> Result<PathBuf, EngineError> {
        let m = self
            .mount(mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        match &m.storage {
            MountStorage::GitBranch { gitdir, .. } => Ok(gitdir.clone()),
            MountStorage::Folder { .. } | MountStorage::Archive { .. } | MountStorage::InMemory => {
                Err(EngineError::Mem(format!(
                    "mem '{mem_name}' has no resolved gitdir"
                )))
            }
        }
    }

    /// Resolve the worktree for a writable mem. Used by
    /// `memstead_health { include_config: true }` to surface per-mem
    /// `vcs.worktree`.
    ///
    /// - `EngineError::UnknownMem` when the name does not resolve.
    /// - `EngineError::Mem` when the mount's backend has no
    ///   worktree concept (git-branch with no working tree, archive).
    ///
    /// Folder mounts surface their on-disk path. Git-branch mounts
    /// follow the `dir: Some(...)` composition pattern: when the
    /// workspace root contains a folder named after the mem with a
    /// `.memstead/config.json` marker, that folder is the worktree
    /// (disk-shape composition). Otherwise — pure mem-repo-backed
    /// — return Err.
    pub fn worktree_for(&self, mem_name: &str) -> Result<PathBuf, EngineError> {
        let m = self
            .mount(mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        match &m.storage {
            MountStorage::Folder { path } => Ok(path.clone()),
            MountStorage::GitBranch { .. } => {
                if let Some(root) = self.workspace_root.as_deref() {
                    let candidate = root.join(mem_name);
                    if candidate
                        .join(crate::mem::MEM_META_DIR)
                        .join("config.json")
                        .is_file()
                    {
                        return Ok(candidate.canonicalize().unwrap_or(candidate));
                    }
                }
                Err(EngineError::Mem(format!(
                    "mem '{mem_name}' has no working tree (mem-repo-backed)"
                )))
            }
            MountStorage::Archive { .. } => Err(EngineError::Mem(format!(
                "mem '{mem_name}' is archive-backed and has no worktree"
            ))),
            MountStorage::InMemory => Err(EngineError::Mem(format!(
                "mem '{mem_name}' is in-memory and has no worktree"
            ))),
        }
    }

    /// Per-mem `.memstead/config.json` payload, when available. Used
    /// by `memstead_health { include_config: true }` to surface the
    /// opaque `write_guidance` map and the catch-all `extra` fields
    /// per mem.
    ///
    /// Folder-backed mounts return `Some(&MemConfig)` when
    /// `<path>/.memstead/config.json` parsed cleanly at construction.
    /// Git-branch and archive backends return `None` until the
    /// read-from-storage-backend path lifts (the V1 unified engine
    /// loads configs only from folder layouts; the file lives
    /// inside the gitdir / archive for the other backends and
    /// needs a backend-level read primitive).
    ///
    /// Unknown mem names return `None` (no error variant — the
    /// accessor is intentionally lenient because memstead_health emits
    /// an empty detail block per missing config rather than
    /// aborting the call).
    pub fn mem_config_for(&self, mem: &str) -> Option<&memstead_schema::config::MemConfig> {
        self.mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| m.mem_config.as_ref())
    }

    /// The authoring-provenance payload an installed mem carries, read
    /// from the archive's `.memstead/provenance.json` at construction.
    /// `None` when the mem carries none (a pre-provenance archive, a
    /// runtime-created mem, or a backend that does not surface one) —
    /// the read path reports provenance as absent. Unknown mem names
    /// return `None`.
    pub fn archive_provenance_for(&self, mem: &str) -> Option<&memstead_schema::ArchiveProvenance> {
        self.mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| m.archive_provenance.as_ref())
    }

    /// Iterate `(mem_name, &MemConfig)` for every mount whose
    /// mem-config payload loaded at construction. Used by callers
    /// that walk every writable mount's config (`memstead health`'s
    /// per-mem dump, the workspace-dump CLI). The yielded `&str` is
    /// the authoritative mem leaf from the mount record.
    ///
    /// Folder-backed mounts yield when their `.memstead/config.json`
    /// parsed cleanly. Git-branch and archive backends are silent in
    /// V1 (the same deferred-read-from-storage gap that
    /// [`Self::mem_config_for`] documents).
    pub fn mem_configs_named(
        &self,
    ) -> impl Iterator<Item = (&str, &memstead_schema::config::MemConfig)> {
        self.mounts
            .iter()
            .filter_map(|m| m.mem_config.as_ref().map(|c| (m.mount.mem.as_str(), c)))
    }

    /// Every configured mount, with its config when one was readable.
    ///
    /// The counterpart to [`Self::mem_configs_named`], whose contract is "mems
    /// WITH config" and which is therefore right to omit the rest. The trouble
    /// was that callers wanting to enumerate mounts reached for it anyway: a
    /// folder mount whose directory is gone returns no config rather than an
    /// error, boot stores none, and the mount became invisible to every one of
    /// them. Nine call sites shared that blind spot, and the omission was a
    /// side effect of config readability rather than anything about the mount
    /// (04/05, criteria 1 and 8).
    ///
    /// Callers that genuinely want only configured mems keep using the other
    /// one; drivers that want every mount ask for every mount.
    pub fn mounts_with_optional_config(
        &self,
    ) -> impl Iterator<Item = (&str, Option<&memstead_schema::config::MemConfig>)> {
        self.mounts
            .iter()
            .map(|m| (m.mount.mem.as_str(), m.mem_config.as_ref()))
    }

    /// Resolved `Arc<Schema>` for a writable mem by name. `None`
    /// when the name is not a registered mount.
    ///
    /// Cheap — `Arc::clone` over the per-mem schema map. Resolved
    /// schemas are stored in `HashMap<String, Arc<Schema>>` so the
    /// lookup is a single hash hit + clone.
    pub fn schema_for(&self, mem: &str) -> Option<std::sync::Arc<memstead_schema::Schema>> {
        self.schemas.get(mem).cloned()
    }

    /// Cached current branch-tip cursor (typically a 40-char hex
    /// SHA for git-branch backends; `None` for fresh mems or
    /// backends that don't track a head — folder / archive).
    ///
    /// The value is the per-mount `last_known_head`, seeded at
    /// construction by `backend.current_head()` and refreshed by
    /// [`Self::reload_if_stale`] / mutation paths after a
    /// successful commit.
    ///
    /// - `EngineError::UnknownMem` when the name does not resolve.
    pub fn mem_head_sha(&self, mem_name: &str) -> Result<Option<String>, EngineError> {
        let m = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        Ok(m.last_known_head.clone())
    }

    /// Whether a sibling writer has advanced this mem's backend past
    /// the engine's cached `last_known_head` — a read-only drift probe
    /// that does **not** reload (unlike [`Self::reload_if_stale`]). One
    /// `backend.current_head()` read compared against the cached cursor;
    /// the comparison clears once the engine re-reads (a `reload` /
    /// `reload_if_stale` refreshes `last_known_head` to the live tip).
    ///
    /// Only git-branch backends track a head, so folder / archive /
    /// in-memory mounts always report `false`. A backend that errors on
    /// the probe (transient refdb hiccup) reports `false` rather than
    /// surfacing the error — drift is advisory, and the next real
    /// operation's reload path is the authoritative sync.
    ///
    /// - `EngineError::UnknownMem` when the name does not resolve.
    pub fn mem_drifted(&self, mem_name: &str) -> Result<bool, EngineError> {
        let m = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem_name)
            .ok_or_else(|| self.unknown_mem_error(mem_name))?;
        let live = m.backend.current_head().ok().flatten();
        Ok(live != m.last_known_head)
    }

    /// Workspace root the engine booted from, when one is known.
    /// `None` for engines built directly from a mount list (tests,
    /// ad-hoc consumers). Set by [`Self::from_workspace_root`] and
    /// the full counterpart.
    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    /// Typed warnings surfaced during mem load — drift findings
    /// the loader pipeline collects per entity. Empty for V1; the
    /// accessor surfaces them so handlers can merge into health
    /// summaries uniformly.
    pub fn load_warnings(&self) -> &[WarningHint] {
        &self.load_warnings
    }

    /// The quarantine roster: mems that failed their mem-level boot
    /// step and serve nothing until repaired + reloaded. Empty on a
    /// fully healthy workspace. Surfaced on overview and health.
    pub fn quarantined_mems(&self) -> &[crate::engine::QuarantinedMem] {
        &self.quarantined
    }

    /// The quarantine entry for `mem`, when it is quarantined.
    pub fn quarantine_reason(&self, mem: &str) -> Option<&crate::engine::QuarantinedMem> {
        self.quarantined.iter().find(|q| q.mount.mem == mem)
    }

    /// Whether the store's entity set for `mem` can be trusted to answer the
    /// question "does this entity still exist?".
    ///
    /// WHY this is a question and not an assumption: an anchor sidecar is
    /// keyed by entity id, and a key with no entity behind it is a dangling
    /// row (consistency-sweep 03/02). Detecting one means reading a NEGATIVE
    /// from the store, and a negative is only evidence when the store is known
    /// to hold everything the mem has. Four states break that, and each of
    /// them would otherwise turn every anchor in the mem into a false dangling
    /// report: the mem is not mounted at all, it is quarantined (serving
    /// nothing), its lazy load has not run yet (mounted, entities absent), or
    /// a file in it failed to parse, in which case an id missing from the
    /// store may be a load failure rather than a deleted entity.
    ///
    /// The last case is deliberately COARSE for non-folder mounts: load-error
    /// paths are normalized to absolute only for folder mounts, so a
    /// git-branch mem's errors carry mem-relative paths that two mems can
    /// spell identically. Attributing them by name would be a guess, and a
    /// wrong guess here fabricates dangling rows. Any load error at all
    /// therefore blocks reconciliation for a non-folder mem. The caller states
    /// the block rather than skipping silently, which is the honest direction.
    pub fn entity_set_is_reconcilable(&self, mem: &str) -> Result<(), &'static str> {
        if self.quarantine_reason(mem).is_some() {
            return Err("the mem is quarantined and serves no entities");
        }
        let Some(mount) = self.mounts.iter().find(|m| m.mount.mem == mem) else {
            return Err("the mem is not mounted here");
        };
        if mount.deferred {
            return Err("the mem's lazy entity load has not run this session");
        }
        if self.load_errors.is_empty() {
            return Ok(());
        }
        match &mount.mount.storage {
            crate::workspace::MountStorage::Folder { path } => {
                if self.load_errors.iter().any(|(p, _)| p.starts_with(path)) {
                    Err(
                        "a file in this mem failed to parse, so an id missing from the store may be a load failure rather than a deleted entity",
                    )
                } else {
                    Ok(())
                }
            }
            _ => Err(
                "this workspace has files that failed to parse, and their paths cannot be attributed to one mem",
            ),
        }
    }

    /// Whether `id` names an entity this mem no longer holds. A STUB counts
    /// as missing: a stub is the placeholder an unresolved wiki-link target
    /// leaves behind, not an entity anyone wrote, so an anchor keyed to one
    /// is dangling exactly as if nothing were there.
    ///
    /// Only meaningful once [`Self::entity_set_is_reconcilable`] says yes.
    pub fn entity_is_absent(&self, id: &EntityId) -> bool {
        self.store.get(id).is_none_or(|e| e.stub)
    }

    /// Mems whose lazy entity load is still DEFERRED — on the mount
    /// roster with a resolved schema pin, but with no entities in the
    /// store yet. Read surfaces that render per-mem counts or
    /// distributions consult this: a count over a deferred mem's slice
    /// of the store is a count over nothing, and rendering it as a
    /// bare zero is the silent absence the lazy-mount contract forbids.
    /// Either trigger the load ([`Self::ensure_mems_loaded`]) or render
    /// the load state explicitly.
    pub fn deferred_mems(&self) -> Vec<&str> {
        self.mounts
            .iter()
            .filter(|m| m.deferred)
            .map(|m| m.mount.mem.as_str())
            .collect()
    }

    /// Whether `mem` is a lazy mount whose entity load has not run yet.
    pub fn mem_is_deferred(&self, mem: &str) -> bool {
        self.mounts.iter().any(|m| m.deferred && m.mount.mem == mem)
    }

    /// The typed error for a mem name that did not resolve to a
    /// serving mount: `MEM_QUARANTINED` (carrying the underlying boot
    /// failure and its repair command) when the mem is on the
    /// quarantine roster, `UNKNOWN_MEM` otherwise. Every lookup site
    /// that fails to find a mem routes here so a quarantined mem is
    /// never misreported as unknown — honest absence, with the reason.
    /// Resolve an entity id that may lack its mem prefix — the one
    /// rule every id-taking mutation verb applies before it reads the
    /// id's mem. A full id (`mem--slug`) returns unchanged with no
    /// hint, byte for byte the path it always took. A bare slug is
    /// looked up across every mounted mem (loading deferred mems
    /// first, so a lazily mounted carrier is not invisible): exactly
    /// one carrier resolves the id and returns the
    /// [`crate::ops::WarningHint::ShortIdResolved`] announcement the
    /// verb rides on its outcome; zero or several carriers refuse
    /// [`EngineError::EntityIdMissingMem`] naming every candidate.
    /// Before this rule a bare slug reached the verbs as an id whose
    /// mem was the empty string, and the caller was told a mem called
    /// "" did not exist (B7 grader finding, 2026-09-02).
    pub fn resolve_entity_id(
        &mut self,
        id: &EntityId,
    ) -> Result<(EntityId, Option<crate::ops::WarningHint>), EngineError> {
        if !id.mem().is_empty() {
            return Ok((id.clone(), None));
        }
        // Grammar before resolution: a malformed bare string is an
        // invalid id, not a slug nobody carries.
        let slug = id.0.as_str();
        if let Err(reason) = crate::entity::id::validate_id_path_grammar(slug) {
            return Err(EngineError::InvalidEntityId {
                id: id.0.clone(),
                reason,
            });
        }
        self.ensure_mems_loaded(None);
        let mut candidates: Vec<EntityId> = self
            .store
            .all_entities()
            .filter(|e| !e.stub && e.id.path() == slug)
            .map(|e| e.id.clone())
            .collect();
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        candidates.dedup();
        if candidates.len() == 1 {
            let resolved = candidates.remove(0);
            let hint = crate::ops::WarningHint::ShortIdResolved {
                given: id.0.clone(),
                resolved: resolved.clone(),
            };
            return Ok((resolved, Some(hint)));
        }
        Err(EngineError::EntityIdMissingMem {
            id: id.0.clone(),
            candidates: candidates.into_iter().map(|c| c.0).collect(),
        })
    }

    pub fn unknown_mem_error(&self, mem: &str) -> EngineError {
        match self.quarantine_reason(mem) {
            Some(q) => EngineError::MemQuarantined {
                mem: mem.to_string(),
                reason_code: q.reason_code.clone(),
                reason_message: q.reason_message.clone(),
            },
            None if self.recently_unmounted(mem) => EngineError::MemUnmounted {
                mem: mem.to_string(),
            },
            None => EngineError::UnknownMem(mem.to_string()),
        }
    }

    /// The workspace-level boot diagnosis a diagnostic-shell engine
    /// carries (`None` on ordinarily booted engines).
    pub fn boot_diagnosis(&self) -> Option<(&str, &str)> {
        self.boot_diagnosis
            .as_ref()
            .map(|(c, m)| (c.as_str(), m.as_str()))
    }

    /// Build a mem-less diagnostic-shell engine for a workspace whose
    /// boot failed at the WORKSPACE level (nothing loadable — e.g. an
    /// unparseable store). It serves no mems and no entities; its one
    /// job is answering overview/health with the typed boot diagnosis
    /// so a session can always ask WHY the graph is gone — the MCP
    /// server serves this instead of exiting into `-32000 Connection
    /// closed` (degrade, never disappear).
    pub fn diagnostic_shell(reason_code: String, reason_message: String) -> Engine {
        let mut engine =
            Engine::from_mounts(Vec::new()).expect("an empty mount list always constructs");
        engine.boot_diagnosis = Some((reason_code, reason_message));
        engine
    }

    /// Append boot-path quarantine entries recorded outside
    /// `from_mounts_inner` (backend-instantiation failures happen
    /// before the mount list reaches the engine constructor). Boot
    /// paths only — quarantine is a boot judgment, never a runtime
    /// mutation.
    pub fn extend_quarantine(&mut self, entries: Vec<crate::engine::QuarantinedMem>) {
        self.quarantined.extend(entries);
    }

    // ---------------------------------------------------------------
    // Read-side delegates onto the kernel ops/graph functions.
    //
    // The mem-router engine exposed each of these directly so the
    // MCP layer could call them without reaching into the store. The
    // unified engine mirrors that surface so the MCP migration is a
    // straight rename rather than a re-architecture.
    //
    // Multi-mem cache strategy: per-mem community detection and
    // per-mem search indexes are unnecessary at this layer — the
    // engine-wide store already carries every mount's edges; Louvain
    // and tantivy run once across the union. `mem_schemas` for
    // health/search is the engine's existing `schemas` field as-is.
    // ---------------------------------------------------------------
}
