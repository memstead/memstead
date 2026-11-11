use super::*;

impl Engine {
    /// Compose phase: the source entity with the edge added or removed,
    /// the remove-time block-tier gates, the planned forward-reference
    /// stub, and the rendered file; an idempotent re-add or an absent
    /// remove ends here as a no-op outcome.
    pub(super) fn compose_relate(
        &self,
        validated: ValidatedRelate,
    ) -> Result<RelatePrepareOutcome, EngineError> {
        let ValidatedRelate { resolved, type_def } = validated;
        let ResolvedRelate {
            args,
            source_mem,
            target_mem,
            mount_idx,
            mut warnings,
            entity,
            target_verified_in_storage,
            target_mem_uncreated,
            ..
        } = resolved;

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
                // stub is only the until-load representation of
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
        crate::engine::mutation::auto_stamp_timestamps(&mut next, type_def.as_ref(), &today);

        let file_path = next.file_path.clone();
        let markdown = crate::engine::mutation::render_for_write(&next, type_def.as_ref())?;

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
}
