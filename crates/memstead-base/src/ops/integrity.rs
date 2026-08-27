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
    missing_required_sections, parse_metadata_value, validate_cross_mem_edge, validate_rel_shape,
    validate_rel_type, validate_section_keys,
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

/// One per-entity BODY OBSERVATION — what an entity's stored body carries
/// that its type does not declare (consistency-sweep 04/01).
///
/// **Deliberately not an [`IntegrityFinding`].** A finding is a thing to fix,
/// and most of what this reports is nothing to fix: absorbing an undeclared
/// heading into the catch-all is the feature working as designed, and making
/// it a violation would fail every mem that uses the catch-all for the prose
/// the schema did not anticipate. The distinction the reader needs is between
/// content that was OBSERVED and content that was LOST, not between clean and
/// dirty, so observations travel on their own channel and no observation can
/// mark an entity unconformant.
///
/// What the conformance axis could see before this was a tautology: it linted
/// `entity.sections.keys()`, which came out of the parser and are declared by
/// construction. Every heading the file actually carried was invisible to it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BodyObservation {
    pub id: String,
    /// `ABSORBED_SECTION` | `UNDECLARED_METADATA_KEY` | `REPEATED_SECTION_HEADING`
    pub code: String,
    /// Whether the content survives the next write. This is the whole point of
    /// the channel: `absorbed` content round-trips, `dropped` content does not,
    /// and before this the reader could not tell which case they were in.
    pub fate: ObservationFate,
    pub detail: serde_json::Value,
}

/// What happens to the observed content on the next write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObservationFate {
    /// Kept, byte-verbatim, in the type's catch-all section. Nothing to fix.
    Absorbed,
    /// NOT kept. The next write drops it, and the reader is told before that
    /// write rather than after it.
    Dropped,
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

impl BodyObservation {
    /// Test convenience: the recorded occurrence count of a repeated heading.
    #[cfg(test)]
    fn occurrences_is(&self, n: u64) -> bool {
        self.detail["occurrences"].as_u64() == Some(n)
    }
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

    /// A conformance finding whose detail the read path knows and the write
    /// path cannot: a write sees one section's content, a read sees which
    /// declared sections that content swallowed.
    fn conformance_with_detail(
        id: &crate::entity::EntityId,
        code: &str,
        detail: serde_json::Value,
    ) -> Self {
        Self {
            id: id.to_string(),
            axis: IntegrityAxis::Conformance,
            code: code.to_string(),
            detail,
        }
    }
}

/// The declared sections an unterminated fence in `body` has swallowed.
///
/// The parser masked their heading lines, so they never became section keys;
/// their bytes sit verbatim inside `body`. Scanning the UNMASKED body for `## `
/// lines and intersecting with the type's declared headings recovers exactly
/// what the entity lost. Headings the type does not declare are left out on
/// purpose: those are the catch-all's business (04/01), and naming them here
/// would report the same bytes under two codes.
pub(crate) fn swallowed_declared_sections(
    body: &str,
    type_def: &memstead_schema::TypeDefinition,
) -> Vec<String> {
    let declared: std::collections::BTreeSet<&str> = type_def
        .sections
        .iter()
        .map(|s| s.heading.as_str())
        .collect();
    let mut out = Vec::new();
    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("## ")
            && declared.contains(heading.trim())
            && !out.iter().any(|h| h == heading.trim())
        {
            out.push(heading.trim().to_string());
        }
    }
    out
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

/// Every body observation for `mem`, in a stable order.
///
/// Reads what the FILE carried, not what the parser kept: `raw_section_headings`
/// is the literal `## ` list in document order, and `entity.metadata` holds
/// every frontmatter key that arrived, declared or not. Linting the parsed
/// section keys instead (which is what the conformance axis does) can only ever
/// answer a question it already knows: those keys are declared by construction.
pub fn body_observations(store: &Store, mem: &str, schema: &Schema) -> Vec<BodyObservation> {
    let mut entities: Vec<&Entity> = store
        .all_entities()
        .filter(|e| e.mem == mem && !e.stub)
        .collect();
    entities.sort_by(|a, b| a.id.0.cmp(&b.id.0));

    let mut out = Vec::new();
    for entity in entities {
        let Some(type_def) = schema.types.get(entity.entity_type.as_str()) else {
            // An unknown type is already a conformance FINDING; observing its
            // body on top would say the same thing twice in a weaker voice.
            continue;
        };
        observe_entity(entity, type_def, &mut out);
    }
    out.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.code.cmp(&b.code))
            .then_with(|| a.detail.to_string().cmp(&b.detail.to_string()))
    });
    out
}

fn observe_entity(
    entity: &Entity,
    type_def: &memstead_schema::TypeDefinition,
    out: &mut Vec<BodyObservation>,
) {
    // Compare on KEYS through `derive_section_key`, and chain the
    // relationships block in, because that is exactly the set the parser's
    // own `build_catch_all` treats as known. Comparing raw heading strings
    // against `s.heading` looks equivalent and is not: it misses the
    // engine's auto-managed `## Relationships` block, which no type
    // declares and which the generator re-emits from the parsed relations
    // on every write. Reporting it made every entity in a real mem carry an
    // observation. The rule must be the parser's own key set, never a
    // second spelling of it.
    let known: std::collections::BTreeSet<String> = type_def
        .sections
        .iter()
        .map(|s| s.key.clone())
        .chain(std::iter::once("relationships".to_string()))
        .collect();
    let catch_all = type_def.catch_all_section();

    // 1. Headings the file carried that the type does not declare. Absorbed
    //    into the catch-all and kept byte-verbatim, UNLESS the body under them
    //    is empty: the catch-all builder skips empty content, so a bare heading
    //    line is the one case that really is dropped. That is the case the
    //    original repro described.
    let mut seen: std::collections::BTreeMap<&str, usize> = Default::default();
    for heading in &entity.raw_section_headings {
        let occurrence = {
            let n = seen.entry(heading.as_str()).or_default();
            *n += 1;
            *n
        };
        if known.contains(&memstead_schema::derive_section_key(heading)) {
            continue;
        }
        // Only the FIRST occurrence is absorbed. Splitting is first-wins, so a
        // later occurrence's body is gone whatever the catch-all does, and
        // emitting a second `ABSORBED_SECTION` for it claimed a survival it
        // does not have. The repeat is reported on its own code below, which
        // is where that loss belongs.
        if occurrence > 1 {
            continue;
        }
        let absorbed_into = catch_all.map(|c| c.key.as_str());
        let kept = absorbed_into.is_some() && heading_has_body(entity, heading, catch_all);
        out.push(BodyObservation {
            id: entity.id.to_string(),
            code: "ABSORBED_SECTION".to_string(),
            fate: if kept {
                ObservationFate::Absorbed
            } else {
                ObservationFate::Dropped
            },
            detail: serde_json::json!({
                "heading": heading,
                "entity_type": entity.entity_type,
                "absorbed_into": absorbed_into,
                "note": if kept {
                    "the type does not declare this heading; its content is kept \
                     byte-verbatim in the catch-all section and survives the next write"
                } else if absorbed_into.is_some() {
                    "the type does not declare this heading and its body is empty; the \
                     catch-all skips empty content, so the next write does NOT keep it"
                } else {
                    "the type does not declare this heading and has no catch-all section, \
                     so the next write does NOT keep it"
                },
            }),
        });
    }

    // 2. A heading that appears twice. Section splitting is first-wins, so
    //    every later body is silently gone. The existing duplicate-heading
    //    warning is filtered through the DECLARED keys with the catch-all
    //    excluded, which is why a repeat of an undeclared heading and a repeat
    //    of the catch-all's own heading both produce no warning anywhere.
    for (heading, count) in seen.iter().filter(|(_, n)| **n > 1) {
        out.push(BodyObservation {
            id: entity.id.to_string(),
            code: "REPEATED_SECTION_HEADING".to_string(),
            fate: ObservationFate::Dropped,
            detail: serde_json::json!({
                "heading": heading,
                "occurrences": count,
                "note": "section splitting is first-wins: the body under the first \
                         occurrence is kept and every later body was NOT kept",
            }),
        });
    }

    // 3. Frontmatter keys the file carried that the type does not declare. The
    //    metadata builder emits only declared fields, so these are dropped on
    //    EVERY write, unconditionally. Reported here, before that write rather
    //    than after it. (A key supplied by a CALLER already refuses today with
    //    `UNKNOWN_METADATA_FIELD`; this is the file-facing half, where the key
    //    was never presented to a validator.)
    for key in entity.metadata.keys() {
        if RESERVED_METADATA.contains(&key.as_str()) || type_def.metadata_field(key).is_some() {
            continue;
        }
        out.push(BodyObservation {
            id: entity.id.to_string(),
            code: "UNDECLARED_METADATA_KEY".to_string(),
            fate: ObservationFate::Dropped,
            detail: serde_json::json!({
                "key": key,
                "entity_type": entity.entity_type,
                "note": "the type does not declare this frontmatter key; the generator \
                         emits only declared fields, so the next write drops it",
            }),
        });
    }
}

/// Engine-stamped frontmatter keys every type carries without declaring.
const RESERVED_METADATA: &[&str] = &["type", "created_date", "last_modified"];

/// Whether an undeclared heading's content actually survived into the
/// catch-all. The catch-all re-emits absorbed content under its original
/// heading line, so the heading appearing there is the evidence that it was
/// kept; a bare heading with no body never reaches it.
fn heading_has_body(
    entity: &Entity,
    heading: &str,
    catch_all: Option<&memstead_schema::SectionDef>,
) -> bool {
    let Some(c) = catch_all else { return false };
    let Some(value) = entity.sections.get(c.key.as_str()) else {
        return false;
    };
    // Line-anchored, not `contains`. The catch-all builder SKIPS empty
    // content, so an undeclared heading survives exactly when its own
    // heading line was re-emitted into the catch-all value — and a
    // substring test answers a different question, saying "kept" for a
    // heading whose text merely appears inside neighbouring prose.
    value.lines().any(|line| {
        line.strip_prefix("## ")
            .is_some_and(|rest| rest.trim() == heading)
    })
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

    // An unterminated fence, before anything else: it is the one condition
    // under which the rest of this walk is reading a body that is not the
    // entity's. Every declared section after the open fence was absorbed into
    // it, so those keys are absent from `entity.sections` and the required-
    // section check below would report them missing without saying why. This
    // finding names the cause; that one names the symptom.
    for (key, value) in &entity.sections {
        let Some(fence) = crate::markdown::closing_fence_if_unterminated(value.trim()) else {
            continue;
        };
        let swallowed = swallowed_declared_sections(value, type_def);
        findings.push(IntegrityFinding::conformance_with_detail(
            &entity.id,
            "UNTERMINATED_FENCE",
            serde_json::json!({
                "section": key,
                "fence": fence,
                "entity_type": entity.entity_type,
                "swallowed_sections": swallowed,
                "note": if swallowed.is_empty() {
                    "this section ends inside an unterminated code fence; no declared section \
                     follows it in the file yet, but the next write would bury whatever does"
                } else {
                    "these declared sections are NOT empty: their content sits verbatim inside \
                     the section above, hidden by an unterminated code fence. Supply a corrected \
                     body for that section; the next write would otherwise close the fence \
                     around them and make the loss permanent"
                },
            }),
        ));
    }

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
                // The linter reports every gate as its own finding
                // (the RequiredFieldUnset finding below), so a
                // pre-announcement here would duplicate it.
                pre_announced_missing_fields: Vec::new(),
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
        let cross_mem_different = target_schema.map(|t| t.id().0 != src_name).unwrap_or(false);
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
no_self_loop_relationships: []
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
no_self_loop_relationships: []
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
                        format!("name: req\ndescription: t\nwhen_to_use: tests\n{PLAIN_TYPE_TAIL}"),
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
            raw_section_headings: Vec::new(),
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

    fn schemas_for(entries: &[(&str, Arc<Schema>)]) -> HashMap<String, Arc<Schema>> {
        entries
            .iter()
            .map(|(v, s)| (v.to_string(), s.clone()))
            .collect()
    }

    fn codes(findings: &[IntegrityFinding]) -> Vec<&str> {
        findings.iter().map(|f| f.code.as_str()).collect()
    }

    /// Criteria 1 and 2 (consistency-sweep 04/01). A heading the type does not
    /// declare is REPORTED, naming the entity, the heading and where the
    /// content went — and it is an observation, never a conformance finding,
    /// because absorbing it is the catch-all working as designed.
    #[test]
    fn an_absorbed_heading_is_observed_and_never_a_violation() {
        let schema = lint_schema();
        let mut store = Store::new();
        let mut e = conformant_entity("lv", "alpha");
        e.raw_section_headings = vec!["Body".into(), "Field Notes".into()];
        // The catch-all re-emits absorbed content under its original heading.
        e.sections.insert(
            "notes".into(),
            "## Field Notes\n\nsomething useful\n".into(),
        );
        let id = e.id.to_string();
        store.upsert(e.id.clone(), e);

        let obs = body_observations(&store, "lv", &schema);
        assert_eq!(obs.len(), 1, "got {obs:?}");
        assert_eq!(obs[0].code, "ABSORBED_SECTION");
        assert_eq!(obs[0].id, id);
        assert_eq!(obs[0].detail["heading"], "Field Notes");
        assert_eq!(
            obs[0].fate,
            ObservationFate::Absorbed,
            "the content survives the next write, and the report must say so"
        );

        // The refusal complement: nothing on the conformance axis.
        let schemas = schemas_for(&[("lv", schema.clone())]);
        let findings = conformance_findings(&store, "lv", &schema, &schemas);
        assert!(
            findings.is_empty(),
            "healthy catch-all use must not be a violation: {:?}",
            codes(&findings)
        );
    }

    /// Criterion 1's other half: a bare heading with no body is the one case
    /// that really is lost, because the catch-all builder skips empty content.
    /// Appending a bare heading line is what the original repro described.
    #[test]
    fn a_bare_undeclared_heading_is_observed_as_dropped() {
        let schema = lint_schema();
        let mut store = Store::new();
        let mut e = conformant_entity("lv", "alpha");
        e.raw_section_headings = vec!["Body".into(), "Scratch".into()];
        // Nothing reached the catch-all: the heading had no body.
        store.upsert(e.id.clone(), e);

        let obs = body_observations(&store, "lv", &schema);
        assert_eq!(obs.len(), 1, "got {obs:?}");
        assert_eq!(obs[0].code, "ABSORBED_SECTION");
        assert_eq!(
            obs[0].fate,
            ObservationFate::Dropped,
            "an empty heading is skipped by the catch-all, so it does NOT survive"
        );
    }

    /// Criterion 3: a frontmatter key the type does not declare is reported
    /// BEFORE the write that drops it. The generator emits only declared
    /// fields, so this one is unconditional loss.
    #[test]
    fn an_undeclared_metadata_key_is_observed_as_dropped() {
        let schema = lint_schema();
        let mut store = Store::new();
        let mut e = conformant_entity("lv", "alpha");
        e.metadata
            .insert("reviewer".into(), MetadataValue::String("ada".into()));
        // Engine-stamped keys are not the caller's and are not reported.
        e.metadata
            .insert("last_modified".into(), MetadataValue::String("x".into()));
        store.upsert(e.id.clone(), e);

        let obs = body_observations(&store, "lv", &schema);
        assert_eq!(obs.len(), 1, "got {obs:?}");
        assert_eq!(obs[0].code, "UNDECLARED_METADATA_KEY");
        assert_eq!(obs[0].detail["key"], "reviewer");
        assert_eq!(obs[0].fate, ObservationFate::Dropped);
    }

    /// Criterion 4: a repeated heading loses every later body, and the two
    /// cases that produce no warning anywhere today are a repeat of an
    /// UNDECLARED heading and a repeat of the CATCH-ALL's own heading.
    #[test]
    fn a_repeated_heading_is_observed_in_both_silent_cases() {
        let schema = lint_schema();
        for (headings, label) in [
            (
                vec!["Body", "Scratch", "Scratch"],
                "undeclared heading twice",
            ),
            (
                vec!["Body", "Notes", "Notes"],
                "the catch-all's own heading twice",
            ),
        ] {
            let mut store = Store::new();
            let mut e = conformant_entity("lv", "alpha");
            e.raw_section_headings = headings.iter().map(|h| h.to_string()).collect();
            e.sections
                .insert("notes".into(), "## Scratch\n\nkept\n".into());
            store.upsert(e.id.clone(), e);

            let obs = body_observations(&store, "lv", &schema);
            let repeats: Vec<_> = obs
                .iter()
                .filter(|o| o.code == "REPEATED_SECTION_HEADING")
                .collect();
            assert_eq!(repeats.len(), 1, "{label}: got {obs:?}");
            assert!(repeats[0].occurrences_is(2), "{label}");
            assert_eq!(repeats[0].fate, ObservationFate::Dropped, "{label}");
        }
    }

    /// Criterion 5, the refusal complement that gives the rest its worth: the
    /// ordinary entity produces nothing. A check that fires on healthy content
    /// is worse than no check, because it teaches readers to ignore it.
    #[test]
    fn an_ordinary_entity_produces_no_observations() {
        let schema = lint_schema();
        let mut store = Store::new();
        let mut e = conformant_entity("lv", "alpha");
        // `Relationships` belongs here deliberately: it is the heading EVERY
        // real entity carries, no type declares it, and a fixture without it
        // is the fixture that lets the ordinary case look clean while a live
        // mem reports one observation per entity. It was exactly that, until
        // a grade read a real mem: 553 entities, 553 observations.
        e.raw_section_headings = vec!["Body".into(), "Notes".into(), "Relationships".into()];
        e.sections.insert("notes".into(), "plain prose\n".into());
        store.upsert(e.id.clone(), e);
        assert!(
            body_observations(&store, "lv", &schema).is_empty(),
            "declared headings, each once, the relationships block, no undeclared keys"
        );
    }

    #[test]
    fn a_repeated_undeclared_heading_claims_survival_only_for_the_first() {
        // Splitting is first-wins, so the second body is gone whatever the
        // catch-all does. Emitting `ABSORBED_SECTION` twice said "survives the
        // next write" about a body that did not (grade caveat, 2026-08-27).
        let schema = lint_schema();
        let mut store = Store::new();
        let mut e = conformant_entity("lv", "alpha");
        e.raw_section_headings = vec!["Body".into(), "Scratch".into(), "Scratch".into()];
        e.sections
            .insert("notes".into(), "## Scratch\n\nkept\n".into());
        store.upsert(e.id.clone(), e);
        let obs = body_observations(&store, "lv", &schema);
        let absorbed: Vec<_> = obs
            .iter()
            .filter(|o| o.code == "ABSORBED_SECTION")
            .collect();
        assert_eq!(
            absorbed.len(),
            1,
            "one per heading, not per occurrence: {obs:?}"
        );
        assert_eq!(absorbed[0].fate, ObservationFate::Absorbed);
        // The loss the repeat causes is still reported, on its own code.
        let repeats: Vec<_> = obs
            .iter()
            .filter(|o| o.code == "REPEATED_SECTION_HEADING")
            .collect();
        assert_eq!(repeats.len(), 1, "got: {obs:?}");
        assert_eq!(repeats[0].detail["occurrences"], 2);
    }

    #[test]
    fn the_auto_managed_relationships_block_is_never_an_observation() {
        // The generator re-emits `## Relationships` from the parsed relations
        // on every write, so it is neither absorbed nor dropped. Pinned apart
        // from the ordinary-entity test because the two fail for different
        // reasons: this one guards the exclusion, that one guards the fixture.
        let schema = lint_schema();
        let mut store = Store::new();
        let mut e = conformant_entity("lv", "alpha");
        e.raw_section_headings = vec!["Relationships".into()];
        store.upsert(e.id.clone(), e);
        assert!(
            body_observations(&store, "lv", &schema).is_empty(),
            "the relationships block is engine-owned, not undeclared content"
        );
    }

    #[test]
    fn a_heading_named_inside_prose_is_not_mistaken_for_a_kept_one() {
        // `heading_has_body` asks whether the catch-all RE-EMITTED the
        // heading line, and a substring test answers a different question:
        // prose that merely mentions the words reported the heading as
        // absorbed when the write had in fact dropped it.
        let schema = lint_schema();
        let mut store = Store::new();
        let mut e = conformant_entity("lv", "alpha");
        e.raw_section_headings = vec!["Body".into(), "Scratch".into()];
        e.sections
            .insert("notes".into(), "we discussed Scratch at length\n".into());
        store.upsert(e.id.clone(), e);
        let obs = body_observations(&store, "lv", &schema);
        let absorbed: Vec<_> = obs
            .iter()
            .filter(|o| o.code == "ABSORBED_SECTION")
            .collect();
        assert_eq!(absorbed.len(), 1, "got: {obs:?}");
        assert_eq!(
            absorbed[0].fate,
            ObservationFate::Dropped,
            "a bare heading whose text appears in prose is still dropped"
        );
    }

    #[test]
    fn an_unterminated_fence_names_the_sections_it_swallowed() {
        // Criteria 3 and 4. The `Notes` heading was masked by the open fence,
        // so it never became a section key: its bytes sit inside `body`. The
        // finding has to name it, because the only other signal the entity
        // gives is a `notes` key that is simply absent.
        let schema = lint_schema();
        let mut store = Store::new();
        let mut e = conformant_entity("lv", "alpha");
        e.sections.insert(
            "body".into(),
            "intro\n\n```rust\nfn main() {}\n\n## Notes\n\nthe real notes\n".into(),
        );
        e.sections.shift_remove("notes");
        store.upsert(e.id.clone(), e);
        let schemas = schemas_for(&[("lv", schema.clone())]);
        let findings = conformance_findings(&store, "lv", &schema, &schemas);
        let fence: Vec<_> = findings
            .iter()
            .filter(|f| f.code == "UNTERMINATED_FENCE")
            .collect();
        assert_eq!(fence.len(), 1, "got: {:?}", codes(&findings));
        assert_eq!(fence[0].id, "lv--alpha");
        assert_eq!(fence[0].detail["section"], "body");
        assert_eq!(fence[0].detail["fence"], "```");
        assert_eq!(
            fence[0].detail["swallowed_sections"],
            serde_json::json!(["Notes"]),
        );
        // Criterion 4: never clean. A finding on the conformance axis is
        // exactly what "not clean" means on this surface.
        assert!(!findings.is_empty());
    }

    #[test]
    fn an_entity_with_no_open_fence_gains_no_fence_finding() {
        // Criterion 7 at the read tier. Both the no-fence and the closed-fence
        // cases, because a guard that fires on any fence character would pass
        // the first and fail the second.
        let schema = lint_schema();
        let schemas = schemas_for(&[("lv", schema.clone())]);
        for body in [
            "just prose",
            "prose\n\n```rust\nfn main() {}\n```\n\nmore",
            "```md\n## Notes\n```",
        ] {
            let mut store = Store::new();
            let mut e = conformant_entity("lv", "alpha");
            e.sections.insert("body".into(), body.into());
            store.upsert(e.id.clone(), e);
            let findings = conformance_findings(&store, "lv", &schema, &schemas);
            assert!(
                !findings.iter().any(|f| f.code == "UNTERMINATED_FENCE"),
                "body {body:?} produced: {:?}",
                codes(&findings)
            );
        }
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
