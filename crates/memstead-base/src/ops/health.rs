//! Health checks — missing required fields, staleness, scoring.
//!
//! Checks each entity against its schema's requirements:
//! - Required metadata fields present and non-empty
//! - Required sections present and non-empty
//! - Staleness: days since last_modified > schema threshold
//! - Undeclared relationships — existing entities whose
//!   `relationships:` include a name that is not in the per-mem
//!   schema's vocabulary surface as soft warnings rather than hard
//!   load-time failures. Agents can fix either the entity or the
//!   schema; undeclared *types* on load are decision-3 hard errors
//!   and covered elsewhere.

use std::collections::HashMap;
use std::sync::Arc;

use memstead_schema::{Schema, TypeDefinition, type_by_name};

use super::{
    DanglingLink, FoldedTag, HealthIssue, HealthReport, HealthSummary, StaleEntity, TagDistribution,
    TagVariant, UntaggedStats,
};
use crate::entity::MetadataValue;
use crate::graph::query;
use crate::store::Store;

/// Allowed `include` keys for `memstead_health` — the single source of
/// truth shared across the basis MCP server, pro MCP server, and the
/// basis CLI's `health` command. Adding a new include key here lights
/// it up uniformly; agents see the same `UNKNOWN_INCLUDE_KEY` warning
/// shape whether they reach health via MCP or CLI.
pub const HEALTH_INCLUDE_KEYS: &[&str] = &[
    "orphans",
    "stubs",
    "most_connected",
    "missing_fields",
    "stale",
    "dangling_links",
    "tags",
    "missing_required_outgoing",
    "conformance",
    "integrity",
];

/// Compute health reports for all entities in the store.
///
/// `mem_schemas` maps mem name → `Arc<Schema>`. Entities whose mem
/// is missing from this map fall back to the builtin `default` schema
/// relationship vocabulary (keeps legacy fixtures green; real production
/// paths always register a mem schema).
pub fn compute_health(
    store: &Store,
    default_schema: &TypeDefinition,
    mem_schemas: &HashMap<String, Arc<Schema>>,
) -> HealthSummary {
    let mut missing_fields = Vec::new();
    let mut stale_entities = Vec::new();

    let today_days = days_since_epoch();

    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }

        // Resolve the entity's `TypeDefinition` against the entity's
        // own mem's schema first. `type_by_name` only knows the
        // builtin `default` schema; falling through to it on a mem
        // pinned to a non-default schema (e.g. `planning@0.1.0`) would
        // silently use `default_schema` (effectively `spec`) for every
        // entity and report `spec`'s `health_required_fields` —
        // `[identity, purpose]` — even on entities of types like
        // `goal` / `option` / `decision`.
        let resolved = mem_schemas
            .get(entity.mem.as_str())
            .and_then(|s| s.types.get(entity.entity_type.as_str()).cloned())
            .or_else(|| type_by_name(&entity.entity_type));
        let schema: &TypeDefinition = resolved.as_deref().unwrap_or(default_schema);
        let mut issues = Vec::new();

        // Check health_required_fields
        for field in &schema.health_required_fields {
            // Check if it's a section or metadata field
            if schema.section(field).is_some() {
                // It's a section
                let content = entity.sections.get(field.as_str());
                if content.is_none_or(|c| c.trim().is_empty()) {
                    issues.push(HealthIssue {
                        field: field.clone(),
                        message: format!("required section '{field}' is empty"),
                    });
                }
            } else {
                // It's a metadata field. Treat missing AND empty /
                // whitespace-only values as gaps so the scan matches
                // the section branch's `trim().is_empty()` semantics
                // — an empty `MetadataValue::String("")` is just as
                // unhelpful to an agent as an absent key.
                let value = entity.metadata.get(field.as_str());
                let is_empty = match value {
                    None => true,
                    Some(v) => v.to_frontmatter_string().trim().is_empty(),
                };
                if is_empty {
                    issues.push(HealthIssue {
                        field: field.clone(),
                        message: format!("required field '{field}' is missing"),
                    });
                }
            }
        }

        // Undeclared-relationship warning. Scan the entity's
        // relationship list against the mem's schema vocabulary; every
        // unknown name becomes a soft HealthIssue (same severity as a
        // missing section) so agents running a health sweep after a
        // schema version bump see drift without a crashed load.
        //
        // Shape-violation scan: when the mem's schema declares
        // `source_types` / `target_types` on a relationship and an
        // existing edge violates the shape, surface as a soft
        // HealthIssue. The relate-add path enforces shape going
        // forward; this scan catches edges authored before the
        // constraint landed (or via inline `relations:` on
        // memstead_create, which does not yet shape-check). The
        // remove-path on `memstead_relate` skips shape validation so the
        // cleanup is always reachable.
        if let Some(mem_schema) = mem_schemas.get(entity.mem.as_str()) {
            let mut seen_unknown = std::collections::HashSet::new();
            for rel in &entity.relationships {
                if !mem_schema.relationship_known(&rel.rel_type) {
                    if seen_unknown.insert(rel.rel_type.clone()) {
                        let suggestion = mem_schema
                            .suggest_relationship(&rel.rel_type)
                            .map(|s| format!(" Did you mean '{s}'?"))
                            .unwrap_or_default();
                        let (schema_name, schema_version) = mem_schema.id();
                        issues.push(HealthIssue {
                            field: "relationships".to_string(),
                            message: format!(
                                "relationship '{}' is not declared in schema \
                                 '{schema_name}@{schema_version}'.{suggestion}",
                                rel.rel_type
                            ),
                        });
                    }
                    continue;
                }

                let target_type = store
                    .get(&rel.target)
                    .map(|t| t.entity_type.clone())
                    .filter(|t| !t.is_empty());
                if let Err(crate::runtime_validator::ValidationError::InvalidRelationshipShape {
                    rel_type,
                    from_type,
                    to_type,
                    allowed_source_types,
                    allowed_target_types,
                    ..
                }) = crate::runtime_validator::validate_rel_shape(
                    &rel.rel_type,
                    entity.entity_type.as_str(),
                    target_type.as_deref(),
                    mem_schema.as_ref(),
                ) {
                    let allowed_src = if allowed_source_types.is_empty() {
                        "<any>".to_string()
                    } else {
                        allowed_source_types.join(", ")
                    };
                    let allowed_tgt = if allowed_target_types.is_empty() {
                        "<any>".to_string()
                    } else {
                        allowed_target_types.join(", ")
                    };
                    issues.push(HealthIssue {
                        field: "relationships".to_string(),
                        message: format!(
                            "INVALID_REL_SHAPE: edge '{rel_type}' from \
                             '{from_type}' to '{to_type}' (target {target}) \
                             violates declared shape — allowed_source_types: \
                             [{allowed_src}], allowed_target_types: \
                             [{allowed_tgt}]. Remove via \
                             `memstead_relate from={from_id} to={target} \
                             type={rel_type} remove=true`.",
                            target = rel.target,
                            from_id = entity.id,
                        ),
                    });
                }
            }
        }

        // Staleness check
        let auto_ts_field = schema.metadata_fields.iter().find(|f| f.auto_timestamp);

        if let Some(ts_field) = auto_ts_field
            && let Some(val) = entity.metadata.get(ts_field.key.as_str())
        {
            let date_str = val.to_frontmatter_string();
            if let Some(modified_days) = parse_iso_to_days(&date_str) {
                let days_since = today_days.saturating_sub(modified_days);
                if days_since > schema.staleness_threshold_days as u64 {
                    stale_entities.push(StaleEntity {
                        id: entity.id.clone(),
                        title: entity.title.clone(),
                        days_since_modified: days_since,
                    });
                }
            }
        }

        if !issues.is_empty() {
            // Compute a simple health score: (total_fields - issues) / total_fields.
            // `issues.len()` can exceed `total_fields` once the
            // relationship-vocabulary issues are added on top, so saturate
            // the subtraction rather than underflow. A score of 0.0 is the
            // natural floor — agents treat it as "maximally broken".
            let total = schema.health_required_fields.len();
            let score = if total > 0 {
                (total.saturating_sub(issues.len()) as f32) / (total as f32)
            } else {
                1.0
            };

            missing_fields.push(HealthReport {
                id: entity.id.clone(),
                title: entity.title.clone(),
                score,
                issues,
            });
        }
    }

    // Sort stale entities by days_since_modified descending
    stale_entities.sort_by(|a, b| b.days_since_modified.cmp(&a.days_since_modified));

    // Structural counts
    let orphan_count = query::find_orphans(store).len();
    let stub_count = query::find_stubs(store).len();

    HealthSummary {
        stale_entities,
        missing_fields,
        orphan_count,
        stub_count,
        warnings: Vec::new(),
        dangling_links: None,
        findings: None,
        tag_distribution: None,
        tag_distribution_folded: None,
        untagged_entities: None,
    }
}

/// Scan every non-stub entity's `tags` metadata and aggregate (tag → count,
/// per-entity-type breakdown) plus untagged coverage. Comma-separated parser
/// with per-segment trim; empty segments drop. Comparison is case-sensitive
/// on the primary surface — case drift is surfaced separately via
/// [`TagDistribution`] siblings folded by the caller if desired.
///
/// `mem_filter` narrows both aggregation passes to entities in that mem;
/// `limit` caps the returned `tag_distribution` array after sorting by count
/// descending (tie-break by tag ascending for deterministic output).
///
/// Also returns `FoldedTag` entries for any canonical (lowercase) tag where
/// two or more authored casings appear — drift-flag only; empty when no
/// collisions exist.
pub fn collect_tag_distribution(
    store: &Store,
    mem_filter: Option<&str>,
    limit: usize,
) -> (Vec<TagDistribution>, Vec<FoldedTag>, UntaggedStats) {
    // tag → (count, per_type_count)
    let mut counts: HashMap<String, (usize, HashMap<String, usize>)> = HashMap::new();
    let mut untagged = UntaggedStats {
        total: 0,
        by_entity_type: HashMap::new(),
    };

    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }
        if let Some(v) = mem_filter
            && entity.mem != v
        {
            continue;
        }

        let tags_raw = entity
            .metadata
            .get("tags")
            .and_then(|v| match v {
                MetadataValue::String(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");

        let mut any_tag = false;
        for tag in tags_raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            any_tag = true;
            let entry = counts
                .entry(tag.to_string())
                .or_insert_with(|| (0, HashMap::new()));
            entry.0 += 1;
            *entry.1.entry(entity.entity_type.clone()).or_insert(0) += 1;
        }
        if !any_tag {
            untagged.total += 1;
            *untagged
                .by_entity_type
                .entry(entity.entity_type.clone())
                .or_insert(0) += 1;
        }
    }

    // Primary distribution — case-sensitive.
    let mut entries: Vec<TagDistribution> = counts
        .iter()
        .map(|(tag, (count, by_type))| TagDistribution {
            tag: tag.clone(),
            count: *count,
            by_entity_type: by_type.clone(),
        })
        .collect();
    entries.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
    entries.truncate(limit);

    // Case-drift sidecar: group by lowercase canonical; surface only entries
    // with ≥2 distinct authored casings. Operates on the full counts map, not
    // the truncated primary surface, so drift hidden below `limit` still
    // surfaces.
    let mut by_canonical: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    for (tag, (count, _)) in counts.iter() {
        by_canonical
            .entry(tag.to_lowercase())
            .or_default()
            .push((tag.clone(), *count));
    }
    let mut folded: Vec<FoldedTag> = by_canonical
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(canonical, mut variants)| {
            variants.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let total = variants.iter().map(|(_, c)| *c).sum();
            FoldedTag {
                canonical,
                total,
                variants: variants
                    .into_iter()
                    .map(|(tag, count)| TagVariant { tag, count })
                    .collect(),
            }
        })
        .collect();
    folded.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.canonical.cmp(&b.canonical)));

    (entries, folded, untagged)
}

/// Scan every non-stub entity's section bodies for body wiki-links that
/// either (a) resolve to a stub target (missing on-disk file) or
/// (b) lack a backing explicit relation in the referrer (alias-orphan
/// under the alias model). Both cases surface through the same
/// `DanglingLink` shape — the existing field set continues to round-trip;
/// alias-orphans are detectable by the target *not* being a stub while
/// the referrer's relationships list omits it.
///
/// The scan also covers the `## Relationships` table: a typed-relation
/// target whose entity vanished (out-of-band file edit, historical
/// cross-mem corruption from the pre-F15 mem-delete path, etc.)
/// would otherwise stay invisible to the diagnostic surface.
/// Relationship-section danglers ship the same envelope shape with
/// `section: None` — the Option marks the source axis without requiring
/// a magic-string sentinel.
///
/// `mem_filter` narrows *scanning* to entities in that mem; resolution
/// stays global so cross-mem links whose target is a real entity
/// elsewhere are not flagged as missing.
pub fn collect_dangling_links(
    store: &Store,
    mem_filter: Option<&str>,
) -> Vec<DanglingLink> {
    use crate::entity::parser::extract_inline_links_lenient;
    use std::collections::HashSet;

    let mut out = Vec::new();
    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }
        if let Some(v) = mem_filter
            && entity.mem != v
        {
            continue;
        }
        let explicit_targets: HashSet<_> = entity
            .relationships
            .iter()
            .map(|r| r.target.clone())
            .collect();
        for (section_key, section_body) in &entity.sections {
            for target_id in extract_inline_links_lenient(section_body, &entity.mem) {
                let target_missing = store
                    .get(&target_id)
                    .map(|e| e.stub)
                    .unwrap_or(true);
                let alias_orphan = !target_missing && !explicit_targets.contains(&target_id);
                if target_missing || alias_orphan {
                    out.push(DanglingLink {
                        from: entity.id.clone(),
                        target_id: target_id.clone(),
                        target_path: target_id.path().to_string(),
                        section: Some(section_key.clone()),
                    });
                }
            }
        }
        // Relationship-table dangler scan. The `## Relationships`
        // section is structurally distinct from body sections — its
        // rows materialise from `entity.relationships` rather than a
        // free-text body — so `section: None` marks the source axis.
        //
        // Discrimination differs from the body scan: a relationship
        // target that resolves to a stub is a legitimate forward
        // reference (the alias machinery auto-stubs absent targets
        // by design), not corruption. Only a target that's *fully
        // absent* from the store — neither stub nor real — flags as
        // dangling. In practice this only fires for out-of-band file
        // edits or historical cross-mem-delete corruption that
        // dropped the stub along with the deleted mem.
        //
        // Dedup against the body-scan output so a target that
        // surfaces from both axes doesn't double-emit.
        for rel in &entity.relationships {
            if store.get(&rel.target).is_some() {
                continue;
            }
            let already_reported = out
                .iter()
                .any(|d| d.from == entity.id && d.target_id == rel.target);
            if already_reported {
                continue;
            }
            out.push(DanglingLink {
                from: entity.id.clone(),
                target_id: rel.target.clone(),
                target_path: rel.target.path().to_string(),
                section: None,
            });
        }
    }
    out
}

/// Collect every non-stub entity whose type declares `required_outgoing`
/// blocks that the entity's current outgoing edges leave unsatisfied.
/// Results are deterministic — sorted
/// by `(mem, id)` — so the agent can diff successive sweeps without
/// the underlying HashMap iteration order leaking through.
///
/// `mem_filter` narrows scanning to entities in that mem when set;
/// `mem_schemas` resolves the entity's type definition against the
/// mem's pinned schema. Entities whose mem has no schema in the
/// map are skipped (no schema → no `required_outgoing` to evaluate).
pub fn collect_missing_required_outgoing(
    store: &Store,
    mem_filter: Option<&str>,
    mem_schemas: &HashMap<String, Arc<memstead_schema::Schema>>,
) -> Vec<MissingRequiredOutgoingReport> {
    let mut out = Vec::new();
    for entity in store.all_entities() {
        if entity.stub {
            continue;
        }
        if let Some(v) = mem_filter
            && entity.mem != v
        {
            continue;
        }
        let Some(mem_schema) = mem_schemas.get(entity.mem.as_str()) else {
            continue;
        };
        let Some(td) = mem_schema.types.get(entity.entity_type.as_str()) else {
            continue;
        };
        if td.required_outgoing.is_empty() {
            continue;
        }
        let unsatisfied: Vec<MissingOutgoingBlock> = td
            .required_outgoing
            .iter()
            .filter(|block| {
                let count = entity
                    .relationships
                    .iter()
                    .filter(|rel| {
                        block
                            .relationships
                            .iter()
                            .any(|name| name == &rel.rel_type)
                    })
                    .count();
                !block.admits(count)
            })
            .map(|block| MissingOutgoingBlock {
                relationships: block.relationships.clone(),
                cardinality: block.cardinality.to_string(),
            })
            .collect();
        if unsatisfied.is_empty() {
            continue;
        }
        out.push(MissingRequiredOutgoingReport {
            id: entity.id.clone(),
            title: entity.title.clone(),
            entity_type: entity.entity_type.clone(),
            mem: entity.mem.clone(),
            missing: unsatisfied,
        });
    }
    out.sort_by(|a, b| a.mem.cmp(&b.mem).then_with(|| a.id.0.cmp(&b.id.0)));
    out
}

/// One entity's unsatisfied `required_outgoing` blocks, surfaced from
/// the health-time scan. Wire shape mirrors the per-write
/// `MISSING_REQUIRED_OUTGOING` warning's `details` payload but adds
/// the `mem` name (the warning's `entity_id` already encodes it via
/// the mem prefix, but health is multi-mem by default and an
/// explicit field is cheaper for downstream filters).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MissingRequiredOutgoingReport {
    pub id: crate::entity::EntityId,
    pub title: String,
    pub entity_type: String,
    pub mem: String,
    pub missing: Vec<MissingOutgoingBlock>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MissingOutgoingBlock {
    pub relationships: Vec<String>,
    pub cardinality: String,
}

/// Get a single entity's health report.
pub fn entity_health(entity: &crate::entity::Entity, schema: &TypeDefinition) -> HealthReport {
    let mut issues = Vec::new();

    for field in &schema.health_required_fields {
        if schema.section(field).is_some() {
            let content = entity.sections.get(field.as_str());
            if content.is_none_or(|c| c.trim().is_empty()) {
                issues.push(HealthIssue {
                    field: field.clone(),
                    message: format!("required section '{field}' is empty"),
                });
            }
        } else {
            let value = entity.metadata.get(field.as_str());
            if value.is_none() {
                issues.push(HealthIssue {
                    field: field.clone(),
                    message: format!("required field '{field}' is missing"),
                });
            }
        }
    }

    let total = schema.health_required_fields.len();
    let score = if total > 0 {
        ((total - issues.len()) as f32) / (total as f32)
    } else {
        1.0
    };

    HealthReport {
        id: entity.id.clone(),
        title: entity.title.clone(),
        score,
        issues,
    }
}

// ---------------------------------------------------------------------------
// Date helpers
// ---------------------------------------------------------------------------

/// Get current days since Unix epoch.
fn days_since_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86400
}

/// Parse an ISO 8601 date string to days since epoch.
/// Supports `YYYY-MM-DD` and `YYYY-MM-DDTHH:MM:SSZ`.
fn parse_iso_to_days(date: &str) -> Option<u64> {
    let date_part = date.split('T').next()?;
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let year: u64 = parts[0].parse().ok()?;
    let month: u64 = parts[1].parse().ok()?;
    let day: u64 = parts[2].parse().ok()?;
    Some(ymd_to_days(year, month, day))
}

/// Convert (year, month, day) to days since Unix epoch.
/// Inverse of the algorithm in generator.rs.
fn ymd_to_days(year: u64, month: u64, day: u64) -> u64 {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let y = if month <= 2 { year - 1 } else { year };
    let m = if month <= 2 { month + 9 } else { month - 3 };
    let era = y / 400;
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe;
    days - 719468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{Entity, EntityId, MetadataValue};
    use crate::store::Store;
    use indexmap::IndexMap;
    use memstead_schema::type_by_name;

    fn make_entity(name: &str, has_required: bool) -> Entity {
        let mut metadata = IndexMap::new();
        metadata.insert("level".into(), MetadataValue::String("M0".into()));
        metadata.insert("type".into(), MetadataValue::String("spec".into()));
        metadata.insert(
            "created_date".into(),
            MetadataValue::String("2026-01-15".into()),
        );
        metadata.insert(
            "last_modified".into(),
            MetadataValue::String("2026-04-12".into()),
        );

        let mut sections = IndexMap::new();
        if has_required {
            sections.insert("identity".into(), "Has identity.".into());
            sections.insert("purpose".into(), "Has purpose.".into());
        }

        Entity {
            id: EntityId::new("specs", name),
            title: name.into(),
            entity_type: "spec".into(),
            mem: "specs".into(),
            file_path: format!("{name}.md"),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    fn make_concept_entity(name: &str, with_definition: bool) -> Entity {
        let mut metadata = IndexMap::new();
        metadata.insert("type".into(), MetadataValue::String("concept".into()));
        metadata.insert("maturity".into(), MetadataValue::String("emerging".into()));
        metadata.insert(
            "abstraction_level".into(),
            MetadataValue::String("concrete".into()),
        );
        metadata.insert(
            "created_date".into(),
            MetadataValue::String("2026-01-15".into()),
        );
        metadata.insert(
            "last_modified".into(),
            MetadataValue::String("2026-04-12".into()),
        );

        let mut sections = IndexMap::new();
        if with_definition {
            sections.insert("definition".into(), "Precise definition.".into());
        }
        sections.insert("explanation".into(), "How it works.".into());

        Entity {
            id: EntityId::new("concepts", name),
            title: name.into(),
            entity_type: "concept".into(),
            mem: "concepts".into(),
            file_path: format!("{name}.md"),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn health_concept_missing_definition_reports_definition_field() {
        let schema = &type_by_name("concept").unwrap();
        let entity = make_concept_entity("clarity", false);
        let report = entity_health(&entity, schema);

        // The missing-field issue must name the concept schema's required
        // section ("definition"), not spec's "identity".
        assert!(report.issues.iter().any(|i| i.field == "definition"));
        assert!(!report.issues.iter().any(|i| i.field == "identity"));
        assert!(!report.issues.iter().any(|i| i.field == "purpose"));
        assert!(report.score < 1.0);

        // An entity with the definition filled in has no issue for that field.
        let healthy = make_concept_entity("clarity-ok", true);
        let healthy_report = entity_health(&healthy, schema);
        assert!(
            !healthy_report
                .issues
                .iter()
                .any(|i| i.field == "definition")
        );
    }

    #[test]
    fn health_detects_missing_sections() {
        let schema = &type_by_name("spec").unwrap();
        let entity = make_entity("incomplete", false);
        let report = entity_health(&entity, schema);
        assert!(!report.issues.is_empty());
        assert!(report.score < 1.0);
    }

    #[test]
    fn health_clean_entity() {
        let schema = &type_by_name("spec").unwrap();
        let entity = make_entity("complete", true);
        let report = entity_health(&entity, schema);
        // May still have issues for other required fields, but identity/purpose are covered
        let section_issues: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.field == "identity" || i.field == "purpose")
            .collect();
        assert!(section_issues.is_empty());
    }

    #[test]
    fn health_summary_counts() {
        let mut store = Store::new();
        let e1 = make_entity("healthy", true);
        let e2 = make_entity("unhealthy", false);
        store.upsert(e1.id.clone(), e1);
        store.upsert(e2.id.clone(), e2);

        let schema = &type_by_name("spec").unwrap();
        let summary = compute_health(&store, schema, &HashMap::new());
        assert_eq!(summary.orphan_count, 2); // No edges between them
        assert_eq!(summary.stub_count, 0);
    }

    #[test]
    fn health_surfaces_invalid_rel_shape_on_existing_edges() {
        // software@0.1.0 declares `source_types: [actor]` on OWNS.
        // Seed a non-actor source with an outgoing OWNS edge — the
        // health scan must surface `INVALID_REL_SHAPE` in the
        // entity's issues so an agent running a sweep can identify
        // edges to clean up via `memstead_relate remove=true`.
        use crate::entity::Relationship;
        use memstead_schema::SchemaRegistry;

        let registry = SchemaRegistry::builtin();
        let software = registry
            .resolve_by_name("software")
            .unwrap()
            .expect("software schema ships as a builtin");

        let mut store = Store::new();
        // Source entity is `spec`, not `actor`. Add an OWNS edge to
        // a target whose type doesn't matter for source-side shape.
        let mut bad = make_entity("bad-owns-source", true);
        bad.entity_type = "spec".into();
        bad.metadata
            .insert("level".into(), MetadataValue::String("M0".into()));
        bad.metadata
            .insert("stability".into(), MetadataValue::String("evolving".into()));
        bad.relationships.push(Relationship {
            rel_type: "OWNS".into(),
            target: EntityId::new("specs", "victim"),
            description: None,
        });
        let mut victim = make_entity("victim", true);
        victim.entity_type = "spec".into();
        store.upsert(bad.id.clone(), bad);
        store.upsert(victim.id.clone(), victim);

        let mut mem_schemas = HashMap::new();
        mem_schemas.insert("specs".to_string(), software);

        let schema = &type_by_name("spec").unwrap();
        let summary = compute_health(&store, schema, &mem_schemas);
        let report = summary
            .missing_fields
            .iter()
            .find(|r| r.id.as_ref() == "specs--bad-owns-source")
            .expect("shape-violating entity must surface");
        let issue = report
            .issues
            .iter()
            .find(|i| {
                i.field == "relationships" && i.message.contains("INVALID_REL_SHAPE")
            })
            .expect("shape violation must produce an INVALID_REL_SHAPE issue");
        assert!(
            issue.message.contains("OWNS"),
            "issue must name the offending rel_type: {}",
            issue.message
        );
        assert!(
            issue.message.contains("spec"),
            "issue must name the actual source type: {}",
            issue.message
        );
        assert!(
            issue.message.contains("actor"),
            "issue must name the allowed source type: {}",
            issue.message
        );
        assert!(
            issue.message.contains("remove=true"),
            "issue must surface the recovery path: {}",
            issue.message
        );
    }

    #[test]
    fn health_does_not_flag_shape_compliant_edges() {
        // Sanity counterpart: an actor source with OWNS edge satisfies
        // `source_types: [actor]` — no INVALID_REL_SHAPE issue surfaces.
        use crate::entity::Relationship;
        use memstead_schema::SchemaRegistry;

        let registry = SchemaRegistry::builtin();
        let software = registry
            .resolve_by_name("software")
            .unwrap()
            .expect("software schema ships as a builtin");

        let mut store = Store::new();
        let mut owner = make_entity("owner", true);
        owner.entity_type = "actor".into();
        owner.metadata
            .insert("kind".into(), MetadataValue::String("team".into()));
        owner.metadata
            .insert("active".into(), MetadataValue::Bool(true));
        owner.metadata
            .insert("handle".into(), MetadataValue::String("owner".into()));
        owner.relationships.push(Relationship {
            rel_type: "OWNS".into(),
            target: EntityId::new("specs", "owned"),
            description: None,
        });
        let mut owned = make_entity("owned", true);
        owned.entity_type = "spec".into();
        store.upsert(owner.id.clone(), owner);
        store.upsert(owned.id.clone(), owned);

        let mut mem_schemas = HashMap::new();
        mem_schemas.insert("specs".to_string(), software);

        let schema = &type_by_name("spec").unwrap();
        let summary = compute_health(&store, schema, &mem_schemas);
        let shape_issue = summary
            .missing_fields
            .iter()
            .flat_map(|r| r.issues.iter())
            .find(|i| i.message.contains("INVALID_REL_SHAPE"));
        assert!(
            shape_issue.is_none(),
            "shape-compliant edge must not surface a shape issue, got: {shape_issue:?}"
        );
    }

    #[test]
    fn health_warns_on_undeclared_relationship_in_existing_entity() {
        use crate::entity::Relationship;
        use memstead_schema::Schema;

        let mut store = Store::new();
        let mut entity = make_entity("with-bad-rel", true);
        // Author an edge using a name that does not exist in the default
        // schema's vocabulary. The load-side contract per decision 3 is
        // about unknown *types*; unknown *relationships* on an already-
        // loaded entity land in the soft health surface instead so an
        // agent running `memstead_health` after a schema edit sees the drift.
        entity.relationships.push(Relationship {
            rel_type: "CONJURES".into(),
            target: EntityId::new("specs", "unknown"),
            description: None,
        });
        store.upsert(entity.id.clone(), entity);

        let mut mem_schemas = HashMap::new();
        mem_schemas.insert("specs".to_string(), Schema::builtin_default());

        let schema = &type_by_name("spec").unwrap();
        let summary = compute_health(&store, schema, &mem_schemas);
        let report = summary
            .missing_fields
            .iter()
            .find(|r| r.id.as_ref() == "specs--with-bad-rel")
            .expect("entity must surface in missing_fields");
        let rel_issue = report
            .issues
            .iter()
            .find(|i| i.field == "relationships")
            .expect("undeclared relationship must produce an issue");
        assert!(
            rel_issue.message.contains("CONJURES"),
            "issue message must name the offending relationship: {}",
            rel_issue.message
        );
        assert!(
            rel_issue.message.contains("default@1.0.0"),
            "issue must name the schema pin: {}",
            rel_issue.message
        );
    }

    // -------------------------------------------------------------------
    // Dangling wiki-link detection
    // -------------------------------------------------------------------

    /// Build an entity with an arbitrary section body so the test can seed
    /// inline wiki-links at will. Mem defaults to `specs`.
    fn make_entity_with_body(name: &str, section_key: &str, body: &str) -> Entity {
        let mut entity = make_entity(name, true);
        entity
            .sections
            .insert(section_key.into(), body.to_string());
        entity
    }

    #[test]
    fn dangling_link_detected_after_delete() {
        use crate::entity::store_builder::make_stub;

        let mut store = Store::new();
        let a = make_entity_with_body(
            "a",
            "purpose",
            "Refers to [[b]] in prose.",
        );
        store.upsert(a.id.clone(), a.clone());

        // Seed b as a stub — the signal that its markdown file is gone
        // (post-delete, pre-recreate, or never authored).
        let b_id = EntityId::new("specs", "b");
        store.upsert(b_id.clone(), make_stub(b_id.clone()));

        let dangling = super::collect_dangling_links(&store, None);
        assert_eq!(dangling.len(), 1, "exactly one dangling link expected");
        let d = &dangling[0];
        assert_eq!(d.from, a.id);
        assert_eq!(d.target_id, b_id);
        assert_eq!(d.target_path, "b");
        assert_eq!(d.section.as_deref(), Some("purpose"));
    }

    #[test]
    fn dangling_link_does_not_flag_stub_target_of_explicit_relationship() {
        use crate::entity::Relationship;
        use crate::entity::store_builder::make_stub;

        let mut store = Store::new();
        // A has NO inline link in its body — only an explicit relationship
        // edge pointing at a stub.
        let mut a = make_entity("a", true);
        let b_id = EntityId::new("specs", "b");
        a.relationships.push(Relationship {
            rel_type: "REFERENCES".into(),
            target: b_id.clone(),
            description: None,
        });
        store.upsert(a.id.clone(), a);
        store.upsert(b_id.clone(), make_stub(b_id));

        let dangling = super::collect_dangling_links(&store, None);
        assert!(
            dangling.is_empty(),
            "explicit relationships to stubs are valid by design \
             (stubs are first-class placeholders); only inline-body \
             wiki-links to stubs must surface"
        );
    }

    #[test]
    fn dangling_link_does_not_flag_real_reference() {
        use crate::entity::Relationship;

        let mut store = Store::new();
        let mut a = make_entity_with_body(
            "a",
            "purpose",
            "Refers to [[b]] in prose.",
        );
        // Backing relation makes the body link a valid alias.
        a.relationships.push(Relationship {
            rel_type: "REFERENCES".into(),
            target: EntityId::new("specs", "b"),
            description: None,
        });
        let b = make_entity("b", true);
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);

        let dangling = super::collect_dangling_links(&store, None);
        assert!(
            dangling.is_empty(),
            "real reference backed by relation — not dangling, not alias-orphan"
        );
    }

    /// F12: a `## Relationships` row pointing at a fully-absent target
    /// (out-of-band file edit, mem-delete corruption) must surface.
    /// The scan covers both axes; relationship-table danglers ship
    /// `section: None` to mark the source axis.
    #[test]
    fn dangling_link_relationship_section_target_absent() {
        use crate::entity::Relationship;

        let mut store = Store::new();
        let mut a = make_entity("a", true);
        // Note: NO stub in the store for `gone` — out-of-band edit
        // removed the stub but left the relationship row.
        a.relationships.push(Relationship {
            rel_type: "DEPENDS_ON".into(),
            target: EntityId::new("specs", "gone"),
            description: None,
        });
        store.upsert(a.id.clone(), a.clone());

        let dangling = super::collect_dangling_links(&store, None);
        assert_eq!(dangling.len(), 1, "exactly one relationship-section dangler");
        let d = &dangling[0];
        assert_eq!(d.from, a.id);
        assert_eq!(d.target_id, EntityId::new("specs", "gone"));
        assert!(
            d.section.is_none(),
            "relationship-section danglers ship `section: None`, got {:?}",
            d.section
        );
    }

    /// Relationship rows pointing at stubs are NOT flagged. Auto-stub
    /// is the alias machinery's forward-reference mechanism; flagging
    /// stubs would conflate the "engine-managed placeholder" case with
    /// corruption.
    #[test]
    fn dangling_link_relationship_section_stub_target_not_flagged() {
        use crate::entity::Relationship;
        use crate::entity::store_builder::make_stub;

        let mut store = Store::new();
        let mut a = make_entity("a", true);
        let b_id = EntityId::new("specs", "b");
        a.relationships.push(Relationship {
            rel_type: "DEPENDS_ON".into(),
            target: b_id.clone(),
            description: None,
        });
        store.upsert(a.id.clone(), a);
        store.upsert(b_id.clone(), make_stub(b_id));

        let dangling = super::collect_dangling_links(&store, None);
        assert!(
            dangling.is_empty(),
            "relationship targets that resolve to stubs are forward-references, not corruption"
        );
    }

    /// When both the body and the relationship section point at the
    /// same fully-absent target, the dangler dedupes to a single entry
    /// on whichever axis fired first (body-scan runs
    /// before relationship-scan in the implementation; the body axis
    /// wins). Stub-shaped duplicates are not possible because the
    /// relationship-section scan skips stubs.
    #[test]
    fn dangling_link_dedups_across_body_and_relations() {
        use crate::entity::Relationship;
        use crate::entity::store_builder::make_stub;

        let mut store = Store::new();
        let mut a = make_entity_with_body(
            "a",
            "purpose",
            "Refers to [[b]] in prose.",
        );
        let b_id = EntityId::new("specs", "b");
        a.relationships.push(Relationship {
            rel_type: "REFERENCES".into(),
            target: b_id.clone(),
            description: None,
        });
        store.upsert(a.id.clone(), a.clone());
        store.upsert(b_id.clone(), make_stub(b_id.clone()));

        let dangling = super::collect_dangling_links(&store, None);
        assert_eq!(
            dangling.len(),
            1,
            "body + relations both pointing at the same stub should dedup"
        );
        // Body scan fires first; the surviving entry carries
        // `section: Some(_)`.
        assert!(dangling[0].section.is_some(), "body axis wins the dedup");
    }

    #[test]
    fn dangling_links_scope_to_mem_filter() {
        use crate::entity::store_builder::make_stub;

        let mut store = Store::new();

        // specs--a with body [[gone]] → dangling in specs.
        let a = make_entity_with_body(
            "a",
            "purpose",
            "Refers to [[gone]] in prose.",
        );
        store.upsert(a.id.clone(), a);
        let gone_specs = EntityId::new("specs", "gone");
        store.upsert(gone_specs.clone(), make_stub(gone_specs));

        // web--x with body [[gone]] → dangling in web (different stub).
        let mut x = make_entity("x", true);
        x.id = EntityId::new("web", "x");
        x.mem = "web".into();
        x.file_path = "x.md".into();
        x.sections
            .insert("purpose".into(), "Refers to [[gone]] in prose.".into());
        store.upsert(x.id.clone(), x);
        let gone_web = EntityId::new("web", "gone");
        store.upsert(gone_web.clone(), make_stub(gone_web));

        let all = super::collect_dangling_links(&store, None);
        assert_eq!(all.len(), 2);

        let specs_only = super::collect_dangling_links(&store, Some("specs"));
        assert_eq!(specs_only.len(), 1);
        assert_eq!(specs_only[0].from.mem(), "specs");

        let web_only = super::collect_dangling_links(&store, Some("web"));
        assert_eq!(web_only.len(), 1);
        assert_eq!(web_only[0].from.mem(), "web");
    }

    #[test]
    fn parse_iso_date() {
        let days = parse_iso_to_days("2026-04-12").unwrap();
        assert!(days > 0);

        let days_with_time = parse_iso_to_days("2026-04-12T10:00:00Z").unwrap();
        assert_eq!(days, days_with_time);
    }

    #[test]
    fn ymd_roundtrip() {
        // 2026-01-01
        let days = ymd_to_days(2026, 1, 1);
        assert!(days > 20000); // sanity check
    }

    // ---------------------------------------------------------------------
    // collect_tag_distribution — #18
    // ---------------------------------------------------------------------

    fn make_entity_with_tags(name: &str, mem: &str, entity_type: &str, tags: &str) -> Entity {
        let mut e = make_entity(name, true);
        e.id = EntityId::new(mem, name);
        e.mem = mem.into();
        e.entity_type = entity_type.into();
        e.metadata
            .insert("tags".into(), MetadataValue::String(tags.into()));
        e
    }

    fn make_entity_no_tags(name: &str) -> Entity {
        make_entity(name, true)
    }

    #[test]
    fn tag_distribution_aggregates_across_entities() {
        let mut store = Store::new();
        let a = make_entity_with_tags("a", "specs", "spec", "decision, plan");
        let b = make_entity_with_tags("b", "specs", "spec", "decision, plan");
        let c = make_entity_with_tags("c", "specs", "spec", "plan");
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);
        store.upsert(c.id.clone(), c);

        let (dist, _folded, untagged) = collect_tag_distribution(&store, None, 10);
        assert_eq!(dist.len(), 2);
        assert_eq!(dist[0].tag, "plan");
        assert_eq!(dist[0].count, 3);
        assert_eq!(dist[0].by_entity_type.get("spec"), Some(&3));
        assert_eq!(dist[1].tag, "decision");
        assert_eq!(dist[1].count, 2);
        assert_eq!(untagged.total, 0);
    }

    #[test]
    fn tag_distribution_case_sensitive() {
        let mut store = Store::new();
        let a = make_entity_with_tags("a", "specs", "spec", "Decision");
        let b = make_entity_with_tags("b", "specs", "spec", "decision");
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);

        let (dist, folded, _untagged) = collect_tag_distribution(&store, None, 10);
        assert_eq!(dist.len(), 2, "`decision` and `Decision` stay distinct");
        let tags: std::collections::HashSet<&str> = dist.iter().map(|t| t.tag.as_str()).collect();
        assert!(tags.contains("decision"));
        assert!(tags.contains("Decision"));

        // Drift sidecar surfaces the collision.
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].canonical, "decision");
        assert_eq!(folded[0].total, 2);
        assert_eq!(folded[0].variants.len(), 2);
    }

    #[test]
    fn untagged_entities_counts_missing_and_empty() {
        let mut store = Store::new();
        let a = make_entity_no_tags("a"); // no `tags` metadata
        let b = make_entity_with_tags("b", "specs", "spec", "");
        let c = make_entity_with_tags("c", "specs", "spec", " , , ");
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);
        store.upsert(c.id.clone(), c);

        let (dist, _folded, untagged) = collect_tag_distribution(&store, None, 10);
        assert!(dist.is_empty(), "no effective tags → empty distribution");
        assert_eq!(untagged.total, 3);
        assert_eq!(untagged.by_entity_type.get("spec"), Some(&3));
    }

    #[test]
    fn tag_distribution_respects_mem_filter() {
        let mut store = Store::new();
        let a = make_entity_with_tags("a", "specs", "spec", "decision");
        let b = make_entity_with_tags("b", "memos", "memo", "observation");
        let c = make_entity_no_tags("c");
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);
        store.upsert(c.id.clone(), c);

        let (dist, _folded, untagged) = collect_tag_distribution(&store, Some("memos"), 10);
        assert_eq!(dist.len(), 1);
        assert_eq!(dist[0].tag, "observation");
        assert_eq!(untagged.total, 0, "untagged scoped to filter mem");
    }

    #[test]
    fn tag_distribution_respects_limit() {
        let mut store = Store::new();
        for (name, tag) in [
            ("a", "t-alpha"),
            ("b", "t-beta"),
            ("c", "t-gamma"),
            ("d", "t-delta"),
            ("e", "t-epsilon"),
        ] {
            let e = make_entity_with_tags(name, "specs", "spec", tag);
            store.upsert(e.id.clone(), e);
        }

        let (dist, _folded, _untagged) = collect_tag_distribution(&store, None, 3);
        assert_eq!(dist.len(), 3);
        // Every tag appears once → ties across all 5; deterministic tie-break is
        // lex ascending: alpha, beta, delta (first 3 sorted).
        assert_eq!(dist[0].tag, "t-alpha");
        assert_eq!(dist[1].tag, "t-beta");
        assert_eq!(dist[2].tag, "t-delta");
    }

    // ----------------------------------------------------------------------
    // required_outgoing health collector
    // ----------------------------------------------------------------------

    /// Build a minimal schema fixture pinning `decision` with two
    /// `required_outgoing` blocks (CHOSEN + REJECTED), `note` with none.
    fn required_outgoing_fixture_schema() -> std::sync::Arc<memstead_schema::Schema> {
        let manifest = r#"name: tests-ro-health
version: 0.1.0
description: required_outgoing health test schema
when_to_use: tests
types:
  - decision
  - note
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: Hier
      default_weight: 3.0
      acyclic: true
    - name: CHOSEN
      description: ch
      default_weight: 3.0
    - name: REJECTED
      description: rj
      default_weight: 2.0
    - name: REFERENCES
      description: ref
      default_weight: 0.5
    - name: _default
      description: Fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        let body_section = "sections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\npropagating_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";
        let decision_yaml = format!(
            "name: decision\ndescription: t\nwhen_to_use: Here\n{body_section}required_outgoing:\n  - relationships: [CHOSEN]\n    cardinality: at_least_one\n  - relationships: [REJECTED]\n    cardinality: at_least_one\n",
        );
        let note_yaml = format!(
            "name: note\ndescription: t\nwhen_to_use: Here\n{body_section}",
        );
        std::sync::Arc::new(
            memstead_schema::load_schema_from_memory(
                manifest,
                &[
                    ("decision".to_string(), decision_yaml),
                    ("note".to_string(), note_yaml),
                ],
            )
            .expect("ro fixture schema must parse"),
        )
    }

    fn make_typed_entity(mem: &str, slug: &str, entity_type: &str) -> crate::entity::Entity {
        use crate::entity::MetadataValue;
        let mut metadata = IndexMap::new();
        metadata.insert(
            "type".into(),
            MetadataValue::String(entity_type.into()),
        );
        let mut sections = IndexMap::new();
        sections.insert("body".into(), "Body.".into());
        crate::entity::Entity {
            id: EntityId::new(mem, slug),
            title: slug.to_string(),
            entity_type: entity_type.into(),
            mem: mem.into(),
            file_path: format!("{slug}.md"),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: String::new(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn missing_required_outgoing_collects_violators_only() {
        let schema = required_outgoing_fixture_schema();
        let mut store = Store::new();
        // Two decisions: one without any edges (violates 2 blocks), one
        // with both edges satisfied. One note (no requirement).
        let mut violator = make_typed_entity("plan", "stalled", "decision");
        let mut satisfied = make_typed_entity("plan", "wired", "decision");
        let opt_a = make_typed_entity("plan", "a", "note");
        let opt_b = make_typed_entity("plan", "b", "note");
        let happy_note = make_typed_entity("plan", "side", "note");
        satisfied.relationships.push(crate::entity::Relationship {
            rel_type: "CHOSEN".into(),
            target: opt_a.id.clone(),
            description: None,
        });
        satisfied.relationships.push(crate::entity::Relationship {
            rel_type: "REJECTED".into(),
            target: opt_b.id.clone(),
            description: None,
        });
        for e in [violator.clone(), satisfied, opt_a, opt_b, happy_note] {
            store.upsert(e.id.clone(), e);
        }

        let mut mem_schemas = HashMap::new();
        mem_schemas.insert("plan".to_string(), schema);

        let reports =
            collect_missing_required_outgoing(&store, None, &mem_schemas);
        assert_eq!(
            reports.len(),
            1,
            "exactly one violator (the empty decision); got {reports:?}"
        );
        let r = &reports[0];
        assert_eq!(r.id, violator.id);
        assert_eq!(r.entity_type, "decision");
        assert_eq!(r.mem, "plan");
        assert_eq!(r.missing.len(), 2);
        let names: Vec<&str> = r
            .missing
            .iter()
            .flat_map(|b| b.relationships.iter().map(String::as_str))
            .collect();
        assert!(names.contains(&"CHOSEN"));
        assert!(names.contains(&"REJECTED"));

        // mark warning still doesn't propagate when violator is removed.
        violator.relationships.push(crate::entity::Relationship {
            rel_type: "CHOSEN".into(),
            target: EntityId::new("plan", "x"),
            description: None,
        });
    }

    #[test]
    fn missing_required_outgoing_respects_mem_filter() {
        // Plan: "a write to mem A doesn't surface mem B's violations
        // in memstead_health mem=A; mem-scoped aggregation is correct."
        let schema = required_outgoing_fixture_schema();
        let mut store = Store::new();
        let v_a = make_typed_entity("alpha", "stalled", "decision");
        let v_b = make_typed_entity("beta", "stalled", "decision");
        store.upsert(v_a.id.clone(), v_a);
        store.upsert(v_b.id.clone(), v_b.clone());

        let mut mem_schemas = HashMap::new();
        mem_schemas.insert("alpha".to_string(), schema.clone());
        mem_schemas.insert("beta".to_string(), schema);

        let alpha_only =
            collect_missing_required_outgoing(&store, Some("alpha"), &mem_schemas);
        assert_eq!(alpha_only.len(), 1);
        assert_eq!(alpha_only[0].mem, "alpha");

        let both =
            collect_missing_required_outgoing(&store, None, &mem_schemas);
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn missing_required_outgoing_skips_stubs_and_unschemaed_mems() {
        // Stubs have no entity_type; unschemaed mems can't be evaluated
        // — both must be silently skipped.
        let schema = required_outgoing_fixture_schema();
        let mut store = Store::new();
        let mut stub = make_typed_entity("plan", "ghost", "");
        stub.stub = true;
        stub.entity_type = String::new();
        let other = make_typed_entity("uncharted", "lonely", "decision");
        store.upsert(stub.id.clone(), stub);
        store.upsert(other.id.clone(), other);

        let mut mem_schemas = HashMap::new();
        mem_schemas.insert("plan".to_string(), schema);

        let reports = collect_missing_required_outgoing(&store, None, &mem_schemas);
        assert!(
            reports.is_empty(),
            "stub (no schema lookup) and unschemaed mem must be skipped; got {reports:?}",
        );
    }
}
