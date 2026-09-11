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
    /// deferred mem (the stub is only the until-load representation —
    /// flywheel W7/02). Meaningless when `stub_target` is `None`.
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
        // Short ids on either end resolve (or refuse) before anything
        // reads their mems; the announcements lead the warnings.
        let mut args = args;
        let (source, source_hint) = self.resolve_entity_id(&args.source)?;
        args.source = source;
        let (target, target_hint) = self.resolve_entity_id(&args.target)?;
        args.target = target;
        let source_mem = args.source.mem().to_string();
        let target_mem = args.target.mem().to_string();

        // Reload-before-operation: reload the source mem (and the
        // target mem, when distinct) if a sibling advanced either
        // ref, so the source `expected_hash` compare and the target
        // existence/stub decisions below run against current truth.
        // The drift notice rides the outcome's `warnings`. Hoisted
        // out of `prepare_relate` so `batch_relate` can probe every
        // touched mem exactly once up front instead of per entry.
        let mut drift_warnings: Vec<WarningHint> =
            source_hint.into_iter().chain(target_hint).collect();
        drift_warnings.extend(self.reload_if_stale(Some(&source_mem)));
        if target_mem != source_mem && !self.mem_is_deferred(&target_mem) {
            // A DEFERRED target mem is deliberately not probed here:
            // the funnel's phase-0 trigger would full-load it, and
            // target verification must never cost a mem's load
            // (flywheel W7/02 — the storage check below answers the
            // existence and type questions against the branch tree /
            // filesystem instead).
            drift_warnings.append(&mut self.reload_if_stale(Some(&target_mem)));
        }
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
        let cycle_guard_needs_full = !args.remove
            && self.schemas.get(&source_mem).is_some_and(|s| {
                s.relationship_acyclic(&args.rel_type)
                    || s.acyclic_set_containing(&args.rel_type).is_some()
            });
        let signal_diff_needs_full = [source_mem.as_str(), args.target.mem()].iter().any(|m| {
            self.schemas
                .get(*m)
                .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
        });
        if cycle_guard_needs_full || signal_diff_needs_full {
            self.ensure_mems_loaded(None);
        }

        let dry_run = args.dry_run;
        let prepared = match self.prepare_relate(args, drift_warnings)? {
            RelatePrepareOutcome::Done(outcome) => {
                // Derivation re-baseline: on a
                // derivation-declared rel-type, the duplicate-add
                // no-op's ONE effect is refreshing the edge's
                // baseline to the target's current hash — the agent's
                // explicit "reviewed, still holds". Sidecar-only:
                // `_hash` unchanged, the edge unchanged; the response
                // states the refresh (warning + the sidecar commit's
                // sha) instead of a bare no-op. Undeclared rel-types
                // keep today's exact no-op response; rehearsals
                // refresh nothing.
                if !dry_run
                    && matches!(
                        outcome.action,
                        super::super::RelateAction::NoOpAlreadyPresent
                    )
                {
                    return self.refresh_derivation_baseline_on_noop(outcome, actor, client, note);
                }
                return Ok(outcome);
            }
            RelatePrepareOutcome::Prepared(p) => p,
        };

        // Rehearsal: the full validation stage ran (identical refusals
        // and warnings, would-be stub included via the prepared
        // AUTO_STUB_CREATED warning) — stop before any write. `_hash`
        // reports the PROSPECTIVE post-write hash; `write_id` stays
        // empty (the marker form). Nothing staged, committed, or
        // stubbed.
        if dry_run {
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
            return Ok(RelateEntityOutcome {
                from: prepared.from,
                to: prepared.to,
                rel_type: prepared.rel_type,
                action: prepared.action,
                content_hash: parse_result.entity.content_hash,
                write_id: String::new(),
                source: "explicit".to_string(),
                orphan_stubs_removed: Vec::new(),
                warnings,
            });
        }

        // Aggregate signals: capture both endpoints' levels before the
        // store mutates — the entities a relate can move are exactly
        // the edge's endpoints. Diffed after the write below.
        let signal_snapshot = crate::ops::signals::snapshot_levels(
            &self.store,
            &self.schemas,
            [&prepared.from, &prepared.to],
        );

        self.stage_prepared_relate(&prepared)?;

        // Derivation baseline: an explicit add
        // on a declared rel-type records the target's CURRENT content
        // hash ("" for a just-stubbed absent target — deriving from
        // nothing, honestly); a remove prunes the row. Staged into
        // the same pending set so baseline and edge ride one commit.
        if let Some(schema) = self.schemas.get(&prepared.source_mem)
            && super::rel_type_declares_derivation(schema, &prepared.rel_type)
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
                    super::stage_derivation_sidecar(backend, |s| s.set(&from, &rel, &to, &hash))?;
                }
                RelateAction::Removed => {
                    super::stage_derivation_sidecar(backend, |s| s.remove(&from, &rel, &to))?;
                }
                _ => {}
            }
        }

        let backend = self.mounts[prepared.mount_idx].backend.as_ref();
        let commit_subject = format!("memstead: relate {}", prepared.from);
        let ctx = self.commit_context(
            Some("relate_entity"),
            actor,
            client.cloned(),
            note.map(String::from),
        );
        let write_id = backend.commit(&commit_subject, &ctx)?;

        backend.append_provenance(
            &Provenance::new(
                std::time::SystemTime::now(),
                ProvenanceKind::Relate,
                Some(prepared.from.to_string()),
                actor,
                client.cloned(),
                note.map(String::from),
            )
            .with_role(self.current_role)
            .with_identity(self.current_identity.clone()),
        )?;

        self.record_self_write(prepared.mount_idx, &write_id);
        let stamp_warnings = self.stamp_mutation_versions(prepared.mount_idx);

        let content_hash = self.apply_prepared_relate_to_store(&prepared)?;

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
                super::gc_orphan_stubs_among(&mut self.store, std::iter::once(&prepared.to))
            } else {
                Vec::new()
            };

        self.invalidate_communities();
        // Incremental (flywheel W8/01): only the SOURCE entity's file
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

        Ok(RelateEntityOutcome {
            from,
            to,
            rel_type,
            action,
            content_hash,
            write_id,
            source: "explicit".to_string(),
            warnings,
            orphan_stubs_removed,
        })
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
        let write_id = backend.commit(
            &format!("memstead: derivation re-baseline {}", outcome.from),
            &ctx,
        )?;
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
        mut drift_warnings: Vec<WarningHint>,
    ) -> Result<RelatePrepareOutcome, EngineError> {
        let mut args = args;
        let source_mem = args.source.mem().to_string();
        let target_mem = args.target.mem().to_string();

        // Target-id grammar gate (shared helper, also called from
        // `Engine::create_entity` for inline relations so both
        // gateways trip the same envelope). Source-id grammar is
        // implicit — a malformed source surfaces as `ENTITY_NOT_FOUND`
        // because it can never have been created.
        //
        // The grammar check runs BEFORE the cross-mem policy check:
        // a bare-string target with no `--` separator (e.g.
        // `bad target`) parses as `mem: ""`, `path: "bad target"`,
        // and without this ordering would surface a cross-mem
        // policy error against an empty mem name — pointing the
        // agent at workspace policy when the actual fix is a
        // malformed id. The grammar check is intrinsic to the target
        // id; it doesn't need to know which mem the target lives
        // in.
        validate_relation_target_grammar(&args.target)?;

        // Track whether the cross-mem target's mem is unmounted —
        // we deferred the warning emission to the canonical
        // `warnings` vec initialisation below, but the policy / RO
        // gates fire first to keep the refusal-before-warning ordering:
        // a policy refusal preempts the warning.
        let mut target_mem_uncreated = false;
        if source_mem != target_mem {
            // Policy gates *new* edges only — remove is structurally
            // cleanup. The same convention governs the acyclic, shape,
            // and schema gates below (each one wraps `if !args.remove`).
            // Without this bypass, a workspace whose cross-mem grant
            // was revoked while edges still existed gets wedged: the
            // grant must be re-introduced just to delete the data it
            // permitted, then re-revoked. The gate-on-add
            // rule holds because `cross_mem_links: named` semantically reads
            // as "only these new edges may be created", not "these
            // edges may exist."
            if !args.remove {
                super::validate_cross_mem_add_policy(self, &source_mem, &args.target)?;
            }
            // ReadOnly target mem: the engine has no write access to
            // persist a stub there, so the target must already exist
            // before relate. (Same-mem and cross-mem-to-Write
            // both retain the auto-stub mechanic below.) The add path
            // already refused above via the shared funnel; this check
            // stays unconditional so the remove path keeps its
            // pre-funnel behaviour.
            if let Some(mount) = self.mount(&target_mem)
                && mount.capability == MountCapability::ReadOnly
                && !self.store.contains(&args.target)
                && !matches!(
                    super::probe_deferred_target(self, &args.target)?,
                    super::DeferredTargetProbe::Exists
                )
            {
                return Err(EngineError::CrossMemTargetNotFound {
                    target_id: args.target.to_string(),
                    target_mem: target_mem.clone(),
                });
            }
            // The target mem isn't mounted in the workspace
            // at all. Policy admitted the edge so the relate must
            // succeed and auto-stub; surface a warning so the operator
            // can distinguish a typo from a deliberate forward
            // reference. The auto-stub still lands via the
            // `AutoStubCreated` path below; this layered warning is
            // additive observability.
            if self.mount(&target_mem).is_none() {
                target_mem_uncreated = true;
            }
        }

        // Canonicalise rel_type to UPPER_SNAKE_CASE so the schema lookup,
        // stored edge, and response all see the same wire-contract form
        // ("case-insensitive on input"). Syntax errors (non-letter
        // characters) fall through to the strict-mode schema check below,
        // which surfaces them as INVALID_REL_TYPE with the declared
        // vocabulary.
        if let Ok(canonical) = crate::entity::id::validate_rel_type(&args.rel_type) {
            args.rel_type = canonical;
        }
        // Normalise the description at the boundary so empty /
        // whitespace-only strings collapse to `None` before the
        // posture check and before the renderer ever sees them.
        args.description = normalise_description(args.description.as_deref());

        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == source_mem)
            .ok_or_else(|| self.unknown_mem_error(&source_mem))?;
        if self.mounts[mount_idx].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(source_mem));
        }

        let schema = self
            .schemas
            .get(&source_mem)
            .expect("schema present for every registered mount");

        // Determine whether this is a cross-mem edge to a mem
        // pinning a schema with a *different name*. Same-name (any
        // version pair — a schema name is a domain) and same-mem
        // stay on the intra-mem validation path, governed by the
        // source mem's pinned version; cross-different-schema
        // routes vocabulary and shape checks through the source
        // schema's `cross_mem_relationships:` section.
        //
        // If the target mem is not mounted (unknown to the engine
        // — typically only in malformed callers), there is no target
        // schema to consult and the validation falls back to the
        // intra-mem path. Real workspaces always mount the target
        // mem before relating.
        let target_schema_ref: Option<SchemaRef> = if source_mem == target_mem {
            None
        } else {
            // Loaded mems answer from the schema catalogue; an
            // unmounted mem with discoverable storage answers from
            // its stored config's pin (flywheel W7/02) — see
            // `target_schema_ref_for_routing`.
            super::target_schema_ref_for_routing(self, &target_mem)
        };
        let cross_mem_different = match (&target_schema_ref, schema.id()) {
            (Some(target), (src_name, _)) => target.name != src_name,
            (None, _) => false,
        };

        let mut warnings: Vec<WarningHint> = Vec::new();
        // Reload-before-operation drift notice, surfaced first.
        warnings.append(&mut drift_warnings);
        // Vocabulary check: intra-mem flow consults the source
        // schema's `relationships.definitions`; cross-different-schema
        // skips this entirely (the cross-mem entry's `definitions`
        // are the sole authority — see the add-path check below).
        if !cross_mem_different {
            match validate_rel_type(&args.rel_type, schema.as_ref())? {
                RelationshipCheck::Ok => {}
                RelationshipCheck::OpenWarning(message) => {
                    warnings.push(WarningHint::UndeclaredRelationshipOpen {
                        rel_type: args.rel_type.clone(),
                        message,
                    });
                }
            }
        }

        // Clone the source entity early so subsequent mutable
        // operations on `self.store` (the stub-creation upsert below)
        // don't conflict with the borrow.
        let entity: Entity = self
            .store
            .get(&args.source)
            .ok_or_else(|| EngineError::NotFound {
                id: args.source.to_string(),
            })?
            .clone();

        // Stubs have no `entity_type`, so the schema lookup below
        // would surface a cryptic `UnknownType { name: "" }`. Surface
        // the actual constraint instead — a stub source has no body
        // to write to and no schema-resolved type to validate
        // against. Promotion via `memstead_create` adopts the stub's
        // incoming references and lets the agent re-issue the relate
        // against a real entity.
        if entity.stub {
            return Err(EngineError::StubCannotRelate {
                id: args.source.to_string(),
            });
        }

        let target_type = self
            .store
            .get(&args.target)
            .map(|e| e.entity_type.clone())
            .filter(|t| !t.is_empty());
        // Storage verification for a target in a DEFERRED mem
        // (flywheel W7/02): the store cannot answer for an unloaded
        // mem, so existence comes from the cheap storage probe and —
        // when the shape check will need it — the type from the one
        // resolved blob. Neither triggers the mem's load.
        let target_verified_in_storage = if target_type.is_none() && target_mem != source_mem {
            matches!(
                super::probe_deferred_target(self, &args.target)?,
                super::DeferredTargetProbe::Exists
            )
        } else {
            false
        };
        let target_type = match target_type {
            Some(t) => Some(t),
            None if target_verified_in_storage && !args.remove => {
                super::peek_deferred_target_type(self, &args.target)?
            }
            None => None,
        };
        if target_verified_in_storage {
            // A verified target's mem is not "uncreated" — for an
            // unmounted mem the discovery hook just found its storage
            // and the entity in it; the layered warning would claim
            // the opposite.
            target_mem_uncreated = false;
        }
        // Shape validation is add-only. Edges that violated the
        // schema's shape before constraints landed must remain
        // removable through `memstead_relate remove=true` — otherwise the
        // graph carries unfixable shape drift. The health scan
        // surfaces the existing violations so an agent can run the
        // cleanup pass. The same posture applies to cross-mem
        // vocabulary: the cleanup path stays permissive so
        // pre-tightening edges can be dropped without first
        // re-declaring them in the source schema.
        // Per-edge description posture (intra-mem and cross-mem).
        // Add-only — the remove path stays permissive so pre-tightening
        // edges remain droppable (mirrors the shape-validation posture
        // below). Posture is a no-op for rel-types not declared in the
        // schema; the vocabulary gate runs first and surfaces those.
        if !args.remove {
            validate_description_posture(
                self,
                &args.rel_type,
                args.description.as_deref(),
                &source_mem,
                &target_mem,
                &args.source,
                &args.target,
            )?;
            // Refuse
            // explicit `memstead_relate` calls for rel-types whose schema
            // declares `manual_authoring: forbidden`. The body-link →
            // relation alias machinery synthesises these relations
            // from wiki-links via a separate path that doesn't go
            // through this validator, so the alias contract stays
            // intact.
            super::validate_manual_authoring_posture(
                self,
                &args.rel_type,
                &source_mem,
                &args.source,
                &args.target,
            )?;
        }

        if !args.remove {
            if cross_mem_different {
                // Safe-by-construction: `cross_mem_different` only
                // becomes true when `target_schema_ref` is `Some`.
                let target_ref = target_schema_ref
                    .as_ref()
                    .expect("target_schema_ref is Some when cross_mem_different");
                match validate_cross_mem_edge(
                    &args.rel_type,
                    entity.entity_type.as_str(),
                    target_type.as_deref(),
                    schema.as_ref(),
                    target_ref,
                ) {
                    CrossMemRelCheck::Ok => {}
                    CrossMemRelCheck::EdgeNotDeclared => {
                        let (src_name, src_version) = schema.id();
                        return Err(EngineError::CrossMemEdgeNotDeclared {
                            source_schema: SchemaRef::new(src_name, src_version).as_display(),
                            target_schema: target_ref.as_display(),
                            rel_type: args.rel_type.clone(),
                            from_id: args.source.to_string(),
                            to_id: args.target.to_string(),
                        });
                    }
                    CrossMemRelCheck::Invalid(v) => {
                        return Err(EngineError::Validation(v));
                    }
                }
            } else {
                validate_rel_shape(
                    &args.rel_type,
                    entity.entity_type.as_str(),
                    target_type.as_deref(),
                    schema.as_ref(),
                )?;
            }
        }

        if let Some(expected) = args.expected_hash.as_deref()
            && entity.content_hash != expected
        {
            return Err(EngineError::HashMismatch {
                id: args.source.to_string(),
                current: entity.content_hash.clone(),
                is_stub: entity.stub,
            });
        }

        // Cycle family on the real-add path — the self-loop refusal
        // (listed no-self-loop rel-types) and the acyclic long-cycle refusal,
        // via the shared gate every edge-writing verb runs
        // (`validate_edge_acyclicity`). Skipped on the remove path:
        // removal can only break cycles, never close one.
        if !args.remove {
            super::validate_edge_acyclicity(
                &self.store,
                schema,
                &args.source,
                entity.entity_type.as_str(),
                &args.target,
                &args.rel_type,
            )?;
        }

        let type_def = schema
            .get_type(&entity.entity_type)
            .ok_or_else(|| unknown_type_error(schema, &entity.entity_type))?;

        let mut next = entity.clone();
        let already = next
            .relationships
            .iter()
            .position(|r| r.rel_type == args.rel_type && r.target == args.target);

        // Alias-existence RESTRICT semantics on the remove path. Under
        // set-membership semantics a body wiki-link `[[X]]` aliases the
        // *set* of relations to X; removing one relation is fine as
        // long as another survives. Refuse only when the removal would
        // empty the relation-set to `b` while body wiki-links to `b`
        // are still present in the source entity's section bodies.
        if args.remove && already.is_some() {
            let other_relation_to_target_exists = entity
                .relationships
                .iter()
                .any(|r| r.target == args.target && r.rel_type != args.rel_type);
            if !other_relation_to_target_exists {
                // Read-side scan over the source entity's existing
                // body. Use the lenient decoder so on-disk drift on
                // pre-strict entities continues to surface in the
                // body-link survival check — the mutation gate sits
                // on the create/update path, not on a relate-remove
                // scan of historical state.
                let mut surviving_sections: Vec<String> = Vec::new();
                for (section_key, body) in entity.sections.iter() {
                    let inline_targets =
                        crate::entity::parser::extract_inline_links_lenient(body, &source_mem);
                    if inline_targets.iter().any(|t| t == &args.target) {
                        surviving_sections.push(section_key.clone());
                    }
                }
                if !surviving_sections.is_empty() {
                    return Err(EngineError::RelationHasBodyLinks {
                        from_id: args.source.to_string(),
                        to_id: args.target.to_string(),
                        rel_type: args.rel_type.clone(),
                        body_links: surviving_sections,
                    });
                }
            }
        }

        let action = if args.remove {
            match already {
                Some(idx) => {
                    next.relationships.remove(idx);
                    RelateAction::Removed
                }
                None => RelateAction::NoOpAbsent,
            }
        } else {
            match already {
                Some(_) => RelateAction::NoOpAlreadyPresent,
                None => {
                    next.relationships.push(Relationship {
                        rel_type: args.rel_type.clone(),
                        target: args.target.clone(),
                        description: normalise_description(args.description.as_deref()),
                    });
                    RelateAction::Added
                }
            }
        };

        // Block-tier `required_outgoing` on the remove path: dropping
        // this edge must not leave a `severity: block` block
        // unsatisfied — the same refusal create/update raise when the
        // written edge set falls short. Warn-tier blocks stay silent
        // here (the health sweep owns standing warn-tier findings;
        // relate-remove has never warned and the no-noise rule keeps
        // it that way).
        if matches!(action, RelateAction::Removed) {
            let blocked: Vec<_> =
                crate::ops::health::unsatisfied_required_outgoing(&next, type_def.as_ref())
                    .into_iter()
                    .filter(|b| b.severity == memstead_schema::ConstraintSeverity::Block)
                    .collect();
            if !blocked.is_empty() {
                return Err(EngineError::RequiredOutgoingUnsatisfied {
                    entity_type: next.entity_type.clone(),
                    entity_id: args.source.to_string(),
                    missing: blocked,
                });
            }
            // Edge-dependent declared constraints: removing this edge
            // must not un-back a block-tier `enum_from_neighbour`
            // value (the edge to the enumerating neighbour is what
            // backs it). Edge-independent forms are filtered out —
            // their verdict is identical before and after a relate,
            // so refusing here would block unrelated repair work.
            // `None` check provider: only `enum_from_neighbour` is
            // kept below, so the checks-gated form never evaluates on
            // this path (a relate changes edges, not the gated field).
            let blocked: Vec<_> = crate::ops::health::unsatisfied_constraints(
                &self.store,
                &next,
                type_def.as_ref(),
                Some(&args.source),
                None,
            )
            .into_iter()
            .filter(|v| {
                matches!(
                    v,
                    crate::ops::health::UnsatisfiedConstraint::EnumFromNeighbour { .. }
                ) && v.severity() == memstead_schema::ConstraintSeverity::Block
            })
            .collect();
            if !blocked.is_empty() {
                return Err(EngineError::ConstraintUnsatisfied {
                    entity_type: next.entity_type.clone(),
                    entity_id: args.source.to_string(),
                    violations: blocked,
                });
            }
        }

        // Plan a stub for an absent target on the real-add path.
        // Skipped on no-op paths (NoOpAlreadyPresent / NoOpAbsent — the
        // edge isn't actually being added) and on the remove path (the
        // edge being dropped, no need to manifest the target). This is
        // the engine's target-materialisation step on the add path;
        // prepare only records the decision — the upsert happens in
        // `stage_prepared_relate` so prepare stays store-neutral.
        // The auto-stub surfaces as a typed `AutoStubCreated` warning
        // on the response's `warnings[]` — the deprecated top-level
        // `stub_warning` field that pre-Item-03 carried this fact has
        // been removed, so every diagnostic now follows the uniform
        // `{ code, message, details }` warning shape.
        let mut stub_target: Option<EntityId> = None;
        let mut stub_kind = crate::entity::StubKind::ForwardReference;
        if matches!(action, RelateAction::Added) && !self.store.contains(&args.target) {
            stub_target = Some(args.target.clone());
            if target_verified_in_storage {
                // The target EXISTS — storage said so. The in-store
                // stub is only plan 01's until-load representation of
                // a cross-mem link into a deferred mem (it resolves
                // when the mem loads), so it carries the load-time
                // kind and no AUTO_STUB_CREATED warning: that warning
                // tells an agent the target awaits creation, which
                // would be false here.
                stub_kind = crate::entity::StubKind::LoadTime;
            } else {
                // Rehearsal honesty: on the dry-run path nothing is
                // written, so the warning's `pending` flag branches the
                // message to the would-be form — the code stays
                // AUTO_STUB_CREATED either way.
                warnings.push(WarningHint::AutoStubCreated {
                    stub_id: args.target.clone(),
                    pending: args.dry_run,
                });
            }
            // If the target mem is unmounted, the
            // auto-stub above has no `_mem_schema` resolution. Layer
            // the typed mem-uncreated warning alongside the
            // `AutoStubCreated` so the operator sees both signals.
            // (Never true on the verified branch — a deferred mem is
            // mounted by definition.)
            if target_mem_uncreated {
                warnings.push(WarningHint::CrossMemTargetMemUncreated {
                    from_mem: source_mem.clone(),
                    to_mem: target_mem.clone(),
                    target_id: args.target.clone(),
                });
            }
        }

        // No-op paths skip the disk write so the provenance log doesn't
        // record a non-event. Return the live `content_hash` so callers
        // can chain follow-ups without refetching. Surface the no-op as
        // a typed warning so an agent re-running a pipeline can tell the
        // call didn't change the graph (mirrors full's wire shape).
        if matches!(
            action,
            RelateAction::NoOpAlreadyPresent | RelateAction::NoOpAbsent
        ) {
            match action {
                RelateAction::NoOpAlreadyPresent => {
                    warnings.push(WarningHint::DuplicateRelationship {
                        rel_type: args.rel_type.clone(),
                        from: args.source.clone(),
                        to: args.target.clone(),
                    });
                }
                RelateAction::NoOpAbsent => {
                    warnings.push(WarningHint::NoSuchRelationship {
                        rel_type: args.rel_type.clone(),
                        from: args.source.clone(),
                        to: args.target.clone(),
                    });
                }
                _ => unreachable!(),
            }
            return Ok(RelatePrepareOutcome::Done(RelateEntityOutcome {
                from: args.source,
                to: args.target,
                rel_type: args.rel_type,
                action,
                content_hash: entity.content_hash.clone(),
                write_id: String::new(),
                source: "explicit".to_string(),
                warnings,
                // No-op branch: nothing changed in the graph, so the
                // orphan-stub sweep can't have anything to collect.
                orphan_stubs_removed: Vec::new(),
            }));
        }

        // The relate path rewrites the on-disk file (the
        // `## Relationships` section materialises from
        // `next.relationships`), so the schema's `auto_timestamp`
        // metadata (default schema: `last_modified`) bumps to the
        // current ISO. Only fires on the commit-producing branch —
        // the no-op early-return above skips this block, so an
        // idempotent re-add or NoOpAbsent never advances the stamp.
        let today = self.now_iso();
        super::auto_stamp_timestamps(&mut next, type_def.as_ref(), &today);

        let file_path = next.file_path.clone();
        let markdown = super::render_for_write(&next, type_def.as_ref())?;

        Ok(RelatePrepareOutcome::Prepared(PreparedRelate {
            mount_idx,
            source_mem,
            from: args.source,
            to: args.target,
            rel_type: args.rel_type,
            action,
            file_path,
            markdown,
            warnings,
            stub_target,
            stub_kind,
            type_def,
        }))
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
        // Short ids on either end resolve once, before the touched-mem
        // probe reads each item's mems; an item that does not resolve
        // fails as its own error below, never the whole batch.
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
        let mut touched_mems: Vec<String> = relates
            .iter()
            .flat_map(|(a, _)| [a.source.mem().to_string(), a.target.mem().to_string()])
            .collect();
        touched_mems.sort();
        touched_mems.dedup();
        // A mem that is only ever a TARGET in this batch and is
        // deferred stays unloaded: target verification runs against
        // storage (flywheel W7/02), and the funnel's phase-0 trigger
        // would otherwise convert verification into a full load.
        // Source mems always reload — a write into a mem needs the mem.
        let source_mems: std::collections::HashSet<&str> =
            relates.iter().map(|(a, _)| a.source.mem()).collect();
        touched_mems.retain(|m| source_mems.contains(m.as_str()) || !self.mem_is_deferred(m));
        for m in &touched_mems {
            self.reload_if_stale(Some(m));
        }
        // Same acyclic-guard rule as the single-item path: any added
        // edge on an ACYCLIC rel-type (or a rel-type in an
        // `acyclic_sets` set, whose guard walks the set's UNION
        // subgraph) walks the whole subgraph, so the walk must see
        // every mem — deferred (lazy, unloaded) ones included — or a
        // cycle through an unloaded mem is admitted (the fifth
        // lazy-mount grade demonstrated exactly that through this
        // path). Declared signals on either endpoint's schema need
        // the full load for the same reason as the single-item path.
        if relates.iter().any(|(a, _)| {
            (!a.remove
                && self.schemas.get(a.source.mem()).is_some_and(|s| {
                    s.relationship_acyclic(&a.rel_type)
                        || s.acyclic_set_containing(&a.rel_type).is_some()
                }))
                || [a.source.mem(), a.target.mem()].iter().any(|m| {
                    self.schemas
                        .get(*m)
                        .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
                })
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
                    // Derivation baseline (plan 12) — same predicate
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
            // `<id>: <note>` lines (decision 3), keyed by the edge's
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
            match self.mounts[m].backend.commit(&subject, &ctx) {
                Ok(sha) => mount_commits.push((m, sha)),
                Err(e) => {
                    // A commit failed: roll back the store and any
                    // still-pending backends. Mems already committed
                    // in this loop stay committed (the family's
                    // per-mem atomicity).
                    self.store = store_snapshot;
                    self.discard_all_pending();
                    return Err(e.into());
                }
            }
        }

        // Provenance per entry, self-write markers per mount.
        let mut batch_warnings: Vec<WarningHint> = short_hints;
        for (p, note) in prepared.iter().zip(notes.iter()) {
            let write_id = mount_commits
                .iter()
                .find(|(m, _)| *m == p.mount_idx)
                .map(|(_, s)| s.clone())
                .unwrap_or_default();
            self.mounts[p.mount_idx].backend.append_provenance(
                &Provenance::new(
                    std::time::SystemTime::now(),
                    ProvenanceKind::Relate,
                    Some(p.from.to_string()),
                    actor,
                    client.cloned(),
                    note.clone(),
                )
                .with_role(self.current_role)
                .with_identity(self.current_identity.clone()),
            )?;
            self.record_self_write(p.mount_idx, &write_id);
            batch_warnings.extend(self.stamp_mutation_versions(p.mount_idx));
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

#[cfg(test)]
mod derivation_tests;
#[cfg(test)]
mod tests;
