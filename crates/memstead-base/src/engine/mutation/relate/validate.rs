use super::*;

impl Engine {
    /// Validate phase: the rel-type's vocabulary, the description and
    /// manual-authoring postures, the edge's shape, the caller's hash and
    /// the acyclicity guard, and the source type's definition.
    pub(super) fn validate_relate(
        &self,
        resolved: ResolvedRelate,
    ) -> Result<ValidatedRelate, EngineError> {
        let ResolvedRelate {
            args,
            source_mem,
            target_mem,
            mount_idx,
            schema: schema_arc,
            target_schema_ref,
            cross_mem_different,
            mut warnings,
            entity,
            target_type,
            target_verified_in_storage,
            target_mem_uncreated,
        } = resolved;
        let schema = &schema_arc;
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
            crate::engine::mutation::validate_manual_authoring_posture(
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
            crate::engine::mutation::validate_edge_acyclicity(
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
        Ok(ValidatedRelate {
            resolved: ResolvedRelate {
                args,
                source_mem,
                target_mem,
                mount_idx,
                schema: schema_arc,
                target_schema_ref,
                cross_mem_different,
                warnings,
                entity,
                target_type,
                target_verified_in_storage,
                target_mem_uncreated,
            },
            type_def,
        })
    }
}
