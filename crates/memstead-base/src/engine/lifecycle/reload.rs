//! Reload: loading mems on demand, reloading one mem or every writable one with their reports, the quarantine reattach, and the settings refresh.

use super::*;

impl Engine {
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
    pub(super) fn set_schema_on_quarantined(
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
            .chain(crate::engine::boot::embedded_archive_schemas(&mount))
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
        let unbacked = crate::engine::boot::unbacked_mount_warning(
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
    pub(super) fn refresh_workspace_settings_if_possible(&mut self) {
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
pub(super) fn changed_config_fields(
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
