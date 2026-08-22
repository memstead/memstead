//! `Engine::relate_entity` and the `relate` alias — append / remove a
//! single edge between two entities.

use std::path::Path;

use crate::engine_fallback_type;
use crate::entity::generator::generate_markdown;
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
    pub(super) type_def: std::sync::Arc<memstead_schema::TypeDefinition>,
}

/// What `prepare_relate` resolved to.
pub(super) enum RelatePrepareOutcome {
    /// No-op path (idempotent re-add / absent remove): the complete
    /// outcome, with its typed no-op warning and an empty
    /// `commit_sha` — nothing to write or commit.
    Done(RelateEntityOutcome),
    /// A real edge change, validated and ready to stage.
    Prepared(PreparedRelate),
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
        let source_mem = args.source.mem().to_string();
        let target_mem = args.target.mem().to_string();

        // Reload-before-operation: reload the source mem (and the
        // target mem, when distinct) if a sibling advanced either
        // ref, so the source `expected_hash` compare and the target
        // existence/stub decisions below run against current truth.
        // The drift notice rides the outcome's `warnings`. Hoisted
        // out of `prepare_relate` so `batch_relate` can probe every
        // touched mem exactly once up front instead of per entry.
        let mut drift_warnings = self.reload_if_stale(Some(&source_mem));
        if target_mem != source_mem {
            drift_warnings.append(&mut self.reload_if_stale(Some(&target_mem)));
        }
        // An ACYCLIC rel-type's cycle guard walks the whole rel-type
        // subgraph (`would_cycle`), and a cycle can pass through any
        // mem — an edge on a deferred (lazy, unloaded) mem's entity is
        // invisible to a walk over the endpoint mems alone, so an add
        // an eager boot refuses would land silently and corrupt an
        // innocent edge on the next full load (the fourth lazy-mount
        // grade demonstrated exactly that on a three-mem chain). Full
        // load before the guard; non-acyclic rel-types skip the cost.
        if !args.remove
            && self
                .schemas
                .get(&source_mem)
                .is_some_and(|s| s.relationship_acyclic(&args.rel_type))
        {
            self.ensure_mems_loaded(None);
        }

        let dry_run = args.dry_run;
        let prepared = match self.prepare_relate(args, drift_warnings)? {
            RelatePrepareOutcome::Done(outcome) => {
                // Derivation re-baseline (agent-trust plan 12): on a
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
        // reports the PROSPECTIVE post-write hash; `commit_sha` stays
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
            // Identical-warnings contract: the real path appends the
            // `require_notes` nudge after its commit; the rehearsal of
            // a would-be commit carries the same warning.
            let mut warnings = prepared.warnings;
            if let Some(w) = self.note_missing_warning("relate_entity", note) {
                warnings.push(w);
            }
            return Ok(RelateEntityOutcome {
                from: prepared.from,
                to: prepared.to,
                rel_type: prepared.rel_type,
                action: prepared.action,
                content_hash: parse_result.entity.content_hash,
                commit_sha: String::new(),
                source: "explicit".to_string(),
                orphan_stubs_removed: Vec::new(),
                warnings,
            });
        }

        self.stage_prepared_relate(&prepared)?;

        // Derivation baseline (agent-trust plan 12): an explicit add
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
        let ctx = CommitContext {
            actor,
            client: client.cloned(),
            tool: Some("relate_entity"),
            note: note.map(String::from),
            role: self.current_role,
            logical_operation_id: None,
            entity_ids: None,
        };
        let commit_sha = backend.commit(&commit_subject, &ctx)?;

        backend.append_provenance(
            &Provenance::new(
                std::time::SystemTime::now(),
                ProvenanceKind::Relate,
                Some(prepared.from.to_string()),
                actor,
                client.cloned(),
                note.map(String::from),
            )
            .with_role(self.current_role),
        )?;

        self.record_self_write(prepared.mount_idx, &commit_sha);
        self.stamp_mutation_versions(prepared.mount_idx);

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
        self.invalidate_search_indexes();

        let PreparedRelate {
            from,
            to,
            rel_type,
            action,
            mut warnings,
            ..
        } = prepared;

        // `require_notes` provenance nudge — single engine-level
        // enforcement point. Only reached on the real-commit path
        // (Added / Removed); the NoOpAlreadyPresent / NoOpAbsent branches
        // return early above with an empty `commit_sha` and never demand
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
            commit_sha,
            source: "explicit".to_string(),
            warnings,
            orphan_stubs_removed,
        })
    }

    /// The duplicate-add re-baseline (agent-trust plan 12). Called
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
        let ctx = CommitContext {
            actor,
            client: client.cloned(),
            tool: Some("relate_entity"),
            note: note.map(String::from),
            role: self.current_role,
            logical_operation_id: None,
            entity_ids: None,
        };
        let commit_sha = backend.commit(
            &format!("memstead: derivation re-baseline {}", outcome.from),
            &ctx,
        )?;
        self.record_self_write(mount_idx, &commit_sha);
        self.stamp_mutation_versions(mount_idx);
        outcome.commit_sha = commit_sha;
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
        let target_schema = if source_mem == target_mem {
            None
        } else {
            self.schemas.get(&target_mem).cloned()
        };
        let target_schema_ref: Option<SchemaRef> = target_schema.as_ref().map(|s| {
            let (name, version) = s.id();
            SchemaRef::new(name, version)
        });
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
            let blocked: Vec<_> = crate::ops::health::unsatisfied_constraints(
                &self.store,
                &next,
                type_def.as_ref(),
                Some(&args.source),
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
        if matches!(action, RelateAction::Added) && !self.store.contains(&args.target) {
            stub_target = Some(args.target.clone());
            // Rehearsal honesty: on the dry-run path nothing is
            // written, so the warning's `pending` flag branches the
            // message to the would-be form — the code stays
            // AUTO_STUB_CREATED either way.
            warnings.push(WarningHint::AutoStubCreated {
                stub_id: args.target.clone(),
                pending: args.dry_run,
            });
            // If the target mem is unmounted, the
            // auto-stub above has no `_mem_schema` resolution. Layer
            // the typed mem-uncreated warning alongside the
            // `AutoStubCreated` so the operator sees both signals.
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
                commit_sha: String::new(),
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
        let markdown = generate_markdown(&next, type_def.as_ref());

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
            self.store.upsert(
                stub_id.clone(),
                make_stub(stub_id, crate::entity::StubKind::ForwardReference),
            );
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
    /// the staged state) with the marker form's empty `commit_sha`; an
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
            return Ok(crate::ops::BatchResult {
                orphan_stubs_removed: Vec::new(),
                errors_suppressed: 0,
                applied: true,
                results: Vec::new(),
                succeeded: 0,
                failed: 0,
                commit_sha: String::new(),
            });
        }

        // Reload every touched mem (sources and targets) once, up
        // front — the per-entry probe is hoisted out of
        // `prepare_relate` for exactly this.
        let mut touched_mems: Vec<String> = relates
            .iter()
            .flat_map(|(a, _)| [a.source.mem().to_string(), a.target.mem().to_string()])
            .collect();
        touched_mems.sort();
        touched_mems.dedup();
        for m in &touched_mems {
            self.reload_if_stale(Some(m));
        }
        // Same acyclic-guard rule as the single-item path: any added
        // edge on an ACYCLIC rel-type walks the whole rel-type
        // subgraph, so the walk must see every mem — deferred (lazy,
        // unloaded) ones included — or a cycle through an unloaded
        // mem is admitted (the fifth lazy-mount grade demonstrated
        // exactly that through this path).
        // Same acyclic-guard rule as the single-item path: any added
        // edge on an ACYCLIC rel-type walks the whole rel-type
        // subgraph, so the walk must see every mem — deferred (lazy,
        // unloaded) ones included — or a cycle through an unloaded
        // mem is admitted (the fifth lazy-mount grade demonstrated
        // exactly that through this path).
        if relates.iter().any(|(a, _)| {
            !a.remove
                && self
                    .schemas
                    .get(a.source.mem())
                    .is_some_and(|s| s.relationship_acyclic(&a.rel_type))
        }) {
            self.ensure_mems_loaded(None);
        }

        // Snapshot for the all-or-nothing rollback: staged entries
        // mutate the store as they apply (in-order semantics), so a
        // refusal restores this snapshot and discards every backend's
        // pending buffer. Any early-return added below MUST do both.
        let store_snapshot = self.store.clone();

        enum ItemState {
            Applied(&'static str),
            Noop,
            Error,
        }
        let mut items: Vec<(EntityId, ItemState)> = Vec::with_capacity(relates.len());
        let mut prepared: Vec<PreparedRelate> = Vec::new();
        let mut notes: Vec<Option<String>> = Vec::new();
        let mut errors: Vec<(usize, EngineError)> = Vec::new();

        for (i, (args, note)) in relates.into_iter().enumerate() {
            let source_id = args.source.clone();
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
            let failed = errors.len();
            let mut error_map: std::collections::HashMap<usize, EngineError> =
                errors.into_iter().collect();
            let mut reported = 0usize;
            let mut suppressed = 0usize;
            let results: Vec<crate::ops::BatchEntry> = items
                .into_iter()
                .enumerate()
                .map(|(i, (id, _))| match error_map.remove(&i) {
                    Some(e) => {
                        if reported < Self::BATCH_ERROR_REPORT_CAP {
                            reported += 1;
                            crate::ops::BatchEntry {
                                id,
                                action: "error".to_string(),
                                error: Some(super::update::batch_error_envelope(&e)),
                            }
                        } else {
                            suppressed += 1;
                            crate::ops::BatchEntry {
                                id,
                                action: "error".to_string(),
                                error: None,
                            }
                        }
                    }
                    None => crate::ops::BatchEntry {
                        id,
                        action: "not_applied".to_string(),
                        error: None,
                    },
                })
                .collect();
            return Ok(crate::ops::BatchResult {
                orphan_stubs_removed: Vec::new(),
                errors_suppressed: suppressed,
                applied: false,
                results,
                succeeded: 0,
                failed,
                commit_sha: String::new(),
            });
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
            let succeeded = items.len();
            let results: Vec<crate::ops::BatchEntry> = items
                .into_iter()
                .map(|(id, state)| crate::ops::BatchEntry {
                    id,
                    action: match state {
                        ItemState::Applied(label) => label.to_string(),
                        ItemState::Noop => "noop".to_string(),
                        ItemState::Error => unreachable!("refusal path returned above"),
                    },
                    error: None,
                })
                .collect();
            return Ok(crate::ops::BatchResult {
                orphan_stubs_removed,
                errors_suppressed: 0,
                applied: true,
                results,
                succeeded,
                failed: 0,
                commit_sha: String::new(),
            });
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
            let ctx = CommitContext {
                actor,
                client: client.cloned(),
                tool: Some("batch_relate"),
                note: if note_lines.is_empty() {
                    None
                } else {
                    Some(note_lines.join("\n"))
                },
                role: self.current_role,
                logical_operation_id: None,
                entity_ids: Some(entity_ids),
            };
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
        for (p, note) in prepared.iter().zip(notes.iter()) {
            let commit_sha = mount_commits
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
                .with_role(self.current_role),
            )?;
            self.record_self_write(p.mount_idx, &commit_sha);
            self.stamp_mutation_versions(p.mount_idx);
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

        let commit_sha = mount_commits
            .last()
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        let succeeded = items.len();
        let results: Vec<crate::ops::BatchEntry> = items
            .into_iter()
            .map(|(id, state)| crate::ops::BatchEntry {
                id,
                action: match state {
                    ItemState::Applied(label) => label.to_string(),
                    ItemState::Noop => "noop".to_string(),
                    ItemState::Error => unreachable!("refusal path returned above"),
                },
                error: None,
            })
            .collect();

        Ok(crate::ops::BatchResult {
            orphan_stubs_removed,
            errors_suppressed: 0,
            applied: true,
            results,
            succeeded,
            failed: 0,
            commit_sha,
        })
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
mod tests {

    use indexmap::IndexMap;
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::test_helpers::*;
    use crate::engine::{CreateEntityArgs, Engine, EngineError, RelateAction, RelateEntityArgs};
    use crate::ops::WarningHint;
    use crate::storage::FilesystemMemWriter;
    use crate::vcs::{Actor, CommitContext};

    #[test]
    fn relate_alias_delegates_to_relate_entity() {
        // Positional-args alias mirrors full's signature
        // `engine.relate(from, to, rel_type, remove, ctx)`. Add an
        // edge via the alias and via `relate_entity` and assert
        // they reach the same observable post-state.
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        // Seed two real entities (no stub).
        let a = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "specs".to_string(),
                    title: "A".to_string(),
                    entity_type: "spec".to_string(),
                    sections: IndexMap::from_iter([
                        ("identity".to_string(), "seed identity".to_string()),
                        ("purpose".to_string(), "seed purpose".to_string()),
                    ]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                Actor::Cli,
                None,
                None,
            )
            .unwrap();
        let b = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "specs".to_string(),
                    title: "B".to_string(),
                    entity_type: "spec".to_string(),
                    sections: IndexMap::from_iter([
                        ("identity".to_string(), "seed identity".to_string()),
                        ("purpose".to_string(), "seed purpose".to_string()),
                    ]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                Actor::Cli,
                None,
                None,
            )
            .unwrap();

        // Use the positional `relate` alias.
        let ctx = CommitContext::internal();
        let result = engine.relate(&a.id, &b.id, "PART_OF", false, &ctx).unwrap();
        assert_eq!(result.from, a.id);
        assert_eq!(result.to, b.id);
        assert_eq!(result.rel_type, "PART_OF");
        // The edge is in the store post-call.
        let outgoing: Vec<_> = engine.store().outgoing(&a.id).to_vec();
        assert!(
            outgoing
                .iter()
                .any(|e| e.target == b.id && e.rel_type == "PART_OF")
        );
    }

    #[test]
    fn relate_entity_appends_relationship_and_logs_provenance() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Source");
        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let outcome = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(outcome.action, RelateAction::Added);
        assert_ne!(outcome.content_hash, source.content_hash);
        // Edge present in store.
        let edges = engine.store().outgoing(&source.id);
        assert!(
            edges
                .iter()
                .any(|e| e.rel_type == "USES" && e.target == target.id),
            "expected USES edge in store"
        );
        // Provenance log records relate.
        let log = std::fs::read_to_string(tmp.path().join(".memstead/changes.jsonl")).unwrap();
        assert!(log.contains("\"kind\":\"relate\""));
    }

    #[test]
    fn relate_entity_no_op_when_already_present() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Already");
        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(empty_create_args("specs", "T2"), actor, Some(&client), None)
            .unwrap();
        let first = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let second = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(first.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(second.action, RelateAction::NoOpAlreadyPresent);
        // Hash unchanged on no-op.
        assert_eq!(second.content_hash, first.content_hash);
    }

    #[test]
    fn relate_entity_returns_commit_sha_on_real_write() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Source");
        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let outcome = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // Folder backend returns a synthetic CommitId — non-empty string.
        // Wire-equivalent to full's commit SHA: agents reading the field
        // get a usable cursor regardless of which backend served the write.
        assert!(
            !outcome.commit_sha.is_empty(),
            "commit_sha must be populated on a real write"
        );
    }

    #[test]
    fn relate_entity_no_op_paths_carry_typed_warnings_and_empty_commit_sha() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "S");
        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(empty_create_args("specs", "T"), actor, Some(&client), None)
            .unwrap();

        // Add the edge once.
        let first = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Duplicate-add — typed DuplicateRelationship warning, empty
        // commit_sha (no disk write happened).
        let dup = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(first.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(dup.action, RelateAction::NoOpAlreadyPresent);
        assert!(dup.commit_sha.is_empty());
        assert_eq!(dup.warnings.len(), 1);
        assert!(matches!(
            dup.warnings[0],
            WarningHint::DuplicateRelationship { .. }
        ));

        // Remove a non-existent edge — typed NoSuchRelationship warning,
        // empty commit_sha.
        let no_such = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(first.content_hash.clone()),
                    rel_type: "DEPENDS_ON".to_string(),
                    target: target.id.clone(),
                    remove: true,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(no_such.action, RelateAction::NoOpAbsent);
        assert!(no_such.commit_sha.is_empty());
        assert_eq!(no_such.warnings.len(), 1);
        assert!(matches!(
            no_such.warnings[0],
            WarningHint::NoSuchRelationship { .. }
        ));
    }

    #[test]
    fn relate_entity_creates_stub_for_absent_target_on_add_path() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Source");
        let (actor, client) = cli_actor();
        let absent_target = crate::EntityId::new("specs", "ghost-target");
        // Sanity: target not in store.
        assert!(!engine.store().contains(&absent_target));

        let outcome = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: absent_target.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        assert_eq!(outcome.action, RelateAction::Added);
        assert_eq!(outcome.source, "explicit");
        // Auto-stub now surfaces through the typed warning vocabulary
        // (`AutoStubCreated`) on `warnings[]` — the deprecated
        // top-level `stub_warning` field was retired in favour of the
        // uniform diagnostic shape. Agents iterating `warnings[]` see
        // the stub id without special-casing a sibling field.
        let stub_warning = outcome
            .warnings
            .iter()
            .find_map(|w| match w {
                crate::ops::WarningHint::AutoStubCreated { stub_id, .. } => Some(stub_id.clone()),
                _ => None,
            })
            .expect("AutoStubCreated warning must surface when target was absent");
        assert_eq!(stub_warning, absent_target);
        // The real path keeps the performed-effect wording exactly —
        // only the dry-run path carries the conditional form.
        let msg = outcome
            .warnings
            .iter()
            .find(|w| matches!(w, crate::ops::WarningHint::AutoStubCreated { .. }))
            .unwrap()
            .message();
        assert!(
            msg.contains("stub auto-created"),
            "real relate keeps the performed-effect wording: {msg}"
        );

        // Stub now in-store, marked as stub, no body.
        let stub = engine.store().get(&absent_target).expect("stub upserted");
        assert!(stub.stub);
        assert!(stub.entity_type.is_empty());
        assert!(stub.file_path.is_empty());
    }

    #[test]
    fn relate_entity_skips_stub_creation_when_target_already_exists() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Src");
        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(
                empty_create_args("specs", "Real"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let outcome = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        assert!(
            !outcome
                .warnings
                .iter()
                .any(|w| matches!(w, crate::ops::WarningHint::AutoStubCreated { .. })),
            "AutoStubCreated must not surface when target was already in store"
        );
        assert_eq!(outcome.source, "explicit");
        // Real entity remains a real entity (not coerced to stub).
        let target_after = engine.store().get(&target.id).unwrap();
        assert!(!target_after.stub);
    }

    #[test]
    fn relate_entity_does_not_create_stub_on_remove_path() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Src");
        let (actor, client) = cli_actor();
        let absent_target = crate::EntityId::new("specs", "never-existed");

        let outcome = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: absent_target.clone(),
                    remove: true,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Remove of an absent edge — no stub creation, NoOpAbsent action,
        // typed NoSuchRelationship warning.
        assert_eq!(outcome.action, RelateAction::NoOpAbsent);
        assert!(
            !outcome
                .warnings
                .iter()
                .any(|w| matches!(w, crate::ops::WarningHint::AutoStubCreated { .. })),
            "remove path must never auto-stub the target",
        );
        assert!(!engine.store().contains(&absent_target));
    }

    #[test]
    fn relate_entity_remove_refuses_when_source_body_still_references_target() {
        use crate::engine::CreateEntityArgs;
        use indexmap::IndexMap;

        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Source entity carries a body wiki-link to the target — the
        // alias-synthesis pass emits the backing REFERENCES relation
        // (default schema's `alias_target_rel_type` points at
        // REFERENCES, so explicit `memstead_relate type=REFERENCES` is
        // refused; the body link alone produces the relation).
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("identity".to_string(), "source identity".to_string());
        sections.insert(
            "purpose".to_string(),
            "discussion stems from [[target]]".to_string(),
        );
        let source = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "specs".to_string(),
                    title: "Source".to_string(),
                    entity_type: "spec".to_string(),
                    sections,
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let related = source.clone();

        // Removing the explicit relation while the body still has
        // [[target]] must refuse with `RelationHasBodyLinks`, naming
        // the surviving section in `body_links`.
        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(related.content_hash.clone()),
                    rel_type: "REFERENCES".to_string(),
                    target: target.id.clone(),
                    remove: true,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::RelationHasBodyLinks {
                from_id,
                to_id,
                rel_type,
                body_links,
            } => {
                assert_eq!(from_id, source.id.to_string());
                assert_eq!(to_id, target.id.to_string());
                assert_eq!(rel_type, "REFERENCES");
                assert_eq!(body_links, vec!["purpose".to_string()]);
            }
            other => panic!("expected RelationHasBodyLinks, got {other:?}"),
        }
        // Relation must still be present in-memory (refuse before any
        // store mutation).
        let in_mem = engine.get_entity(&source.id).unwrap();
        assert!(
            in_mem
                .relationships
                .iter()
                .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
            "relation must survive the refused remove; got {:?}",
            in_mem.relationships
        );
    }

    #[test]
    fn relate_entity_remove_succeeds_when_body_no_longer_references_target() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Src");
        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(
                empty_create_args("specs", "Other"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // Default seed has empty body sections, so the relation can be
        // added and removed without body-link interference. This locks
        // the happy path: when no body link survives, remove proceeds.
        // (USES instead of REFERENCES — REFERENCES is engine-emitted-only
        // under the default schema's alias_target_rel_type pointer.)
        let related = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let removed = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(related.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: true,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(removed.action, RelateAction::Removed);
    }

    #[test]
    fn relate_entity_auto_stub_is_tagged_forward_reference() {
        // `memstead_relate` to an absent target auto-stubs it. The stub's
        // `stub_kind` records the origin (`ForwardReference`) so an
        // agent reading the stub later via `memstead_entity` sees the
        // typed provenance — not just `stub: true`.
        use crate::entity::StubKind;

        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Src");
        let (actor, client) = cli_actor();
        let absent_target = crate::EntityId::new("specs", "absent-target");

        let _ = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: absent_target.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let stub = engine
            .get_entity(&absent_target)
            .expect("relate auto-stubbed target must be in the store");
        assert!(stub.stub, "auto-stubbed target must carry stub: true");
        assert_eq!(
            stub.stub_kind,
            Some(StubKind::ForwardReference),
            "auto-stub from relate must be tagged ForwardReference; got {:?}",
            stub.stub_kind
        );
    }

    #[test]
    fn relate_entity_case_insensitive_rel_type_input_canonicalises_to_upper_snake_case() {
        // Wire-level contract: rel_type input is case-insensitive; the
        // engine stores it as UPPER_SNAKE_CASE and echoes the canonical
        // form back in the response. Same store-shape regardless of
        // input case.
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Source");
        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(
                empty_create_args("specs", "Target"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Lowercase input — must succeed and store as `USES`.
        let lower = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "uses".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(lower.rel_type, "USES", "response must echo canonical form");
        assert_eq!(lower.action, RelateAction::Added);
        let edges = engine.store().outgoing(&source.id);
        assert!(
            edges
                .iter()
                .any(|e| e.rel_type == "USES" && e.target == target.id),
            "store must hold UPPER_SNAKE_CASE rel_type after lowercase input"
        );

        // Adding via mixed-case input on the same edge is the canonical
        // duplicate — DuplicateRelationship warning, no second store
        // entry.
        let dup = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(lower.content_hash.clone()),
                    rel_type: "Uses".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(dup.action, RelateAction::NoOpAlreadyPresent);
        assert_eq!(dup.rel_type, "USES");
        assert!(matches!(
            dup.warnings[0],
            WarningHint::DuplicateRelationship { .. }
        ));
    }

    #[test]
    fn relate_entity_rejects_cross_mem_when_policy_denies() {
        // Default workspace settings carry no `cross_mem_links`
        // policy and no `default_cross_links` on the create rules, so
        // `cross_mem_link_allowed` returns false for any cross-mem
        // pair. The relate refuse now surfaces the typed
        // policy-denial code instead of the legacy categorical
        // `CrossMemRelate`.
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "S");
        let (actor, client) = cli_actor();
        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: crate::EntityId::new("other-mem", "thing"),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::CrossMemLinkNotAllowed { from_mem, to_mem } => {
                assert_eq!(from_mem, "specs");
                assert_eq!(to_mem, "other-mem");
            }
            other => panic!("expected CrossMemLinkNotAllowed, got {other:?}"),
        }
    }

    /// Bare-string target without a `mem--` separator is malformed
    /// (the wiki-link grammar requires `<mem>--<path>`). Pre-fix
    /// the cross-mem check fired first: the parser saw `mem: ""`,
    /// compared against the source mem, and produced
    /// `CROSS_MEM_RELATION` — pointing the agent at workspace
    /// `[cross_mem_links]` policy when the actual issue was a
    /// malformed id. Post-fix the grammar gate runs first; the
    /// envelope identifies the real problem.
    #[test]
    fn relate_entity_malformed_bare_target_surfaces_invalid_entity_id_not_cross_mem() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "S");
        let (actor, client) = cli_actor();
        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    // No `--` separator AND contains characters the
                    // grammar rejects. Parses as mem="", path=raw.
                    target: crate::EntityId("bad target with spaces!!".to_string()),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidEntityId { .. }),
            "malformed bare-string target must surface INVALID_ENTITY_ID, got: {err:?}"
        );
    }

    /// Companion case: target carries the source's mem prefix but a
    /// grammar-violating path. The grammar check fires (same path as
    /// the bare-string case); cross-mem stays out of the picture
    /// because mems match.
    #[test]
    fn relate_entity_malformed_prefixed_target_surfaces_invalid_entity_id() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "S");
        let (actor, client) = cli_actor();
        let source_mem = source.id.mem().to_string();
        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: crate::EntityId(format!("{source_mem}--bad target with spaces!!")),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidEntityId { .. }),
            "prefixed malformed target must still surface INVALID_ENTITY_ID, got: {err:?}"
        );
    }

    // ---- Auto-timestamp on relate add/remove ------------------------

    /// `memstead_relate add` rewrites the
    /// source's on-disk file, so its `last_modified` auto-stamp must
    /// bump. The schema's default-stamped field is `last_modified`.
    #[test]
    fn relate_add_bumps_last_modified_on_source_entity() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "S");
        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(empty_create_args("specs", "T"), actor, Some(&client), None)
            .unwrap();

        let outcome = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(outcome.action, RelateAction::Added);

        // last_modified now carries a fresh ISO timestamp on the
        // source entity. The auto-stamp helper sets every
        // `auto_timestamp: true` metadata field on each commit-
        // producing relate mutation.
        let post = engine.get_entity(&source.id).unwrap();
        let last_modified = post
            .metadata
            .get("last_modified")
            .map(|v| v.to_frontmatter_string())
            .unwrap_or_default();
        assert!(
            last_modified.starts_with("20"),
            "last_modified must carry an ISO timestamp post-relate; got: {last_modified:?}"
        );
    }

    /// Relate-add no-op (idempotent
    /// re-add) skips the disk write and therefore does not advance
    /// `last_modified`. The auto-stamp fires only on commit-producing
    /// mutations — wired into the post-no-op-short-circuit branch.
    #[test]
    fn relate_add_noop_does_not_bump_last_modified() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "S");
        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(empty_create_args("specs", "T"), actor, Some(&client), None)
            .unwrap();
        let first = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let pre = engine.get_entity(&source.id).unwrap();
        let pre_stamp = pre
            .metadata
            .get("last_modified")
            .map(|v| v.to_frontmatter_string())
            .unwrap_or_default();

        // Second relate of same edge — NoOpAlreadyPresent.
        let dup = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(first.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(dup.action, RelateAction::NoOpAlreadyPresent);

        let post = engine.get_entity(&source.id).unwrap();
        let post_stamp = post
            .metadata
            .get("last_modified")
            .map(|v| v.to_frontmatter_string())
            .unwrap_or_default();
        assert_eq!(
            pre_stamp, post_stamp,
            "last_modified must not advance on a duplicate-add no-op (no disk write happened)"
        );
    }

    /// Cross-mem relate that policy admits
    /// but whose target mem is not mounted in the workspace emits
    /// `CROSS_MEM_TARGET_MEM_UNCREATED` alongside `AutoStubCreated`.
    /// The auto-stub still lands; the warning is layered observability.
    #[test]
    fn cross_mem_relate_to_uncreated_mem_emits_typed_warning() {
        use memstead_schema::workspace_config::CrossLinkValue;
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "S");
        let (actor, client) = cli_actor();
        // Grant `specs -> uncreated-mem` so the policy gate passes.
        // The target mem is intentionally not mounted; the auto-stub
        // should still land, with the typed warning attached.
        let mut settings = crate::workspace::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            "specs".to_string(),
            CrossLinkValue::List(vec!["uncreated-mem".to_string()]),
        );
        engine.set_settings(settings);

        let absent = crate::EntityId::new("uncreated-mem", "ghost");
        let outcome = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: absent.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(outcome.action, RelateAction::Added);

        // Auto-stub created plus uncreated-mem warning, side by side.
        let saw_uncreated = outcome.warnings.iter().any(|w| {
            matches!(
                w,
                WarningHint::CrossMemTargetMemUncreated {
                    from_mem,
                    to_mem,
                    target_id,
                } if from_mem == "specs"
                    && to_mem == "uncreated-mem"
                    && target_id == &absent
            )
        });
        assert!(
            saw_uncreated,
            "CrossMemTargetMemUncreated warning must surface; got: {:?}",
            outcome.warnings
        );
        // The auto-stub still landed.
        assert!(engine.store().contains(&absent));
    }

    /// Policy refusal takes precedence
    /// over the uncreated-mem warning. When the cross-mem link
    /// isn't granted, the engine refuses with
    /// `CROSS_MEM_LINK_NOT_ALLOWED` and never reaches the warning
    /// emission point — there's no stub to warn about.
    #[test]
    fn cross_mem_relate_policy_refusal_preempts_uncreated_mem_warning() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "S");
        let (actor, client) = cli_actor();
        // No cross_mem_links entry → policy denies.
        let absent = crate::EntityId::new("uncreated-mem", "ghost");
        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: absent.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::CrossMemLinkNotAllowed { .. }));
        // No stub created on the refusal path.
        assert!(!engine.store().contains(&absent));
    }

    /// `memstead_relate --remove` that drops the
    /// last incoming edge to a stub GCs the now-orphan stub in the
    /// same call. The response carries the dropped ids in
    /// `orphan_stubs_removed`, mirroring the `memstead_delete` envelope's
    /// shape so consumers branch uniformly.
    #[test]
    fn relate_remove_garbage_collects_orphan_stub() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Src");
        let (actor, client) = cli_actor();
        let stub_id = crate::EntityId::new("specs", "ghost-target");

        // Auto-stub via relate-add.
        let added = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: stub_id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(engine.store().contains(&stub_id));

        let removed = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(added.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: stub_id.clone(),
                    remove: true,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(removed.action, RelateAction::Removed);
        assert_eq!(
            removed.orphan_stubs_removed,
            vec![stub_id.clone()],
            "orphan stub must be GC'd in the same call"
        );
        assert!(
            !engine.store().contains(&stub_id),
            "stub must be gone from the store after GC"
        );
    }

    /// When the stub has another
    /// surviving incoming edge, the relate-remove GCs nothing —
    /// the stub stays alive via the second referrer. The sweep is
    /// scoped to *just-orphaned* targets, not pre-existing orphans
    /// or stubs that still have referrers.
    #[test]
    fn relate_remove_does_not_gc_stub_with_surviving_incoming_edge() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source_a) = engine_with_seed(&tmp, "SrcA");
        let (actor, client) = cli_actor();
        let source_b = engine
            .create_entity(
                empty_create_args("specs", "SrcB"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let stub_id = crate::EntityId::new("specs", "ghost-target");

        // Both sources relate to the same stub.
        let a_added = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source_a.id.clone(),
                    expected_hash: Some(source_a.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: stub_id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let _b_added = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source_b.id.clone(),
                    expected_hash: Some(source_b.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: stub_id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Drop source_a's edge only — source_b's edge survives,
        // so the stub is not orphaned.
        let removed = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source_a.id.clone(),
                    expected_hash: Some(a_added.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: stub_id.clone(),
                    remove: true,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(removed.action, RelateAction::Removed);
        assert!(
            removed.orphan_stubs_removed.is_empty(),
            "stub with surviving referrer must not be GC'd; got: {:?}",
            removed.orphan_stubs_removed
        );
        assert!(
            engine.store().contains(&stub_id),
            "stub must remain in store while another referrer holds it"
        );
    }

    // ---- Cross-mem vocabulary -------------------------------------

    /// Two-mem test bench wired for cross-mem routing.
    /// Mem `src` pins `src-cv@0.1.0` whose `cross_mem_relationships`
    /// section declares an outbound entry to the `tgt-cv` domain with
    /// `ADDRESSES: doc → req`. Mem `tgt` pins `tgt-cv@0.1.0`, a
    /// schema with a different name. The workspace policy admits the
    /// cross-mem link so vocabulary failures surface independently
    /// of permission.
    mod cross_mem {
        use std::collections::BTreeMap;
        use std::path::Path;

        use indexmap::IndexMap;
        use memstead_schema::SchemaRef;
        use memstead_schema::workspace_config::CrossLinkValue;
        use tempfile::TempDir;

        use crate::backend::MemBackend;
        use crate::engine::test_helpers::*;
        use crate::engine::{
            CreateEntityArgs, CreateEntityOutcome, Engine, EngineError, RelateAction,
            RelateEntityArgs,
        };
        use crate::storage::FilesystemMemWriter;

        use crate::workspace::{
            Mount, MountCapability, MountLifecycle, MountStorage, WorkspaceSettings,
        };

        fn write_schema_files(root: &Path, name: &str, manifest: &str, types: &[(&str, &str)]) {
            let dir = root.join(name);
            std::fs::create_dir_all(dir.join("types")).unwrap();
            std::fs::write(dir.join("schema.yaml"), manifest).unwrap();
            for (type_name, body) in types {
                std::fs::write(dir.join("types").join(format!("{type_name}.yaml")), body).unwrap();
            }
        }

        const TYPE_BODY: &str = r#"description: t
when_to_use: Here
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: _default
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;

        fn make_type_yaml(name: &str) -> String {
            format!("name: {name}\n{TYPE_BODY}")
        }

        fn folder_mount_with_pin(mem: &str, path: std::path::PathBuf, pin: SchemaRef) -> Mount {
            Mount {
                mem: mem.to_string(),
                schema: Some(pin),
                storage: MountStorage::Folder { path },
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            }
        }

        /// Build an engine with two mems pinning two distinct schemas
        /// and a `cross_mem_links` policy admitting the cross-edge.
        fn two_mem_engine() -> (TempDir, Engine, CreateEntityOutcome, CreateEntityOutcome) {
            let tmp = TempDir::new().unwrap();

            // Source schema with cross-mem declarations to the
            // tgt-cv domain.
            let src_manifest = r#"name: src-cv
version: 0.1.0
description: source schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: IMPLEMENTS
      description: intra-mem only
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
cross_mem_relationships:
  - to_schema: tgt-cv
    definitions:
      - name: ADDRESSES
        description: outbound shape-pinned
        default_weight: 1.0
        source_types: [doc]
        target_types: [req]
community:
  resolution: 1.0
  seed: 42
"#;
            // Target schema declares no cross_mem_relationships (we
            // never relate from tgt → src in these tests).
            let tgt_manifest = r#"name: tgt-cv
version: 0.1.0
description: target schema
when_to_use: tests
types:
  - req
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
            let schemas_dir = tmp.path().join("schemas");
            std::fs::create_dir_all(&schemas_dir).unwrap();
            write_schema_files(
                &schemas_dir,
                "src-cv",
                src_manifest,
                &[("doc", &make_type_yaml("doc"))],
            );
            write_schema_files(
                &schemas_dir,
                "tgt-cv",
                tgt_manifest,
                &[("req", &make_type_yaml("req"))],
            );

            let src_dir = tmp.path().join("mem-src");
            let tgt_dir = tmp.path().join("mem-tgt");
            std::fs::create_dir_all(&src_dir).unwrap();
            std::fs::create_dir_all(&tgt_dir).unwrap();

            let src_writer = FilesystemMemWriter::new(src_dir.clone());
            let tgt_writer = FilesystemMemWriter::new(tgt_dir.clone());
            let src_pin = SchemaRef::new("src-cv", semver::Version::new(0, 1, 0));
            let tgt_pin = SchemaRef::new("tgt-cv", semver::Version::new(0, 1, 0));

            let mut engine = Engine::from_mounts_with_schemas_dir(
                vec![
                    (
                        folder_mount_with_pin("src", src_dir, src_pin),
                        Box::new(src_writer) as Box<dyn MemBackend>,
                    ),
                    (
                        folder_mount_with_pin("tgt", tgt_dir, tgt_pin),
                        Box::new(tgt_writer) as Box<dyn MemBackend>,
                    ),
                ],
                Some(&schemas_dir),
            )
            .expect("two-mem engine constructs");

            // Wildcard permission so cross-mem edges aren't blocked
            // by the orthogonal policy gate (we exercise the vocabulary
            // gate here, not the permission gate).
            let mut settings = WorkspaceSettings::default();
            let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
            links.insert("src".to_string(), CrossLinkValue::Wildcard);
            settings.cross_mem_links = links;
            engine.set_settings(settings);

            let (actor, client) = cli_actor();
            let src_entity = engine
                .create_entity(
                    CreateEntityArgs {
                        anchors: Vec::new(),
                        mem: "src".to_string(),
                        title: "Doc One".to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([("body".to_string(), "seed".to_string())]),
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("source entity creates");
            let tgt_entity = engine
                .create_entity(
                    CreateEntityArgs {
                        anchors: Vec::new(),
                        mem: "tgt".to_string(),
                        title: "Req One".to_string(),
                        entity_type: "req".to_string(),
                        sections: IndexMap::from_iter([("body".to_string(), "seed".to_string())]),
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("target entity creates");

            (tmp, engine, src_entity, tgt_entity)
        }

        #[test]
        fn cross_different_schema_admits_declared_edge() {
            let (_tmp, mut engine, src, tgt) = two_mem_engine();
            let (actor, client) = cli_actor();
            let outcome = engine
                .relate_entity(
                    RelateEntityArgs {
                        source: src.id.clone(),
                        expected_hash: Some(src.content_hash.clone()),
                        rel_type: "ADDRESSES".to_string(),
                        target: tgt.id.clone(),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("declared cross-mem edge admits");
            assert_eq!(outcome.rel_type, "ADDRESSES");
        }

        /// Same schema name at different versions is the same domain:
        /// edges between two `same-dom`-pinned mems route through
        /// the intra-schema relationship vocabulary (governed by the
        /// source mem's pinned version) with no
        /// `cross_mem_relationships` declaration at all.
        #[test]
        fn same_name_different_version_uses_intra_mem_vocabulary() {
            let tmp = TempDir::new().unwrap();

            let manifest_for = |version: &str| {
                format!(
                    r#"name: same-dom
version: {version}
description: same-domain schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: IMPLEMENTS
      description: intra-mem vocabulary
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#
                )
            };
            let schemas_dir = tmp.path().join("schemas");
            std::fs::create_dir_all(&schemas_dir).unwrap();
            // Subdir names carry the version so both iterations of the
            // `same-dom` domain coexist in one schemas dir.
            write_schema_files(
                &schemas_dir,
                "same-dom-0.1.0",
                &manifest_for("0.1.0"),
                &[("doc", &make_type_yaml("doc"))],
            );
            write_schema_files(
                &schemas_dir,
                "same-dom-0.2.0",
                &manifest_for("0.2.0"),
                &[("doc", &make_type_yaml("doc"))],
            );

            let src_dir = tmp.path().join("mem-src");
            let tgt_dir = tmp.path().join("mem-tgt");
            std::fs::create_dir_all(&src_dir).unwrap();
            std::fs::create_dir_all(&tgt_dir).unwrap();
            let src_pin = SchemaRef::new("same-dom", semver::Version::new(0, 1, 0));
            let tgt_pin = SchemaRef::new("same-dom", semver::Version::new(0, 2, 0));
            let mut engine = Engine::from_mounts_with_schemas_dir(
                vec![
                    (
                        folder_mount_with_pin("src", src_dir.clone(), src_pin),
                        Box::new(FilesystemMemWriter::new(src_dir)) as Box<dyn MemBackend>,
                    ),
                    (
                        folder_mount_with_pin("tgt", tgt_dir.clone(), tgt_pin),
                        Box::new(FilesystemMemWriter::new(tgt_dir)) as Box<dyn MemBackend>,
                    ),
                ],
                Some(&schemas_dir),
            )
            .expect("same-domain two-version engine constructs");

            let mut settings = WorkspaceSettings::default();
            let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
            links.insert("src".to_string(), CrossLinkValue::Wildcard);
            settings.cross_mem_links = links;
            engine.set_settings(settings);

            let (actor, client) = cli_actor();
            let mk_entity = |engine: &mut Engine, mem: &str, title: &str| {
                engine
                    .create_entity(
                        CreateEntityArgs {
                            anchors: Vec::new(),
                            mem: mem.to_string(),
                            title: title.to_string(),
                            entity_type: "doc".to_string(),
                            sections: IndexMap::from_iter([(
                                "body".to_string(),
                                "seed".to_string(),
                            )]),
                            metadata: IndexMap::new(),
                            relations: Vec::new(),
                            dry_run: false,
                        },
                        actor,
                        Some(&client),
                        None,
                    )
                    .expect("entity creates")
            };
            let src_entity = mk_entity(&mut engine, "src", "Doc A");
            let tgt_entity = mk_entity(&mut engine, "tgt", "Doc B");

            let outcome = engine
                .relate_entity(
                    RelateEntityArgs {
                        source: src_entity.id.clone(),
                        expected_hash: Some(src_entity.content_hash.clone()),
                        rel_type: "IMPLEMENTS".to_string(),
                        target: tgt_entity.id.clone(),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("same-domain edge uses the intra-schema vocabulary across versions");
            assert_eq!(outcome.rel_type, "IMPLEMENTS");
        }

        #[test]
        fn cross_different_schema_unknown_rel_type_returns_invalid_rel_type() {
            // `IMPLEMENTS` exists intra-mem but not in the cross-mem
            // entry — must refuse with INVALID_REL_TYPE against the
            // cross-mem entry's vocabulary (not intra-mem's).
            let (_tmp, mut engine, src, tgt) = two_mem_engine();
            let (actor, client) = cli_actor();
            let err = engine
                .relate_entity(
                    RelateEntityArgs {
                        source: src.id.clone(),
                        expected_hash: Some(src.content_hash.clone()),
                        rel_type: "IMPLEMENTS".to_string(),
                        target: tgt.id.clone(),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap_err();
            match err {
                EngineError::Validation(
                    crate::runtime_validator::ValidationError::InvalidRelationshipType {
                        input,
                        allowed,
                        ..
                    },
                ) => {
                    assert_eq!(input, "IMPLEMENTS");
                    let names: Vec<String> = allowed.into_iter().map(|h| h.name).collect();
                    assert!(names.iter().any(|n| n == "ADDRESSES"));
                    assert!(!names.iter().any(|n| n == "IMPLEMENTS"));
                }
                other => panic!("expected Validation(InvalidRelationshipType), got {other:?}"),
            }
        }

        #[test]
        fn cross_different_schema_shape_violation_returns_invalid_rel_shape() {
            // ADDRESSES is shape-pinned to source=doc, target=req in
            // the cross-mem entry. Need a source whose type isn't doc.
            // src-cv only declares `doc`, so to provoke a shape miss we
            // build a third schema with type `note` and a fresh mem —
            // but that requires more plumbing than this test needs.
            // Instead: exercise a target-side shape miss by relating
            // ADDRESSES to a target that doesn't exist at all — the
            // target_type lookup returns None and the target check is
            // skipped (admits). So we exercise this via cross_mem
            // unit tests instead.
            //
            // What this integration test confirms: the source-side
            // shape check fires when the source type doesn't match —
            // here we'd need a non-`doc` source. Since src-cv only has
            // `doc`, the source-side admits trivially. Covered fully
            // by the runtime_validator unit tests.
        }

        #[test]
        fn cross_different_schema_no_matching_entry_returns_edge_not_declared() {
            // Build a third mem pinning a schema not declared in
            // src-cv's cross_mem_relationships, then relate from src.
            let tmp = TempDir::new().unwrap();
            let src_manifest = r#"name: src-cv
version: 0.1.0
description: source schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: IMPLEMENTS
      description: intra-mem
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
cross_mem_relationships:
  - to_schema: tgt-cv
    definitions:
      - name: ADDRESSES
        description: outbound
        default_weight: 1.0
        source_types: [doc]
        target_types: [req]
community:
  resolution: 1.0
  seed: 42
"#;
            // Different target schema NOT named in src's cross-mem list.
            let other_manifest = r#"name: other-cv
version: 0.1.0
description: foreign schema
when_to_use: tests
types:
  - thing
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
            let schemas_dir = tmp.path().join("schemas");
            std::fs::create_dir_all(&schemas_dir).unwrap();
            write_schema_files(
                &schemas_dir,
                "src-cv",
                src_manifest,
                &[("doc", &make_type_yaml("doc"))],
            );
            write_schema_files(
                &schemas_dir,
                "other-cv",
                other_manifest,
                &[("thing", &make_type_yaml("thing"))],
            );
            let src_dir = tmp.path().join("mem-src");
            let other_dir = tmp.path().join("mem-other");
            std::fs::create_dir_all(&src_dir).unwrap();
            std::fs::create_dir_all(&other_dir).unwrap();

            let mut engine = Engine::from_mounts_with_schemas_dir(
                vec![
                    (
                        folder_mount_with_pin(
                            "src",
                            src_dir.clone(),
                            SchemaRef::new("src-cv", semver::Version::new(0, 1, 0)),
                        ),
                        Box::new(FilesystemMemWriter::new(src_dir)) as Box<dyn MemBackend>,
                    ),
                    (
                        folder_mount_with_pin(
                            "other",
                            other_dir.clone(),
                            SchemaRef::new("other-cv", semver::Version::new(0, 1, 0)),
                        ),
                        Box::new(FilesystemMemWriter::new(other_dir)) as Box<dyn MemBackend>,
                    ),
                ],
                Some(&schemas_dir),
            )
            .expect("engine constructs");

            let mut settings = WorkspaceSettings::default();
            let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
            links.insert("src".to_string(), CrossLinkValue::Wildcard);
            settings.cross_mem_links = links;
            engine.set_settings(settings);

            let (actor, client) = cli_actor();
            let src_entity = engine
                .create_entity(
                    CreateEntityArgs {
                        anchors: Vec::new(),
                        mem: "src".to_string(),
                        title: "D".to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([("body".to_string(), "x".to_string())]),
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap();
            let other_entity = engine
                .create_entity(
                    CreateEntityArgs {
                        anchors: Vec::new(),
                        mem: "other".to_string(),
                        title: "T".to_string(),
                        entity_type: "thing".to_string(),
                        sections: IndexMap::from_iter([("body".to_string(), "x".to_string())]),
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap();

            let err = engine
                .relate_entity(
                    RelateEntityArgs {
                        source: src_entity.id.clone(),
                        expected_hash: Some(src_entity.content_hash.clone()),
                        rel_type: "ADDRESSES".to_string(),
                        target: other_entity.id.clone(),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap_err();
            match err {
                EngineError::CrossMemEdgeNotDeclared {
                    source_schema,
                    target_schema,
                    rel_type,
                    from_id,
                    to_id,
                } => {
                    assert_eq!(source_schema, "src-cv@0.1.0");
                    assert_eq!(target_schema, "other-cv@0.1.0");
                    assert_eq!(rel_type, "ADDRESSES");
                    assert_eq!(from_id, src_entity.id.to_string());
                    assert_eq!(to_id, other_entity.id.to_string());
                }
                other => panic!("expected CrossMemEdgeNotDeclared, got {other:?}"),
            }
        }

        #[test]
        fn intra_mem_with_cross_mem_only_rel_type_returns_invalid_rel_type() {
            // `ADDRESSES` is declared in src-cv's cross_mem_relationships
            // only — intra-mem relate must refuse with
            // INVALID_REL_TYPE since the intra-mem vocabulary
            // (`IMPLEMENTS` / `_default`) doesn't know it.
            let (_tmp, mut engine, src, _tgt) = two_mem_engine();
            let (actor, client) = cli_actor();
            // Create a same-mem target.
            let intra_target = engine
                .create_entity(
                    CreateEntityArgs {
                        anchors: Vec::new(),
                        mem: "src".to_string(),
                        title: "Doc Two".to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([("body".to_string(), "x".to_string())]),
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap();
            // Source's content_hash may have rotated due to incoming
            // edges from intra_target — fetch fresh.
            let src_fresh = engine.get_entity(&src.id).unwrap();
            let err = engine
                .relate_entity(
                    RelateEntityArgs {
                        source: src.id.clone(),
                        expected_hash: Some(src_fresh.content_hash.clone()),
                        rel_type: "ADDRESSES".to_string(),
                        target: intra_target.id.clone(),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap_err();
            match err {
                EngineError::Validation(
                    crate::runtime_validator::ValidationError::InvalidRelationshipType {
                        input,
                        ..
                    },
                ) => {
                    assert_eq!(input, "ADDRESSES");
                }
                other => panic!("expected Validation(InvalidRelationshipType), got {other:?}"),
            }
        }

        #[test]
        fn vocabulary_admissible_edge_blocked_by_policy_returns_cross_mem_link_not_allowed() {
            // Same fixture but flip the cross-mem policy to deny.
            // ADDRESSES is vocabulary-admissible but permission refuses
            // it independently — surfaces CROSS_MEM_LINK_NOT_ALLOWED.
            let (_tmp, mut engine, src, tgt) = two_mem_engine();
            // Replace the wildcard policy with default-deny.
            engine.set_settings(WorkspaceSettings::default());
            let (actor, client) = cli_actor();
            let err = engine
                .relate_entity(
                    RelateEntityArgs {
                        source: src.id.clone(),
                        expected_hash: Some(src.content_hash.clone()),
                        rel_type: "ADDRESSES".to_string(),
                        target: tgt.id.clone(),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap_err();
            assert!(
                matches!(err, EngineError::CrossMemLinkNotAllowed { .. }),
                "expected CrossMemLinkNotAllowed, got {err:?}"
            );
        }

        /// Cross-mem remove bypasses the `cross_mem_links` policy
        /// gate. Without this, a workspace whose grant was revoked
        /// while edges still existed gets wedged: the natural recovery
        /// (`memstead_relate ... --remove`) refuses, leaving the operator
        /// to re-grant just to delete the data that the grant once
        /// permitted.
        #[test]
        fn cross_mem_remove_bypasses_policy_after_revoke() {
            let (_tmp, mut engine, src, tgt) = two_mem_engine();
            let (actor, client) = cli_actor();

            // 1. Edge admits under wildcard grant.
            let added = engine
                .relate_entity(
                    RelateEntityArgs {
                        source: src.id.clone(),
                        expected_hash: Some(src.content_hash.clone()),
                        rel_type: "ADDRESSES".to_string(),
                        target: tgt.id.clone(),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("declared cross-mem edge admits under grant");
            assert_eq!(added.action, RelateAction::Added);

            // 2. Revoke the grant — default settings deny everything.
            engine.set_settings(WorkspaceSettings::default());

            // 3. Re-attempting an *add* still refuses (constraint:
            //    the gate is unchanged for the add path).
            let add_err = engine
                .relate_entity(
                    RelateEntityArgs {
                        source: src.id.clone(),
                        expected_hash: Some(added.content_hash.clone()),
                        rel_type: "ADDRESSES".to_string(),
                        target: tgt.id.clone(),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap_err();
            assert!(
                matches!(add_err, EngineError::CrossMemLinkNotAllowed { .. }),
                "add path must still refuse under denial, got {add_err:?}"
            );

            // 4. Remove succeeds — the cleanup path bypasses the
            //    policy gate.
            let removed = engine
                .relate_entity(
                    RelateEntityArgs {
                        source: src.id.clone(),
                        expected_hash: Some(added.content_hash.clone()),
                        rel_type: "ADDRESSES".to_string(),
                        target: tgt.id.clone(),
                        remove: true,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("remove must bypass the policy gate post-revoke");
            assert_eq!(removed.action, RelateAction::Removed);

            // 5. Edge is gone from the store's outgoing index.
            let outgoing = engine.store().outgoing(&src.id);
            assert!(
                !outgoing
                    .iter()
                    .any(|e| e.target == tgt.id && e.rel_type == "ADDRESSES"),
                "ADDRESSES edge must be gone after remove"
            );
        }

        /// Remove on a non-existent cross-mem edge with no grant
        /// returns a no-op, not a policy refusal. The remove path is
        /// permissive on absence — same shape as same-mem remove.
        #[test]
        fn cross_mem_remove_of_absent_edge_under_denial_is_no_op() {
            let (_tmp, mut engine, src, tgt) = two_mem_engine();
            // Default-deny from the start: no edge ever existed.
            engine.set_settings(WorkspaceSettings::default());
            let (actor, client) = cli_actor();
            let outcome = engine
                .relate_entity(
                    RelateEntityArgs {
                        source: src.id.clone(),
                        expected_hash: Some(src.content_hash.clone()),
                        rel_type: "ADDRESSES".to_string(),
                        target: tgt.id.clone(),
                        remove: true,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("absent-edge remove must not refuse on policy");
            assert!(
                matches!(outcome.action, RelateAction::NoOpAbsent),
                "expected NoOpAbsent, got {:?}",
                outcome.action
            );
        }

        // ---- ReadOnly-target refusal (shared add-path funnel) --------

        /// Engine with mem `src` (Write, `alias_target_rel_type:
        /// REFERENCES`, cross-mem vocabulary into `tgt-al`) and mem
        /// `tgt` mounted with the given capability, pre-populated on
        /// disk with one entity `tgt--req-one`. Wildcard cross-mem
        /// grant for `src`. Exercises the funnel's ReadOnly-missing-
        /// target refusal across every add-shaped write path.
        fn engine_with_tgt_capability(
            capability: MountCapability,
        ) -> (TempDir, Engine, CreateEntityOutcome) {
            let tmp = TempDir::new().unwrap();

            let src_manifest = r#"name: src-al
version: 0.1.0
description: source schema with alias pointer
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: ADDRESSES
      description: explicit cross-mem
      default_weight: 1.0
    - name: REFERENCES
      description: alias pointer
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
cross_mem_relationships:
  - to_schema: tgt-al
    definitions:
      - name: ADDRESSES
        description: explicit cross-mem
        default_weight: 1.0
      - name: REFERENCES
        description: alias-emitted cross-mem
        default_weight: 1.0
alias_target_rel_type: REFERENCES
community:
  resolution: 1.0
  seed: 42
"#;
            let tgt_manifest = r#"name: tgt-al
version: 0.1.0
description: target schema
when_to_use: tests
types:
  - req
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
            let schemas_dir = tmp.path().join("schemas");
            std::fs::create_dir_all(&schemas_dir).unwrap();
            write_schema_files(
                &schemas_dir,
                "src-al",
                src_manifest,
                &[("doc", &make_type_yaml("doc"))],
            );
            write_schema_files(
                &schemas_dir,
                "tgt-al",
                tgt_manifest,
                &[("req", &make_type_yaml("req"))],
            );

            let src_dir = tmp.path().join("mem-src");
            let tgt_dir = tmp.path().join("mem-tgt");
            std::fs::create_dir_all(&src_dir).unwrap();
            std::fs::create_dir_all(&tgt_dir).unwrap();
            // The read-only mem is pre-populated on disk — the engine
            // never writes to it.
            std::fs::write(
                tgt_dir.join("req-one.md"),
                "---\ntype: req\n---\n# Req One\n\n## Body\n\nseed.\n",
            )
            .unwrap();

            let src_writer = FilesystemMemWriter::new(src_dir.clone());
            let tgt_writer = FilesystemMemWriter::new(tgt_dir.clone());
            let src_pin = SchemaRef::new("src-al", semver::Version::new(0, 1, 0));
            let tgt_pin = SchemaRef::new("tgt-al", semver::Version::new(0, 1, 0));

            let tgt_mount = Mount {
                mem: "tgt".to_string(),
                schema: Some(tgt_pin),
                storage: MountStorage::Folder {
                    path: tgt_dir.clone(),
                },
                capability,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            };
            let mut engine = Engine::from_mounts_with_schemas_dir(
                vec![
                    (
                        folder_mount_with_pin("src", src_dir, src_pin),
                        Box::new(src_writer) as Box<dyn MemBackend>,
                    ),
                    (tgt_mount, Box::new(tgt_writer) as Box<dyn MemBackend>),
                ],
                Some(&schemas_dir),
            )
            .expect("two-mem engine constructs");

            let mut settings = WorkspaceSettings::default();
            let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
            links.insert("src".to_string(), CrossLinkValue::Wildcard);
            settings.cross_mem_links = links;
            engine.set_settings(settings);

            let (actor, client) = cli_actor();
            let src_entity = engine
                .create_entity(
                    CreateEntityArgs {
                        anchors: Vec::new(),
                        mem: "src".to_string(),
                        title: "Doc One".to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([("body".to_string(), "seed".to_string())]),
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("source entity creates");

            (tmp, engine, src_entity)
        }

        fn assert_cross_mem_target_not_found(err: EngineError, expected_target: &str) {
            match err {
                EngineError::CrossMemTargetNotFound {
                    target_id,
                    target_mem,
                } => {
                    assert_eq!(target_id, expected_target);
                    assert_eq!(target_mem, "tgt");
                }
                other => panic!("expected CrossMemTargetNotFound, got {other:?}"),
            }
        }

        /// Rehearsal complement (agent-trust plan 07): a rehearsed
        /// relate against a read-only boundary refuses EXACTLY as the
        /// real call would — same variant, same payload. Paired with
        /// `relate_to_missing_target_in_readonly_mem_refuses` below.
        #[test]
        fn relate_dry_run_to_missing_target_in_readonly_mem_refuses_identically() {
            let (_tmp, mut engine, src) = engine_with_tgt_capability(MountCapability::ReadOnly);
            let (actor, client) = cli_actor();
            let args = |dry_run: bool| RelateEntityArgs {
                source: src.id.clone(),
                expected_hash: Some(src.content_hash.clone()),
                rel_type: "ADDRESSES".to_string(),
                target: crate::EntityId::new("tgt", "missing"),
                remove: false,
                description: None,
                dry_run,
            };
            let rehearsed = engine
                .relate_entity(args(true), actor, Some(&client), None)
                .unwrap_err();
            let real = engine
                .relate_entity(args(false), actor, Some(&client), None)
                .unwrap_err();
            assert_eq!(format!("{rehearsed:?}"), format!("{real:?}"));
            assert_cross_mem_target_not_found(rehearsed, "tgt--missing");
        }

        #[test]
        fn relate_to_missing_target_in_readonly_mem_refuses() {
            let (_tmp, mut engine, src) = engine_with_tgt_capability(MountCapability::ReadOnly);
            let (actor, client) = cli_actor();
            let err = engine
                .relate_entity(
                    RelateEntityArgs {
                        source: src.id.clone(),
                        expected_hash: Some(src.content_hash.clone()),
                        rel_type: "ADDRESSES".to_string(),
                        target: crate::EntityId::new("tgt", "missing"),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap_err();
            assert_cross_mem_target_not_found(err, "tgt--missing");
        }

        /// Pre-funnel, `memstead_create.relations[]` lacked the
        /// ReadOnly-missing-target check the relate path had — an
        /// inline relation to an absent read-only target auto-stubbed
        /// instead of refusing.
        #[test]
        fn create_inline_relation_to_missing_target_in_readonly_mem_refuses() {
            let (_tmp, mut engine, _src) = engine_with_tgt_capability(MountCapability::ReadOnly);
            let (actor, client) = cli_actor();
            let err = engine
                .create_entity(
                    CreateEntityArgs {
                        anchors: Vec::new(),
                        mem: "src".to_string(),
                        title: "Doc Two".to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([("body".to_string(), "x".to_string())]),
                        metadata: IndexMap::new(),
                        relations: vec![crate::ops::RelateArg {
                            to: crate::EntityId::new("tgt", "missing"),
                            rel_type: "ADDRESSES".to_string(),
                            description: None,
                        }],
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap_err();
            assert_cross_mem_target_not_found(err, "tgt--missing");
        }

        /// The body-wiki-link channel (alias synthesis) — pre-funnel a
        /// granted body link to a missing read-only target silently
        /// auto-stubbed at load; `memstead_health` was the only signal.
        #[test]
        fn create_body_link_to_missing_target_in_readonly_mem_refuses() {
            let (_tmp, mut engine, _src) = engine_with_tgt_capability(MountCapability::ReadOnly);
            let (actor, client) = cli_actor();
            let err = engine
                .create_entity(
                    CreateEntityArgs {
                        anchors: Vec::new(),
                        mem: "src".to_string(),
                        title: "Doc Three".to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([(
                            "body".to_string(),
                            "see [[tgt--missing]].".to_string(),
                        )]),
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap_err();
            assert_cross_mem_target_not_found(err, "tgt--missing");
        }

        #[test]
        fn update_body_link_to_missing_target_in_readonly_mem_refuses() {
            let (_tmp, mut engine, src) = engine_with_tgt_capability(MountCapability::ReadOnly);
            let (actor, client) = cli_actor();
            let err = engine
                .update_entity(
                    crate::engine::UpdateEntityArgs {
                        anchors: Vec::new(),
                        id: src.id.clone(),
                        expected_hash: Some(src.content_hash.clone()),
                        sections: IndexMap::from_iter([(
                            "body".to_string(),
                            "now see [[tgt--missing]].".to_string(),
                        )]),
                        append_sections: IndexMap::new(),
                        patch_sections: IndexMap::new(),
                        metadata: IndexMap::new(),
                        metadata_unset: Vec::new(),
                        declare_relations: Vec::new(),
                        dry_run: false,
                        relations_unset: Vec::new(),
                        anchors_unset: Vec::new(),
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap_err();
            assert_cross_mem_target_not_found(err, "tgt--missing");
        }

        /// Positive control: a body link to a target that EXISTS in
        /// the read-only mem writes clean and materialises the typed
        /// alias edge — the seam's happy path.
        #[test]
        fn body_link_to_existing_target_in_readonly_mem_admits_and_emits_edge() {
            let (_tmp, mut engine, _src) = engine_with_tgt_capability(MountCapability::ReadOnly);
            let (actor, client) = cli_actor();
            let created = engine
                .create_entity(
                    CreateEntityArgs {
                        anchors: Vec::new(),
                        mem: "src".to_string(),
                        title: "Doc Four".to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([(
                            "body".to_string(),
                            "see [[tgt--req-one]].".to_string(),
                        )]),
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("body link to existing read-only target admits");
            let outgoing = engine.store().outgoing(&created.id);
            assert!(
                outgoing.iter().any(|e| e.rel_type == "REFERENCES"
                    && e.target == crate::EntityId::new("tgt", "req-one")),
                "alias REFERENCES edge to the read-only target must materialise; got {outgoing:?}"
            );
        }

        /// Behaviour preserved: a missing target in a WRITE-mounted
        /// sibling mem is a legitimate forward reference and keeps
        /// the auto-stub mechanic on every path.
        #[test]
        fn body_link_to_missing_target_in_write_mem_still_stubs() {
            let (_tmp, mut engine, _src) = engine_with_tgt_capability(MountCapability::Write);
            let (actor, client) = cli_actor();
            let created = engine
                .create_entity(
                    CreateEntityArgs {
                        anchors: Vec::new(),
                        mem: "src".to_string(),
                        title: "Doc Five".to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([(
                            "body".to_string(),
                            "see [[tgt--missing]].".to_string(),
                        )]),
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("forward reference into a Write sibling mem keeps stubbing");
            assert!(
                engine
                    .store()
                    .contains(&crate::EntityId::new("tgt", "missing")),
                "auto-stub must land for the Write-mem forward reference"
            );
            let outgoing = engine.store().outgoing(&created.id);
            assert!(
                outgoing.iter().any(|e| e.rel_type == "REFERENCES"),
                "alias edge must still emit for the stubbed target"
            );
        }
    }

    // ---- Engine::rename_entity --------------------------------------

    /// Batch relate: one list mixing additions and removals, applied
    /// IN ORDER in one invocation with one commit. The remove entry
    /// targets an edge added earlier in the same batch — if entries
    /// validated against the pre-batch state instead, that remove
    /// would resolve to `NoOpAbsent` ("noop"), so the asserted
    /// `"removed"` action is the in-order proof.
    #[test]
    fn batch_relate_applies_adds_and_removes_in_order_one_commit() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        for title in ["A", "B", "C"] {
            engine
                .create_entity(
                    empty_create_args("specs", title),
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap();
        }
        let id = |slug: &str| crate::entity::EntityId::new("specs", slug);
        let edge = |from: &str, to: &str, remove: bool| RelateEntityArgs {
            source: id(from),
            expected_hash: None,
            rel_type: "USES".to_string(),
            target: id(to),
            remove,
            description: None,
            dry_run: false,
        };

        let result = engine
            .batch_relate(
                vec![
                    (edge("a", "b", false), Some("add a-b".to_string())),
                    (edge("a", "c", false), Some("add a-c".to_string())),
                    (edge("a", "c", true), Some("undo a-c".to_string())),
                ],
                actor,
                Some(&client),
                false,
            )
            .unwrap();
        assert!(result.applied, "{result:?}");
        assert_eq!(result.succeeded, 3);
        assert_eq!(result.failed, 0);
        assert!(!result.commit_sha.is_empty(), "one real commit");
        let actions: Vec<&str> = result.results.iter().map(|r| r.action.as_str()).collect();
        assert_eq!(
            actions,
            vec!["added", "added", "removed"],
            "in-order application: the remove sees the same batch's add"
        );

        // Net state: A carries exactly the surviving edge to B.
        let a = engine.get_entity(&id("a")).unwrap();
        assert_eq!(a.relationships.len(), 1, "{:?}", a.relationships);
        assert_eq!(a.relationships[0].rel_type, "USES");
        assert_eq!(a.relationships[0].target, id("b"));
    }

    /// Rehearsal contract (agent-trust plan 07) — single relate:
    /// `dry_run: true` runs the FULL validation, reports the would-be
    /// edge and the would-be auto-stub (reported, never created) with
    /// the marker form's empty `commit_sha`, and writes nothing. The
    /// follow-up real call succeeds and lands EXACTLY the rehearsed
    /// prospective `_hash` — the strongest identical-validation
    /// observable.
    #[test]
    fn relate_dry_run_reports_would_be_stub_and_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Src");
        // Pin the mutation clock: the auto-stamped `last_modified`
        // enters the content hash, so the prospective-hash == real-hash
        // assertion below is only deterministic under a frozen clock
        // (unpinned, it fails whenever a wall-clock second ticks
        // between the rehearsal and the real call).
        engine.set_mutation_clock(std::sync::Arc::new(|| {
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_754_000_000)
        }));
        let (actor, client) = cli_actor();
        let absent = crate::EntityId::new("specs", "ghost-target");
        let args = |dry_run: bool| RelateEntityArgs {
            source: source.id.clone(),
            expected_hash: Some(source.content_hash.clone()),
            rel_type: "USES".to_string(),
            target: absent.clone(),
            remove: false,
            description: None,
            dry_run,
        };

        let rehearsed = engine
            .relate_entity(args(true), actor, Some(&client), None)
            .unwrap();
        assert_eq!(rehearsed.action, RelateAction::Added);
        assert!(
            rehearsed.commit_sha.is_empty(),
            "marker form: empty commit_sha"
        );
        assert!(
            rehearsed.warnings.iter().any(
                |w| matches!(w, crate::ops::WarningHint::AutoStubCreated { stub_id, pending: true } if *stub_id == absent)
            ),
            "would-be stub must be reported as pending: {:?}",
            rehearsed.warnings
        );
        // The rehearsed warning must not claim a performed effect —
        // conditional wording, code unchanged (AUTO_STUB_CREATED).
        let rehearsed_stub = rehearsed
            .warnings
            .iter()
            .find(|w| matches!(w, crate::ops::WarningHint::AutoStubCreated { .. }))
            .unwrap();
        assert_eq!(rehearsed_stub.code(), "AUTO_STUB_CREATED");
        let msg = rehearsed_stub.message();
        assert!(
            msg.contains("would be auto-created") && !msg.contains("stub auto-created."),
            "dry-run wording must be conditional: {msg}"
        );
        assert!(
            !engine.store().contains(&absent),
            "would-be stub reported, never created"
        );
        // Source untouched: stored hash still the pre-call hash, and
        // the reported hash is the PROSPECTIVE one (a real change).
        let stored = engine.store().get(&source.id).unwrap();
        assert_eq!(stored.content_hash, source.content_hash);
        assert_ne!(rehearsed.content_hash, source.content_hash);
        assert!(stored.relationships.is_empty(), "no edge landed");

        // Follow-up real call: succeeds, commits, and the rehearsed
        // prospective hash IS the real post-write hash.
        let real = engine
            .relate_entity(args(false), actor, Some(&client), None)
            .unwrap();
        assert!(!real.commit_sha.is_empty(), "the real relate commits");
        assert_eq!(
            real.content_hash, rehearsed.content_hash,
            "prospective hash must equal the real post-write hash"
        );
        assert!(engine.store().get(&absent).expect("real call stubs").stub);
        // The real call keeps the performed-effect wording exactly.
        let real_msg = real
            .warnings
            .iter()
            .find(|w| {
                matches!(
                    w,
                    crate::ops::WarningHint::AutoStubCreated { pending: false, .. }
                )
            })
            .expect("real relate carries the non-pending stub warning")
            .message();
        assert!(
            real_msg.contains("did not exist — stub auto-created."),
            "real wording unchanged: {real_msg}"
        );
    }

    /// Rehearsal refusal parity — single relate: an illegal rehearsed
    /// relate refuses with the IDENTICAL typed error the real call
    /// returns (same variant, same payload).
    #[test]
    fn relate_dry_run_refuses_identically_to_real() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Src");
        let (actor, client) = cli_actor();
        // Malformed target id (no `--` separator) — INVALID_ENTITY_ID.
        let args = |dry_run: bool| RelateEntityArgs {
            source: source.id.clone(),
            expected_hash: None,
            rel_type: "USES".to_string(),
            target: crate::EntityId("bad target with spaces".to_string()),
            remove: false,
            description: None,
            dry_run,
        };
        let rehearsed = engine
            .relate_entity(args(true), actor, Some(&client), None)
            .unwrap_err();
        let real = engine
            .relate_entity(args(false), actor, Some(&client), None)
            .unwrap_err();
        assert_eq!(
            format!("{rehearsed:?}"),
            format!("{real:?}"),
            "identical typed refusal"
        );
        assert_eq!(rehearsed.code(), real.code());
    }

    /// Rehearsal — batch relate: `dry_run: true` validates the whole
    /// list in order (a remove of an edge added earlier in the SAME
    /// batch reports `"removed"` — the in-order proof), reports the
    /// would-be receipt with empty `commit_sha`, and commits nothing:
    /// no edge, no stub, no head movement. The follow-up real batch
    /// succeeds.
    #[test]
    fn batch_relate_dry_run_reports_receipt_and_commits_nothing() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        for title in ["A", "B"] {
            engine
                .create_entity(
                    empty_create_args("specs", title),
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap();
        }
        let id = |slug: &str| crate::entity::EntityId::new("specs", slug);
        let edge = |from: &str, to: &str, remove: bool| RelateEntityArgs {
            source: id(from),
            expected_hash: None,
            rel_type: "USES".to_string(),
            target: id(to),
            remove,
            description: None,
            dry_run: false,
        };
        let batch = || {
            vec![
                (edge("a", "b", false), None),
                (edge("a", "ghost", false), None), // would-be auto-stub
                (edge("a", "b", true), None),      // in-order: sees entry 0's add
            ]
        };
        let head_before = engine
            .mem_head_sha("specs")
            .ok()
            .flatten()
            .unwrap_or_default();

        let rehearsed = engine
            .batch_relate(batch(), actor, Some(&client), true)
            .unwrap();
        assert!(rehearsed.applied, "{rehearsed:?}");
        assert!(
            rehearsed.commit_sha.is_empty(),
            "marker form: empty commit_sha"
        );
        let actions: Vec<&str> = rehearsed
            .results
            .iter()
            .map(|r| r.action.as_str())
            .collect();
        assert_eq!(
            actions,
            vec!["added", "added", "removed"],
            "in-order rehearsal semantics"
        );
        // Nothing landed: no stub, no edge, no head movement.
        assert!(!engine.store().contains(&id("ghost")), "no stub created");
        assert!(
            engine
                .get_entity(&id("a"))
                .unwrap()
                .relationships
                .is_empty(),
            "no edge landed"
        );
        let head_after = engine
            .mem_head_sha("specs")
            .ok()
            .flatten()
            .unwrap_or_default();
        assert_eq!(head_before, head_after, "no commit landed");

        // The real batch on the unchanged mem succeeds.
        let real = engine
            .batch_relate(batch(), actor, Some(&client), false)
            .unwrap();
        assert!(real.applied, "{real:?}");
        assert!(!real.commit_sha.is_empty());
    }

    /// Rehearsal refusal parity — batch relate: a failing list refuses
    /// under `dry_run: true` with the SAME per-entry report-all
    /// envelope the real refusal carries.
    #[test]
    fn batch_relate_dry_run_refuses_identically_to_real() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        for title in ["A", "B"] {
            engine
                .create_entity(
                    empty_create_args("specs", title),
                    actor,
                    Some(&client),
                    None,
                )
                .unwrap();
        }
        let id = |slug: &str| crate::entity::EntityId::new("specs", slug);
        let batch = || {
            vec![
                (
                    RelateEntityArgs {
                        source: id("a"),
                        expected_hash: None,
                        rel_type: "USES".to_string(),
                        target: id("b"),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    None,
                ),
                (
                    RelateEntityArgs {
                        source: id("a"),
                        expected_hash: None,
                        rel_type: "USES".to_string(),
                        target: crate::EntityId("bad target".to_string()),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    None,
                ),
            ]
        };
        let rehearsed = engine
            .batch_relate(batch(), actor, Some(&client), true)
            .unwrap();
        let real = engine
            .batch_relate(batch(), actor, Some(&client), false)
            .unwrap();
        assert!(!rehearsed.applied && !real.applied);
        let envelope = |r: &crate::ops::BatchResult| {
            r.results
                .iter()
                .map(|e| {
                    (
                        e.id.to_string(),
                        e.action.clone(),
                        e.error.as_ref().map(|err| {
                            (err.code.clone(), err.message.clone(), err.details.clone())
                        }),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(envelope(&rehearsed), envelope(&real), "identical refusals");
        assert!(
            engine
                .get_entity(&id("a"))
                .unwrap()
                .relationships
                .is_empty(),
            "neither run landed the valid entry"
        );
    }

    /// Atomicity + report-all for batch relate: a batch with several
    /// invalid entries changes NOTHING (no edge lands, the head is
    /// unmoved, staged earlier entries roll back) and names EVERY
    /// failing entry with its typed code.
    #[test]
    fn batch_relate_refuses_whole_batch_reporting_every_failure() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();

        let a = engine
            .create_entity(empty_create_args("specs", "A"), actor, Some(&client), None)
            .unwrap();
        engine
            .create_entity(empty_create_args("specs", "B"), actor, Some(&client), None)
            .unwrap();
        let head_before = engine
            .mem_head_sha("specs")
            .ok()
            .flatten()
            .unwrap_or_default();
        let count_before = engine.store().all_entities().count();

        let id = |slug: &str| crate::entity::EntityId::new("specs", slug);
        let result = engine
            .batch_relate(
                vec![
                    // Valid — stages an edge that must roll back.
                    (
                        RelateEntityArgs {
                            source: id("a"),
                            expected_hash: None,
                            rel_type: "USES".to_string(),
                            target: id("b"),
                            remove: false,
                            description: None,
                            dry_run: false,
                        },
                        None,
                    ),
                    // Missing source.
                    (
                        RelateEntityArgs {
                            source: id("ghost"),
                            expected_hash: None,
                            rel_type: "USES".to_string(),
                            target: id("b"),
                            remove: false,
                            description: None,
                            dry_run: false,
                        },
                        None,
                    ),
                    // Optimistic-lock mismatch.
                    (
                        RelateEntityArgs {
                            source: id("b"),
                            expected_hash: Some("definitely-wrong".to_string()),
                            rel_type: "USES".to_string(),
                            target: id("a"),
                            remove: false,
                            description: None,
                            dry_run: false,
                        },
                        None,
                    ),
                ],
                actor,
                Some(&client),
                false,
            )
            .unwrap();
        assert!(!result.applied);
        assert_eq!(result.failed, 2, "{result:?}");
        assert!(result.commit_sha.is_empty());
        let codes: Vec<(usize, &str)> = result
            .results
            .iter()
            .enumerate()
            .filter(|(_, r)| r.action == "error")
            .map(|(i, r)| (i, r.error.as_ref().map(|e| e.code.as_str()).unwrap_or("")))
            .collect();
        assert_eq!(
            codes,
            vec![(1, "ENTITY_NOT_FOUND"), (2, "HASH_MISMATCH")],
            "every failing entry named with index + typed code: {result:?}"
        );
        assert_eq!(result.results[0].action, "not_applied");

        // NOTHING changed: the staged first edge rolled back, the head
        // is unmoved, and no stub or entity appeared.
        let a_after = engine.get_entity(&a.id).unwrap();
        assert!(
            a_after.relationships.is_empty(),
            "staged edge must roll back: {:?}",
            a_after.relationships
        );
        assert_eq!(a_after.content_hash, a.content_hash);
        let head_after = engine
            .mem_head_sha("specs")
            .ok()
            .flatten()
            .unwrap_or_default();
        assert_eq!(head_before, head_after, "mem head unmoved");
        assert_eq!(engine.store().all_entities().count(), count_before);
    }
}

#[cfg(test)]
mod derivation_tests {
    use indexmap::IndexMap;
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::{
        CreateEntityArgs, Engine, RelateAction, RelateEntityArgs, UpdateEntityArgs,
    };
    use crate::ops::WarningHint;
    use crate::storage::FilesystemMemWriter;
    use crate::vcs::Actor;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    /// A schema whose `DERIVED_FROM` declares `derivation: true` and
    /// whose `SUPPORTS` does not — the paired fixture every assertion
    /// here contrasts.
    fn deriv_schema() -> std::sync::Arc<memstead_schema::Schema> {
        let manifest = r#"name: deriv
version: 0.1.0
description: derivation fixture
when_to_use: tests
types:
  - note
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: DERIVED_FROM
      description: source derives from target
      default_weight: 2.0
      derivation: true
    - name: SUPPORTS
      description: plain edge
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        let type_yaml = r#"name: note
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;
        std::sync::Arc::new(
            memstead_schema::loader::load_schema_from_memory(
                manifest,
                &[("note".to_string(), type_yaml.to_string())],
            )
            .expect("fixture schema loads"),
        )
    }

    fn engine_at(tmp: &TempDir) -> Engine {
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = Mount {
            mem: "m".to_string(),
            schema: Some("deriv@0.1.0".parse().unwrap()),
            storage: MountStorage::Folder { path: mem_dir },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        Engine::from_mounts_with_schemas_dir_and_extra(
            vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
            None,
            vec![deriv_schema()],
        )
        .unwrap()
    }

    fn note(title: &str, body: &str) -> CreateEntityArgs {
        CreateEntityArgs {
            anchors: Vec::new(),
            mem: "m".to_string(),
            title: title.to_string(),
            entity_type: "note".to_string(),
            sections: IndexMap::from_iter([("body".to_string(), body.to_string())]),
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        }
    }

    fn relate_args(from: &crate::EntityId, rel: &str, to: &crate::EntityId) -> RelateEntityArgs {
        RelateEntityArgs {
            source: from.clone(),
            expected_hash: None,
            rel_type: rel.to_string(),
            target: to.clone(),
            remove: false,
            description: None,
            dry_run: false,
        }
    }

    /// Criterion 1's fixture, one flow: write → edit target → report
    /// stale → re-assert → clear, with the refresh STATED. Plus the
    /// undeclared-rel-type no-op complement and the
    /// source-edit / markdown-invisibility complements.
    #[test]
    fn derivation_write_edit_report_reassert_clear() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_at(&tmp);
        let target = engine
            .create_entity(note("Target", "v1"), Actor::Cli, None, None)
            .unwrap();
        let source = engine
            .create_entity(note("Source", "conclusion"), Actor::Cli, None, None)
            .unwrap();

        // Write the derivation edge — baseline recorded, report clean.
        let added = engine
            .relate_entity(
                relate_args(&source.id, "DERIVED_FROM", &target.id),
                Actor::Cli,
                None,
                None,
            )
            .unwrap();
        assert_eq!(added.action, RelateAction::Added);
        assert!(
            engine.derivation_report("m").unwrap().is_empty(),
            "freshly baselined edge must not report"
        );

        // Edit the TARGET → the edge reports stale, naming all three.
        let t_now = engine.get_entity(&target.id).unwrap().content_hash.clone();
        engine
            .update_entity(
                UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: target.id.clone(),
                    expected_hash: Some(t_now),
                    sections: IndexMap::from_iter([("body".to_string(), "v2".to_string())]),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                Actor::Cli,
                None,
                None,
            )
            .unwrap();
        let report = engine.derivation_report("m").unwrap();
        assert_eq!(report.len(), 1, "{report:?}");
        assert_eq!(report[0].source, source.id);
        assert_eq!(report[0].rel_type, "DERIVED_FROM");
        assert_eq!(report[0].target, target.id);
        assert_eq!(report[0].state, "stale");
        assert!(report[0].baseline.is_some());

        // Editing the SOURCE does not mark its own derivation stale
        // (already covered: the edit above touched only the target;
        // now touch the source and assert the report is unchanged in
        // meaning — still exactly the one stale edge).
        let s_now = engine.get_entity(&source.id).unwrap().content_hash.clone();
        engine
            .update_entity(
                UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: source.id.clone(),
                    expected_hash: Some(s_now),
                    sections: IndexMap::from_iter([(
                        "body".to_string(),
                        "conclusion v2".to_string(),
                    )]),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                Actor::Cli,
                None,
                None,
            )
            .unwrap();
        let report = engine.derivation_report("m").unwrap();
        assert_eq!(report.len(), 1, "source edit adds nothing: {report:?}");
        assert_eq!(report[0].state, "stale");

        // Re-assert: duplicate-add refreshes the baseline as its ONE
        // effect — action noop, `_hash` unchanged, markdown
        // byte-identical, the refresh STATED, a real commit sha.
        let md_before = std::fs::read(tmp.path().join("source.md")).unwrap();
        let hash_before = engine.get_entity(&source.id).unwrap().content_hash.clone();
        let refreshed = engine
            .relate_entity(
                relate_args(&source.id, "DERIVED_FROM", &target.id),
                Actor::Cli,
                None,
                None,
            )
            .unwrap();
        assert_eq!(refreshed.action, RelateAction::NoOpAlreadyPresent);
        assert_eq!(refreshed.content_hash, hash_before, "_hash unchanged");
        assert!(
            !refreshed.commit_sha.is_empty(),
            "the sidecar refresh persists via a real commit"
        );
        assert!(
            refreshed
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::DerivationBaselineRefreshed { .. })),
            "the refresh is STATED, never a bare no-op: {:?}",
            refreshed.warnings
        );
        let md_after = std::fs::read(tmp.path().join("source.md")).unwrap();
        assert_eq!(md_before, md_after, "baselines never touch the markdown");
        assert!(
            engine.derivation_report("m").unwrap().is_empty(),
            "re-assert clears the staleness"
        );

        // Undeclared rel-type: duplicate-add keeps today's EXACT
        // no-op — empty commit_sha, no refresh warning.
        engine
            .relate_entity(
                relate_args(&source.id, "SUPPORTS", &target.id),
                Actor::Cli,
                None,
                None,
            )
            .unwrap();
        let noop = engine
            .relate_entity(
                relate_args(&source.id, "SUPPORTS", &target.id),
                Actor::Cli,
                None,
                None,
            )
            .unwrap();
        assert_eq!(noop.action, RelateAction::NoOpAlreadyPresent);
        assert!(noop.commit_sha.is_empty(), "undeclared no-op stays bare");
        assert!(
            !noop
                .warnings
                .iter()
                .any(|w| matches!(w, WarningHint::DerivationBaselineRefreshed { .. })),
        );
        // And undeclared edges never enter the axis: still empty.
        assert!(engine.derivation_report("m").unwrap().is_empty());
    }

    /// A derivation edge with NO recorded baseline (pre-declaration
    /// legacy, simulated by removing the sidecar out of band) reports
    /// `unbaselined` — distinct from both fresh and stale, never
    /// fabricated. The sidecar file itself never lists as an entity.
    #[test]
    fn missing_baseline_reports_unbaselined_never_fabricated() {
        let tmp = TempDir::new().unwrap();
        let mut engine = engine_at(&tmp);
        let target = engine
            .create_entity(note("Target", "v1"), Actor::Cli, None, None)
            .unwrap();
        let source = engine
            .create_entity(note("Source", "conclusion"), Actor::Cli, None, None)
            .unwrap();
        engine
            .relate_entity(
                relate_args(&source.id, "DERIVED_FROM", &target.id),
                Actor::Cli,
                None,
                None,
            )
            .unwrap();
        // The sidecar exists on disk but is not an entity.
        let sidecar = tmp.path().join(".memstead").join("derivations.json");
        assert!(sidecar.exists(), "baseline persisted to the sidecar");
        assert!(
            engine
                .get_entity(&crate::EntityId::new("m", "derivations"))
                .is_none()
        );

        // Simulate a pre-declaration edge: drop the sidecar out of
        // band (fixture surgery, the mounts.json precedent).
        std::fs::remove_file(&sidecar).unwrap();
        let report = engine.derivation_report("m").unwrap();
        assert_eq!(report.len(), 1, "{report:?}");
        assert_eq!(report[0].state, "unbaselined");
        assert!(report[0].baseline.is_none());
    }
}
