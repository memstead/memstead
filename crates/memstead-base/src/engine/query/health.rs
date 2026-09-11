//! Derived health: communities and labelling, the orphan, stub and connectivity reads, the constraint, conformance and consistency findings, the signals, and the health report they compose into.

use super::*;

impl Engine {
    /// Lazy community-detection cache. First call runs Louvain
    /// against the current store using one pinned schema for
    /// `community.{resolution, seed}` and the per-rel weights.
    /// Subsequent calls return the cached result. Mutations invalidate
    /// the cache via [`Self::invalidate_communities`].
    ///
    /// One detection run per engine. The partition is workspace-global,
    /// so it needs a single source for the Louvain parameters; that
    /// source is the schema of the lexicographically-first mem name —
    /// a stable key, so the partition is deterministic across processes
    /// even when mounts pin heterogeneous schemas. For a single-schema
    /// workspace every mem's schema is identical, so the choice of
    /// key is immaterial there.
    pub fn communities(&self) -> &LouvainOutput {
        // Generation sanity: a stored memo must match the live store
        // generation — the mutation hooks clear stale memos before any
        // read can arrive here. A mismatch would mean a mutation path
        // skipped its invalidation call (the exact silent-staleness
        // bug the generation exists to catch).
        if let Some((memo_key, _)) = self.community_memo.get() {
            debug_assert_eq!(
                *memo_key,
                self.derived_key(),
                "community memo key lags the engine — a mutation path missed invalidate_communities"
            );
        }
        &self
            .community_memo
            .get_or_init(|| (self.derived_key(), self.compute_communities()))
            .1
    }

    /// The current validity key for derived-structure memos: store
    /// generation plus schemas epoch (see [`crate::engine::DerivedKey`]).
    pub fn derived_key(&self) -> crate::engine::DerivedKey {
        crate::engine::DerivedKey {
            store_generation: self.store.generation(),
            schemas_epoch: self.schemas_epoch,
        }
    }

    fn compute_communities(&self) -> LouvainOutput {
        {
            // Select the parameter schema by a stable key (smallest
            // mem name) rather than unordered-map iteration, so the
            // partition does not vary between processes. Fall back to
            // the builtin default for the empty-mounts case (caller
            // still gets a valid empty Louvain result against an empty
            // store).
            let schema = self
                .schemas
                .iter()
                .min_by(|a, b| a.0.cmp(b.0))
                .map(|(_, s)| s.clone())
                .unwrap_or_else(Schema::builtin_default);
            let manifest = &schema.manifest;
            let resolution = manifest.community.resolution;
            let seed = manifest.community.seed;
            let schema_for_weights = schema.clone();
            detect_communities(&self.store, resolution, seed, move |rel_type| {
                schema_for_weights
                    .manifest
                    .relationships
                    .definitions
                    .iter()
                    .find(|d| d.name == rel_type)
                    .map(|d| d.default_weight as f64)
                    .unwrap_or(1.0)
            })
        }
    }

    /// Drop the cached community detection result and the grounded
    /// labelling memo — unless the store still sits at the generation
    /// each memo was computed from (flywheel W8/01). The keep case is
    /// exactly the batch-rollback path: the restored snapshot restored
    /// the generation with it, so the memo describes the live state
    /// and recomputing it would be pure waste. Every real mutation
    /// bumps the generation first, so those clears behave as before.
    /// Coupling the labelling reset here means every site that already
    /// invalidates communities (all mutation paths, drift reload,
    /// quarantine attach/detach, apply-commit) invalidates the
    /// labelling too, so a stale label can never outlive the state
    /// change that moved it.
    pub fn invalidate_communities(&mut self) {
        let key = self.derived_key();
        if !matches!(self.community_memo.get(), Some((k, _)) if *k == key) {
            self.community_memo = OnceCell::new();
        }
        if !matches!(self.labelling_memo.get(), Some((k, _)) if *k == key) {
            self.labelling_memo = OnceCell::new();
        }
    }

    /// The grounded labelling of one mem — `None` when its pinned
    /// schema declares no `relationships.labelling`. Computed on
    /// first access for every declaring mem, memoised until the next
    /// invalidation; generation-keyed like `community_memo`.
    pub fn mem_labelling(&self, mem: &str) -> Option<&crate::ops::labelling::MemLabelling> {
        // Generation sanity, same contract as `communities()`: a
        // stored memo must match the live derived key — a mismatch
        // means a mutation path missed invalidate_communities.
        if let Some((memo_key, _)) = self.labelling_memo.get() {
            debug_assert_eq!(
                *memo_key,
                self.derived_key(),
                "labelling memo key lags the engine — a mutation path missed invalidate_communities"
            );
        }
        let (_, map) = self.labelling_memo.get_or_init(|| {
            let mut out = std::collections::HashMap::new();
            for (mem_name, schema) in &self.schemas {
                if let Some(lab) = crate::ops::labelling::labelling_of(schema) {
                    out.insert(
                        mem_name.clone(),
                        crate::ops::labelling::compute_mem_labelling(
                            &self.store,
                            mem_name,
                            &lab.attack,
                        ),
                    );
                }
            }
            (self.derived_key(), out)
        });
        map.get(mem)
    }

    /// One entity's served labelling view — `None` when the entity is
    /// a stub or its mem's schema declares no labelling; serving
    /// surfaces then keep their byte-identical payloads. The shape
    /// block is present exactly when the declaration carries a
    /// `support` walk.
    pub fn computed_labelling(
        &self,
        entity: &Entity,
    ) -> Option<crate::ops::labelling::LabellingView> {
        use crate::ops::labelling::{Label, compute_shape, labelling_of};
        if entity.stub {
            return None;
        }
        let schema = self.schemas.get(entity.mem.as_str())?;
        let lab = labelling_of(schema)?;
        let mem_lab = self.mem_labelling(entity.mem.as_str())?;
        let label = *mem_lab.labels.get(entity.id.0.as_str())?;
        let defeated_by = if label == Label::Defeated {
            mem_lab.accepted_attackers_of(entity.id.0.as_str())
        } else {
            Vec::new()
        };
        let undecided_by = if label == Label::Undecided {
            mem_lab.undecided_attackers_of(entity.id.0.as_str())
        } else {
            Vec::new()
        };
        let shape = lab.support.as_ref().map(|walk| {
            let label_of = |id: &EntityId| -> Option<Label> {
                self.mem_labelling(id.mem())
                    .and_then(|ml| ml.labels.get(id.0.as_str()).copied())
            };
            compute_shape(&self.store, &entity.id, walk, &label_of)
        });
        Some(crate::ops::labelling::LabellingView {
            label,
            defeated_by,
            undecided_by,
            shape,
        })
    }

    /// The `labelling` health axis payload — per declaring mem:
    /// counts per label, the defeated list with its accepted
    /// attackers, the undecided list with its open attacker set, and
    /// the excluded cross-mem attack-edge count. One composer shared
    /// by the CLI health command and the MCP server.
    pub fn health_labelling_axis(&self, mem_filter: Option<&str>) -> serde_json::Value {
        use crate::ops::labelling::Label;
        let mut mems = serde_json::Map::new();
        let mut mem_names: Vec<&String> = self.schemas.keys().collect();
        mem_names.sort();
        for mem in mem_names {
            if let Some(v) = mem_filter
                && mem != v
            {
                continue;
            }
            let Some(ml) = self.mem_labelling(mem) else {
                continue;
            };
            let mut accepted = 0usize;
            let mut defeated: Vec<serde_json::Value> = Vec::new();
            let mut undecided: Vec<serde_json::Value> = Vec::new();
            for (id, label) in &ml.labels {
                match label {
                    Label::Accepted => accepted += 1,
                    Label::Defeated => defeated.push(serde_json::json!({
                        "id": id,
                        "defeated_by": ml.accepted_attackers_of(id),
                    })),
                    Label::Undecided => undecided.push(serde_json::json!({
                        "id": id,
                        "undecided_by": ml.undecided_attackers_of(id),
                    })),
                }
            }
            mems.insert(
                mem.clone(),
                serde_json::json!({
                    "counts": {
                        "accepted": accepted,
                        "defeated": defeated.len(),
                        "undecided": undecided.len(),
                    },
                    "defeated": defeated,
                    "undecided": undecided,
                    "cross_mem_edges_excluded": ml.cross_mem_edges_excluded,
                }),
            );
        }
        serde_json::Value::Object(mems)
    }

    /// Real entities with no incoming or outgoing edges — leaf-declared
    /// types exempt (their edge-less entities are terminal by
    /// construction; see [`Self::leaf_population`]).
    pub fn orphans(&self) -> Vec<EntityId> {
        crate::graph::query::find_orphans_with_schemas(&self.store, &self.schemas)
    }

    /// Count of real entities per leaf-declared type, keyed
    /// `<schema_ref>:<type>` — the visible population the orphan
    /// exemption covers.
    pub fn leaf_population(&self) -> std::collections::BTreeMap<String, usize> {
        crate::graph::query::leaf_population(&self.store, &self.schemas)
    }

    /// Stub entities with their referencer ids.
    pub fn stubs(&self) -> Vec<(EntityId, Vec<EntityId>)> {
        crate::graph::query::find_stubs(&self.store)
    }

    /// Top `limit` entities by total degree.
    pub fn most_connected(&self, limit: usize) -> Vec<crate::graph::query::Connectivity> {
        crate::graph::query::most_connected(&self.store, limit)
    }

    /// Entities whose type's `required_outgoing` blocks are not yet
    /// satisfied. `mem_filter = None` scans every mem; `Some(v)`
    /// scans only that mem.
    pub fn missing_required_outgoing(
        &self,
        mem_filter: Option<&str>,
    ) -> Vec<crate::ops::health::MissingRequiredOutgoingReport> {
        crate::ops::health::collect_missing_required_outgoing(
            &self.store,
            mem_filter,
            &self.schemas,
        )
    }

    /// Standing violations of declared `constraints` (the health
    /// `constraints` include) — every non-stub entity whose type
    /// declares constraints its current state violates, in
    /// deterministic `(mem, id)` order.
    pub fn constraint_findings(
        &self,
        mem_filter: Option<&str>,
    ) -> Vec<crate::ops::health::ConstraintFindingReport> {
        let check_provider = self.check_standing_provider();
        crate::ops::health::collect_constraint_findings(
            &self.store,
            mem_filter,
            &self.schemas,
            Some(&check_provider),
        )
    }

    /// The evaluated aggregate signals for one entity — `None` when
    /// the mem has no schema, the type is unknown or a stub, or the
    /// type declares no signals; serving surfaces then keep their
    /// byte-identical payloads.
    pub fn computed_signals(
        &self,
        entity: &Entity,
    ) -> Option<Vec<crate::ops::signals::ComputedSignal>> {
        if entity.stub {
            return None;
        }
        let schema = self.schemas.get(entity.mem.as_str())?;
        let td = schema.types.get(entity.entity_type.as_str())?;
        if td.signals.is_empty() {
            return None;
        }
        Some(crate::ops::signals::compute_signals(
            &self.store,
            td,
            &entity.id,
        ))
    }

    /// Every entity carrying at least one signal above `none` — the
    /// include-gated `signals` health axis.
    pub fn signal_reports(
        &self,
        mem_filter: Option<&str>,
    ) -> Vec<crate::ops::health::SignalReport> {
        crate::ops::health::collect_signal_reports(&self.store, mem_filter, &self.schemas)
    }

    /// The `signals` health axis payload — the entity roster plus
    /// per-level counts. One composer shared by the CLI health
    /// command and the MCP server so the axis cannot drift
    /// between surfaces.
    pub fn health_signals_axis(&self, mem_filter: Option<&str>) -> serde_json::Value {
        use memstead_schema::SignalLevel;
        let reports = self.signal_reports(mem_filter);
        let mut notice = 0usize;
        let mut warn = 0usize;
        for r in &reports {
            for s in &r.signals {
                match s.level {
                    Some(SignalLevel::Notice) => notice += 1,
                    Some(SignalLevel::Warn) => warn += 1,
                    None => {}
                }
            }
        }
        serde_json::json!({
            "entities": reports,
            "counts": { "notice": notice, "warn": warn },
        })
    }

    /// Defective section-format declarations the loaded schemas carry
    /// (lenient boot recorded them; install would have refused).
    pub fn schema_format_defects(&self) -> Vec<crate::ops::health::SchemaFormatDefect> {
        crate::ops::health::collect_schema_format_defects(&self.schemas)
    }

    /// Conformance-axis integrity findings for one mem — which
    /// entities a write would refuse under the effective schema, and
    /// why. `target_schema = None` lints against the mem's current
    /// pin; `Some(ref)` lints against that schema instead (resolved
    /// among mem-pinned, workspace, and built-in schemas).
    pub fn conformance_findings(
        &self,
        mem: &str,
        target_schema: Option<&memstead_schema::SchemaRef>,
    ) -> Result<Vec<crate::ops::integrity::IntegrityFinding>, EngineError> {
        let pinned = self
            .schemas
            .get(mem)
            .ok_or_else(|| self.unknown_mem_error(mem))?;
        let effective: Arc<Schema> = match target_schema {
            None => pinned.clone(),
            Some(target) => self.resolve_schema_by_ref(target).ok_or_else(|| {
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
            })?,
        };
        Ok(crate::ops::integrity::conformance_findings(
            &self.store,
            mem,
            &effective,
            &self.schemas,
        ))
    }

    /// What `mem`'s entity BODIES carry that their types do not declare
    /// (consistency-sweep 04/01): headings absorbed into the catch-all,
    /// headings repeated so that later bodies were not kept, and frontmatter
    /// keys the next write will drop.
    ///
    /// A folder mem's ledger set against its file set, per mem.
    ///
    /// **Folder mems only, and that is the point.** On a
    /// git-branch mem the change set is a real two-tree diff against the
    /// committed tree, so ledger-versus-files divergence is structurally
    /// impossible; emitting an always-clean version of this check there would
    /// be a surface asserting something it never had to establish, which is
    /// the failure class this bundle exists to remove. Such a mem is absent
    /// from the map rather than present and empty.
    pub fn ledger_reconciliation(
        &self,
    ) -> std::collections::BTreeMap<String, crate::filesystem::changelog::LedgerReconciliation>
    {
        let mut out = std::collections::BTreeMap::new();
        for m in &self.mounts {
            let crate::workspace::MountStorage::Folder { path } = &m.mount.storage else {
                continue;
            };
            if let Ok(r) = crate::filesystem::changelog::reconcile_ledger(path) {
                out.insert(m.mount.mem.clone(), r);
            }
        }
        out
    }

    /// Separate from [`Self::conformance_findings`] on purpose. These are
    /// observations, not violations: absorbing an undeclared heading is the
    /// catch-all working as designed, and reporting it as a finding would fail
    /// every mem that uses the feature. What a reader needs here is whether the
    /// content SURVIVES, which each observation states.
    pub fn body_observations(
        &self,
        mem: &str,
        target_schema: Option<&memstead_schema::SchemaRef>,
    ) -> Result<Vec<crate::ops::integrity::BodyObservation>, EngineError> {
        let pinned = self
            .schemas
            .get(mem)
            .ok_or_else(|| self.unknown_mem_error(mem))?;
        let effective: Arc<Schema> = match target_schema {
            None => pinned.clone(),
            Some(target) => {
                self.resolve_schema_by_ref(target)
                    .ok_or_else(|| EngineError::SchemaNotFound {
                        mem: mem.to_string(),
                        pin: target.as_display(),
                        sources: Vec::new(),
                        install_hint: None,
                    })?
            }
        };
        Ok(crate::ops::integrity::body_observations(
            &self.store,
            mem,
            &effective,
        ))
    }

    /// Resolve an exact `name@version` ref against every schema this
    /// engine can see: mem-pinned, workspace-authored, built-in.
    /// `None` when no loaded schema matches.
    pub(crate) fn resolve_schema_by_ref(
        &self,
        target: &memstead_schema::SchemaRef,
    ) -> Option<Arc<Schema>> {
        self.schemas
            .values()
            .chain(self.workspace_schemas.iter())
            .chain(self.builtin_schemas.iter())
            .find(|s| {
                let (name, version) = s.id();
                name == target.name && version == target.version
            })
            .cloned()
    }

    /// The mem's `Mount.schema` expectation assertion, when set.
    /// `None` for unknown mems *and* for mems whose mount carries no
    /// assertion (the authoritative pin then lives in the backend
    /// config; the resolved active schema, not this, is the effective pin).
    pub fn schema_pin(&self, mem: &str) -> Option<memstead_schema::SchemaRef> {
        self.mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| m.mount.schema.clone())
    }

    /// The mem's in-flight migration target, when dual-pin state is
    /// active. `None` for settled or unknown mems.
    pub fn migration_target(&self, mem: &str) -> Option<memstead_schema::SchemaRef> {
        self.mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .and_then(|m| m.mount.migration_target.clone())
    }

    /// Consistency-axis integrity findings for one mem — the
    /// pre-existing graph-coherence categories (dangling links, stubs)
    /// plus cross-mem edges the workspace grant table no longer permits,
    /// projected into the `{ id, axis, code, detail }` finding shape.
    pub fn consistency_findings(
        &self,
        mem: &str,
    ) -> Result<Vec<crate::ops::integrity::IntegrityFinding>, EngineError> {
        if !self.schemas.contains_key(mem) {
            return Err(self.unknown_mem_error(mem));
        }
        let mut findings = crate::ops::integrity::consistency_findings(
            &self.store,
            mem,
            // The one grant resolver, the same one the write gate calls. Every
            // consumer of the axis reaches it through this funnel, so there is
            // no site where a second answer could be written (04/07).
            &|from, to| self.cross_mem_link_allowed(from, to),
            // The mount set, so an edge into a mem that is not mounted is
            // reported as the dangling finding it is and never as a grant
            // the table still names.
            &|target| self.mount(target).is_some(),
        );
        // The provenance layer's own consistency: a sidecar the engine cannot
        // read is a finding on the mem, beside the entity-level ones. Without
        // it every anchor surface reads zero rows over this mem and calls
        // that clean (backlog, found by the evidence-engine grader 2026-09-02).
        if let Some(why) = self.anchors_sidecar_error(mem) {
            findings.push(crate::ops::integrity::IntegrityFinding {
                id: mem.to_string(),
                axis: crate::ops::integrity::IntegrityAxis::Consistency,
                code: "ANCHORS_SIDECAR_UNREADABLE".to_string(),
                detail: serde_json::json!({
                    "mem": mem,
                    "reason": why,
                    "repair": "the anchors sidecar is unreadable, so every anchor surface for this mem reports a condition instead of rows; fix or remove the sidecar file, then re-record anchors",
                }),
            });
        }
        Ok(findings)
    }

    /// Every cross-mem edge the workspace's current grant resolution does
    /// not permit, across every visible mem.
    ///
    /// A projection of [`Self::consistency_findings`] filtered to the one
    /// code, not a second scan: the revoke path and the health axis must
    /// never be able to answer differently about the same edge, and the
    /// surest way to guarantee that is for one of them to BE the other.
    ///
    /// Call it after the grant edit has landed and the settings have been
    /// reloaded — the answer is "what does the CURRENT policy leave
    /// unbacked", which is what an operator revoking a grant wants to know.
    ///
    /// Takes `&mut self` because it must load lazily-deferred mems first. A
    /// deferred mem's entities are not in the store, so scanning without the
    /// load would report zero ungranted edges for it and call that clean —
    /// which is precisely the silent all-clear this whole axis exists to
    /// prevent. The long-lived server engines carry lazy mounts; the CLI's
    /// fresh boot does not, so this only bites on the surface where it is
    /// hardest to notice.
    pub fn ungranted_cross_mem_edges(&mut self) -> Vec<crate::ops::integrity::IntegrityFinding> {
        self.ensure_mems_loaded(None);
        let mut mems: Vec<&String> = self.schemas.keys().collect();
        mems.sort();
        mems.into_iter()
            .filter_map(|mem| self.consistency_findings(mem).ok())
            .flatten()
            .filter(|f| f.code == "CROSS_MEM_EDGE_UNGRANTED")
            .collect()
    }

    /// The edges that went from permitted to unpermitted between two
    /// readings of [`Self::ungranted_cross_mem_edges`] — what a policy edit
    /// just orphaned, as opposed to what was already orphaned before it.
    ///
    /// A revocation that reported the whole standing set would blame this
    /// edit for every edge some earlier unrelated revocation left behind,
    /// which is a different and less useful claim.
    pub fn newly_ungranted(
        before: &[crate::ops::integrity::IntegrityFinding],
        after: Vec<crate::ops::integrity::IntegrityFinding>,
    ) -> Vec<crate::ops::integrity::IntegrityFinding> {
        let seen: std::collections::BTreeSet<(String, String)> = before
            .iter()
            .map(|f| (f.id.clone(), f.detail["target_id"].to_string()))
            .collect();
        after
            .into_iter()
            .filter(|f| !seen.contains(&(f.id.clone(), f.detail["target_id"].to_string())))
            .collect()
    }

    /// Engine-wide health summary across every mount.
    pub fn health(&self) -> crate::ops::HealthSummary {
        self.health_inner(None)
    }

    /// Health summary scoped to one visible mem. The scans and
    /// structural counts narrow to that mem; workspace-level facts
    /// (quarantine roster, boot diagnosis, workspace-scoped warnings)
    /// stay global — an agent scoping to one mem must still see them.
    /// A name that is quarantined or not on the visible roster refuses
    /// `UNKNOWN_MEM` — the same gate `search` applies to its `mem`
    /// filter, so "no such mem" and "healthy mem, nothing to report"
    /// can never be confused. `None` is the engine-wide sweep.
    pub fn health_scoped(
        &self,
        mem: Option<&str>,
    ) -> Result<crate::ops::HealthSummary, crate::EngineError> {
        if let Some(name) = mem
            && (self.quarantine_reason(name).is_some() || !self.mem_router.is_visible(name))
        {
            return Err(self.unknown_mem_error(name));
        }
        Ok(self.health_inner(mem))
    }

    fn health_inner(&self, mem: Option<&str>) -> crate::ops::HealthSummary {
        let fallback = engine_fallback_type();
        let mut summary =
            crate::ops::health::compute_health(&self.store, fallback.as_ref(), &self.schemas, mem);
        self.apply_anchor_clock(&mut summary, mem);
        // Merge in load-time drift warnings so every caller of
        // Engine::health — MCP handler, Swift FFI, direct CLI —
        // sees the SuspiciousNestedPrefix / DuplicateSectionHeading
        // findings without reaching into private engine state. The
        // MCP handler further appends request-scoped warnings on
        // top. Mirrors full's merge.
        if !self.load_warnings.is_empty() {
            let mut merged = self.load_warnings.clone();
            merged.append(&mut summary.warnings);
            summary.warnings = merged;
        }
        // A standing property, reported here rather than on every boot: a
        // folder mem's drift cursor is its own ledger, which only the engine
        // writes, so an edit made to its files by anything else is invisible
        // and reads keep serving the pre-edit content. Silence about that is
        // the one outcome this forbids. Git-branch mems are
        // absent: their change set is a real two-tree diff, so the condition
        // cannot arise.
        for m in &self.mounts {
            if matches!(
                m.mount.storage,
                crate::workspace::MountStorage::Folder { .. }
            ) && mem.is_none_or(|scope| scope == m.mount.mem)
            {
                summary
                    .warnings
                    .push(crate::ops::WarningHint::OutOfBandEditsUndetected {
                        mem: m.mount.mem.clone(),
                    });
            }
        }
        // Quarantine roster — a boot-honesty fact, present whenever
        // non-empty, never behind an include gate. Empty (and omitted
        // from the wire) on a healthy workspace.
        summary.quarantined = self
            .quarantined
            .iter()
            .map(|q| crate::ops::QuarantinedMemReport {
                mem: q.mount.mem.clone(),
                reason_code: q.reason_code.clone(),
                reason_message: q.reason_message.clone(),
            })
            .collect();
        // Per-file load failures ride the report unconditionally, like
        // the quarantine roster — each entry's message names the remedy
        // (the merge-conflict refusal names `memstead conflicts
        // resolve`), and a remedy only a library accessor carries is a
        // capability nobody finds at the moment it is needed.
        summary.load_errors = self
            .load_errors
            .iter()
            .map(|(path, msg)| crate::ops::LoadErrorReport {
                file: path.display().to_string(),
                error: msg.clone(),
            })
            .collect();
        summary.boot_diagnosis = self
            .boot_diagnosis
            .as_ref()
            .map(|(code, message)| serde_json::json!({ "code": code, "message": message }));
        // Surface OUTER_REPO_NOT_IGNORING_MEM_REPO when the
        // workspace is embedded inside a git repository whose
        // .gitignore does not list `mem-repo/`. Skipped when
        // workspace_root is unset (engine built ad-hoc from a mount
        // list).
        if let Some(root) = self.workspace_root.as_deref()
            && let Some(outer) = crate::workspace_root::find_enclosing_git_repo(root)
            && !crate::workspace_root::outer_repo_ignores_mem_repo(&outer, root)
        {
            summary
                .warnings
                .push(WarningHint::OuterRepoNotIgnoringMemRepo {
                    outer_repo_root: outer.display().to_string(),
                    workspace_root: root.display().to_string(),
                });
        }
        // Authoring-drift axis: for every pinned schema whose sealed
        // copy carries an install-provenance stamp, report a MISSING
        // authoring package (stamped path gone) or a DIVERGED one
        // (present but no longer parsed-equivalent to the seal).
        // Unstamped schemas — sealed pre-stamp, built-ins, archive
        // installs — produce no finding. Read-only on both copies.
        summary.warnings.extend(self.authoring_drift_findings());
        // Rot axis for the pins the drift axis skips: an UNSTAMPED
        // sealed package whose content no longer passes current-
        // language authoring validation gets its own low-tier hint —
        // the holding runs fine on the tolerant seal, but the package
        // (and the unlocatable authoring source it came from) is no
        // longer installable, and nothing else would say so before the
        // next install attempt. A parsing unstamped package stays
        // silent; stamped pins are the drift axis's business.
        summary.warnings.extend(self.unstamped_rot_findings());
        // Under a mem scope, mem-attributable warnings narrow to the
        // scoped mem; workspace- and request-scoped warnings return
        // `None` from `source_mem()` and stay visible regardless.
        // Mirrors the health composer's filter.
        if let Some(v) = mem {
            summary
                .warnings
                .retain(|w| w.source_mem().is_none_or(|wv| wv == v));
        }
        summary
    }

    /// Compute the authoring-drift findings for every stamped pinned
    /// schema. See the call site in [`Self::health`] for the axis
    /// contract; returns an empty list when no workspace root is set
    /// (ad-hoc mount-list engines have no authoring tree to check).
    fn authoring_drift_findings(&self) -> Vec<WarningHint> {
        let Some(root) = self.workspace_root.as_deref() else {
            return Vec::new();
        };
        // Group pinning mems by (name, version) — BTreeMap for a
        // deterministic finding order.
        let mut pins: std::collections::BTreeMap<(String, String), Vec<String>> =
            std::collections::BTreeMap::new();
        for (mem, schema) in &self.schemas {
            let (name, version) = schema.id();
            pins.entry((name.to_string(), version.to_string()))
                .or_default()
                .push(mem.clone());
        }
        let mut out = Vec::new();
        for ((name, version), mut mems) in pins {
            mems.sort();
            let Some(stamped_path) = self.read_install_provenance(root, &name, &version) else {
                continue;
            };
            let schema_ref = format!("{name}@{version}");
            // A workspace-relative stamp (the portable form the installer
            // writes for in-workspace authoring dirs) resolves against
            // THIS workspace root, so the axis checks real drift on every
            // clone instead of reporting another machine's absolute path
            // as missing. Absolute stamps stay machine-pinned as-is.
            let stamped = std::path::Path::new(&stamped_path);
            let resolved: std::path::PathBuf = if stamped.is_absolute() {
                stamped.to_path_buf()
            } else {
                root.join(stamped)
            };
            let authoring = resolved.as_path();
            if !authoring.is_dir() {
                out.push(WarningHint::SchemaAuthoringSourceMissing {
                    schema_ref,
                    stamped_path,
                    mems,
                });
                continue;
            }
            let sealed = self
                .schemas
                .get(&mems[0])
                .expect("mems collected from self.schemas keys")
                .clone();
            match memstead_schema::load_schema_from_dir(authoring) {
                Err(e) => out.push(WarningHint::SchemaAuthoringSourceDiverged {
                    schema_ref,
                    stamped_path,
                    mems,
                    detail: format!("the authoring package no longer loads: {e}"),
                }),
                Ok(authored) => {
                    if schema_parsed_fingerprint(&authored) != schema_parsed_fingerprint(&sealed) {
                        out.push(WarningHint::SchemaAuthoringSourceDiverged {
                            schema_ref,
                            stamped_path,
                            mems,
                            detail: "the parsed authoring package differs from the sealed copy \
                                     the engine runs on"
                                .to_string(),
                        });
                    }
                }
            }
        }
        out
    }

    /// Compute the rot findings for every UNSTAMPED pinned schema: read
    /// the sealed package's content back (folder seal directory, or the
    /// `__MEMSTEAD:schemas/` ref via the ops bundle) and run the
    /// authoring-tier check over it. A pin with a stamp is skipped (the
    /// divergence axis owns it); a pin with no readable sealed package
    /// — built-ins resolving from the embedded catalogue — is skipped
    /// too (nothing on disk can rot). See the call site in
    /// [`Self::health`] for the axis contract.
    fn unstamped_rot_findings(&self) -> Vec<WarningHint> {
        let Some(root) = self.workspace_root.as_deref() else {
            return Vec::new();
        };
        let mut pins: std::collections::BTreeMap<(String, String), Vec<String>> =
            std::collections::BTreeMap::new();
        for (mem, schema) in &self.schemas {
            let (name, version) = schema.id();
            pins.entry((name.to_string(), version.to_string()))
                .or_default()
                .push(mem.clone());
        }
        let mut out = Vec::new();
        for ((name, version), mut mems) in pins {
            mems.sort();
            if self
                .read_install_provenance(root, &name, &version)
                .is_some()
            {
                continue; // stamped — the divergence axis checks it
            }
            let schema_ref = format!("{name}@{version}");
            // Folder seal: the sealed package is a real directory the
            // authoring loader can probe directly.
            let sealed_dir = root.join(".memstead").join("schemas").join(&schema_ref);
            let detail: Option<String> = if sealed_dir.join("schema.yaml").is_file() {
                memstead_schema::load_schema_from_dir(&sealed_dir)
                    .err()
                    .map(|e| e.to_string())
            } else if let Some((manifest, types)) = self.read_sealed_package_yamls(&name, &version)
            {
                memstead_schema::loader::check_package_reauthorable(&manifest, &types)
                    .err()
                    .map(|e| e.to_string())
            } else {
                None // no sealed copy anywhere — embedded builtin
            };
            if let Some(detail) = detail {
                out.push(WarningHint::SchemaUnstampedSourceRot {
                    schema_ref,
                    mems,
                    detail,
                });
            }
        }
        out
    }

    /// Read a sealed package's `schema.yaml` + `types/*.yaml` back from
    /// the `__MEMSTEAD:schemas/` ref, reconstructing the type-file names
    /// from the pinned parsed schema's type roster (seal-time authoring
    /// enforces stem == declared type name). `None` when the ops bundle
    /// or the package is absent — the embedded-builtin state.
    fn read_sealed_package_yamls(
        &self,
        name: &str,
        version: &str,
    ) -> Option<(String, Vec<(String, String)>)> {
        let ops = self.git_branch_ops()?;
        let root = self.workspace_root.as_deref()?;
        let gitdir = self
            .mounts
            .iter()
            .find_map(|m| match &m.mount.storage {
                crate::workspace::MountStorage::GitBranch { gitdir, .. } => Some(gitdir.clone()),
                _ => None,
            })
            .or_else(|| {
                let g = root.join("mem-repo").join(".git");
                g.is_dir().then_some(g)
            })?;
        let read = |rel: &str| -> Option<String> {
            (ops.read_schema_file)(&gitdir, name, version, rel)
                .ok()
                .flatten()
                .and_then(|bytes| String::from_utf8(bytes).ok())
        };
        let manifest = read("schema.yaml")?;
        let schema = self
            .schemas
            .values()
            .find(|s| {
                let (n, v) = s.id();
                n == name && v.to_string() == version
            })?
            .clone();
        let mut type_names: Vec<String> = schema.types.keys().cloned().collect();
        type_names.sort();
        let mut types = Vec::new();
        for t in type_names {
            if let Some(body) = read(&format!("types/{t}.yaml")) {
                types.push((t, body));
            }
        }
        Some((manifest, types))
    }

    /// Read the install-provenance stamp for a sealed schema package,
    /// checking the folder location first
    /// (`.memstead/schemas/<name>@<version>/`) and falling back to the
    /// `__MEMSTEAD:schemas/` ref via the git-branch ops bundle when
    /// wired. `None` when no stamp exists anywhere — the normal state
    /// for pre-stamp seals, built-ins, and archive installs.
    fn read_install_provenance(&self, root: &Path, name: &str, version: &str) -> Option<String> {
        let folder_stamp = root
            .join(".memstead")
            .join("schemas")
            .join(format!("{name}@{version}"))
            .join(memstead_schema::INSTALL_PROVENANCE_FILE);
        let bytes = if folder_stamp.is_file() {
            std::fs::read(&folder_stamp).ok()
        } else {
            let ops = self.git_branch_ops()?;
            let gitdir = self
                .mounts
                .iter()
                .find_map(|m| match &m.mount.storage {
                    crate::workspace::MountStorage::GitBranch { gitdir, .. } => {
                        Some(gitdir.clone())
                    }
                    _ => None,
                })
                .or_else(|| {
                    let g = root.join("mem-repo").join(".git");
                    g.is_dir().then_some(g)
                })?;
            (ops.read_schema_file)(
                &gitdir,
                name,
                version,
                memstead_schema::INSTALL_PROVENANCE_FILE,
            )
            .ok()
            .flatten()
        }?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        v.get("authoring_path")?.as_str().map(String::from)
    }
}
