//! Integrity linter — read-time conformance findings.
//!
//! The engine's schema validation runs at write time as refusals on
//! `memstead_create` / `memstead_update` / `memstead_relate`. This module runs the
//! same checks in a read context over the entities already on disk, so
//! `memstead_health` can report the *conformance* axis: which entities of a
//! mem would a write refuse under a given schema, and why.
//!
//! One validation truth, two contexts: every finding carries the same
//! typed code (and the same recovery payload, via
//! [`EngineError::code`] / [`EngineError::details`]) the corresponding
//! write would refuse with. An entity that lints clean against schema
//! S is accepted by a write under S, and vice versa — the linter never
//! invents a parallel conformance vocabulary.
//!
//! Determinism: same store state and schema produce the same findings
//! in the same order, byte for byte. Entities are visited in lexical
//! id order; within one entity, checks run in a fixed sequence (type,
//! section keys, required sections, metadata, required fields,
//! relationships) and map/list iteration follows the entity's own
//! deterministic on-disk order (`IndexMap` / `Vec`).

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use memstead_schema::{Schema, SchemaRef};
use serde::Serialize;

use crate::engine::EngineError;
use crate::engine::mutation::unknown_type_error;
use crate::entity::Entity;
use crate::runtime_validator::{
    CrossMemRelCheck, READ_ONLY_METADATA_KEYS, RelationshipCheck, missing_required_fields,
    missing_required_sections, parse_metadata_value, validate_cross_mem_edge,
    validate_rel_shape, validate_rel_type, validate_section_keys,
};
use crate::store::Store;

/// Which integrity axis a finding belongs to. Consistency findings
/// (graph coherence: orphans, stubs, dangling links) come from the
/// pre-existing health categories; conformance findings (entity vs
/// schema) come from this linter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IntegrityAxis {
    Consistency,
    Conformance,
}

/// One per-entity integrity finding — the stable wire shape
/// `{ id, axis, code, detail }`.
///
/// `code` is drawn from the write-time typed-code vocabulary
/// ([`EngineError::code`]) and `detail` mirrors that code's write-time
/// recovery payload ([`EngineError::details`]).
#[derive(Debug, Clone, Serialize)]
pub struct IntegrityFinding {
    pub id: String,
    pub axis: IntegrityAxis,
    pub code: String,
    pub detail: serde_json::Value,
}

impl IntegrityFinding {
    fn conformance(id: &crate::entity::EntityId, err: &EngineError) -> Self {
        Self {
            id: id.to_string(),
            axis: IntegrityAxis::Conformance,
            code: err.code().to_string(),
            detail: err.details(),
        }
    }
}

/// Run the conformance axis over every non-stub entity of `mem`,
/// validating against `schema` (the mem's current pin, or an
/// arbitrary target schema — the caller chooses the effective schema).
///
/// `mem_schemas` maps mem name → pinned schema for *every* mounted
/// mem; it is consulted only to route relationship checks the same
/// way the write path routes them (same schema *name* → intra-mem
/// vocabulary of `schema`; different name → `schema`'s
/// `cross_mem_relationships`). For cross-mem edges this is the
/// read-time twin of the write-time `validate_cross_mem_edge` —
/// including the target-entity type fetch — so target-type drift on
/// existing edges surfaces here.
pub fn conformance_findings(
    store: &Store,
    mem: &str,
    schema: &Schema,
    mem_schemas: &HashMap<String, Arc<Schema>>,
) -> Vec<IntegrityFinding> {
    let mut entities: Vec<&Entity> = store
        .all_entities()
        .filter(|e| e.mem == mem && !e.stub)
        .collect();
    entities.sort_by(|a, b| a.id.0.cmp(&b.id.0));

    let mut findings = Vec::new();
    for entity in entities {
        lint_entity(store, entity, schema, mem_schemas, &mut findings);
    }
    findings
}

/// Run the consistency axis over `mem`, projecting the pre-existing
/// graph-coherence checks into the integrity-finding shape: dangling
/// wiki-links (`DANGLING_LINK`, on the linking entity) and stubs with
/// their referrers (`ORPHAN_STUB`, on the stub). The category
/// collectors are the same ones the dedicated health includes use —
/// `integrity` is a projection, not a second implementation.
pub fn consistency_findings(store: &Store, mem: &str) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();
    for link in super::health::collect_dangling_links(store, Some(mem)) {
        findings.push(IntegrityFinding {
            id: link.from.to_string(),
            axis: IntegrityAxis::Consistency,
            code: "DANGLING_LINK".to_string(),
            detail: serde_json::json!({
                "from": link.from,
                "target_id": link.target_id,
                "target_path": link.target_path,
                "section": link.section,
            }),
        });
    }
    for (stub_id, referrers) in crate::graph::query::find_stubs(store) {
        if stub_id.mem() != mem {
            continue;
        }
        findings.push(IntegrityFinding {
            id: stub_id.to_string(),
            axis: IntegrityAxis::Consistency,
            code: "ORPHAN_STUB".to_string(),
            detail: serde_json::json!({ "referrers": referrers }),
        });
    }
    // The collectors iterate the HashMap-backed store, so impose the
    // full order here: id, code, then the rendered detail as the
    // tiebreak for several same-code findings on one entity.
    findings.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.code.cmp(&b.code))
            .then_with(|| a.detail.to_string().cmp(&b.detail.to_string()))
    });
    findings
}

/// Conformance findings for a single entity — the per-entity slice of
/// [`conformance_findings`], exposed for callers that gate on one
/// entity's current conformance (the `memstead_update` repair-power gate).
/// Empty result == the entity is conformant: a write of this entity
/// under `schema` would be accepted.
pub fn entity_conformance_findings(
    store: &Store,
    entity: &Entity,
    schema: &Schema,
    mem_schemas: &HashMap<String, Arc<Schema>>,
) -> Vec<IntegrityFinding> {
    let mut findings = Vec::new();
    lint_entity(store, entity, schema, mem_schemas, &mut findings);
    findings
}

fn lint_entity(
    store: &Store,
    entity: &Entity,
    schema: &Schema,
    mem_schemas: &HashMap<String, Arc<Schema>>,
    findings: &mut Vec<IntegrityFinding>,
) {
    // Type lookup gates everything else: an unknown type means no
    // type definition to validate sections/metadata against, exactly
    // as a write of this entity would refuse before any other check.
    let Some(type_def) = schema.types.get(entity.entity_type.as_str()) else {
        findings.push(IntegrityFinding::conformance(
            &entity.id,
            &unknown_type_error(schema, &entity.entity_type),
        ));
        return;
    };

    // Section keys — one finding per unknown key (the write path stops
    // at the first; the linter reports all so one repair pass fixes
    // the entity).
    for key in entity.sections.keys() {
        if let Err(v) = validate_section_keys(std::iter::once(key.as_str()), type_def) {
            findings.push(IntegrityFinding::conformance(
                &entity.id,
                &EngineError::Validation(v),
            ));
        }
    }

    // Required sections — one finding per entity, carrying every
    // missing section, mirroring the create path's bundled refusal.
    let missing_sections = missing_required_sections(type_def, &entity.sections);
    if !missing_sections.is_empty() {
        let mut type_guidance: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        if !type_def.write_rules.is_empty() {
            type_guidance.insert(entity.entity_type.clone(), type_def.write_rules.clone());
        }
        findings.push(IntegrityFinding::conformance(
            &entity.id,
            &EngineError::MissingRequiredSection {
                entity_type: entity.entity_type.clone(),
                missing_count: missing_sections.len(),
                sections: missing_sections,
                type_guidance,
            },
        ));
    }

    // Metadata — unknown keys, enum violations, malformed typed values.
    // Engine-managed keys (`mem`, `id`, `type`) are skipped exactly
    // as the write path treats them (read-only, never caller-supplied).
    let mut supplied: IndexMap<String, String> = IndexMap::new();
    for (key, value) in &entity.metadata {
        let raw = value.to_frontmatter_string();
        supplied.insert(key.clone(), raw.clone());
        if READ_ONLY_METADATA_KEYS.iter().any(|k| k == key) {
            continue;
        }
        if let Err(v) = parse_metadata_value(key, &raw, type_def) {
            findings.push(IntegrityFinding::conformance(
                &entity.id,
                &EngineError::Validation(v),
            ));
        }
    }

    // Required metadata fields the schema does not auto-fill — one
    // finding per entity mirroring the create path's accumulator.
    let missing_fields = missing_required_fields(type_def, &supplied);
    if let Some(first) = missing_fields.first() {
        findings.push(IntegrityFinding::conformance(
            &entity.id,
            &EngineError::RequiredFieldUnset {
                field: first.key.clone(),
                entity_type: entity.entity_type.clone(),
                field_description: Some(first.description.clone()),
                enum_values: first.enum_values.clone(),
                type_write_rules: type_def.write_rules.clone(),
                on_create: true,
                missing: missing_fields.clone(),
            },
        ));
    }

    // Relationships — routed exactly as the write path routes them:
    // same schema *name* on both ends (any version pair) consults the
    // intra-mem vocabulary of the effective schema; a different name
    // consults its `cross_mem_relationships`. An unmounted target
    // mem falls back to the intra path, mirroring the relate path.
    let (src_name, src_version) = schema.id();
    for rel in &entity.relationships {
        let target_mem = rel.target.mem();
        let target_schema = if target_mem == entity.mem {
            None
        } else {
            mem_schemas.get(target_mem)
        };
        let cross_mem_different = target_schema
            .map(|t| t.id().0 != src_name)
            .unwrap_or(false);
        let target_type = store
            .get(&rel.target)
            .map(|e| e.entity_type.clone())
            .filter(|t| !t.is_empty());

        if cross_mem_different {
            let target = target_schema.expect("Some when cross_mem_different");
            let (t_name, t_version) = target.id();
            let target_ref = SchemaRef::new(t_name, t_version.clone());
            match validate_cross_mem_edge(
                &rel.rel_type,
                &entity.entity_type,
                target_type.as_deref(),
                schema,
                &target_ref,
            ) {
                CrossMemRelCheck::Ok => {}
                CrossMemRelCheck::EdgeNotDeclared => {
                    findings.push(IntegrityFinding::conformance(
                        &entity.id,
                        &EngineError::CrossMemEdgeNotDeclared {
                            source_schema: format!("{src_name}@{src_version}"),
                            target_schema: target_ref.as_display(),
                            rel_type: rel.rel_type.clone(),
                            from_id: entity.id.to_string(),
                            to_id: rel.target.to_string(),
                        },
                    ));
                }
                CrossMemRelCheck::Invalid(v) => {
                    findings.push(IntegrityFinding::conformance(
                        &entity.id,
                        &EngineError::Validation(v),
                    ));
                }
            }
        } else {
            match validate_rel_type(&rel.rel_type, schema) {
                // Open-mode schemas admit unknown names at write time
                // (warning, not refusal) — so they lint clean too.
                Ok(RelationshipCheck::Ok) | Ok(RelationshipCheck::OpenWarning(_)) => {}
                Err(v) => {
                    findings.push(IntegrityFinding::conformance(
                        &entity.id,
                        &EngineError::Validation(v),
                    ));
                    continue;
                }
            }
            if let Err(v) = validate_rel_shape(
                &rel.rel_type,
                &entity.entity_type,
                target_type.as_deref(),
                schema,
            ) {
                findings.push(IntegrityFinding::conformance(
                    &entity.id,
                    &EngineError::Validation(v),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{EntityId, MetadataValue, Relationship};

    const TYPE_TAIL: &str = r#"sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: false
    write_rules: []
  - key: notes
    heading: Notes
    required: false
    search_weight: 1.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: status
    description: Lifecycle state
    field_type: string
    enum_values:
      - open
      - closed
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: _default
propagating_relationships: []
updatable_fields:
  - title
  - body
  - notes
  - status
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;

    const PLAIN_TYPE_TAIL: &str = r#"sections:
  - key: body
    heading: Body
    required: false
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: _default
propagating_relationships: []
updatable_fields:
  - title
  - body
health_required_fields: []
staleness_threshold_days: 90
write_rules: []
"#;

    /// `lint-src@0.1.0`: strict vocabulary with shape-pinned
    /// `IMPLEMENTS: doc → doc`, a cross-mem declaration to the
    /// `other` domain (`ADDRESSES: doc → requirement`), and a `doc`
    /// type carrying a required `body` section and a required enum
    /// `status` field with no default.
    fn lint_schema() -> Arc<Schema> {
        let manifest = r#"name: lint-src
version: 0.1.0
description: linter test schema
when_to_use: tests
types:
  - doc
  - req
relationships:
  mode: strict
  definitions:
    - name: IMPLEMENTS
      description: shape-pinned
      default_weight: 1.0
      source_types: [doc]
      target_types: [doc]
    - name: _default
      description: fallback
      default_weight: 1.0
cross_mem_relationships:
  - to_schema: other
    definitions:
      - name: ADDRESSES
        description: outbound
        default_weight: 1.0
        source_types: [doc]
        target_types: [requirement]
community:
  resolution: 1.0
  seed: 42
"#;
        Arc::new(
            memstead_schema::load_schema_from_memory(
                manifest,
                &[
                    (
                        "doc".to_string(),
                        format!("name: doc\ndescription: t\nwhen_to_use: tests\n{TYPE_TAIL}"),
                    ),
                    (
                        "req".to_string(),
                        format!(
                            "name: req\ndescription: t\nwhen_to_use: tests\n{PLAIN_TYPE_TAIL}"
                        ),
                    ),
                ],
            )
            .expect("lint schema loads"),
        )
    }

    /// `other@1.0.0`: the cross-mem target domain, declaring a
    /// `requirement` and a `task` type.
    fn other_schema() -> Arc<Schema> {
        let manifest = r#"name: other
version: 1.0.0
description: target schema
when_to_use: tests
types:
  - requirement
  - task
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
        Arc::new(
            memstead_schema::load_schema_from_memory(
                manifest,
                &[
                    (
                        "requirement".to_string(),
                        format!(
                            "name: requirement\ndescription: t\nwhen_to_use: tests\n{PLAIN_TYPE_TAIL}"
                        ),
                    ),
                    (
                        "task".to_string(),
                        format!("name: task\ndescription: t\nwhen_to_use: tests\n{PLAIN_TYPE_TAIL}"),
                    ),
                ],
            )
            .expect("other schema loads"),
        )
    }

    fn entity(mem: &str, slug: &str, entity_type: &str) -> Entity {
        Entity {
            id: EntityId::new(mem, slug),
            title: slug.to_string(),
            entity_type: entity_type.to_string(),
            mem: mem.to_string(),
            file_path: format!("{slug}.md"),
            metadata: IndexMap::new(),
            sections: IndexMap::new(),
            relationships: Vec::new(),
            content_hash: "h".to_string(),
            stub: false,
            stub_kind: None,
            heading_spans: Default::default(),
        }
    }

    fn conformant_entity(mem: &str, slug: &str) -> Entity {
        let mut e = entity(mem, slug, "doc");
        e.sections.insert("body".to_string(), "content".to_string());
        e.metadata.insert(
            "status".to_string(),
            MetadataValue::String("open".to_string()),
        );
        e
    }

    fn schemas_for(
        entries: &[(&str, Arc<Schema>)],
    ) -> HashMap<String, Arc<Schema>> {
        entries
            .iter()
            .map(|(v, s)| (v.to_string(), s.clone()))
            .collect()
    }

    fn codes(findings: &[IntegrityFinding]) -> Vec<&str> {
        findings.iter().map(|f| f.code.as_str()).collect()
    }

    #[test]
    fn clean_mem_produces_no_findings() {
        let schema = lint_schema();
        let mut store = Store::new();
        let a = conformant_entity("lv", "alpha");
        let mut b = conformant_entity("lv", "beta");
        b.relationships
            .push(Relationship::new("IMPLEMENTS", a.id.clone()));
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);
        let schemas = schemas_for(&[("lv", schema.clone())]);
        let findings = conformance_findings(&store, "lv", &schema, &schemas);
        assert!(findings.is_empty(), "got: {:?}", codes(&findings));
    }

    #[test]
    fn missing_required_section_and_field_carry_write_time_codes() {
        let schema = lint_schema();
        let mut store = Store::new();
        // No body section, no status field — both required.
        let e = entity("lv", "broken", "doc");
        let id = e.id.to_string();
        store.upsert(e.id.clone(), e);
        let schemas = schemas_for(&[("lv", schema.clone())]);
        let findings = conformance_findings(&store, "lv", &schema, &schemas);
        let cs = codes(&findings);
        assert!(cs.contains(&"MISSING_REQUIRED_SECTION"), "got: {cs:?}");
        assert!(cs.contains(&"REQUIRED_FIELD_UNSET"), "got: {cs:?}");
        for f in &findings {
            assert_eq!(f.id, id);
            assert_eq!(f.axis, IntegrityAxis::Conformance);
        }
        // Detail mirrors the write-time recovery payload.
        let section_finding = findings
            .iter()
            .find(|f| f.code == "MISSING_REQUIRED_SECTION")
            .unwrap();
        assert_eq!(
            section_finding.detail["sections"][0]["key"].as_str(),
            Some("body")
        );
        let field_finding = findings
            .iter()
            .find(|f| f.code == "REQUIRED_FIELD_UNSET")
            .unwrap();
        assert_eq!(field_finding.detail["field"].as_str(), Some("status"));
    }

    #[test]
    fn invalid_enum_unknown_section_and_unknown_metadata_surface() {
        let schema = lint_schema();
        let mut store = Store::new();
        let mut e = conformant_entity("lv", "drifted");
        e.metadata.insert(
            "status".to_string(),
            MetadataValue::String("banana".to_string()),
        );
        e.metadata
            .insert("wat".to_string(), MetadataValue::String("x".to_string()));
        e.sections.insert("bogus".to_string(), "text".to_string());
        store.upsert(e.id.clone(), e);
        let schemas = schemas_for(&[("lv", schema.clone())]);
        let findings = conformance_findings(&store, "lv", &schema, &schemas);
        let cs = codes(&findings);
        assert!(cs.contains(&"INVALID_ENUM_VALUE"), "got: {cs:?}");
        assert!(cs.contains(&"UNKNOWN_SECTION"), "got: {cs:?}");
        assert!(cs.contains(&"UNKNOWN_METADATA_FIELD"), "got: {cs:?}");
        let enum_finding = findings
            .iter()
            .find(|f| f.code == "INVALID_ENUM_VALUE")
            .unwrap();
        assert_eq!(enum_finding.detail["value"].as_str(), Some("banana"));
        assert_eq!(
            enum_finding.detail["allowed"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["open", "closed"]
        );
    }

    #[test]
    fn unknown_type_short_circuits_with_unknown_entity_type() {
        let schema = lint_schema();
        let mut store = Store::new();
        let e = entity("lv", "mystery", "ghost");
        store.upsert(e.id.clone(), e);
        let schemas = schemas_for(&[("lv", schema.clone())]);
        let findings = conformance_findings(&store, "lv", &schema, &schemas);
        assert_eq!(codes(&findings), vec!["UNKNOWN_ENTITY_TYPE"]);
        assert_eq!(findings[0].detail["name"].as_str(), Some("ghost"));
    }

    #[test]
    fn invalid_rel_type_and_shape_surface() {
        let schema = lint_schema();
        let mut store = Store::new();
        let mut req_target = conformant_entity("lv", "target");
        req_target.entity_type = "req".to_string();
        // `req` has no required section/field constraints (plain type).
        req_target.metadata.clear();
        req_target.sections.clear();
        let mut e = conformant_entity("lv", "edges");
        e.relationships
            .push(Relationship::new("UNDECLARED", req_target.id.clone()));
        // IMPLEMENTS pins doc → doc; the target is a `req`.
        e.relationships
            .push(Relationship::new("IMPLEMENTS", req_target.id.clone()));
        store.upsert(req_target.id.clone(), req_target);
        store.upsert(e.id.clone(), e);
        let schemas = schemas_for(&[("lv", schema.clone())]);
        let findings = conformance_findings(&store, "lv", &schema, &schemas);
        let cs = codes(&findings);
        assert!(cs.contains(&"INVALID_REL_TYPE"), "got: {cs:?}");
        assert!(cs.contains(&"INVALID_REL_SHAPE"), "got: {cs:?}");
    }

    #[test]
    fn cross_mem_edges_lint_like_the_write_path() {
        let schema = lint_schema();
        let other = other_schema();
        let mut store = Store::new();
        let mut requirement = entity("tv", "goal", "requirement");
        requirement
            .sections
            .insert("body".to_string(), "x".to_string());
        let mut task = entity("tv", "chore", "task");
        task.sections.insert("body".to_string(), "x".to_string());

        let mut e = conformant_entity("lv", "linker");
        // Declared domain + matching target type → clean.
        e.relationships
            .push(Relationship::new("ADDRESSES", requirement.id.clone()));
        // Declared domain, target type drifted off `target_types` →
        // the write-time shape code resurfaces at lint time.
        e.relationships
            .push(Relationship::new("ADDRESSES", task.id.clone()));
        // Rel-type absent from the cross-mem entry entirely.
        e.relationships
            .push(Relationship::new("IMPLEMENTS", requirement.id.clone()));
        store.upsert(requirement.id.clone(), requirement);
        store.upsert(task.id.clone(), task);
        store.upsert(e.id.clone(), e);
        let schemas = schemas_for(&[("lv", schema.clone()), ("tv", other)]);
        let findings = conformance_findings(&store, "lv", &schema, &schemas);
        let cs = codes(&findings);
        assert_eq!(
            cs,
            vec!["INVALID_REL_SHAPE", "INVALID_REL_TYPE"],
            "declared+conformant edge must stay silent; got: {cs:?}"
        );
    }

    #[test]
    fn stub_entities_are_skipped() {
        let schema = lint_schema();
        let mut store = Store::new();
        let mut stub = entity("lv", "ghost-stub", "");
        stub.stub = true;
        store.upsert(stub.id.clone(), stub);
        let schemas = schemas_for(&[("lv", schema.clone())]);
        let findings = conformance_findings(&store, "lv", &schema, &schemas);
        assert!(findings.is_empty());
    }

    #[test]
    fn other_mems_are_out_of_scope() {
        let schema = lint_schema();
        let mut store = Store::new();
        let e = entity("elsewhere", "broken", "doc");
        store.upsert(e.id.clone(), e);
        let schemas = schemas_for(&[("lv", schema.clone())]);
        let findings = conformance_findings(&store, "lv", &schema, &schemas);
        assert!(findings.is_empty());
    }

    #[test]
    fn findings_are_deterministic_and_id_ordered() {
        let schema = lint_schema();
        let mut store = Store::new();
        // Insert in non-lexical order; several findings per entity.
        for slug in ["zeta", "alpha", "mid"] {
            let e = entity("lv", slug, "doc");
            store.upsert(e.id.clone(), e);
        }
        let schemas = schemas_for(&[("lv", schema.clone())]);
        let first = conformance_findings(&store, "lv", &schema, &schemas);
        let second = conformance_findings(&store, "lv", &schema, &schemas);
        let a = serde_json::to_string(&first).unwrap();
        let b = serde_json::to_string(&second).unwrap();
        assert_eq!(a, b, "two runs must be byte-identical");
        let ids: Vec<&str> = first.iter().map(|f| f.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "findings must be in lexical id order");
    }

    #[test]
    fn lint_against_target_schema_differs_from_pin() {
        // The caller picks the effective schema: the same entity lints
        // clean against the `other` schema's `task` type but fails
        // against `lint-src` (which has no `task` type) — the
        // `target_schema` selector semantics.
        let pin = lint_schema();
        let target = other_schema();
        let mut store = Store::new();
        let mut e = entity("lv", "shifting", "task");
        e.sections.insert("body".to_string(), "x".to_string());
        store.upsert(e.id.clone(), e);
        let schemas = schemas_for(&[("lv", pin.clone())]);
        let against_pin = conformance_findings(&store, "lv", &pin, &schemas);
        assert_eq!(codes(&against_pin), vec!["UNKNOWN_ENTITY_TYPE"]);
        let against_target = conformance_findings(&store, "lv", &target, &schemas);
        assert!(
            against_target.is_empty(),
            "got: {:?}",
            codes(&against_target)
        );
    }
}
