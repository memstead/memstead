//! Anchor resolution: the per-entity and per-mem anchor reads, the observation and verification passes with their clock, and the free functions that resolve an artifact ref across a binding's sources.

use super::*;

impl Engine {
    /// Convenience: look up a parsed entity by id. Returns `None` for
    /// unknown ids, including stub entries created for unresolved
    /// inline-link targets — callers that want to distinguish real
    /// from stub branch on `Entity::stub`.
    pub fn get_entity(&self, id: &EntityId) -> Option<&Entity> {
        self.store.get(id)
    }

    /// The stored provenance anchors for `id`, read from its mem's
    /// anchors sidecar. Empty for an entity with none, an unknown mem, or
    /// a backend that does not persist anchors (a pre-anchor archive / any
    /// sealed read-only mount). Additive read surface: the
    /// resolution *model* lives in [`crate::anchor`]
    /// ([`crate::anchor::resolve_anchor`] / [`crate::anchor::compose_entity_anchors`]);
    /// the live per-anchor *state* (which requires observing the source
    /// artifacts through the medium/preparation pipeline) is the anchor
    /// observation pass's concern.
    /// The anchors sidecar's parse error for `mem`, if it has one.
    ///
    /// The anchor readers below degrade a malformed sidecar to "no anchors",
    /// which keeps a read path alive but makes a corrupt file
    /// indistinguishable from an empty one. For a *reader* that is the right
    /// trade; for anything that draws a conclusion from the absence of
    /// anchors it is not — a fidelity pass would report every artifact
    /// uncovered and call it a finding, when the truth is that it could not
    /// read the file. Callers that need that distinction ask here first and
    /// refuse. `None` means the sidecar is absent (legitimately no anchors
    /// yet) or parses cleanly; the binding store draws the same distinction
    /// with its quarantine path.
    /// The entities of `mem` that hold at least one anchor row — the
    /// sidecar's key set, no observation performed. What a coverage reading
    /// subtracts from the mem's entity roster to name the entities no
    /// artifact stands behind. Empty for a mem with no sidecar, an unmounted
    /// mem, or an unreadable sidecar (the latter is reported by
    /// [`Self::anchors_sidecar_error`], never inferred from an empty set).
    pub fn mem_anchor_holders(&self, mem: &str) -> std::collections::BTreeSet<String> {
        let Some(mount) = self.mounts.iter().find(|m| m.mount.mem == mem) else {
            return Default::default();
        };
        let Ok(Some(bytes)) = mount.backend.read_anchors_sidecar() else {
            return Default::default();
        };
        let Ok(sc) = crate::anchor::AnchorSidecar::from_bytes(&bytes) else {
            return Default::default();
        };
        sc.entities
            .iter()
            .filter(|(_, rows)| !rows.is_empty())
            .map(|(eid, _)| eid.clone())
            .collect()
    }

    pub fn anchors_sidecar_error(&self, mem: &str) -> Option<String> {
        let mount = self.mounts.iter().find(|m| m.mount.mem == mem)?;
        // Three distinct ways to be unreadable, and only one of them is a
        // parse error. `.ok().flatten()` would collapse the first into
        // "absent", which is the very confusion this exists to prevent.
        let bytes = match mount.backend.read_anchors_sidecar() {
            // A backend error — permission denied, an IO fault. The file may
            // be perfectly well-formed; we simply could not look at it.
            Err(e) => return Some(format!("could not read the sidecar: {e}")),
            // Genuinely absent: a mem with no anchors yet. Not an error.
            Ok(None) => return None,
            Ok(Some(b)) => b,
        };
        // An empty or whitespace-only file parses as "no anchors" by a
        // deliberate tolerance in `from_bytes`. That tolerance is right for a
        // reader and wrong here: a sidecar truncated to zero by an
        // interrupted write is not a mem that never had anchors.
        if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            return Some(
                "the sidecar file is empty — an interrupted write leaves this state, and it is                  not the same as having no anchors; remove the file if the mem genuinely has none"
                    .to_string(),
            );
        }
        match crate::anchor::AnchorSidecar::from_bytes(&bytes) {
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        }
    }

    pub fn entity_anchors(&self, id: &EntityId) -> Vec<crate::anchor::Anchor> {
        let Some(mount) = self.mounts.iter().find(|m| m.mount.mem == id.mem()) else {
            return Vec::new();
        };
        let Ok(Some(bytes)) = mount.backend.read_anchors_sidecar() else {
            return Vec::new();
        };
        match crate::anchor::AnchorSidecar::from_bytes(&bytes) {
            Ok(sc) => sc.get(id.as_ref()).to_vec(),
            // Deliberate degrade-to-empty for the read path; a caller that
            // must not confuse "unreadable" with "none" checks
            // [`Self::anchors_sidecar_error`] first.
            Err(_) => Vec::new(),
        }
    }

    /// The stored anchors for `id`, each paired with its **live** resolution
    /// state when the engine could observe the source artifact this pass.
    ///
    /// Additive over [`Self::entity_anchors`]: the durable data is unchanged;
    /// `state` is the [`crate::anchor::resolve_anchor`] outcome against an
    /// observation the engine produces here. A `path`-namespace anchor
    /// (codebase / filesystem / git) is observed against the working tree at
    /// the current HEAD; an `entity`-namespace anchor is observed against the
    /// live graph by [`Self::observe_entity_anchor`]:
    ///
    /// - artifact absent ⇒ [`AnchorState::Orphaned`](crate::anchor::AnchorState::Orphaned);
    /// - artifact present, non-hash class (`authored` / `informed-by`) ⇒
    ///   [`Resolves`](crate::anchor::AnchorState::Resolves);
    /// - artifact present, hash-bearing class (`anchored` / `derived`) ⇒ the
    ///   prepared-content hash comparison decides:
    ///   [`Resolves`](crate::anchor::AnchorState::Resolves) on a match,
    ///   [`Drifted`](crate::anchor::AnchorState::Drifted) on a stable-medium
    ///   mismatch, [`Recheck`](crate::anchor::AnchorState::Recheck) on an
    ///   unstable medium or when a hash is unavailable on either side (a
    ///   hash-less anchor, a `tree` grain, an unreadable artifact).
    ///
    /// For an `entity` grain the same table applies, read off the store
    /// rather than the filesystem: the entity missing (or present only as a
    /// stub) is `Orphaned`, and a hash-bearing class compares the canonical
    /// rendered markdown.
    ///
    /// `state` is `None` (unobserved — never a fabricated state) when there is
    /// no workspace root, when the grain is `url` (the engine never fetches;
    /// a url anchor's hash is the registry's prepared form of the content
    /// its observer supplied at write time), when an `entity` anchor's
    /// source declares a preparation the registry does not know (the form
    /// cannot be computed), or when an `entity` anchor's mem is **not
    /// mounted** — an unmounted mem is not a mem of deleted entities, and
    /// saying so would route a deletion proposal to prune.
    pub fn entity_anchors_resolved(&self, id: &EntityId) -> Vec<ResolvedAnchor> {
        let anchors = self.entity_anchors(id);
        let source_roots = self.anchor_source_roots(id.mem());
        let none = SuppliedObservations::new();
        anchors
            .into_iter()
            .map(|anchor| self.resolve_one(anchor, &source_roots, &none))
            .collect()
    }

    /// Observe one stored anchor and pair it with its resolution — the
    /// shared step behind every anchor read.
    fn resolve_one(
        &self,
        anchor: crate::anchor::Anchor,
        source_roots: &std::collections::BTreeMap<String, AnchorSourceJoin>,
        supplied: &SuppliedObservations,
    ) -> ResolvedAnchor {
        let observed = self.observe_anchor(&anchor, source_roots, supplied);
        let (state, observed_hash, observed_at) = match observed {
            Some(o) => (Some(o.state), o.hash, o.at),
            None => (None, None, None),
        };
        ResolvedAnchor {
            anchor,
            state,
            observed_hash,
            observed_at,
        }
    }

    /// Per-anchor observation — THE one resolution mechanism, shared by
    /// binding-backed verify (`mem_anchors_resolved`, which the ingest
    /// render/report/prune/findings paths consume), the per-entity read
    /// (`entity_anchors_resolved`), and the standalone
    /// `verify_mem_anchors` operation. Path-shaped grains
    /// (`span`/`file`/`tree`) observe under the **decision-29 candidate
    /// priority** (an earlier plana): the anchor's artifact path is
    /// SOURCE-relative first — when its `source` name resolves through
    /// `source_roots` to a declared pointer, the pointer-joined path is
    /// authoritative — and workspace-relative only as the fallback, tried
    /// when the source-join does not resolve. A path resolving under both
    /// joins is decided by that priority, deterministically. An anchor
    /// without a `source` (a hand-authored mem, a binding-less write)
    /// observes workspace-relative exactly as before. A `url`
    /// grain has no engine-side observation and returns `None` (the report
    /// vocabulary's `unresolvable`) — the engine never fetches; its hash is
    /// recorded from observation-supplied content at write time — as does a
    /// workspace-root-less engine. An `entity`
    /// grain is not a filesystem path but is not unobservable either — it
    /// resolves against the live graph via
    /// [`Self::observe_entity_anchor`], under the preparation its source
    /// declares (touchpoint A of [`crate::preparation`]: the registry
    /// decides the prepared form the artifact hashes as). This replaces the retired `single_path_medium_root` gate,
    /// whose single-source assumption nulled every anchor of a mem with
    /// zero or several bindings — the honest per-anchor answer supersedes
    /// the all-or-nothing mem-level one.
    ///
    /// A `url` grain is the one grain the engine cannot observe itself: it
    /// resolves from a SUPPLIED observation when the caller passed one for
    /// its artifact (`memstead verify-anchors --observations`), else from
    /// the row's recorded `last_observed` (sidecar version 2) — an
    /// observation that was made, carrying its own date so the row ages
    /// visibly — else it is unobserved (`None`), never fabricated. A
    /// supplied `absent` resolves `recheck`, not `orphaned`: an observer
    /// failing to retrieve a web resource is not the medium saying the
    /// artifact is gone, and prune must never act on it.
    fn observe_anchor(
        &self,
        anchor: &crate::anchor::Anchor,
        source_roots: &std::collections::BTreeMap<String, AnchorSourceJoin>,
        supplied: &SuppliedObservations,
    ) -> Option<Observed> {
        if anchor.grain == crate::anchor::AnchorGrain::Url {
            if let Some(obs) = supplied.get(&anchor.artifact) {
                return Some(match &obs.outcome {
                    crate::anchor::SuppliedOutcome::Absent => Observed {
                        state: crate::anchor::AnchorState::Recheck,
                        hash: None,
                        at: Some(obs.at.clone()),
                    },
                    crate::anchor::SuppliedOutcome::Present { hash, content } => {
                        // Touchpoint A for a url row: supplied CONTENT is
                        // re-prepared under the anchor's source preparation,
                        // the rule the write path applied to the anchor's
                        // own `content`, so a `quoted-phrase` row is
                        // adjudicated on its phrase. A phrase the retrieved
                        // page no longer carries is the medium saying the
                        // unit is gone — `orphaned` — unlike a failed
                        // retrieval, which stays `recheck`. A supplied hash,
                        // or a source with no preparation, compares as is.
                        let preparation = anchor
                            .source
                            .as_deref()
                            .and_then(|name| source_roots.get(name))
                            .and_then(|j| j.preparation.as_deref());
                        let current = match (preparation, content) {
                            (Some(_), Some(text)) => crate::preparation::path_prepared_hash(
                                preparation,
                                &anchor.artifact,
                                anchor.grain,
                                text.as_bytes(),
                            ),
                            _ => crate::preparation::PathPrepared::NoHash,
                        };
                        match current {
                            crate::preparation::PathPrepared::UnitAbsent => Observed {
                                state: crate::anchor::resolve_anchor(
                                    anchor,
                                    &crate::anchor::ArtifactObservation::Absent,
                                ),
                                hash: None,
                                at: Some(obs.at.clone()),
                            },
                            crate::preparation::PathPrepared::Hash(h) => Observed {
                                state: crate::anchor::resolve_anchor(
                                    anchor,
                                    &crate::anchor::ArtifactObservation::Present {
                                        current_hash: Some(h.clone()),
                                    },
                                ),
                                hash: Some(h),
                                at: Some(obs.at.clone()),
                            },
                            crate::preparation::PathPrepared::NoHash => Observed {
                                state: crate::anchor::resolve_anchor(
                                    anchor,
                                    &crate::anchor::ArtifactObservation::Present {
                                        current_hash: Some(hash.clone()),
                                    },
                                ),
                                hash: Some(hash.clone()),
                                at: Some(obs.at.clone()),
                            },
                        }
                    }
                });
            }
            return anchor.last_observed.as_ref().map(|rec| Observed {
                state: rec.state,
                hash: rec.hash.clone(),
                at: Some(rec.at.clone()),
            });
        }
        let join = anchor
            .source
            .as_deref()
            .and_then(|name| source_roots.get(name));
        // An `entity`-grain anchor points into a mem's graph, not a file
        // tree. It has always returned `None` here — "unobserved this pass" —
        // which meant it could never be drifted, never be orphaned, and always
        // blocked prune. That is the bail the S1b pilot demonstrated: a
        // deliberately stale anchor over a changed source entity went unflagged
        // while the capability matrix claimed full parity.
        if anchor.grain == crate::anchor::AnchorGrain::Entity {
            return self
                .observe_entity_anchor(anchor, join.and_then(|j| j.preparation.as_deref()))
                .map(Observed::live);
        }
        let root = self.workspace_root.as_deref()?;
        observe_path_anchor(root, anchor, join).map(Observed::live)
    }

    /// Observe an `entity`-grain anchor against the live graph — the entity-
    /// namespace counterpart of [`observe_path_anchor`], and the mechanism
    /// that makes a graph-medium binding's drift real.
    ///
    /// The artifact is an entity id. Present/absent comes from the store, so
    /// this works uniformly across backends — a git-branch mem has no
    /// working-tree file to stat, which is exactly why observation cannot go
    /// through the filesystem here. The compared form is the preparation
    /// registry's **prepared form** for `preparation` (the anchor's source's
    /// declared preparation, [`crate::preparation::entity_prepared_hash`]):
    /// the **canonical rendered markdown** when the source declares none —
    /// byte-for-byte today's form — or the load-bearing serialization under
    /// `entity-load-bearing`; hashed with the same `prepared_content_hash`
    /// the path arm uses, so an anchor's recorded hash means the same thing
    /// in both namespaces. An identifier the registry does not know cannot
    /// be prepared: the anchor is reported unobserved (`None`), never hashed
    /// under a fabricated form.
    ///
    /// A stub is treated as absent: a stub is the engine's placeholder for an
    /// unresolved reference, not the entity the anchor claims to pin. Scoring
    /// it as present would let a dangling anchor resolve clean.
    fn observe_entity_anchor(
        &self,
        anchor: &crate::anchor::Anchor,
        preparation: Option<&str>,
    ) -> Option<(crate::anchor::AnchorState, Option<String>)> {
        // A `#<locator>` on an entity artifact is the preparation's business
        // (`quoted-phrase`: the phrase the entity must still carry); the id
        // is what stands before it.
        let (id_part, locator) = crate::preparation::split_unit_id(&anchor.artifact);
        let id = EntityId::canonical(id_part);

        // The mem the anchor points into must be MOUNTED before a store miss
        // can mean anything. If it is not, every entity in it is missing from
        // the store, and reporting `Orphaned` would say "the source deleted
        // these" about entities sitting untouched on disk. `None` — genuinely
        // unobserved — is the honest answer.
        //
        // This guard lives here, at the one observation site, and not at the
        // callers. An earlier fix put it in `run_verify` alone; `prune` reaches
        // anchor resolution by its own path (`mem_anchors_resolved`), so the
        // sync brief went on proposing the deletion of every destination
        // entity — a data-loss suggestion routed to the graph's only
        // maintenance writer, from a mem merely being unmounted. A guard that
        // protects one caller is not a guard on the behaviour.
        if !self.mounts.iter().any(|m| m.mount.mem == id.mem()) {
            return None;
        }

        let entity = self.store.get(&id).filter(|e| !e.stub);
        let Some(entity) = entity else {
            return Some((
                crate::anchor::resolve_anchor(anchor, &crate::anchor::ArtifactObservation::Absent),
                None,
            ));
        };
        let current_hash = if anchor.class.is_hash_bearing() {
            let type_def = self
                .schema_for(id.mem())
                .and_then(|schema| schema.get_type(&entity.entity_type));
            match crate::preparation::entity_prepared(
                entity,
                type_def.as_deref(),
                preparation,
                locator,
            ) {
                crate::preparation::PathPrepared::Hash(h) => Some(h),
                // The entity stands but no longer carries the addressed
                // unit (a `quoted-phrase` the markdown lost): an absent
                // artifact, adjudicated like a file's vanished unit.
                crate::preparation::PathPrepared::UnitAbsent => {
                    return Some((
                        crate::anchor::resolve_anchor(
                            anchor,
                            &crate::anchor::ArtifactObservation::Absent,
                        ),
                        None,
                    ));
                }
                crate::preparation::PathPrepared::NoHash => return None,
            }
        } else {
            None
        };
        let observation = crate::anchor::ArtifactObservation::Present {
            current_hash: current_hash.clone(),
        };
        Some((
            crate::anchor::resolve_anchor(anchor, &observation),
            current_hash,
        ))
    }

    /// The `source name → join` map for `mem`'s bindings: the filesystem
    /// roots that anchors written in the source dialect join onto (decision
    /// 26: anchor artifact paths are source-relative first) and the
    /// preparation each source declares (touchpoint A: what the registry
    /// prepares the artifact as before hashing). Empty when the workspace
    /// has no root, the pipeline store does not load, or `mem` has no
    /// bindings — resolution then degrades to the workspace-relative dialect
    /// alone with no preparation, which is exactly the hand-authored-mem
    /// posture.
    pub(crate) fn anchor_source_roots(
        &self,
        mem: &str,
    ) -> std::collections::BTreeMap<String, AnchorSourceJoin> {
        let mut roots = std::collections::BTreeMap::new();
        let Some(root) = self.workspace_root.as_deref() else {
            return roots;
        };
        let Ok(configs) = crate::pipeline_store::load_pipeline_configs(root) else {
            return roots;
        };
        for record in configs.bindings.iter().filter(|r| r.mem == mem) {
            for source in &record.config.sources {
                roots
                    .entry(source.name.clone())
                    .or_insert_with(|| AnchorSourceJoin {
                        pointer: source.pointer.clone(),
                        preparation: source.preparation.clone(),
                        source: source.clone(),
                        deny_paths: record.config.deny_paths.clone(),
                    });
            }
        }
        roots
    }

    /// Reverse anchor lookup: every `(entity_id, anchor)` across all mems
    /// whose anchor references `artifact_path`. This is the query the
    /// rebuilt check-realization hook consumes — given the file an agent
    /// just edited, which entities anchored to it. A `span`/`file`/`tree`
    /// anchor references the path when its base path (locator suffix
    /// `@commit` / `#span` stripped) equals the path, or — for a `tree`
    /// grain — when the path lies under the tree. Path-shaped grains only;
    /// `url` / `entity` anchors are matched by exact base equality.
    pub fn anchors_referencing_artifact(
        &self,
        artifact_path: &str,
    ) -> Vec<(EntityId, crate::anchor::Anchor)> {
        let mut out = Vec::new();
        for mount in &self.mounts {
            let Ok(Some(bytes)) = mount.backend.read_anchors_sidecar() else {
                continue;
            };
            let Ok(sc) = crate::anchor::AnchorSidecar::from_bytes(&bytes) else {
                continue;
            };
            // Source-dialect anchors (decision 26) reference the same
            // artifact under its pointer-joined workspace form — match both.
            let source_roots = self.anchor_source_roots(&mount.mount.mem);
            for (eid, anchors) in &sc.entities {
                for a in anchors {
                    // The shared decision-29 candidate rule: the join
                    // candidate exists only where the rule produces one (a
                    // climbing `../…` artifact never joins); the
                    // workspace-relative form is the `anchor_references_path`
                    // arm below.
                    let joined = a
                        .source
                        .as_deref()
                        .and_then(|name| source_roots.get(name))
                        .map(|join| {
                            artifact_candidates(&join.pointer, anchor_base_path(&a.artifact))
                        })
                        .and_then(|mut c| (c.len() > 1).then(|| c.remove(0)));
                    if anchor_references_path(a, artifact_path)
                        || joined.is_some_and(|j| {
                            path_references(
                                &j,
                                a.grain == crate::anchor::AnchorGrain::Tree,
                                artifact_path,
                            )
                        })
                    {
                        out.push((EntityId(eid.clone()), a.clone()));
                    }
                }
            }
        }
        out
    }

    /// Every `(entity_id, resolved anchor)` in `mem`, read from its anchors
    /// sidecar once and each paired with its **live** resolution state (the
    /// same observation [`Self::entity_anchors_resolved`] produces per entity,
    /// computed here mem-wide in a single sidecar read). Empty for an unknown
    /// mem, a backend that persists no anchors, or a mem with none.
    ///
    /// Additive read surface: the durable data is unchanged; `state` is the
    /// [`crate::anchor::resolve_anchor`] outcome against an observation the
    /// engine produces — the working tree for a `path`-namespace anchor, the
    /// live graph for an `entity` one — or `None` when unobserved (never
    /// fabricated). The verify pipeline consumes it to adjudicate a mem's
    /// anchors against the source; audit/health can reuse it.
    /// Whether `mem`'s anchor sidecar records ANY anchor row — the cheap
    /// existence check [`Self::mem_anchors_resolved`] cannot serve: that
    /// walk OBSERVES every anchor (hashing live source artifacts, and for
    /// path anchors enumerating the facet's file scope) when a caller only
    /// needs to know the sidecar is non-empty. Parses the sidecar and stops
    /// there; false for an unknown mem, a backend without anchors, or an
    /// empty sidecar — the same population `mem_anchors_resolved` would
    /// report empty for, minus the observation cost.
    pub fn mem_has_anchors(&self, mem: &str) -> bool {
        let Some(mount) = self.mounts.iter().find(|m| m.mount.mem == mem) else {
            return false;
        };
        let Ok(Some(bytes)) = mount.backend.read_anchors_sidecar() else {
            return false;
        };
        let Ok(sc) = crate::anchor::AnchorSidecar::from_bytes(&bytes) else {
            return false;
        };
        sc.entities.values().any(|anchors| !anchors.is_empty())
    }

    pub fn mem_anchors_resolved(&self, mem: &str) -> Vec<(EntityId, ResolvedAnchor)> {
        self.mem_anchors_resolved_with(mem, &SuppliedObservations::new())
    }

    /// [`Self::mem_anchors_resolved`] with observer-supplied observations
    /// for the grains the engine cannot observe itself (`url`), keyed by
    /// artifact. Rows without a supplied observation resolve as they would
    /// without the map.
    pub fn mem_anchors_resolved_with(
        &self,
        mem: &str,
        supplied: &SuppliedObservations,
    ) -> Vec<(EntityId, ResolvedAnchor)> {
        let Some(mount) = self.mounts.iter().find(|m| m.mount.mem == mem) else {
            return Vec::new();
        };
        let Ok(Some(bytes)) = mount.backend.read_anchors_sidecar() else {
            return Vec::new();
        };
        let Ok(sc) = crate::anchor::AnchorSidecar::from_bytes(&bytes) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let source_roots = self.anchor_source_roots(mem);
        for (eid, anchors) in &sc.entities {
            for anchor in anchors {
                out.push((
                    EntityId(eid.clone()),
                    self.resolve_one(anchor.clone(), &source_roots, supplied),
                ));
            }
        }
        out
    }

    /// Standalone anchor verification — "do my sources still say what I
    /// recorded?" for one mem, regardless of how it was built. Walks the
    /// mem's anchor sidecar through the shared per-anchor mechanism
    /// ([`Self::observe_anchor`] via [`Self::mem_anchors_resolved`]) and
    /// classifies every anchor into the report vocabulary: `resolved`
    /// (source present, hash matches or non-hash class), `drifted`
    /// (present, hash differs, stability `stable`), `recheck` (hash
    /// differs under `unstable`, or a hash is missing on either side),
    /// `unresolvable` (source absent, or a grain/medium the mechanism
    /// does not reach — never fabricated into drift). Read-only on mem
    /// content: pure sidecar read + filesystem observation, no commit on
    /// any backend. A mem with no anchors returns an empty report.
    pub fn verify_mem_anchors(&self, mem: &str) -> Result<MemAnchorVerification, EngineError> {
        self.verify_mem_anchors_with(mem, &SuppliedObservations::new())
    }

    /// The stale axis defers to anchor state: an entity carrying at least
    /// one adjudicated hash-bearing anchor (`resolves`, `drifted` or
    /// `recheck`) reads by its anchors, not by the day threshold. `drifted`
    /// and `recheck` list the entity as their own condition whatever its
    /// age; `resolves` keeps it off the list, and when the threshold would
    /// have listed it the row moves to `anchor_fresh` so the reading names
    /// the clock that overruled the threshold. Entities with no adjudicated
    /// anchor (none at all, or only `unresolvable` / `unobserved` rows)
    /// keep the day-threshold reading, byte for byte. Derived at read time
    /// from the same verification the anchors axis runs; nothing stored.
    pub(super) fn apply_anchor_clock(
        &self,
        summary: &mut crate::ops::HealthSummary,
        mem: Option<&str>,
    ) {
        use std::collections::HashMap;
        // Dominant adjudicated state per entity: drifted > recheck > resolves.
        fn rank(state: &str) -> Option<u8> {
            match state {
                "drifted" => Some(3),
                "recheck" => Some(2),
                "resolves" => Some(1),
                _ => None,
            }
        }
        let mut by_entity: HashMap<String, (u8, String)> = HashMap::new();
        let mut mems: Vec<String> = self.mem_names().iter().map(|s| s.to_string()).collect();
        mems.retain(|m| mem.is_none_or(|scope| scope == m));
        for m in mems {
            let Ok(report) = self.verify_mem_anchors(&m) else {
                continue;
            };
            for row in &report.anchors {
                let Some(r) = rank(&row.state) else { continue };
                let entry = by_entity
                    .entry(row.entity_id.clone())
                    .or_insert((0, String::new()));
                if r > entry.0 {
                    *entry = (r, row.state.clone());
                }
            }
        }
        if by_entity.is_empty() {
            return;
        }
        let today = crate::ops::health::days_since_epoch();
        let mut stale = std::mem::take(&mut summary.stale_entities);
        // Day-threshold rows on anchored entities leave the list: a
        // resolving anchor makes them fresh, a drifted or recheck one is
        // re-listed below under its own condition.
        let mut fresh = Vec::new();
        stale.retain(|row| match by_entity.get(row.id.0.as_str()) {
            None => true,
            Some((_, state)) => {
                if state == "resolves" {
                    fresh.push(crate::ops::StaleEntity {
                        id: row.id.clone(),
                        title: row.title.clone(),
                        days_since_modified: row.days_since_modified,
                        anchor_state: Some(state.clone()),
                    });
                }
                false
            }
        });
        let mut by_anchor: Vec<crate::ops::StaleEntity> = by_entity
            .iter()
            .filter(|(_, (_, state))| state != "resolves")
            .filter_map(|(id, (_, state))| {
                let entity = self.store.get(&crate::EntityId(id.clone()))?;
                if entity.stub || mem.is_some_and(|scope| entity.mem != scope) {
                    return None;
                }
                let days = entity
                    .metadata
                    .get("last_modified")
                    .and_then(|v| crate::ops::health::parse_iso_to_days(&v.to_frontmatter_string()))
                    .map(|d| today.saturating_sub(d))
                    .unwrap_or(0);
                Some(crate::ops::StaleEntity {
                    id: entity.id.clone(),
                    title: entity.title.clone(),
                    days_since_modified: days,
                    anchor_state: Some(state.clone()),
                })
            })
            .collect();
        stale.append(&mut by_anchor);
        stale.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        fresh.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        summary.stale_entities = stale;
        summary.anchor_fresh = fresh;
    }

    /// [`Self::verify_mem_anchors`] with observer-supplied observations
    /// (the `--observations` file of `memstead verify-anchors`). A `url`
    /// row whose artifact has a supplied observation adjudicates through
    /// the shared funnel exactly like a file row; a supplied row matching
    /// no `url` anchor of the mem is reported in `unmatched_observations`
    /// and changes nothing. The report's `recordable_observations` are the
    /// `last_observed` records the caller commits with
    /// [`Self::record_anchor_observations`] — the verification itself
    /// writes nothing.
    pub fn verify_mem_anchors_with(
        &self,
        mem: &str,
        supplied: &SuppliedObservations,
    ) -> Result<MemAnchorVerification, EngineError> {
        if !self.mem_router.is_visible(mem) {
            return Err(self.unknown_mem_error(mem));
        }
        let now = self.now_iso();
        // An unreadable sidecar is a condition, never zero rows: the readers
        // below would degrade it to "no anchors" and the report would then
        // describe a clean mem it never measured.
        if let Some(why) = self.anchors_sidecar_error(mem) {
            let mut report = MemAnchorVerification {
                mem: mem.to_string(),
                sidecar_error: Some(why),
                ..Default::default()
            };
            report.figure = report.figure_over(0);
            return Ok(report);
        }
        let mut report = MemAnchorVerification {
            mem: mem.to_string(),
            unreconciled: self
                .entity_set_is_reconcilable(mem)
                .err()
                .map(str::to_string),
            ..Default::default()
        };
        let reconciled = report.unreconciled.is_none();
        let mut resolves = 0usize;
        let mut matched: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (eid, resolved) in self.mem_anchors_resolved_with(mem, supplied) {
            let is_url = resolved.anchor.grain == crate::anchor::AnchorGrain::Url;
            let supplied_here = is_url && supplied.contains_key(&resolved.anchor.artifact);
            if supplied_here {
                matched.insert(resolved.anchor.artifact.clone());
            }
            let observed_at = resolved.observed_at.clone();
            let unobserved_for_days = observed_at
                .as_deref()
                .and_then(|at| crate::anchor::days_between(at, &now));
            if let (true, Some(state), Some(at)) = (supplied_here, resolved.state, &observed_at) {
                report.recordable_observations.push(RecordedObservation {
                    entity: eid.to_string(),
                    artifact: resolved.anchor.artifact.clone(),
                    observation: crate::anchor::AnchorObservation {
                        at: at.clone(),
                        hash: resolved.observed_hash.clone(),
                        state,
                    },
                });
            }
            // The entity end first: a row whose holder is gone is adjudicated
            // against its artifact alone otherwise, and a matching hash then
            // reports it as `resolved` for an entity that does not exist.
            if reconciled && self.entity_is_absent(&eid) {
                report.dangling += 1;
                report.anchors.push(VerifiedAnchor {
                    entity_id: eid.to_string(),
                    artifact: resolved.anchor.artifact.clone(),
                    grain: resolved.anchor.grain.as_wire().to_string(),
                    class: resolved.anchor.class.as_wire().to_string(),
                    state: "dangling".to_string(),
                    observed_hash: resolved.observed_hash,
                    observed_at,
                    unobserved_for_days,
                    observation_supplied: supplied_here,
                });
                continue;
            }
            let state = match resolved.state {
                Some(crate::anchor::AnchorState::Resolves) => {
                    resolves += 1;
                    crate::anchor::AnchorState::Resolves.as_wire()
                }
                Some(crate::anchor::AnchorState::Drifted) => {
                    report.drifted += 1;
                    crate::anchor::AnchorState::Drifted.as_wire()
                }
                Some(crate::anchor::AnchorState::Recheck) => {
                    report.recheck += 1;
                    crate::anchor::AnchorState::Recheck.as_wire()
                }
                // The row spells the enum's wire name (`orphaned`); the
                // summary count beside it keeps its surface name,
                // `unresolvable` (the artifact is gone: a measured failure).
                Some(crate::anchor::AnchorState::Orphaned) => {
                    report.unresolvable += 1;
                    crate::anchor::AnchorState::Orphaned.as_wire()
                }
                // Split from `unresolvable`: the artifact
                // being GONE is a measurement; the pass not reaching the
                // artifact at all is the absence of one, and the repairs
                // differ.
                None => {
                    report.unobserved += 1;
                    "unobserved"
                }
            };
            report.anchors.push(VerifiedAnchor {
                entity_id: eid.to_string(),
                artifact: resolved.anchor.artifact.clone(),
                grain: resolved.anchor.grain.as_wire().to_string(),
                class: resolved.anchor.class.as_wire().to_string(),
                state: state.to_string(),
                observed_hash: resolved.observed_hash,
                observed_at,
                unobserved_for_days,
                observation_supplied: supplied_here,
            });
        }
        report.unmatched_observations = supplied
            .keys()
            .filter(|artifact| !matched.contains(*artifact))
            .cloned()
            .collect();
        report.figure = report.figure_over(resolves);
        Ok(report)
    }
}

/// The base path of an anchor artifact ref — the locator suffixes a
/// medium may append (`@<commit>`, `#<span>`) stripped so the reverse
/// lookup compares paths, not versioned/located refs.
pub fn anchor_base_path(artifact: &str) -> &str {
    let cut = artifact.find(['@', '#']).unwrap_or(artifact.len());
    &artifact[..cut]
}

/// One mem's standalone anchor-verification report — the counts plus
/// the per-anchor rows, in sidecar order.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MemAnchorVerification {
    pub mem: String,
    /// The anchors sidecar could not be read: the parse or IO reason. When
    /// set, no row was counted — every count below is zero because nothing
    /// was measured, not because the mem is clean — `fully_adjudicated`
    /// reads false and the population reads unknown. One condition, one
    /// code (`ANCHORS_SIDECAR_UNREADABLE`), rendered by every surface from
    /// this field; no surface parses the sidecar on its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidecar_error: Option<String>,
    /// The positive figure with the population it was computed over
    /// ([`crate::anchor::AnchorResolutionFigure`]): `resolves` (the wire
    /// name is the `AnchorState` wire form; `resolved` was retired on
    /// 2026-09-02, one name per state), `population` and
    /// `fully_adjudicated`, serialized at this level. The count is never
    /// reachable apart from its statement.
    #[serde(flatten)]
    pub figure: crate::anchor::AnchorResolutionFigure,
    /// Source present, hash differs, stability `stable` — real drift.
    pub drifted: usize,
    /// Hash differs under `unstable` stability, or a hash is missing on
    /// either side — flagged for re-examination, never called drift.
    pub recheck: usize,
    /// Source absent: a MEASURED failure. The artifact the anchor names is
    /// not there.
    pub unresolvable: usize,
    /// The anchor could not be observed at all this pass, so nothing about it
    /// was measured. Its own count,
    /// because `unresolvable` used to swallow it: a reader on the surface you
    /// reach WITHOUT a binding could not tell a measured failure from an
    /// absent measurement, which is the one distinction that surface exists to
    /// make.
    pub unobserved: usize,
    /// Rows whose ENTITY is gone (consistency-sweep 03/02). Its own class,
    /// counted apart from the states above: those describe the artifact end,
    /// and a vanished entity says nothing about the source. Folding it into
    /// `unresolvable` would name the wrong repair.
    pub dangling: usize,
    /// Why the entity end could not be reconciled this pass, when it could
    /// not. `dangling: 0` means "none found" only when this is `None`.
    pub unreconciled: Option<String>,
    pub anchors: Vec<VerifiedAnchor>,
    /// Supplied observations whose artifact matched no `url` anchor of the
    /// mem — reported, never silently dropped, never applied to anything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmatched_observations: Vec<String>,
    /// The `last_observed` records this verification produced from supplied
    /// observations, for the caller to commit through
    /// [`Engine::record_anchor_observations`]. Engine-internal.
    #[serde(skip)]
    pub recordable_observations: Vec<RecordedObservation>,
}

/// One observation to record onto a sidecar row (the `last_observed`
/// field), addressed by the `(entity, artifact)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedObservation {
    pub entity: String,
    pub artifact: String,
    pub observation: crate::anchor::AnchorObservation,
}

/// Observer-supplied observations keyed by artifact — the input of
/// [`Engine::verify_mem_anchors_with`].
pub type SuppliedObservations =
    std::collections::BTreeMap<String, crate::anchor::SuppliedObservation>;

/// What one observation of an anchor yielded: the resolved state, the
/// hash it saw (when any), and — for an observation that was not made
/// live this pass — when it was made.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    state: crate::anchor::AnchorState,
    hash: Option<String>,
    at: Option<String>,
}

impl Observed {
    /// A live observation made this pass (path and entity grains).
    fn live((state, hash): (crate::anchor::AnchorState, Option<String>)) -> Self {
        Observed {
            state,
            hash,
            at: None,
        }
    }
}

impl MemAnchorVerification {
    /// The figure this report's counts yield for `resolves` resolving rows:
    /// the count with the population statement it was computed over
    /// (consistency-sweep 03/05, criteria 1 and 3): what the figures cover,
    /// and how much of it the pass could not adjudicate.
    ///
    /// A resolution figure alone is read as health. Every W3 finding made that
    /// figure mean less than a reader assumes, and none of them made it wrong
    /// in a way anyone could see. The figure type renders the number and its
    /// population as ONE unit, so a surface cannot show the number and omit
    /// the caveat: it gets both or neither.
    fn figure_over(&self, resolves: usize) -> crate::anchor::AnchorResolutionFigure {
        // `recheck` belongs on ONE side of this sentence. A first version put
        // it in both: counted as adjudicated and then reported as not, so the
        // same rows appeared twice and the two numbers could not be reconciled
        // by a reader. A recheck row is a row whose drift could NOT be
        // asserted, which is the definition of unadjudicated.
        if let Some(why) = &self.sidecar_error {
            return crate::anchor::AnchorResolutionFigure::uncounted(format!(
                "population unknown: the anchors sidecar could not be read ({why}); no row was \
                 counted, and zero counts here are not a clean mem"
            ));
        }
        let adjudicated = resolves + self.drifted + self.unresolvable;
        let unadjudicated = self.recheck + self.unobserved;
        let mut s = format!(
            "over {} counted row(s): {adjudicated} adjudicated, {unadjudicated} not (recheck {}, unobserved {})",
            adjudicated + unadjudicated,
            self.recheck,
            self.unobserved
        );
        if self.dangling > 0 {
            s.push_str(&format!(
                "; {} row(s) excluded, naming an entity the mem no longer holds",
                self.dangling
            ));
        }
        if let Some(why) = &self.unreconciled {
            s.push_str(&format!(
                "; the entity end was NOT reconciled ({why}), so dangling rows would not have been detected"
            ));
        }
        // False means the figures rest on an incomplete measurement, which
        // is not the same as a failed one.
        let fully_adjudicated =
            self.recheck == 0 && self.unobserved == 0 && self.unreconciled.is_none();
        crate::anchor::AnchorResolutionFigure::new(resolves, s, fully_adjudicated)
            .expect("the population statement is never empty")
    }
}

/// One anchor's verification row.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VerifiedAnchor {
    pub entity_id: String,
    pub artifact: String,
    pub grain: String,
    pub class: String,
    /// `resolves` | `drifted` | `recheck` | `unresolvable` (artifact gone) |
    /// `unobserved` (not measured this pass) | `dangling` (the entity is
    /// gone). The wire vocabulary of this field, which is NOT the engine's
    /// `AnchorState` enum: that has four variants describing the artifact
    /// end, and the last two here are conditions beside them.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_hash: Option<String>,
    /// When the observation this row's state rests on was made — present
    /// only for a row resolved from a supplied or recorded observation (a
    /// `url` row); live observations carry none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    /// Whole days between `observed_at` and this run — how long the row has
    /// gone unobserved. `Some(0)` for an observation made today.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unobserved_for_days: Option<u64>,
    /// Whether this row's state came from an observation supplied to this
    /// run (as opposed to a live or a previously recorded one).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub observation_supplied: bool,
}

/// Whether `anchor` references `path`. `tree`-grain anchors match `path`
/// itself and anything beneath the tree; every other grain matches by
/// exact base-path equality.
/// A stored anchor paired with its live resolution state, when observable.
/// See [`Engine::entity_anchors_resolved`] for how `state` is produced and
/// when it is `None` (unobserved, never fabricated).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ResolvedAnchor {
    /// The durable anchor record (flattened on the wire so the resolved shape
    /// is the stored anchor plus a `state` field).
    #[serde(flatten)]
    pub anchor: crate::anchor::Anchor,
    /// The live resolution state, or `None` when the engine could not observe
    /// the source artifact this pass: a `url` grain, no workspace root, an
    /// ambiguous or absent path medium, or an `entity` grain whose mem is not
    /// mounted. That last case is load-bearing — an unmounted mem is not a mem
    /// of deleted entities, so it must read as unobserved rather than
    /// `Orphaned`, which prune would act on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::anchor::AnchorState>,
    /// The prepared-content hash the observation computed this pass —
    /// present only for a hash-bearing (`anchored` / `derived`) anchor whose
    /// artifact could be read: a `file` / `span` anchor resolving to a
    /// readable file, or an `entity` anchor whose mem is mounted (hashed over
    /// the canonical rendered markdown, so the value means the same thing in
    /// both namespaces). The verify pass's backfill leg records it onto a
    /// hash-less anchor. Engine-internal
    /// observation detail, deliberately not serialized: the wire shape stays
    /// the stored anchor plus `state`.
    #[serde(skip)]
    pub observed_hash: Option<String>,
    /// When the observation `state` rests on was made, for a row whose
    /// observation is supplied or recorded rather than live (a `url` row).
    /// Additive: absent on every live-observed row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

/// What an anchor's `source` name resolves to in its mem's bindings: the
/// declared pointer (the filesystem root a source-dialect artifact path
/// joins onto, decision 26) and the declared preparation (what the
/// preparation registry prepares the artifact as before hashing —
/// touchpoint A of [`crate::preparation`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnchorSourceJoin {
    /// The source's declared `pointer`.
    pub(crate) pointer: String,
    /// The source's declared `preparation`, if any.
    pub(crate) preparation: Option<String>,
    /// The declaring source itself — its scope is what a `tree` anchor's
    /// prepared form enumerates under a code-map preparation.
    pub(crate) source: crate::pipeline::Source,
    /// The binding's `deny_paths`, applied on top of the source scope.
    pub(crate) deny_paths: Vec<String>,
}

/// Observe a single path-namespace anchor against `root` (its medium's
/// filesystem root) and resolve its live state plus — for a present
/// hash-bearing (`anchored` / `derived`) `file` / `span` anchor — the
/// artifact's **prepared-content hash**
/// ([`crate::anchor::prepared_content_hash`]). `None` when the anchor's
/// grain does not reference a filesystem path.
///
/// The computed hash is what lets [`crate::anchor::resolve_anchor`]
/// adjudicate `drifted` vs `resolves` deterministically against the recorded
/// hash. The prepared form is the registry's rule for the anchor's
/// source's preparation ([`crate::preparation::path_prepared_hash`]): a
/// `span` anchor hashes its whole containing file (the span locator selects
/// within it; the file is the hashed unit), except under a **delivery
/// preparation**, where a `<path>#<key>` span names one delivery unit (the
/// unit's own text is the hashed unit, and a key the file no longer yields
/// is an absent artifact); under a **code-map** preparation a file or span
/// hashes the interface digest, and a `tree` hashes the code map of every
/// scoped file under it. A `tree` under any other preparation hashes the
/// plain per-file prepared-content map of its scoped files, so tree anchors
/// adjudicate deterministically like file anchors. A `tree` with no
/// resolvable source-join (the enumeration scope is undefined without one),
/// a partial enumeration, and a read failure observe no hash — those
/// resolve `recheck`, never a fabricated `drifted`. Non-hash classes
/// (`authored` / `informed-by`) skip the read entirely, so an anchor-less or
/// hash-free mem pays no observation cost.
fn observe_path_anchor(
    root: &Path,
    anchor: &crate::anchor::Anchor,
    join: Option<&AnchorSourceJoin>,
) -> Option<(crate::anchor::AnchorState, Option<String>)> {
    use crate::anchor::AnchorGrain;
    match anchor.grain {
        AnchorGrain::Span | AnchorGrain::File | AnchorGrain::Tree => {}
        AnchorGrain::Url | AnchorGrain::Entity => return None,
    }
    let source_pointer = join.map(|j| j.pointer.as_str());
    let preparation = join.and_then(|j| j.preparation.as_deref());
    let base = anchor_base_path(&anchor.artifact);
    // Decision 29: the source-join is authoritative — an artifact path is
    // source-relative first (joined onto the declaring source's pointer,
    // which may deliberately leave the workspace root for out-of-root
    // pointers); the workspace-relative form is tried only when the
    // source-join does not resolve. The candidate set is the shared
    // [`artifact_candidates`] rule, so resolution, the write gate, and the
    // population matcher read one artifact the same way.
    let path = {
        let candidates = artifact_candidates(source_pointer.unwrap_or(""), base);
        candidates
            .iter()
            .map(|c| root.join(c))
            .find(|p| p.exists())
            .unwrap_or_else(|| root.join(base))
    };
    if !path.exists() {
        return Some((
            crate::anchor::resolve_anchor(anchor, &crate::anchor::ArtifactObservation::Absent),
            None,
        ));
    }
    let current_hash = if !anchor.class.is_hash_bearing() {
        None
    } else if matches!(anchor.grain, AnchorGrain::File | AnchorGrain::Span) && path.is_file() {
        match std::fs::read(&path).ok().map(|bytes| {
            crate::preparation::path_prepared_hash(
                preparation,
                &anchor.artifact,
                anchor.grain,
                &bytes,
            )
        }) {
            Some(crate::preparation::PathPrepared::Hash(h)) => Some(h),
            Some(crate::preparation::PathPrepared::UnitAbsent) => {
                return Some((
                    crate::anchor::resolve_anchor(
                        anchor,
                        &crate::anchor::ArtifactObservation::Absent,
                    ),
                    None,
                ));
            }
            Some(crate::preparation::PathPrepared::NoHash) | None => None,
        }
    } else if anchor.grain == AnchorGrain::Tree
        && path.is_dir()
        && let Some(join) = join
    {
        // The tree's prepared form: a digest over every scoped file under
        // the tree, by the declaring source's own scope and the binding's
        // deny paths — the code map under a code-map preparation, the plain
        // per-file prepared-content map otherwise, so a tree anchor
        // adjudicates deterministically instead of resting in `recheck`
        // forever. The path the anchor names is workspace-relative or
        // source-relative; the enumeration is workspace-relative, so compare
        // the resolved absolute paths. A PARTIAL enumeration (malformed or
        // retired-dialect scope pattern) observes no hash — a digest over a
        // set that is not the population would silently change a stored
        // tree-anchor hash; no-hash resolves `recheck`, the same posture as
        // a failed read.
        let enumeration = crate::source_scope::enumerate_facet_files_reported(
            &join.source,
            &join.deny_paths,
            root,
        );
        if enumeration.is_partial() {
            None
        } else if preparation == Some(crate::preparation::CODE_MAP) {
            let files: Vec<(String, String)> = enumeration
                .files
                .into_iter()
                .filter(|f| root.join(f).starts_with(&path))
                .filter_map(|f| {
                    std::fs::read(root.join(&f))
                        .ok()
                        .map(|bytes| (f, String::from_utf8_lossy(&bytes).into_owned()))
                })
                .collect();
            Some(crate::anchor::prepared_content_hash(
                crate::preparation::code_map_tree_digest(&files).as_bytes(),
            ))
        } else {
            let files: Vec<(String, Vec<u8>)> = enumeration
                .files
                .into_iter()
                .filter(|f| root.join(f).starts_with(&path))
                .filter_map(|f| std::fs::read(root.join(&f)).ok().map(|bytes| (f, bytes)))
                .collect();
            Some(crate::anchor::prepared_content_hash(
                crate::preparation::plain_tree_digest(&files).as_bytes(),
            ))
        }
    } else {
        None
    };
    let observation = crate::anchor::ArtifactObservation::Present {
        current_hash: current_hash.clone(),
    };
    Some((
        crate::anchor::resolve_anchor(anchor, &observation),
        current_hash,
    ))
}

/// Deterministic fingerprint of a PARSED schema for the
/// authoring-drift equivalence check. Compares semantic content, never
/// raw bytes: YAML comments (the CLI-injected editor-header lines) and
/// whitespace vanish at parse time, and `Schema.types` — a `HashMap`
/// with nondeterministic iteration order — is rendered sorted by type
/// name so two loads of equivalent packages always fingerprint alike.
pub(super) fn schema_parsed_fingerprint(schema: &memstead_schema::Schema) -> String {
    let mut keys: Vec<&String> = schema.types.keys().collect();
    keys.sort();
    let types: Vec<String> = keys
        .iter()
        .map(|k| format!("{k}={:?}", schema.types[k.as_str()]))
        .collect();
    format!(
        "{:?}|{}|{}",
        schema.manifest,
        schema.version,
        types.join(";")
    )
}

fn anchor_references_path(anchor: &crate::anchor::Anchor, path: &str) -> bool {
    let base = anchor_base_path(&anchor.artifact);
    path_references(base, anchor.grain == crate::anchor::AnchorGrain::Tree, path)
}

/// Whether `base` (a file path, or a tree root when `is_tree`) references
/// `path` — exact match, or containment for a tree.
fn path_references(base: &str, is_tree: bool, path: &str) -> bool {
    if base == path {
        return true;
    }
    if is_tree {
        let prefix = base.strip_suffix('/').unwrap_or(base);
        return path.starts_with(&format!("{prefix}/"));
    }
    false
}

/// Join a source pointer and a source-relative artifact path into the
/// pointer-joined (workspace-relative) form — the decision-26 dialect
/// bridge. Plain string concatenation with a separator: the pointer is
/// workspace-relative (and may climb out via `..`), the artifact is
/// source-relative; no canonicalization here, the filesystem resolves it.
pub fn join_pointer(pointer: &str, base: &str) -> String {
    let pointer = pointer.trim_end_matches('/');
    if pointer.is_empty() || pointer == "." {
        base.to_string()
    } else {
        format!("{pointer}/{base}")
    }
}

/// How a source-relative artifact id resolved ACROSS a binding's primary
/// sources — the cross-source counterpart of [`artifact_candidates`], which
/// answers only within ONE source.
///
/// The two questions are genuinely different and only this one can be
/// ambiguous. `artifact_candidates` settles which reading wins under a single
/// pointer (decision 29: source-join first, workspace-relative fallback). It
/// takes one pointer and has no opinion on two sources both carrying
/// `docs/a.md`. That case has exactly one home, the exclude fold, and used to
/// take the first source that matched — so the source listed first in the
/// binding silently won, which is the wrong-target write this type exists to
/// prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossSourceArtifact {
    /// No source's join lands on a member of the enumerated set.
    Unresolved,
    /// Exactly one does; this is the canonical id to record.
    Unique(String),
    /// Several do. Sorted and deduplicated, so the refusal names them in a
    /// stable order and the caller never has to pick.
    Ambiguous(Vec<String>),
}

/// Resolve a requested artifact id against a binding's primary sources.
///
/// `canonical_for` maps one source's base and the requested id to the
/// workspace-relative form, returning `None` when that form is not a member
/// of the enumerated source set. The caller owns that predicate because the
/// membership test differs by surface; what is shared, and what lives here,
/// is the rule that several matches are an ambiguity to refuse rather than a
/// choice to make silently.
pub fn resolve_across_sources<'a, I, F>(
    bases: I,
    requested: &str,
    canonical_for: F,
) -> CrossSourceArtifact
where
    I: IntoIterator<Item = &'a std::path::PathBuf>,
    F: Fn(&std::path::PathBuf, &str) -> Option<String>,
{
    let mut hits: Vec<String> = bases
        .into_iter()
        .filter_map(|base| canonical_for(base, requested))
        .collect();
    hits.sort();
    hits.dedup();
    match hits.len() {
        0 => CrossSourceArtifact::Unresolved,
        1 => CrossSourceArtifact::Unique(hits.remove(0)),
        _ => CrossSourceArtifact::Ambiguous(hits),
    }
}

/// The candidate workspace-relative forms an anchor artifact could denote
/// under its declaring source's pointer, in the ratified priority (bundle
/// decision 29): the source-join first, the workspace-relative form as the
/// fallback. **The single implementation of that rule** — resolution, the
/// write-time gate, and the population scope matcher all construct their
/// candidate set here, so one artifact cannot read differently across the
/// three (each site then applies its own predicate: existence for
/// resolution and the gate, glob membership for the matcher).
///
/// One lexical clarification rides with the rule: an artifact that climbs
/// (`../…`) never joins. An artifact of a source never climbs OUT of that
/// source, so such a path is already the workspace-relative form; joining
/// anyway fabricates `<ptr>/../…`, which the filesystem then resolves into a
/// sibling tree — a false resolution for an existence test and a false
/// in-scope for a `**` glob. An artifact already carrying the pointer prefix
/// is NOT suppressed: on a self-nested layout both readings exist and the
/// decision's priority — source-join wins — settles it deterministically.
pub fn artifact_candidates(pointer: &str, base: &str) -> Vec<String> {
    let pointer = pointer.trim_end_matches('/');
    if pointer.is_empty() || pointer == "." {
        return vec![base.to_string()];
    }
    if base.starts_with("../") || base == ".." {
        return vec![base.to_string()];
    }
    vec![format!("{pointer}/{base}"), base.to_string()]
}
