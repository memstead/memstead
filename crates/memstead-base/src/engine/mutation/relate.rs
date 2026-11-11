//! `Engine::relate_entity` and the `relate` alias — append / remove a
//! single edge between two entities.

use std::path::Path;

use crate::engine_fallback_type;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::entity::{Entity, EntityId, Relationship, normalise_description};
use crate::ops::WarningHint;
use crate::provenance::{Provenance, ProvenanceKind};
use crate::runtime_validator::{
    CrossMemRelCheck, RelationshipCheck, validate_cross_mem_edge, validate_rel_shape,
    validate_rel_type,
};
use crate::vcs::{Actor, ClientId, CommitContext};
use crate::workspace::MountCapability;
use memstead_schema::SchemaRef;

use super::super::{Engine, EngineError, RelateAction, RelateEntityArgs, RelateEntityOutcome};
use super::{
    make_stub, unknown_type_error, validate_description_posture, validate_relation_target_grammar,
};

/// A fully validated relate — every gate has passed and the source
/// entity's next markdown is generated, but nothing has been written
/// to the store, the disk, or the mem-repo yet. `stage_prepared_relate`
/// performs the store-side effects and the (uncommitted) file write;
/// the caller then commits and applies via
/// `apply_prepared_relate_to_store`.
pub(super) struct PreparedRelate {
    pub(super) mount_idx: usize,
    pub(super) source_mem: String,
    pub(super) from: EntityId,
    pub(super) to: EntityId,
    pub(super) rel_type: String,
    pub(super) action: RelateAction,
    pub(super) file_path: String,
    pub(super) markdown: String,
    pub(super) warnings: Vec<WarningHint>,
    /// `Some` when the add path must materialise a forward-reference
    /// stub for an absent target. Prepare plans the stub; the stage
    /// step upserts it — prepare itself never mutates the store, so a
    /// refused batch has nothing to undo from prepares alone.
    pub(super) stub_target: Option<EntityId>,
    /// The planned stub's kind: `ForwardReference` for a genuinely
    /// absent target, `LoadTime` for a storage-verified target in a
    /// deferred mem (the stub is only the until-load representation).
    /// Meaningless when `stub_target` is `None`.
    pub(super) stub_kind: crate::entity::StubKind,
    pub(super) type_def: std::sync::Arc<memstead_schema::TypeDefinition>,
}

/// What `prepare_relate` resolved to.
pub(super) enum RelatePrepareOutcome {
    /// No-op path (idempotent re-add / absent remove): the complete
    /// outcome, with its typed no-op warning and an empty
    /// `write_id` — nothing to write or commit.
    Done(RelateEntityOutcome),
    /// A real edge change, validated and ready to stage.
    Prepared(PreparedRelate),
}

/// The edge a relate names, read under the lock: everything later phases
/// read from the workspace before they decide anything.
struct ResolvedRelate {
    args: RelateEntityArgs,
    source_mem: String,
    target_mem: String,
    mount_idx: usize,
    schema: std::sync::Arc<memstead_schema::Schema>,
    target_schema_ref: Option<SchemaRef>,
    cross_mem_different: bool,
    warnings: Vec<WarningHint>,
    entity: Entity,
    target_type: Option<String>,
    target_verified_in_storage: bool,
    target_mem_uncreated: bool,
}

/// The edge with every add-time gate passed, and the source's type.
struct ValidatedRelate {
    resolved: ResolvedRelate,
    type_def: std::sync::Arc<memstead_schema::TypeDefinition>,
}

/// What a relate's commit returns to the outcome phase.
struct CommittedRelate {
    prepared: PreparedRelate,
    write_id: String,
    landed_warnings: Vec<WarningHint>,
    stamp_warnings: Vec<WarningHint>,
}

/// Per-item state of a batch relate, carrying the applied item's
/// action label from its prepared state.
enum ItemState {
    Applied(&'static str),
    Noop,
    Error,
}

/// The relate verb's action words: the applied item's own label,
/// `noop` for an applied no-op.
fn relate_actions(items: Vec<(EntityId, ItemState)>) -> Vec<(EntityId, String)> {
    items
        .into_iter()
        .map(|(id, state)| {
            let action = match state {
                ItemState::Applied(label) => label,
                ItemState::Noop => "noop",
                ItemState::Error => unreachable!("refusal path returned above"),
            };
            (id, action.to_string())
        })
        .collect()
}

impl Engine {
    /// Add or remove a typed relationship on `args.source`.
    ///
    /// Cross-mem relate is policy-gated through
    /// [`Engine::cross_mem_link_allowed`] — the workspace's
    /// `[cross_mem_links]` table (or per-create-rule
    /// `default_cross_links` synthesis) decides whether the edge is
    /// permitted. Disallowed pairings surface
    /// [`EngineError::CrossMemLinkNotAllowed`]. Cross-mem relate
    /// only writes the source entity's markdown — the target mem is
    /// never written to. Auto-stub for absent targets works for
    /// Write target mems; ReadOnly target mems reject absent
    /// targets with [`EngineError::CrossMemTargetNotFound`] because
    /// the engine cannot persist a stub through the read-only
    /// boundary.
    ///
    /// Schema-undeclared rel types surface either as validation
    /// errors (strict mode) or as ride-along warnings on the outcome
    /// (open mode).
    pub fn relate_entity(
        &mut self,
        args: RelateEntityArgs,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<RelateEntityOutcome, EngineError> {
        // The lock first, then the ids. A relate locks the mems of both
        // ends and reloads the source's (a DEFERRED target mem is
        // deliberately not loaded: target verification must never cost
        // a mem's load, the storage check below answers the existence
        // and type questions against the branch tree / filesystem
        // instead). It takes the whole workspace when an end is named
        // by a bare slug, resolved under the lock, or when it adds an
        // edge on an acyclic relationship, whose cycle guard walks
        // edges in every mem. The announcements of resolved ids lead
        // the warnings. Hoisted out of `prepare_relate` so
        // `batch_relate` can lock and reload once up front instead of
        // per entry.
        let mut args = args;
        let (named_source, named_target) =
            (args.source.mem().to_string(), args.target.mem().to_string());
        let short = named_source.is_empty() || named_target.is_empty();
        let (_write_lock, reloaded) = {
            let (rel_type, adds) = (&args.rel_type, !args.remove);
            // Both ends' mems, and a mem a link in the description names.
            let mut locked = crate::engine::write_lock::referred_mems(
                &named_source,
                std::iter::empty(),
                args.description.as_deref(),
            );
            locked.push(named_source.clone());
            locked.push(named_target.clone());
            self.lock_for_verb(
                !args.dry_run,
                &locked,
                std::slice::from_ref(&named_source),
                |engine| short || (adds && engine.rel_type_is_acyclic(&named_source, rel_type)),
            )?
        };
        let (source, source_hint) = self.resolve_entity_id(&args.source)?;
        args.source = source;
        let (target, target_hint) = self.resolve_entity_id(&args.target)?;
        args.target = target;
        let source_mem = args.source.mem().to_string();
        let mut drift_warnings: Vec<WarningHint> =
            source_hint.into_iter().chain(target_hint).collect();
        drift_warnings.extend(reloaded);
        // An ACYCLIC rel-type's cycle guard walks the whole rel-type
        // subgraph (`would_cycle`), and a cycle can pass through any
        // mem — an edge on a deferred (lazy, unloaded) mem's entity is
        // invisible to a walk over the endpoint mems alone, so an add
        // an eager boot refuses would land silently and corrupt an
        // innocent edge on the next full load (the fourth lazy-mount
        // grade demonstrated exactly that on a three-mem chain). The
        // same holds for a rel-type in an `acyclic_sets` set, whose
        // guard walks the set's UNION subgraph, and for declared
        // aggregate signals on either endpoint's schema: the
        // threshold-crossing diff counts edges that can originate in
        // any mem, so a partial store silently mis-levels it. Full
        // load before the guard; writes touching neither skip the
        // cost.
        if self.write_needs_full_load(
            &source_mem,
            [crate::engine::mutation::EdgeWrite {
                rel_type: &args.rel_type,
                target_mem: args.target.mem(),
                remove: args.remove,
            }],
        ) {
            self.ensure_mems_loaded(None);
        }

        let dry_run = args.dry_run;
        let stubs_before = self.stub_ids();
        let landed = self
            .prepare_relate(args, drift_warnings)
            .and_then(|p| match p {
                RelatePrepareOutcome::Done(outcome) => {
                    self.relate_noop_outcome(outcome, dry_run, actor, client, note)
                }
                RelatePrepareOutcome::Prepared(prepared) if dry_run => {
                    self.relate_preview(prepared)
                }
                RelatePrepareOutcome::Prepared(prepared) => {
                    self.commit_prepared_relate(prepared, actor, client, note)
                }
            });
        self.drop_stubs_of_an_unlanded_write(&stubs_before, &landed, dry_run);
        landed
    }

    /// The duplicate-add re-baseline. Called
    /// from the `NoOpAlreadyPresent` path when the rel-type is
    /// derivation-declared: stages a sidecar-only refresh of the
    /// edge's baseline to the target's current hash and commits it
    /// (the anchor-only-update precedent — a real persisted effect
    /// rides a real commit), then returns the outcome with the
    /// refresh STATED (`DERIVATION_BASELINE_REFRESHED` warning + the
    /// sidecar commit's sha). `_hash` and the edge are untouched. On
    /// an undeclared rel-type the outcome passes through unchanged —
    /// today's exact no-op response.
    fn refresh_derivation_baseline_on_noop(
        &mut self,
        mut outcome: RelateEntityOutcome,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<RelateEntityOutcome, EngineError> {
        let source_mem = outcome.from.mem().to_string();
        let declared = self
            .schemas
            .get(&source_mem)
            .is_some_and(|s| super::rel_type_declares_derivation(s, &outcome.rel_type));
        if !declared {
            return Ok(outcome);
        }
        let Some(mount_idx) = self.mounts.iter().position(|m| m.mount.mem == source_mem) else {
            return Ok(outcome);
        };
        let hash = self
            .store
            .get(&outcome.to)
            .map(|e| e.content_hash.clone())
            .unwrap_or_default();
        let (from, rel, to) = (
            outcome.from.to_string(),
            outcome.rel_type.clone(),
            outcome.to.to_string(),
        );
        let backend = self.mounts[mount_idx].backend.as_ref();
        super::stage_derivation_sidecar(backend, |s| s.set(&from, &rel, &to, &hash))?;
        let ctx = self.commit_context(
            Some("relate_entity"),
            actor,
            client.cloned(),
            note.map(String::from),
        );
        let landed = self.commit_staged(
            mount_idx,
            &format!("memstead: derivation re-baseline {}", outcome.from),
            &ctx,
            &[],
        )?;
        let write_id = landed.write_id;
        outcome.warnings.extend(landed.warnings);
        self.record_self_write(mount_idx, &write_id);
        // `relate_entity` returns early into this path, so the stamp call on
        // the ordinary path never runs and its report has to be collected
        // here (found by the re-grade).
        outcome
            .warnings
            .extend(self.stamp_mutation_versions(mount_idx));
        outcome.write_id = write_id;
        outcome
            .warnings
            .push(WarningHint::DerivationBaselineRefreshed {
                from: outcome.from.clone(),
                rel_type: outcome.rel_type.clone(),
                to: outcome.to.clone(),
            });
        Ok(outcome)
    }

    /// A relate that changes nothing: an idempotent re-add on a real
    /// run refreshes a derivation edge's baseline (the agent's explicit
    /// "reviewed, still holds"); every other no-op is its outcome.
    fn relate_noop_outcome(
        &mut self,
        outcome: RelateEntityOutcome,
        dry_run: bool,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<RelateEntityOutcome, EngineError> {
        if !dry_run && matches!(outcome.action, RelateAction::NoOpAlreadyPresent) {
            return self.refresh_derivation_baseline_on_noop(outcome, actor, client, note);
        }
        Ok(outcome)
    }

    /// A dry run's outcome: the prospective hash of the rendered file,
    /// nothing written.
    fn relate_preview(&self, prepared: PreparedRelate) -> Result<RelateEntityOutcome, EngineError> {
        let parse_result = parse_markdown(
            &prepared.markdown,
            &prepared.file_path,
            prepared.type_def.as_ref(),
            &prepared.source_mem,
        )
        .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
        // A rehearsal demands no note: `NOTE_MISSING` attributes a
        // write that landed without provenance, and nothing lands
        // here (create and update rehearse the same way; a relate
        // rehearsal once carried the nudge and so reported a
        // provenance gap on a commit that did not exist).
        let warnings = prepared.warnings;
        Ok(RelateEntityOutcome {
            from: prepared.from,
            to: prepared.to,
            rel_type: prepared.rel_type,
            action: prepared.action,
            content_hash: parse_result.entity.content_hash,
            write_id: String::new(),
            source: "explicit".to_string(),
            orphan_stubs_removed: Vec::new(),
            warnings,
        })
    }

    /// Stage, commit and report a prepared relate; the signal levels
    /// before the write are read first, for the crossing warnings.
    fn commit_prepared_relate(
        &mut self,
        prepared: PreparedRelate,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<RelateEntityOutcome, EngineError> {
        let signal_snapshot = crate::ops::signals::snapshot_levels(
            &self.store,
            &self.schemas,
            [&prepared.from, &prepared.to],
        );

        self.stage_relate(&prepared)?;
        let committed = self.commit_relate(prepared, actor, client, note)?;
        Ok(self.relate_outcome(committed, signal_snapshot, note))
    }

    /// Stage phase: the planned stub and the source's file, and on a
    /// derivation-declared rel-type the edge's baseline.
    fn stage_relate(&mut self, prepared: &PreparedRelate) -> Result<(), EngineError> {
        self.stage_prepared_relate(prepared)?;
        if let Some(schema) = self.schemas.get(&prepared.source_mem)
            && crate::engine::mutation::rel_type_declares_derivation(schema, &prepared.rel_type)
        {
            let backend = self.mounts[prepared.mount_idx].backend.as_ref();
            let (from, rel, to) = (
                prepared.from.to_string(),
                prepared.rel_type.clone(),
                prepared.to.to_string(),
            );
            match prepared.action {
                RelateAction::Added => {
                    let hash = self
                        .store
                        .get(&prepared.to)
                        .map(|e| e.content_hash.clone())
                        .unwrap_or_default();
                    crate::engine::mutation::stage_derivation_sidecar(backend, |s| {
                        s.set(&from, &rel, &to, &hash)
                    })?;
                }
                RelateAction::Removed => {
                    crate::engine::mutation::stage_derivation_sidecar(backend, |s| {
                        s.remove(&from, &rel, &to)
                    })?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Commit phase: one commit with the relate's provenance, recorded
    /// as this engine's own write.
    fn commit_relate(
        &mut self,
        prepared: PreparedRelate,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<CommittedRelate, EngineError> {
        let commit_subject = format!("memstead: relate {}", prepared.from);
        let ctx = self.commit_context(
            Some("relate_entity"),
            actor,
            client.cloned(),
            note.map(String::from),
        );
        let record =
            self.provenance_record(ProvenanceKind::Relate, &prepared.from, actor, client, note);
        let landed = self.commit_staged(prepared.mount_idx, &commit_subject, &ctx, &[record])?;
        let write_id = landed.write_id;
        let follow_up_warnings = landed.warnings;

        self.record_self_write(prepared.mount_idx, &write_id);
        let stamp_warnings = self.stamp_mutation_versions(prepared.mount_idx);

        // A failed store update reloaded the mem from storage, where the
        // write is; the receipt reads from that view.
        Ok(CommittedRelate {
            prepared,
            write_id,
            landed_warnings: follow_up_warnings,
            stamp_warnings,
        })
    }

    /// Outcome phase: the store and the indexes take the write, and the
    /// receipt reports it with every warning in the order the verb has
    /// always reported them.
    fn relate_outcome(
        &mut self,
        committed: CommittedRelate,
        signal_snapshot: crate::ops::signals::SignalSnapshot,
        note: Option<&str>,
    ) -> RelateEntityOutcome {
        let CommittedRelate {
            prepared,
            write_id,
            landed_warnings: mut follow_up_warnings,
            stamp_warnings,
        } = committed;
        let content_hash = self
            .after_commit(
                prepared.mount_idx,
                "store_update",
                &mut follow_up_warnings,
                |engine| engine.apply_prepared_relate_to_store(&prepared),
            )
            .unwrap_or_else(|| {
                self.store
                    .get(&prepared.from)
                    .map(|e| e.content_hash.clone())
                    .unwrap_or_default()
            });

        // On the `--remove` path, the edge we just dropped may
        // have been the last incoming edge to a stub. The orphan-stub
        // GC hook fired from `memstead_delete` already; mirror it here so
        // every mutation that can leave orphans cleans them up.
        // Scoped sweep — only inspect the just-severed target. The
        // only possible new orphan from a relate-remove is the
        // target whose incoming edge we removed; checking the entire
        // store would catch pre-existing orphans which aren't this
        // mutation's responsibility (and which `memstead_delete`'s full
        // sweep also leaves alone before its own removal). Funnels
        // through the shared `gc_orphan_stubs_among` predicate so the
        // relate-remove, delete, and update-via-alias-resync paths
        // can't drift on what counts as a GC-able orphan.
        let orphan_stubs_removed: Vec<EntityId> =
            if matches!(prepared.action, RelateAction::Removed) {
                crate::engine::mutation::gc_orphan_stubs_among(
                    &mut self.store,
                    std::iter::once(&prepared.to),
                )
            } else {
                Vec::new()
            };

        self.invalidate_communities();
        // Incremental: only the SOURCE entity's file
        // was rewritten; stub targets and GC'd orphan stubs were never
        // indexed.
        self.maintain_search_indexes(std::slice::from_ref(&prepared.from));

        let PreparedRelate {
            from,
            to,
            rel_type,
            action,
            mut warnings,
            ..
        } = prepared;

        warnings.extend(follow_up_warnings);
        warnings.extend(stamp_warnings);

        // Signal crossings ride the out-of-band warning channel beside
        // the success payload — never error-shaped, never changing the
        // mutation's success semantics.
        warnings.extend(crate::ops::signals::crossing_warnings(
            &self.store,
            &self.schemas,
            &signal_snapshot,
        ));

        // `require_notes` provenance nudge — single engine-level
        // enforcement point. Only reached on the real-commit path
        // (Added / Removed); the NoOpAlreadyPresent / NoOpAbsent branches
        // return early above with an empty `write_id` and never demand
        // a note (nothing landed to attribute).
        if let Some(w) = self.note_missing_warning("relate_entity", note) {
            warnings.push(w);
        }

        RelateEntityOutcome {
            from,
            to,
            rel_type,
            action,
            content_hash,
            write_id,
            source: "explicit".to_string(),
            warnings,
            orphan_stubs_removed,
        }
    }

    /// Every validation gate and the mutation plan for one relate —
    /// shared verbatim by the single-item path above and
    /// [`Self::batch_relate`], so batching can never drift from the
    /// single-item gates. Never mutates the store, writes no file,
    /// commits nothing: refusals are side-effect-free by
    /// construction. The reload-before-operation probe is the
    /// caller's job (hoisted so a batch probes each mem once).
    fn prepare_relate(
        &mut self,
        args: RelateEntityArgs,
        drift_warnings: Vec<WarningHint>,
    ) -> Result<RelatePrepareOutcome, EngineError> {
        let resolved = self.resolve_relate(args, drift_warnings)?;
        let validated = self.validate_relate(resolved)?;
        self.compose_relate(validated)
    }

    /// Perform a prepared relate's pre-commit side effects: upsert the
    /// planned forward-reference stub (if any) and write the source
    /// entity's regenerated markdown into the backend's pending
    /// buffer. Nothing is committed; a caller that aborts afterwards
    /// rolls back with a store snapshot + `discard_all_pending`.
    fn stage_prepared_relate(&mut self, p: &PreparedRelate) -> Result<(), EngineError> {
        if let Some(stub_id) = &p.stub_target {
            self.store
                .upsert(stub_id.clone(), make_stub(stub_id, p.stub_kind.clone()));
        }
        self.mounts[p.mount_idx]
            .backend
            .write_entity(Path::new(&p.file_path), p.markdown.as_bytes())?;
        Ok(())
    }

    /// Parse the prepared markdown back and push it into the store
    /// (replacing the pre-mutation source entity), then re-run the
    /// alias-edge remap. Returns the new `content_hash`. In the
    /// single-item path this runs after the commit (preserving the
    /// pre-split ordering); `batch_relate` runs it immediately after
    /// staging each entry so later entries in the same batch validate
    /// against this entry's effect — applied-in-order semantics.
    fn apply_prepared_relate_to_store(
        &mut self,
        p: &PreparedRelate,
    ) -> Result<String, EngineError> {
        let parse_result = parse_markdown(
            &p.markdown,
            &p.file_path,
            p.type_def.as_ref(),
            &p.source_mem,
        )
        .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
        let content_hash = parse_result.entity.content_hash.clone();
        let fallback = engine_fallback_type();
        push_entities_into_store(&mut self.store, vec![parse_result], fallback.as_ref(), None);
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );
        Ok(content_hash)
    }

    /// Atomic batch relate — the edge-side sibling of
    /// [`Self::batch_create`] / [`Self::batch_update`]. One list
    /// carrying both additions and removals, **applied in order**:
    /// each entry validates against the graph state produced by every
    /// prior valid entry (an add followed by a remove of the same edge
    /// nets to no edge; an acyclic check sees edges added earlier in
    /// the same batch). Per-entry shape mirrors what `relate` accepts.
    ///
    /// - **All-or-nothing, report-all.** A single invalid entry
    ///   refuses the whole batch — no edge changes, no head movement —
    ///   and the refusal identifies EVERY failing entry with its typed
    ///   `{code, message, details}` envelope, bounded at
    ///   [`Self::BATCH_ERROR_REPORT_CAP`] with `errors_suppressed`
    ///   counting the rest. An entry after a failing one validates
    ///   against the state as of the prior *valid* entries, so a
    ///   dependent entry may cascade — every reported code is still a
    ///   true refusal of the submitted file.
    /// - **One commit per touched mem** (subject
    ///   `memstead: batch-relate (N edges)`), per-entry provenance
    ///   notes, exactly like the rest of the family. No-op entries
    ///   (idempotent re-add / absent remove) report `"noop"` and
    ///   produce no write.
    /// - Orphan-stub GC runs over every removed edge's target after
    ///   the commit, same predicate as the single-item path (the
    ///   collected ids are not part of `BatchResult`'s fixed family
    ///   shape).
    ///
    /// **Rehearsal** (`dry_run: true`): the FULL in-order validation
    /// pass runs — each entry staged against the state its
    /// predecessors produced, identical refusals, identical report-all
    /// envelope — then the batch stops before any commit and rolls the
    /// staged state back. A legal batch returns the would-be receipt
    /// (per-entry actions, would-be `orphan_stubs_removed` computed on
    /// the staged state) with the marker form's empty `write_id`; an
    /// illegal one returns the same refusal a real call would. No
    /// edge, stub, or commit lands.
    pub fn batch_relate(
        &mut self,
        relates: Vec<(RelateEntityArgs, Option<String>)>,
        actor: Actor,
        client: Option<&ClientId>,
        dry_run: bool,
    ) -> Result<crate::ops::BatchResult, EngineError> {
        if relates.is_empty() {
            return Ok(super::batch_empty());
        }

        // Reload every touched mem (sources and targets) once, up
        // front — the per-entry probe is hoisted out of
        // `prepare_relate` for exactly this.
        // The lock first, then the ids. Every mem an item names is
        // locked, a deferred target included: its lock costs no load,
        // and the storage check of the target runs under it. A mem that
        // is only ever a TARGET in this batch and is deferred stays
        // unloaded: target verification runs against storage, and
        // loading it would convert verification into a
        // full load. Source mems always load: a write into a mem needs
        // the mem. The batch takes the whole workspace when an item
        // names an end by a bare slug (resolved under the lock) or adds
        // an edge on an acyclic relationship (the cycle guard walks
        // edges in every mem). What the roster reconcile and the
        // reloads report rides the receipt like on a single verb.
        let mut touched_mems: Vec<String> = relates
            .iter()
            .flat_map(|(a, _)| [a.source.mem().to_string(), a.target.mem().to_string()])
            .collect();
        touched_mems.sort();
        touched_mems.dedup();
        let mut source_mems: Vec<String> = relates
            .iter()
            .map(|(a, _)| a.source.mem().to_string())
            .collect();
        source_mems.sort();
        source_mems.dedup();
        let (_write_lock, drift_warnings) = {
            let items = &relates;
            let mut locked = touched_mems.clone();
            for (a, _) in items {
                locked.extend(crate::engine::write_lock::referred_mems(
                    a.source.mem(),
                    std::iter::empty(),
                    a.description.as_deref(),
                ));
            }
            self.lock_for_verb(!dry_run, &locked, &source_mems, |engine| {
                items.iter().any(|(a, _)| {
                    a.source.mem().is_empty()
                        || a.target.mem().is_empty()
                        || (!a.remove && engine.rel_type_is_acyclic(a.source.mem(), &a.rel_type))
                })
            })?
        };
        // An item whose short id does not resolve fails as its own
        // error below, never the whole batch.
        let mut short_hints: Vec<WarningHint> = Vec::new();
        let mut short_errors: Vec<(usize, EngineError)> = Vec::new();
        let relates: Vec<(RelateEntityArgs, Option<String>)> = relates
            .into_iter()
            .enumerate()
            .map(|(i, (mut a, n))| {
                match self.resolve_entity_id(&a.source) {
                    Ok((s, hint)) => {
                        a.source = s;
                        short_hints.extend(hint);
                    }
                    Err(e) => short_errors.push((i, e)),
                }
                match self.resolve_entity_id(&a.target) {
                    Ok((t, hint)) => {
                        a.target = t;
                        short_hints.extend(hint);
                    }
                    Err(e) => short_errors.push((i, e)),
                }
                (a, n)
            })
            .collect();
        // Same acyclic-guard rule as the single-item path: any added
        // edge on an ACYCLIC rel-type (or a rel-type in an
        // `acyclic_sets` set, whose guard walks the set's UNION
        // subgraph) walks the whole subgraph, so the walk must see
        // every mem — deferred (lazy, unloaded) ones included — or a
        // cycle through an unloaded mem is admitted through this
        // path. Declared signals on either endpoint's schema need
        // the full load for the same reason as the single-item path.
        if relates.iter().any(|(a, _)| {
            self.write_needs_full_load(
                a.source.mem(),
                [crate::engine::mutation::EdgeWrite {
                    rel_type: &a.rel_type,
                    target_mem: a.target.mem(),
                    remove: a.remove,
                }],
            )
        }) {
            self.ensure_mems_loaded(None);
        }

        // Snapshot for the all-or-nothing rollback: staged entries
        // mutate the store as they apply (in-order semantics), so a
        // refusal restores this snapshot and discards every backend's
        // pending buffer. Any early-return added below MUST do both.
        let store_snapshot = self.store.clone();

        let mut items: Vec<(EntityId, ItemState)> = Vec::with_capacity(relates.len());
        let mut prepared: Vec<PreparedRelate> = Vec::new();
        let mut notes: Vec<Option<String>> = Vec::new();
        let mut errors: Vec<(usize, EngineError)> = Vec::new();

        for (i, (args, note)) in relates.into_iter().enumerate() {
            let source_id = args.source.clone();
            if let Some(pos) = short_errors.iter().position(|(j, _)| *j == i) {
                let (_, e) = short_errors.remove(pos);
                items.push((source_id, ItemState::Error));
                errors.push((i, e));
                continue;
            }
            // Rehearsal is batch-level (the `dry_run` parameter) —
            // per-entry dry-run stays forced off; the staging below is
            // what gives later entries in-order semantics, and the
            // batch-level rollback undoes it.
            let mut args = args;
            args.dry_run = false;
            match self.prepare_relate(args, Vec::new()) {
                Ok(RelatePrepareOutcome::Done(_)) => {
                    items.push((source_id, ItemState::Noop));
                }
                Ok(RelatePrepareOutcome::Prepared(p)) => {
                    // Stage + apply NOW so later entries validate
                    // against this entry's effect (applied-in-order).
                    if let Err(e) = self.stage_prepared_relate(&p) {
                        self.store = store_snapshot;
                        self.discard_all_pending();
                        return Err(e);
                    }
                    if let Err(e) = self.apply_prepared_relate_to_store(&p) {
                        self.store = store_snapshot;
                        self.discard_all_pending();
                        return Err(e);
                    }
                    // Derivation baseline — same predicate
                    // and staging as the single-item path; rides the
                    // batch commit, rolls back with a refusal. (Batch
                    // no-op entries do NOT re-baseline — the explicit
                    // "reviewed, still holds" gesture is the single
                    // relate / single-op MCP list.)
                    if let Some(schema) = self.schemas.get(&p.source_mem)
                        && super::rel_type_declares_derivation(schema, &p.rel_type)
                    {
                        let backend = self.mounts[p.mount_idx].backend.as_ref();
                        let (from, rel, to) =
                            (p.from.to_string(), p.rel_type.clone(), p.to.to_string());
                        let staged = match p.action {
                            RelateAction::Added => {
                                let hash = self
                                    .store
                                    .get(&p.to)
                                    .map(|e| e.content_hash.clone())
                                    .unwrap_or_default();
                                super::stage_derivation_sidecar(backend, |s| {
                                    s.set(&from, &rel, &to, &hash)
                                })
                            }
                            RelateAction::Removed => {
                                super::stage_derivation_sidecar(backend, |s| {
                                    s.remove(&from, &rel, &to)
                                })
                            }
                            _ => Ok(()),
                        };
                        if let Err(e) = staged {
                            self.store = store_snapshot;
                            self.discard_all_pending();
                            return Err(e);
                        }
                    }
                    let label = match p.action {
                        RelateAction::Added => "added",
                        RelateAction::Removed => "removed",
                        _ => unreachable!("no-ops resolve to Done"),
                    };
                    items.push((source_id, ItemState::Applied(label)));
                    prepared.push(p);
                    notes.push(note);
                }
                Err(e) => {
                    items.push((source_id, ItemState::Error));
                    errors.push((i, e));
                }
            }
        }

        if !errors.is_empty() {
            // Refuse the whole batch; roll back every staged entry.
            self.store = store_snapshot;
            self.discard_all_pending();
            return Ok(super::batch_refusal(
                items.into_iter().map(|(id, _)| id).collect(),
                errors,
            ));
        }

        // Rehearsal: every entry validated in order against the state
        // its predecessors produced and nothing failed — stop before
        // any commit. The would-be orphan GC is computed on the staged
        // store (honest: it is exactly what the real call would
        // collect), then the whole staged state rolls back.
        if dry_run {
            let removed_targets: Vec<EntityId> = prepared
                .iter()
                .filter(|p| matches!(p.action, RelateAction::Removed))
                .map(|p| p.to.clone())
                .collect();
            let orphan_stubs_removed =
                super::gc_orphan_stubs_among(&mut self.store, removed_targets.iter());
            self.store = store_snapshot;
            self.discard_all_pending();
            return Ok(super::batch_receipt(
                relate_actions(items),
                Vec::new(),
                orphan_stubs_removed,
                String::new(),
            ));
        }

        // --- Commit once per touched mount, in first-seen order. ---
        let mut distinct_mounts: Vec<usize> = Vec::new();
        for p in &prepared {
            if !distinct_mounts.contains(&p.mount_idx) {
                distinct_mounts.push(p.mount_idx);
            }
        }
        let mut mount_commits: Vec<(usize, String)> = Vec::with_capacity(distinct_mounts.len());
        let mut batch_warnings: Vec<WarningHint> = drift_warnings;
        batch_warnings.extend(short_hints);
        for &m in &distinct_mounts {
            // Distinct source ids for this mount (an entity may carry
            // several edge changes in one batch).
            let mut entity_ids: Vec<String> = Vec::new();
            let mut edge_count = 0usize;
            for p in prepared.iter().filter(|p| p.mount_idx == m) {
                edge_count += 1;
                let s = p.from.to_string();
                if !entity_ids.contains(&s) {
                    entity_ids.push(s);
                }
            }
            let subject = format!("memstead: batch-relate ({edge_count} edges)");
            // Per-entry notes ride the batch commit's note record as
            // `<id>: <note>` lines, keyed by the edge's
            // source entity — `append_provenance` is a no-op on the
            // git-branch backend. No notes → no note record.
            let note_lines: Vec<String> = prepared
                .iter()
                .zip(notes.iter())
                .filter(|(p, _)| p.mount_idx == m)
                .filter_map(|(p, n)| n.as_ref().map(|n| format!("{}: {n}", p.from)))
                .collect();
            let mut ctx = self.commit_context(
                Some("batch_relate"),
                actor,
                client.cloned(),
                if note_lines.is_empty() {
                    None
                } else {
                    Some(note_lines.join("\n"))
                },
            );
            ctx.entity_ids = Some(entity_ids);
            // One provenance record per entry, landing with the mem's
            // commit.
            let records: Vec<Provenance> = prepared
                .iter()
                .zip(notes.iter())
                .filter(|(p, _)| p.mount_idx == m)
                .map(|(p, note)| {
                    self.provenance_record(
                        ProvenanceKind::Relate,
                        &p.from,
                        actor,
                        client,
                        note.as_deref(),
                    )
                })
                .collect();
            match self.commit_or_complete(m, self.commit_staged(m, &subject, &ctx, &records)) {
                Ok(landed) => {
                    batch_warnings.extend(landed.warnings);
                    mount_commits.push((m, landed.write_id));
                }
                Err(e) => {
                    // A commit failed: roll back the store and any
                    // still-pending backends. Mems already committed
                    // in this loop stay committed (the family's
                    // per-mem atomicity), so the view of each is
                    // reloaded from storage.
                    self.store = store_snapshot;
                    self.discard_all_pending();
                    let items: Vec<(usize, EntityId)> = prepared
                        .iter()
                        .map(|p| (p.mount_idx, p.from.clone()))
                        .collect();
                    return Err(self.batch_commit_failed(
                        &distinct_mounts,
                        &items,
                        mount_commits,
                        m,
                        e,
                    ));
                }
            }
        }

        // Self-write markers per mount.
        for (m, write_id) in &mount_commits {
            self.record_self_write(*m, write_id);
            batch_warnings.extend(self.stamp_mutation_versions(*m));
        }

        // Orphan-stub GC over every removed edge's target — same
        // scoped sweep and shared predicate as the single-item path.
        let removed_targets: Vec<EntityId> = prepared
            .iter()
            .filter(|p| matches!(p.action, RelateAction::Removed))
            .map(|p| p.to.clone())
            .collect();
        let orphan_stubs_removed =
            super::gc_orphan_stubs_among(&mut self.store, removed_targets.iter());

        self.invalidate_communities();
        self.invalidate_search_indexes();

        let write_id = mount_commits
            .last()
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        Ok(super::batch_receipt(
            relate_actions(items),
            batch_warnings,
            orphan_stubs_removed,
            write_id,
        ))
    }

    /// Positional-args alias for [`Self::relate_entity`]. Bundles
    /// the positional inputs into a [`RelateEntityArgs`] (with
    /// `expected_hash: None`) and delegates to
    /// [`Self::relate_entity`]. The `CommitContext` is destructured
    /// into the 4-tuple (actor, client, note) the unified mutation
    /// surface accepts.
    pub fn relate(
        &mut self,
        from: &EntityId,
        to: &EntityId,
        rel_type: &str,
        remove: bool,
        ctx: &CommitContext<'_>,
    ) -> Result<RelateEntityOutcome, EngineError> {
        let args = RelateEntityArgs {
            source: from.clone(),
            expected_hash: None,
            rel_type: rel_type.to_string(),
            target: to.clone(),
            remove,
            description: None,
            dry_run: false,
        };
        self.relate_entity(args, ctx.actor, ctx.client.as_ref(), ctx.note.as_deref())
    }
}

mod compose;
#[cfg(test)]
mod derivation_tests;
mod resolve;
#[cfg(test)]
mod tests;
mod validate;
