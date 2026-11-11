use super::*;

impl Engine {
    /// Validate phase: the entity's sections, metadata, edges and the
    /// target type's block-tier constraints, every problem collected
    /// into one refusal before anything is written.
    pub(super) fn validate_retype(
        &self,
        resolved: ResolvedRetype,
    ) -> Result<ValidatedRetype, EngineError> {
        let mut problems: Vec<RetypeProblem> = Vec::new();
        let mut warnings: Vec<WarningHint> = Vec::new();
        let mut next = resolved.entity.clone();
        next.entity_type = resolved.args.target_type.clone();
        let sections_renamed = self.retype_sections(&resolved, &mut next, &mut problems);
        self.retype_metadata(&resolved, &mut next, &mut problems);
        let edges_rechecked = self.retype_edges(&resolved, &next, &mut problems)?;
        self.retype_constraints(&resolved, &next, &mut problems, &mut warnings);
        self.refuse_retype_problems(&resolved, problems)?;
        Ok(ValidatedRetype {
            resolved,
            next,
            sections_renamed,
            edges_rechecked,
            warnings,
        })
    }

    /// Sections: apply the map, then validate against the target.
    fn retype_sections(
        &self,
        resolved: &ResolvedRetype,
        next: &mut crate::entity::Entity,
        problems: &mut Vec<RetypeProblem>,
    ) -> Vec<(String, String)> {
        let (args, entity, target_def) = (&resolved.args, &resolved.entity, &resolved.target_def);
        let mut sections_renamed: Vec<(String, String)> = Vec::new();
        {
            let mut mapped: IndexMap<String, String> = IndexMap::new();
            let mut taken: BTreeMap<String, String> = BTreeMap::new(); // new key -> old key
            for (key, body) in &entity.sections {
                let new_key = args
                    .section_map
                    .get(key.as_str())
                    .cloned()
                    .unwrap_or_else(|| key.clone());
                if let Some(prev) = taken.get(&new_key) {
                    problems.push(RetypeProblem::SectionMapCollision {
                        from: key.clone(),
                        to: new_key.clone(),
                        also_from: prev.clone(),
                    });
                    continue;
                }
                taken.insert(new_key.clone(), key.clone());
                if new_key != *key {
                    sections_renamed.push((key.clone(), new_key.clone()));
                }
                mapped.insert(new_key, body.clone());
            }
            for from in args.section_map.keys() {
                if !entity.sections.contains_key(from.as_str()) {
                    problems.push(RetypeProblem::SectionMapSourceMissing {
                        key: from.clone(),
                        present: entity.sections.keys().cloned().collect(),
                    });
                }
            }
            next.sections = mapped;
        }
        // Every mapped key against the target's declared sections — one
        // problem per unknown key (the validator refuses on the first, so
        // it is asked one key at a time).
        for key in next.sections.keys() {
            if let Err(ValidationError::UnknownSection {
                key,
                declared,
                suggestion,
                ..
            }) = validate_section_keys(std::iter::once(key.as_str()), target_def.as_ref())
            {
                problems.push(RetypeProblem::UnknownSection {
                    key,
                    declared,
                    suggestion,
                });
            }
        }
        // Body content under the target's catch-all posture.
        {
            let declared_headings: Vec<&str> = target_def
                .sections
                .iter()
                .map(|s| s.heading.as_str())
                .collect();
            let catch_all = target_def.catch_all_section().map(|s| CatchAllContext {
                key: s.key.as_str(),
                entity_type: target_def.name.as_str(),
                declared_headings: &declared_headings,
            });
            if let Err(e) = validate_section_content(
                next.sections.iter().map(|(k, v)| (k.as_str(), v.as_str())),
                catch_all,
            ) {
                problems.push(RetypeProblem::Validation {
                    code: e.code(),
                    message: e.to_string(),
                    details: e.details(),
                });
            }
        }
        for missing in missing_required_sections(target_def.as_ref(), &next.sections) {
            problems.push(RetypeProblem::MissingRequiredSection {
                key: missing.key,
                heading: missing.heading,
                write_rules: missing.write_rules,
            });
        }
        sections_renamed
    }

    /// Metadata: every carried value must parse for the target.
    fn retype_metadata(
        &self,
        resolved: &ResolvedRetype,
        next: &mut crate::entity::Entity,
        problems: &mut Vec<RetypeProblem>,
    ) {
        let (args, entity, target_def) = (&resolved.args, &resolved.entity, &resolved.target_def);
        {
            let mut parsed: IndexMap<String, crate::entity::MetadataValue> = IndexMap::new();
            let mut supplied: IndexMap<String, String> = IndexMap::new();
            for (key, value) in &entity.metadata {
                if args.drop_metadata.iter().any(|k| k == key) {
                    continue;
                }
                if READ_ONLY_METADATA_KEYS.contains(&key.as_str()) {
                    // The identity triple is engine-authoritative: carried
                    // as is, with `type` set to the target — the one field
                    // this verb exists to move.
                    let carried = if key == "type" {
                        crate::entity::MetadataValue::String(args.target_type.clone())
                    } else {
                        value.clone()
                    };
                    parsed.insert(key.clone(), carried);
                    continue;
                }
                let raw = value.to_frontmatter_string();
                match parse_metadata_value(key, &raw, target_def.as_ref()) {
                    Ok(v) => {
                        parsed.insert(key.clone(), v);
                        supplied.insert(key.clone(), raw);
                    }
                    Err(e) => {
                        // A field the target does not declare has one honest
                        // exit: the caller drops it by name.
                        let message = if e.code() == "UNKNOWN_METADATA_FIELD" {
                            format!("{e}; drop it explicitly with drop_metadata [{key}]")
                        } else {
                            e.to_string()
                        };
                        problems.push(RetypeProblem::Validation {
                            code: e.code(),
                            message,
                            details: e.details(),
                        })
                    }
                }
            }
            // The target's defaults fill fields the entity never carried,
            // the way create seeds them.
            for field in &target_def.metadata_fields {
                if parsed.contains_key(field.key.as_str()) || supplied.contains_key(&field.key) {
                    continue;
                }
                if let Some(default) = &field.default_value
                    && let Ok(v) = parse_metadata_value(&field.key, default, target_def.as_ref())
                {
                    parsed.insert(field.key.clone(), v);
                    supplied.insert(field.key.clone(), default.clone());
                }
            }
            for missing in missing_required_fields(target_def.as_ref(), &supplied) {
                problems.push(RetypeProblem::MissingRequiredField {
                    key: missing.key,
                    description: missing.description,
                    enum_values: missing.enum_values,
                });
            }
            next.metadata = parsed;
        }
    }

    /// Edges: both directions, same-mem and cross-mem. Returns how many
    /// were rechecked.
    fn retype_edges(
        &self,
        resolved: &ResolvedRetype,
        next: &crate::entity::Entity,
        problems: &mut Vec<RetypeProblem>,
    ) -> Result<usize, EngineError> {
        let (args, schema, mem) = (&resolved.args, &resolved.schema, &resolved.mem);
        let id = &args.id;
        let mut edges_rechecked = 0usize;
        // Outgoing: this entity is the source; its type changes.
        for rel in &next.relationships {
            edges_rechecked += 1;
            let target_type = self
                .store
                .get(&rel.target)
                .filter(|e| !e.stub)
                .map(|e| e.entity_type.clone());
            let target_mem = rel.target.mem();
            let violation = self.edge_shape_violation(
                schema,
                mem,
                target_mem,
                &rel.rel_type,
                &args.target_type,
                target_type.as_deref(),
            );
            if let Some(e) = violation {
                problems.push(RetypeProblem::EdgeShape(RetypeEdge {
                    direction: RetypeEdgeDirection::Outgoing,
                    from: id.to_string(),
                    to: rel.target.to_string(),
                    rel_type: rel.rel_type.clone(),
                    cross_mem: target_mem != mem,
                    detail: e.details(),
                }));
            }
        }
        // Incoming, loaded referrers: this entity is the target.
        let incoming: Vec<(EntityId, String)> = self
            .store
            .incoming(id)
            .iter()
            .map(|e| (e.from.clone(), e.rel_type.clone()))
            .collect();
        for (from, rel_type) in incoming {
            let Some(referrer) = self.store.get(&from) else {
                continue;
            };
            edges_rechecked += 1;
            let from_mem = from.mem().to_string();
            let referrer_type = referrer.entity_type.clone();
            // The edge's source schema is the referrer's; the target is this
            // mem. Same-schema cross-mem edges take the shared schema's pins
            // exactly as relate and the loader do.
            let violation = match self.schemas.get(&from_mem).cloned() {
                Some(referrer_schema) => self.edge_shape_violation(
                    &referrer_schema,
                    &from_mem,
                    mem,
                    &rel_type,
                    &referrer_type,
                    Some(&args.target_type),
                ),
                None => None,
            };
            if let Some(e) = violation {
                problems.push(RetypeProblem::EdgeShape(RetypeEdge {
                    direction: RetypeEdgeDirection::Incoming,
                    from: from.to_string(),
                    to: id.to_string(),
                    rel_type,
                    cross_mem: from_mem != *mem,
                    detail: e.details(),
                }));
            }
        }
        // Incoming, deferred referrers: a lazy mem's entities are not in
        // the store. Enumerate its storage and check every relation that
        // names this entity. A mem that cannot be enumerated refuses.
        edges_rechecked += self.recheck_deferred_referrers(id, &args.target_type, problems)?;
        Ok(edges_rechecked)
    }

    /// Block-tier constraints of the target type; warn-tier ones become
    /// warnings on the outcome.
    fn retype_constraints(
        &self,
        resolved: &ResolvedRetype,
        next: &crate::entity::Entity,
        problems: &mut Vec<RetypeProblem>,
        warnings: &mut Vec<WarningHint>,
    ) {
        let (args, target_def) = (&resolved.args, &resolved.target_def);
        let id = &args.id;
        let unsatisfied =
            crate::ops::health::unsatisfied_required_outgoing(next, target_def.as_ref());
        if !unsatisfied.is_empty() {
            let blocked: Vec<_> = unsatisfied
                .iter()
                .filter(|b| b.severity == memstead_schema::ConstraintSeverity::Block)
                .cloned()
                .collect();
            if !blocked.is_empty() {
                problems.push(RetypeProblem::RequiredOutgoingUnsatisfied(blocked));
            }
            warnings.push(WarningHint::MissingRequiredOutgoing {
                entity_type: args.target_type.clone(),
                entity_id: id.clone(),
                missing: unsatisfied,
            });
        }
        {
            let check_provider = self.check_standing_provider();
            let violated = crate::ops::health::unsatisfied_constraints(
                &self.store,
                next,
                target_def.as_ref(),
                Some(id),
                Some(&check_provider),
            );
            if !violated.is_empty() {
                let blocked: Vec<_> = violated
                    .iter()
                    .filter(|v| v.severity() == memstead_schema::ConstraintSeverity::Block)
                    .cloned()
                    .collect();
                if !blocked.is_empty() {
                    problems.push(RetypeProblem::ConstraintUnsatisfied(blocked));
                }
                warnings.push(WarningHint::ConstraintUnsatisfied {
                    entity_type: args.target_type.clone(),
                    entity_id: id.clone(),
                    violations: violated,
                });
            }
        }
    }

    /// One refusal naming every problem, with the target's sections, its
    /// catch-all and a proposed section map as the recovery payload.
    fn refuse_retype_problems(
        &self,
        resolved: &ResolvedRetype,
        problems: Vec<RetypeProblem>,
    ) -> Result<(), EngineError> {
        let (args, entity, target_def) = (&resolved.args, &resolved.entity, &resolved.target_def);
        let id = &args.id;
        if !problems.is_empty() {
            let mut declared: Vec<String> =
                target_def.sections.iter().map(|s| s.key.clone()).collect();
            declared.sort();
            let catch_all = target_def.catch_all_section().map(|s| s.key.clone());
            // A proposed map: every entity section the target does not
            // declare, pointed at the closest declared key or the catch-all.
            let mut proposed: BTreeMap<String, String> = BTreeMap::new();
            for key in entity.sections.keys() {
                if key == "relationships" || target_def.section(key).is_some() {
                    continue;
                }
                if let Some(to) = target_def
                    .suggest_section(key)
                    .or_else(|| catch_all.clone())
                {
                    proposed.insert(key.clone(), to);
                }
            }
            return Err(EngineError::RetypeRefused {
                id: id.to_string(),
                from_type: entity.entity_type.clone(),
                to_type: args.target_type.clone(),
                problems,
                target_sections: declared,
                target_catch_all: catch_all,
                proposed_section_map: proposed,
            });
        }
        Ok(())
    }

    /// The shape violation, if any, of one edge `from_type --rel_type-->
    /// to_type` whose source entity lives in `source_mem` (schema
    /// `source_schema`) and whose target lives in `target_mem` — the rule
    /// relate applies at write time and the loader at boot, so the three
    /// can never disagree: a same-mem edge, and a cross-mem edge between
    /// mems pinning the SAME schema, is judged by that schema's own
    /// relationship pins; a cross-mem edge between different schemas is
    /// judged by the source schema's `cross_mem_relationships` entry for
    /// the target schema. An edge whose entry is not declared at all is
    /// not a shape violation (it already exists; the loader drops it on
    /// its own grounds), and a target mem whose schema cannot be resolved
    /// falls back to the intra-mem rule, as relate does.
    fn edge_shape_violation(
        &self,
        source_schema: &memstead_schema::Schema,
        source_mem: &str,
        target_mem: &str,
        rel_type: &str,
        from_type: &str,
        to_type: Option<&str>,
    ) -> Option<ValidationError> {
        let target_ref: Option<memstead_schema::SchemaRef> = if source_mem == target_mem {
            None
        } else {
            crate::engine::mutation::target_schema_ref_for_routing(self, target_mem)
        };
        let cross_mem_different = match (&target_ref, source_schema.id()) {
            (Some(target), (src_name, _)) => target.name != src_name,
            (None, _) => false,
        };
        if cross_mem_different {
            let target_ref = target_ref.expect("Some when cross_mem_different");
            match validate_cross_mem_edge(rel_type, from_type, to_type, source_schema, &target_ref)
            {
                CrossMemRelCheck::Ok | CrossMemRelCheck::EdgeNotDeclared => None,
                CrossMemRelCheck::Invalid(e) => Some(e),
            }
        } else {
            validate_rel_shape(rel_type, from_type, to_type, source_schema).err()
        }
    }

    /// Re-check the edges that reach `id` from mems whose content is not
    /// in the store — deferred (lazy, unloaded) mounts. Their entities
    /// are enumerated and read through the mount's own backend, the same
    /// storage the relate path probes deferred targets through; every
    /// relation naming `id` is shape-checked against the target type.
    /// Returns the number of edges examined; a mem whose storage cannot
    /// be enumerated refuses typed rather than being assumed fine.
    fn recheck_deferred_referrers(
        &self,
        id: &EntityId,
        target_type: &str,
        problems: &mut Vec<RetypeProblem>,
    ) -> Result<usize, EngineError> {
        let mut examined = 0usize;
        let id_str = id.to_string();
        let fallback = engine_fallback_type();
        for mounted in self.mounts.iter().filter(|m| m.deferred) {
            let referrer_mem = mounted.mount.mem.clone();
            let paths = mounted.backend.list_entities().map_err(|e| {
                EngineError::RetypeReferrerUnprobeable {
                    id: id_str.clone(),
                    mem: referrer_mem.clone(),
                    reason: e.to_string(),
                }
            })?;
            let referrer_schema = self.schemas.get(&referrer_mem);
            for path in paths {
                let rel_path = path.to_string_lossy().into_owned();
                let Some(bytes) =
                    mounted
                        .backend
                        .read_entity(Path::new(&rel_path))
                        .map_err(|e| EngineError::RetypeReferrerUnprobeable {
                            id: id_str.clone(),
                            mem: referrer_mem.clone(),
                            reason: format!("{rel_path}: {e}"),
                        })?
                else {
                    continue;
                };
                let text = String::from_utf8_lossy(&bytes);
                // Cheap gate before parsing: a relation row names the
                // entity as `mem--slug` or `mem:slug`; both carry the slug.
                if !text.contains(id.name()) {
                    continue;
                }
                // Parse under the referrer's own type when its schema is
                // known, the fallback type otherwise: relationships are
                // parsed identically either way.
                let declared_type = crate::entity::parser::peek_type_from_frontmatter(&text);
                let type_def = referrer_schema
                    .and_then(|s| declared_type.as_deref().and_then(|t| s.get_type(t)))
                    .unwrap_or_else(|| fallback.clone());
                let Ok(parsed) = parse_markdown(&text, &rel_path, type_def.as_ref(), &referrer_mem)
                else {
                    continue;
                };
                for rel in &parsed.entity.relationships {
                    if rel.target != *id {
                        continue;
                    }
                    examined += 1;
                    let violation = match referrer_schema {
                        Some(referrer_schema) => self.edge_shape_violation(
                            referrer_schema,
                            &referrer_mem,
                            id.mem(),
                            &rel.rel_type,
                            &parsed.entity.entity_type,
                            Some(target_type),
                        ),
                        None => None,
                    };
                    if let Some(e) = violation {
                        problems.push(RetypeProblem::EdgeShape(RetypeEdge {
                            direction: RetypeEdgeDirection::Incoming,
                            from: parsed.entity.id.to_string(),
                            to: id_str.clone(),
                            rel_type: rel.rel_type.clone(),
                            cross_mem: true,
                            detail: e.details(),
                        }));
                    }
                }
            }
        }
        Ok(examined)
    }
}
