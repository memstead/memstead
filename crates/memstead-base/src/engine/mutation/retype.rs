//! `Engine::retype_entity` — change an entity's type in place.
//!
//! The identity triple (`mem` / `id` / `type`) is reserved on `update`
//! by decision: `type` set refuses `READ_ONLY_FIELD`. A type change is
//! its own verb because it is its own validation regime — the existing
//! sections and metadata have to satisfy the TARGET type, and every edge
//! touching the entity, incoming and outgoing, same-mem and cross-mem,
//! has to fit the target type's relationship pins — and its own
//! provenance kind. The id, the file path, and every incoming edge stay:
//! nothing is deleted and re-created, so history and provenance survive.
//!
//! Report-all refusal: every unknown section, missing required section,
//! unknown or invalid metadata value, unsatisfied block-tier constraint,
//! and shape-violating edge is collected and returned in ONE envelope
//! (`RetypeRefused`), with the target's declared sections, its catch-all,
//! and a proposed `section_map` in the recovery payload, so a second
//! attempt can be the right one. Nothing is written before the set is
//! empty.
//!
//! The edge re-check on both directions is mandatory, not thoroughness:
//! the loader drops a shape-invalid edge at the next boot with only a
//! `PARSED_RELATION_INVALID` warning, so a retype that skipped it would
//! amputate the graph on restart. Referrers that live in a lazy (deferred,
//! unloaded) mem are not in the store; they are probed through storage —
//! the same backend the relate path probes deferred targets through — and
//! when a deferred mem cannot be enumerated the retype refuses naming it,
//! never proceeding on the assumption that its edges are fine.

use std::collections::BTreeMap;
use std::path::Path;

use indexmap::IndexMap;

use crate::engine_fallback_type;
use crate::entity::EntityId;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::ops::WarningHint;
use crate::provenance::{Provenance, ProvenanceKind};
use crate::runtime_validator::{
    CatchAllContext, CrossMemRelCheck, READ_ONLY_METADATA_KEYS, ValidationError,
    missing_required_fields, missing_required_sections, parse_metadata_value,
    validate_cross_mem_edge, validate_rel_shape, validate_section_content, validate_section_keys,
};
use crate::vcs::{Actor, ClientId, CommitContext};
use crate::workspace::MountCapability;

use super::super::{Engine, EngineError, RetypeEntityArgs, RetypeEntityOutcome};
use super::unknown_type_error;
use crate::engine::outcomes::{RetypeEdge, RetypeEdgeDirection, RetypeProblem};

impl Engine {
    /// Change `args.id`'s type to `args.target_type` in place. See the
    /// module docs for the contract; the outcome names the old and new
    /// type, the renamed sections, the edges re-checked, and states that
    /// check records and derivation baselines on the entity are stale
    /// (its content hash moved).
    pub fn retype_entity(
        &mut self,
        args: RetypeEntityArgs,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<RetypeEntityOutcome, EngineError> {
        let id = &args.id;
        let mem = id.mem().to_string();

        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| self.unknown_mem_error(&mem))?;
        if self.mounts[mount_idx].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem));
        }

        // Reload-before-operation, so the CAS compare and the edge walk
        // run against current truth.
        let mut drift_warnings = self.reload_if_stale(Some(&mem));

        let entity = self
            .store
            .get(id)
            .ok_or_else(|| EngineError::NotFound { id: id.to_string() })?
            .clone();
        if entity.stub {
            return Err(EngineError::StubNotUpdatable { id: id.to_string() });
        }
        if !args.dry_run
            && let Some(expected) = args.expected_hash.as_deref()
            && entity.content_hash != expected
        {
            return Err(EngineError::HashMismatch {
                id: id.to_string(),
                current: entity.content_hash.clone(),
                is_stub: false,
            });
        }

        let schema = self
            .schemas
            .get(&mem)
            .expect("schema present for every registered mount")
            .clone();
        let target_def = schema
            .get_type(&args.target_type)
            .ok_or_else(|| unknown_type_error(&schema, &args.target_type))?;
        if entity.entity_type == args.target_type {
            return Err(EngineError::RetypeNoOp {
                id: id.to_string(),
                entity_type: entity.entity_type.clone(),
            });
        }

        let mut problems: Vec<RetypeProblem> = Vec::new();
        let mut warnings: Vec<WarningHint> = Vec::new();

        // ----- Sections: apply the map, then validate against the target -----
        let mut next = entity.clone();
        next.entity_type = args.target_type.clone();
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

        // ----- Metadata: every carried value must parse for the target -----
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

        // ----- Edges: both directions, same-mem and cross-mem -----
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
                &schema,
                &mem,
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
                    &mem,
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
                    cross_mem: from_mem != mem,
                    detail: e.details(),
                }));
            }
        }
        // Incoming, deferred referrers: a lazy mem's entities are not in
        // the store. Enumerate its storage and check every relation that
        // names this entity. A mem that cannot be enumerated refuses.
        edges_rechecked += self.recheck_deferred_referrers(id, &args.target_type, &mut problems)?;

        // ----- Block-tier constraints of the target type -----
        let unsatisfied =
            crate::ops::health::unsatisfied_required_outgoing(&next, target_def.as_ref());
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
            let check_provider = self.check_state_provider();
            let violated = crate::ops::health::unsatisfied_constraints(
                &self.store,
                &next,
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

        // ----- Render, and either preview or write -----
        let today = self.now_iso();
        super::auto_stamp_timestamps(&mut next, target_def.as_ref(), &today);
        let markdown = super::render_for_write(&next, target_def.as_ref())?;
        let staleness_note = |from: &str, to: &str| {
            format!(
                "check records and derivation baselines on {id} are stale: its content hash \
                 moved from {from} to {to} with the type change; re-check and re-baseline \
                 deliberately"
            )
        };
        if args.dry_run {
            let parsed = parse_markdown(&markdown, &entity.file_path, target_def.as_ref(), &mem)
                .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
            let prospective = parsed.entity.content_hash.clone();
            return Ok(RetypeEntityOutcome {
                id: id.clone(),
                file_path: entity.file_path.clone(),
                old_type: entity.entity_type.clone(),
                new_type: args.target_type.clone(),
                content_hash: entity.content_hash.clone(),
                prospective_hash: Some(prospective.clone()),
                write_id: String::new(),
                sections_renamed,
                edges_rechecked,
                checks_stale: true,
                staleness_note: staleness_note(&entity.content_hash, &prospective),
                warnings,
            });
        }

        let backend = self.mounts[mount_idx].backend.as_ref();
        backend.write_entity(Path::new(&entity.file_path), markdown.as_bytes())?;
        // `memstead: <verb> <id>` — the subject grammar the history reader
        // attributes touches by; the type change rides the outcome and the
        // provenance verb, not the subject.
        let commit_subject = format!("memstead: retype {id}");
        let ctx = CommitContext {
            actor,
            client: client.cloned(),
            tool: Some("retype_entity"),
            note: note.map(String::from),
            role: self.current_role,
            identity: self.current_identity.clone(),
            logical_operation_id: None,
            entity_ids: None,
        };
        let write_id = backend.commit(&commit_subject, &ctx)?;
        backend.append_provenance(
            &Provenance::new(
                std::time::SystemTime::now(),
                ProvenanceKind::Retype,
                Some(id.to_string()),
                actor,
                client.cloned(),
                note.map(String::from),
            )
            .with_role(self.current_role)
            .with_identity(self.current_identity.clone()),
        )?;
        self.record_self_write(mount_idx, &write_id);
        let stamp_warnings = self.stamp_mutation_versions(mount_idx);

        let parse_result = parse_markdown(&markdown, &entity.file_path, target_def.as_ref(), &mem)
            .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
        let content_hash = parse_result.entity.content_hash.clone();
        let fallback = engine_fallback_type();
        push_entities_into_store(&mut self.store, vec![parse_result], fallback.as_ref(), None);
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );
        self.invalidate_communities();
        self.maintain_search_indexes(std::slice::from_ref(id));

        let mut outcome_warnings = Vec::new();
        outcome_warnings.append(&mut drift_warnings);
        outcome_warnings.extend(stamp_warnings);
        outcome_warnings.extend(warnings);
        if let Some(w) = self.note_missing_warning("retype_entity", note) {
            outcome_warnings.push(w);
        }

        Ok(RetypeEntityOutcome {
            id: id.clone(),
            file_path: entity.file_path.clone(),
            old_type: entity.entity_type.clone(),
            new_type: args.target_type.clone(),
            staleness_note: staleness_note(&entity.content_hash, &content_hash),
            content_hash,
            prospective_hash: None,
            write_id,
            sections_renamed,
            edges_rechecked,
            checks_stale: true,
            warnings: outcome_warnings,
        })
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
            super::target_schema_ref_for_routing(self, target_mem)
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use indexmap::IndexMap;
    use memstead_schema::SchemaRef;
    use memstead_schema::workspace_config::CrossLinkValue;
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::outcomes::{RetypeEdgeDirection, RetypeProblem};
    use crate::engine::test_helpers::*;
    use crate::engine::{
        CreateEntityArgs, Engine, EngineError, RelateEntityArgs, RetypeEntityArgs,
    };
    use crate::ops::WarningHint;
    use crate::storage::FilesystemMemWriter;
    use crate::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, WorkspaceSettings,
    };

    const MAIN_MANIFEST: &str = r#"name: rt
version: 0.1.0
description: retype fixture
when_to_use: tests
types:
  - claim
  - finding
relationships:
  mode: strict
  definitions:
    - name: SUPPORTS
      description: pinned both ends to claim
      default_weight: 1.0
      source_types: [claim]
      target_types: [claim]
    - name: CITES
      description: unpinned
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;

    const PEER_MANIFEST: &str = r#"name: rt-peer
version: 0.1.0
description: peer fixture
when_to_use: tests
types:
  - remark
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
cross_mem_relationships:
  - to_schema: rt
    definitions:
      - name: COMMENTS_ON
        description: pinned to claim on the target side
        default_weight: 1.0
        source_types: [remark]
        target_types: [claim]
community:
  resolution: 1.0
  seed: 42
"#;

    fn type_yaml(name: &str, main_section: &str, main_heading: &str) -> String {
        format!(
            r#"name: {name}
description: t
when_to_use: Here
sections:
  - key: {main_section}
    heading: {main_heading}
    required: true
    search_weight: 10.0
    write_rules: []
  - key: notes
    heading: Notes
    required: false
    search_weight: 1.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - {main_section}
hierarchy_relationship: _default
no_self_loop_relationships: []
updatable_fields:
  - title
  - {main_section}
  - notes
health_required_fields:
  - {main_section}
staleness_threshold_days: 90
write_rules: []
"#
        )
    }

    fn write_schema(root: &Path, name: &str, manifest: &str, types: &[(&str, String)]) {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("types")).unwrap();
        std::fs::write(dir.join("schema.yaml"), manifest).unwrap();
        for (t, body) in types {
            std::fs::write(dir.join("types").join(format!("{t}.yaml")), body).unwrap();
        }
    }

    fn mount(
        mem: &str,
        path: std::path::PathBuf,
        pin: SchemaRef,
        lifecycle: MountLifecycle,
    ) -> Mount {
        Mount {
            mem: mem.to_string(),
            schema: Some(pin),
            storage: MountStorage::Folder { path },
            capability: MountCapability::Write,
            lifecycle,
            cross_linkable: true,
            migration_target: None,
        }
    }

    struct Fixture {
        _tmp: TempDir,
        schemas_dir: std::path::PathBuf,
        main_dir: std::path::PathBuf,
        peer_dir: std::path::PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();
        write_schema(
            &schemas_dir,
            "rt",
            MAIN_MANIFEST,
            &[
                ("claim", type_yaml("claim", "statement", "Statement")),
                ("finding", type_yaml("finding", "conclusion", "Conclusion")),
            ],
        );
        write_schema(
            &schemas_dir,
            "rt-peer",
            PEER_MANIFEST,
            &[("remark", type_yaml("remark", "body", "Body"))],
        );
        let main_dir = tmp.path().join("mem-main");
        let peer_dir = tmp.path().join("mem-peer");
        std::fs::create_dir_all(&main_dir).unwrap();
        std::fs::create_dir_all(&peer_dir).unwrap();
        Fixture {
            _tmp: tmp,
            schemas_dir,
            main_dir,
            peer_dir,
        }
    }

    fn boot(f: &Fixture, peer_lifecycle: MountLifecycle) -> Engine {
        let main_pin = SchemaRef::new("rt", semver::Version::new(0, 1, 0));
        let peer_pin = SchemaRef::new("rt-peer", semver::Version::new(0, 1, 0));
        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![
                (
                    mount("main", f.main_dir.clone(), main_pin, MountLifecycle::Eager),
                    Box::new(FilesystemMemWriter::new(f.main_dir.clone())) as Box<dyn MemBackend>,
                ),
                (
                    mount("peer", f.peer_dir.clone(), peer_pin, peer_lifecycle),
                    Box::new(FilesystemMemWriter::new(f.peer_dir.clone())) as Box<dyn MemBackend>,
                ),
            ],
            Some(&f.schemas_dir),
        )
        .expect("engine boots");
        let mut settings = WorkspaceSettings::default();
        let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
        links.insert("peer".to_string(), CrossLinkValue::Wildcard);
        settings.cross_mem_links = links;
        engine.set_settings(settings);
        engine
    }

    fn create(
        engine: &mut Engine,
        mem: &str,
        title: &str,
        ty: &str,
        section: &str,
    ) -> crate::EntityId {
        let (actor, client) = cli_actor();
        engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: mem.to_string(),
                    title: title.to_string(),
                    entity_type: ty.to_string(),
                    sections: IndexMap::from_iter([
                        (section.to_string(), format!("{title} says so.")),
                        ("notes".to_string(), "Some notes.".to_string()),
                    ]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("creates")
            .id
    }

    fn relate(engine: &mut Engine, from: &crate::EntityId, rel: &str, to: &crate::EntityId) {
        let (actor, client) = cli_actor();
        engine
            .relate_entity(
                RelateEntityArgs {
                    source: from.clone(),
                    expected_hash: None,
                    rel_type: rel.to_string(),
                    target: to.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("relates");
    }

    fn retype(
        engine: &mut Engine,
        id: &crate::EntityId,
        target: &str,
        map: &[(&str, &str)],
    ) -> Result<crate::engine::RetypeEntityOutcome, EngineError> {
        let (actor, client) = cli_actor();
        let hash = engine.get_entity(id).unwrap().content_hash.clone();
        engine.retype_entity(
            RetypeEntityArgs {
                id: id.clone(),
                expected_hash: Some(hash),
                target_type: target.to_string(),
                section_map: map
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
                drop_metadata: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            Some("test retype"),
        )
    }

    fn file_bytes(f: &Fixture, id: &crate::EntityId) -> Vec<u8> {
        std::fs::read(f.main_dir.join(format!("{}.md", id.name()))).unwrap()
    }

    /// AC1: the type changes in place with the mapped section; id, path and
    /// incoming edges stay; the response says checks are stale; a fresh
    /// boot loads the result with no shape drops and the same edge count.
    #[test]
    fn retype_keeps_identity_and_edges_and_states_staleness() {
        let f = fixture();
        let mut engine = boot(&f, MountLifecycle::Eager);
        let a = create(&mut engine, "main", "Claim A", "claim", "statement");
        let b = create(&mut engine, "main", "Claim B", "claim", "statement");
        // Unpinned incoming edge onto B, and an unpinned outgoing one.
        relate(&mut engine, &a, "CITES", &b);
        relate(&mut engine, &b, "CITES", &a);
        let edges_before = engine.store().incoming(&b).len() + engine.store().outgoing(&b).len();
        let hash_before = engine.get_entity(&b).unwrap().content_hash.clone();

        let out = retype(&mut engine, &b, "finding", &[("statement", "conclusion")])
            .expect("retype succeeds");
        assert_eq!(out.id, b);
        assert_eq!(out.file_path, "claim-b.md");
        assert_eq!(
            (out.old_type.as_str(), out.new_type.as_str()),
            ("claim", "finding")
        );
        assert_eq!(
            out.sections_renamed,
            vec![("statement".to_string(), "conclusion".to_string())]
        );
        assert_eq!(out.edges_rechecked, 2);
        assert!(out.checks_stale);
        assert!(
            out.staleness_note
                .contains("check records and derivation baselines")
        );
        assert!(out.staleness_note.contains(&hash_before));
        assert!(!out.write_id.is_empty());

        let e = engine.get_entity(&b).unwrap();
        assert_eq!(
            e.entity_type,
            "finding",
            "store entity after retype: {e:?}\nfile: {}",
            String::from_utf8(file_bytes(&f, &b)).unwrap()
        );
        assert_eq!(e.file_path, "claim-b.md");
        assert_eq!(
            e.sections.get("conclusion").map(String::as_str),
            Some("Claim B says so.")
        );
        assert!(!e.sections.contains_key("statement"));
        assert_ne!(e.content_hash, hash_before);
        assert_eq!(engine.store().incoming(&b).len(), 1, "incoming edge stays");
        assert_eq!(engine.store().incoming(&b)[0].from, a);
        let text = String::from_utf8(file_bytes(&f, &b)).unwrap();
        assert!(text.contains("type: finding"), "{text}");
        assert!(text.contains("## Conclusion"), "{text}");

        // Restart: nothing dropped, edge count unchanged.
        let fresh = boot(&f, MountLifecycle::Eager);
        let dropped = fresh
            .load_warnings()
            .iter()
            .any(|w| matches!(w, WarningHint::ParsedRelationInvalid { .. }));
        assert!(!dropped, "{:?}", fresh.load_warnings());
        assert_eq!(
            fresh.store().incoming(&b).len() + fresh.store().outgoing(&b).len(),
            edges_before
        );
        assert_eq!(
            fresh.get_entity(&b).unwrap().entity_type,
            "finding",
            "warnings: {:?}\nfile: {}",
            fresh.load_warnings(),
            String::from_utf8(file_bytes(&f, &b)).unwrap()
        );
    }

    /// AC1 refusal complement: a map naming a key the target does not
    /// declare refuses UNKNOWN_SECTION with the target's declared sections
    /// and a proposed map, and the file is byte-identical afterwards.
    #[test]
    fn unknown_section_refuses_with_declared_sections_and_touches_nothing() {
        let f = fixture();
        let mut engine = boot(&f, MountLifecycle::Eager);
        let b = create(&mut engine, "main", "Claim B", "claim", "statement");
        let before = file_bytes(&f, &b);
        let err = retype(&mut engine, &b, "finding", &[("statement", "bogus")]).unwrap_err();
        assert_eq!(err.code(), "UNKNOWN_SECTION");
        let d = err.details();
        assert_eq!(
            d["target_sections"],
            serde_json::json!(["conclusion", "notes"])
        );
        assert_eq!(d["target_catch_all"], "notes");
        assert_eq!(d["proposed_section_map"]["statement"], "notes", "{d}");
        // Report-all: the unknown key AND the required section it leaves
        // empty arrive together; the envelope code is the map defect.
        let codes: Vec<&str> = d["problems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["code"].as_str().unwrap())
            .collect();
        assert_eq!(codes, vec!["UNKNOWN_SECTION", "MISSING_REQUIRED_SECTION"]);
        assert_eq!(d["problems"][0]["key"], "bogus");
        assert_eq!(
            file_bytes(&f, &b),
            before,
            "refusal leaves the file untouched"
        );
        assert_eq!(engine.get_entity(&b).unwrap().entity_type, "claim");

        // No map at all: the undeclared `statement` refuses the same way,
        // and the required `conclusion` is reported in the same envelope.
        let err = retype(&mut engine, &b, "finding", &[]).unwrap_err();
        assert_eq!(
            err.code(),
            "UNKNOWN_SECTION",
            "a section-map defect dominates the envelope code"
        );
        let codes: Vec<String> = err.details()["problems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["code"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(codes, vec!["UNKNOWN_SECTION", "MISSING_REQUIRED_SECTION"]);
        assert_eq!(file_bytes(&f, &b), before);
        assert!(matches!(
            retype(&mut engine, &b, "claim", &[]).unwrap_err(),
            EngineError::RetypeNoOp { .. }
        ));
    }

    /// AC2: an incoming edge whose rel-type pins its target to the current
    /// type refuses; so does an outgoing one and a cross-mem one; two
    /// violations at once arrive in one envelope; every refusal leaves the
    /// file byte-identical.
    #[test]
    fn edge_shapes_are_rechecked_in_both_directions_and_across_mems() {
        let f = fixture();
        let mut engine = boot(&f, MountLifecycle::Eager);
        let a = create(&mut engine, "main", "Claim A", "claim", "statement");
        let b = create(&mut engine, "main", "Claim B", "claim", "statement");
        relate(&mut engine, &a, "SUPPORTS", &b); // pinned: target must stay claim
        let before_b = file_bytes(&f, &b);

        // Incoming.
        let err = retype(&mut engine, &b, "finding", &[("statement", "conclusion")]).unwrap_err();
        assert_eq!(err.code(), "INVALID_REL_SHAPE", "{err}");
        let EngineError::RetypeRefused { problems, .. } = &err else {
            panic!("{err:?}")
        };
        assert_eq!(problems.len(), 1);
        let RetypeProblem::EdgeShape(edge) = &problems[0] else {
            panic!("{problems:?}")
        };
        assert_eq!(edge.direction, RetypeEdgeDirection::Incoming);
        assert_eq!(
            (edge.from.as_str(), edge.rel_type.as_str()),
            (a.as_ref(), "SUPPORTS")
        );
        assert!(!edge.cross_mem);
        assert_eq!(file_bytes(&f, &b), before_b);

        // Outgoing.
        let before_a = file_bytes(&f, &a);
        let err = retype(&mut engine, &a, "finding", &[("statement", "conclusion")]).unwrap_err();
        assert_eq!(err.code(), "INVALID_REL_SHAPE");
        let EngineError::RetypeRefused { problems, .. } = &err else {
            panic!("{err:?}")
        };
        let RetypeProblem::EdgeShape(edge) = &problems[0] else {
            panic!("{problems:?}")
        };
        assert_eq!(edge.direction, RetypeEdgeDirection::Outgoing);
        assert_eq!(edge.to, b.to_string());
        assert_eq!(file_bytes(&f, &a), before_a);

        // Report-all: B also supports A now — retyping B violates an
        // incoming AND an outgoing pin, both in one envelope.
        relate(&mut engine, &b, "SUPPORTS", &a);
        let before_b = file_bytes(&f, &b);
        let err = retype(&mut engine, &b, "finding", &[("statement", "conclusion")]).unwrap_err();
        assert_eq!(err.code(), "INVALID_REL_SHAPE");
        let d = err.details();
        let dirs: Vec<String> = d["problems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["direction"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(dirs, vec!["outgoing", "incoming"]);
        assert_eq!(file_bytes(&f, &b), before_b);

        // Cross-mem: a remark in the peer mem comments on a claim; the
        // cross-mem entry pins the target to claim.
        let c = create(&mut engine, "main", "Claim C", "claim", "statement");
        let r = create(&mut engine, "peer", "Remark R", "remark", "body");
        relate(&mut engine, &r, "COMMENTS_ON", &c);
        let before_c = file_bytes(&f, &c);
        let err = retype(&mut engine, &c, "finding", &[("statement", "conclusion")]).unwrap_err();
        assert_eq!(err.code(), "INVALID_REL_SHAPE", "{err}");
        let EngineError::RetypeRefused { problems, .. } = &err else {
            panic!("{err:?}")
        };
        let RetypeProblem::EdgeShape(edge) = &problems[0] else {
            panic!("{problems:?}")
        };
        assert!(edge.cross_mem);
        assert_eq!(edge.from, r.to_string());
        assert_eq!(file_bytes(&f, &c), before_c);
    }

    /// Two mems pinning the SAME schema: a cross-mem edge is judged by that
    /// schema's own pins, exactly as relate and the loader judge it, so a
    /// retype that would strand the referrer's edge refuses — the
    /// same-schema case the grader of 2026-09-02 found waved through.
    #[test]
    fn same_schema_cross_mem_edges_are_rechecked() {
        let f = fixture();
        let tmp_twin = f._tmp.path().join("mem-twin");
        std::fs::create_dir_all(&tmp_twin).unwrap();
        let main_pin = SchemaRef::new("rt", semver::Version::new(0, 1, 0));
        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![
                (
                    mount(
                        "main",
                        f.main_dir.clone(),
                        main_pin.clone(),
                        MountLifecycle::Eager,
                    ),
                    Box::new(FilesystemMemWriter::new(f.main_dir.clone())) as Box<dyn MemBackend>,
                ),
                (
                    mount("twin", tmp_twin.clone(), main_pin, MountLifecycle::Eager),
                    Box::new(FilesystemMemWriter::new(tmp_twin.clone())) as Box<dyn MemBackend>,
                ),
            ],
            Some(&f.schemas_dir),
        )
        .unwrap();
        let mut settings = WorkspaceSettings::default();
        let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
        links.insert("twin".to_string(), CrossLinkValue::Wildcard);
        links.insert("main".to_string(), CrossLinkValue::Wildcard);
        settings.cross_mem_links = links;
        engine.set_settings(settings);

        let v = create(&mut engine, "main", "Claim V", "claim", "statement");
        let w = create(&mut engine, "twin", "Claim W", "claim", "statement");
        relate(&mut engine, &w, "SUPPORTS", &v); // pinned target: claim
        let before = file_bytes(&f, &v);
        let err = retype(&mut engine, &v, "finding", &[("statement", "conclusion")]).unwrap_err();
        assert_eq!(err.code(), "INVALID_REL_SHAPE", "{err}");
        let EngineError::RetypeRefused { problems, .. } = &err else {
            panic!("{err:?}")
        };
        let RetypeProblem::EdgeShape(edge) = &problems[0] else {
            panic!("{problems:?}")
        };
        assert!(edge.cross_mem);
        assert_eq!(edge.from, w.to_string());
        assert_eq!(file_bytes(&f, &v), before);

        // Outgoing across the twin: W supports V, retyping W refuses too.
        let err = retype(&mut engine, &w, "finding", &[("statement", "conclusion")]).unwrap_err();
        assert_eq!(err.code(), "INVALID_REL_SHAPE", "{err}");
    }

    /// The referrer lives in a LAZY mem: its edge is not in the store, so
    /// the retype probes the mem's storage and finds it; the same graph
    /// with the peer loaded eagerly refuses identically, and the deferred
    /// probe counts the edge it examined.
    #[test]
    fn deferred_referrers_are_probed_through_storage() {
        let f = fixture();
        {
            let mut engine = boot(&f, MountLifecycle::Eager);
            let c = create(&mut engine, "main", "Claim C", "claim", "statement");
            let r = create(&mut engine, "peer", "Remark R", "remark", "body");
            relate(&mut engine, &r, "COMMENTS_ON", &c);
        }
        let mut lazy = boot(&f, MountLifecycle::Lazy);
        let c = crate::EntityId::canonical("main--claim-c");
        assert!(
            lazy.mem_is_deferred("peer"),
            "peer stays unloaded until touched"
        );
        assert!(
            lazy.store().incoming(&c).is_empty(),
            "the lazy referrer's edge is not in the store"
        );
        let err = retype(&mut lazy, &c, "finding", &[("statement", "conclusion")]).unwrap_err();
        assert_eq!(err.code(), "INVALID_REL_SHAPE", "{err}");
        let EngineError::RetypeRefused { problems, .. } = &err else {
            panic!("{err:?}")
        };
        let RetypeProblem::EdgeShape(edge) = &problems[0] else {
            panic!("{problems:?}")
        };
        assert!(edge.cross_mem);
        assert_eq!(edge.from, "peer--remark-r");
        assert_eq!(lazy.get_entity(&c).unwrap().entity_type, "claim");

        // Without the pinned referrer, a lazily mounted peer does not
        // block, and the probe reports the edges it examined (none).
        let d = create(&mut lazy, "main", "Claim D", "claim", "statement");
        let out = retype(&mut lazy, &d, "finding", &[("statement", "conclusion")]).unwrap();
        assert_eq!(out.edges_rechecked, 0);
    }

    /// Dry run: the same validation, the prospective hash, no write.
    #[test]
    fn dry_run_validates_and_writes_nothing() {
        let f = fixture();
        let mut engine = boot(&f, MountLifecycle::Eager);
        let b = create(&mut engine, "main", "Claim B", "claim", "statement");
        let before = file_bytes(&f, &b);
        let (actor, client) = cli_actor();
        let out = engine
            .retype_entity(
                RetypeEntityArgs {
                    id: b.clone(),
                    expected_hash: None,
                    target_type: "finding".to_string(),
                    section_map: IndexMap::from_iter([(
                        "statement".to_string(),
                        "conclusion".to_string(),
                    )]),
                    drop_metadata: Vec::new(),
                    dry_run: true,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(out.write_id.is_empty());
        assert!(out.prospective_hash.is_some());
        assert_ne!(
            out.prospective_hash.as_deref(),
            Some(out.content_hash.as_str())
        );
        assert_eq!(file_bytes(&f, &b), before);
        assert_eq!(engine.get_entity(&b).unwrap().entity_type, "claim");
    }
}
