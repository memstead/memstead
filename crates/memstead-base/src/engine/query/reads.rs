//! Plain reads: status and context, the search indexes and their maintenance, list and search, and the entity, provenance and capability reads.

use super::*;

impl Engine {
    /// Engine-wide [`crate::ops::Status`] across every mount — the graph
    /// counts behind `memstead status` (renamed from `stats` with the
    /// command, D11; fields unchanged).
    pub fn status(&self) -> crate::ops::Status {
        let mut types_in_use: Vec<String> = self
            .store
            .all_entities()
            .filter(|e| !e.stub && !e.entity_type.is_empty())
            .map(|e| e.entity_type.clone())
            .collect();
        types_in_use.sort();
        types_in_use.dedup();

        let mut edge_types: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for id in self.store.all_ids() {
            for edge in self.store.outgoing(id) {
                *edge_types.entry(edge.rel_type.clone()).or_insert(0) += 1;
            }
        }

        crate::ops::Status {
            entity_count: self.store.all_entities().filter(|e| !e.stub).count(),
            edge_count: self.store.edge_count(),
            edge_types,
            community_count: self.communities().count,
            mem_count: self.mounts.len(),
            types_in_use,
        }
    }

    /// Build a [`ContextResult`] for `id`: the community cluster id
    /// (or `None` when the entity is a stub or not present), plus the
    /// outgoing + incoming neighbour lists.
    pub fn context(&self, id: &EntityId) -> Option<ContextResult> {
        let entity = self.store.get(id)?;
        let community = self
            .communities()
            .entity_cluster_map
            .get(id.as_ref())
            .cloned();
        let mut neighbors = Vec::new();
        for edge in self.store.outgoing(id) {
            if let Some(target) = self.store.get(&edge.target) {
                neighbors.push(NeighborInfo {
                    id: target.id.clone(),
                    title: target.title.clone(),
                    relationship: edge.rel_type.clone(),
                    direction: Direction::Outgoing,
                });
            }
        }
        for edge in self.store.incoming(id) {
            if let Some(source) = self.store.get(&edge.from) {
                neighbors.push(NeighborInfo {
                    id: source.id.clone(),
                    title: source.title.clone(),
                    relationship: edge.rel_type.clone(),
                    direction: Direction::Incoming,
                });
            }
        }
        Some(ContextResult {
            entity_id: entity.id.clone(),
            community,
            neighbors,
        })
    }

    /// Lazily-built per-mem search index map. The map carries one
    /// entry per writable mem. Build cost scales with entity count;
    /// expect hundreds-of-ms for thousand-entity workspaces. Not
    /// available on `wasm32` targets — search lives behind the bridge
    /// (see [`Self::search`] for the typed refuse).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn search_indexes(&self) -> &HashMap<String, MemIndex> {
        if let Some((memo_key, _)) = self.search_indexes_memo.get() {
            debug_assert_eq!(
                *memo_key,
                self.derived_key(),
                "search memo key lags the engine — a mutation path missed invalidate_search_indexes"
            );
        }
        &self
            .search_indexes_memo
            .get_or_init(|| (self.derived_key(), build_all(&self.store, &self.schemas)))
            .1
    }

    /// Drop the cached per-mem search index map. No-op on `wasm32`
    /// where no index exists; the method stays present so mutation
    /// hooks can call it unconditionally.
    /// Incrementally maintain the search-index memo for a known
    /// touched-id set: replace or remove
    /// exactly the touched documents in place and advance the memo's
    /// key, instead of dropping the whole map. Semantics:
    ///
    /// - Memo empty → nothing to maintain; the next read builds fresh.
    /// - Memo current (key matches) → no-op (rollback already landed).
    /// - Schemas epoch moved → the index FIELD SET may have changed:
    ///   the named, scoped fallback — drop the memo for a full
    ///   rebuild. Never a silent widening: this is the one case the
    ///   plan names (schema-shape change).
    /// - Otherwise: per touched id, a real non-stub entity in the
    ///   store is re-indexed (delete-then-add on the id term), and an
    ///   absent or stub entry is removed — stubs stay excluded exactly
    ///   as the bulk build excludes them. Touched mems' writers
    ///   commit; any tantivy error falls back to dropping the memo
    ///   (warn-logged), never to serving a stale index.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn maintain_search_indexes(&mut self, touched: &[crate::EntityId]) {
        let current = crate::engine::DerivedKey {
            store_generation: self.store.generation(),
            schemas_epoch: self.schemas_epoch,
        };
        let Some((memo_key, indexes)) = self.search_indexes_memo.get_mut() else {
            return;
        };
        if *memo_key == current {
            return;
        }
        if memo_key.schemas_epoch != current.schemas_epoch {
            self.search_indexes_memo = OnceCell::new();
            return;
        }
        let mut touched_mems: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for id in touched {
            let Some(idx) = indexes.get_mut(id.mem()) else {
                continue;
            };
            let result = match self.store.get(id) {
                Some(entity) if !entity.stub => idx.index_entity(entity),
                _ => idx.remove_entity(id),
            };
            if let Err(e) = result {
                tracing::warn!(
                    id = id.as_ref(),
                    error = %e,
                    "incremental index maintenance failed; dropping the memo for a full rebuild"
                );
                self.search_indexes_memo = OnceCell::new();
                return;
            }
            touched_mems.insert(id.mem());
        }
        for (mem, idx) in indexes.iter_mut() {
            if !touched_mems.contains(mem.as_str()) {
                continue;
            }
            if let Err(e) = idx.commit() {
                tracing::warn!(
                    mem = mem.as_str(),
                    error = %e,
                    "incremental index commit failed; dropping the memo for a full rebuild"
                );
                self.search_indexes_memo = OnceCell::new();
                return;
            }
        }
        *memo_key = current;
    }

    /// No-op shim on `wasm32`, mirroring `invalidate_search_indexes`
    /// so mutation paths call it unconditionally.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn maintain_search_indexes(&mut self, _touched: &[crate::EntityId]) {}

    /// Unconditionally drop the search-index memo, regardless of its
    /// generation. The forced variant exists for embedders that want
    /// to release the index's memory in a long-lived process (or force
    /// a from-scratch rebuild for verification); the generation-checked
    /// [`Self::invalidate_search_indexes`] stays the mutation-path
    /// hook.
    pub fn drop_search_indexes(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.search_indexes_memo = OnceCell::new();
        }
    }

    pub fn invalidate_search_indexes(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Same generation check as `invalidate_communities`: keep
            // the memo when the store still sits at its generation
            // (the batch-rollback case), clear otherwise.
            if let Some((memo_key, _)) = self.search_indexes_memo.get()
                && *memo_key == self.derived_key()
            {
                return;
            }
            self.search_indexes_memo = OnceCell::new();
        }
    }

    /// Filter the in-memory store by metadata only (no text match).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn list(&self, scope: &SearchScope) -> crate::ops::ListResult {
        let fallback = engine_fallback_type();
        crate::ops::search::list(&self.store, scope, fallback.as_ref(), &self.schemas)
    }

    /// Run a search against the lazily-built index map. Returns
    /// [`EngineError::SearchUnavailable`] on `wasm32` targets — browser
    /// consumers route search to the bridge; the local
    /// engine never builds a tantivy index in WASM. Native targets get
    /// the same shape as before, wrapped in `Ok`.
    pub fn search(&self, scope: &SearchScope) -> Result<SearchResult, EngineError> {
        // A mem filter naming a quarantined OR nonexistent mem refuses
        // typed `UNKNOWN_MEM`, matching every other mem-naming surface. A
        // success with 0 hits (the old nonexistent-mem behaviour, with a
        // missing-index warning) is indistinguishable from a true empty
        // result — the one thing a typed surface must never be.
        if let Some(mem) = scope.mem.as_deref()
            && (self.quarantine_reason(mem).is_some() || !self.mem_router.is_visible(mem))
        {
            return Err(self.unknown_mem_error(mem));
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = scope;
            return Err(EngineError::SearchUnavailable);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let fallback = engine_fallback_type();
            Ok(crate::ops::search::search(
                &self.store,
                scope,
                fallback.as_ref(),
                self.search_indexes(),
                &self.schemas,
            ))
        }
    }

    /// All mem-relative entity paths under `mem`. Delegates to
    /// the backend's `list_entities`. Order is backend-defined.
    pub fn list_entities(&self, mem: &str) -> Result<Vec<PathBuf>, EngineError> {
        let m = self.find_mount(mem)?;
        m.backend.list_entities().map_err(EngineError::Backend)
    }

    /// Raw bytes for a single entity (`Ok(None)` if absent).
    pub fn read_entity(&self, mem: &str, rel_path: &Path) -> Result<Option<Vec<u8>>, EngineError> {
        let m = self.find_mount(mem)?;
        m.backend
            .read_entity(rel_path)
            .map_err(EngineError::Backend)
    }

    /// Provenance entries for `mem` since `cursor`. Cursor shape is
    /// backend-specific (RFC-3339 timestamp for folder, commit SHA for
    /// git-branch); `None` means "from the beginning".
    pub fn read_provenance(
        &self,
        mem: &str,
        cursor: Option<&str>,
    ) -> Result<Vec<Provenance>, EngineError> {
        let m = self.find_mount(mem)?;
        m.backend
            .read_provenance(cursor)
            .map_err(EngineError::Backend)
    }

    /// Capability declared on the mount for `mem`. Surfaced for
    /// callers that need to gate before dispatching a write — the
    /// engine itself does not yet enforce capability (mutation paths
    /// land in a later session).
    pub fn capability(&self, mem: &str) -> Result<crate::workspace::MountCapability, EngineError> {
        let m = self.find_mount(mem)?;
        Ok(m.mount.capability)
    }

    /// Returns `true` when `from`'s source mem is mounted with
    /// [`crate::workspace::MountCapability::ReadOnly`]. Returns
    /// `false` for Write-Mems and for mems whose mount is absent
    /// from the router (no mount → no ReadOnly assertion can be
    /// made; the absence is treated as not-ReadOnly so consumers
    /// don't trip on transient lookup misses).
    ///
    /// Plan body §"Single edge source in the store" specifies this
    /// helper as the derived-on-demand alternative to adding a new
    /// field on [`crate::store::Edge`]. Strict-invariant validators
    /// and surfaces that want to highlight cross-mount references
    /// call this rather than pattern-matching on a per-edge marker.
    /// The information is fully derivable from the current mount
    /// roster, so no new state needs to live on the edge itself.
    pub fn edge_is_from_readonly(&self, from: &EntityId) -> bool {
        match self.capability(from.mem()) {
            Ok(crate::workspace::MountCapability::ReadOnly) => true,
            Ok(crate::workspace::MountCapability::Write) | Err(_) => false,
        }
    }
}
