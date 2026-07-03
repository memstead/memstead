//! Graph construction + community detection for the validator.
//!
//! Builds a `Store` from validated entities using the same stub +
//! edge rules as the runtime loader (`entity::store_builder`), then
//! runs Louvain with a fixed seed. Defence-in-depth cross-mem guard
//! rejects any relationship whose target lives in another mem
//! (structurally impossible inside a single archive because
//! `parse_markdown` tags every relationship with the archive's single
//! mem, but the guard stays to catch engine refactors).

use std::sync::Arc;

use memstead_schema::{TypeDefinition, type_by_name};

use super::ValidationError;
use crate::entity::ParseResult;
use crate::entity::store_builder::push_entities_into_store;
use crate::graph::{LouvainOutput, community::detect_communities};
use crate::store::Store;

/// Fixed seed for Louvain — published once so both the validator and
/// any downstream reproducibility checker can assert on the same
/// value. Value `1` is arbitrary but pinned.
pub const VALIDATOR_LOUVAIN_SEED: u32 = 1;

/// Default resolution parameter for Louvain at ingress. Matches the
/// `community.resolution` default used by every shipped schema so
/// validator-built stores land at the same modularity the runtime
/// would produce for the same bytes.
pub const VALIDATOR_RESOLUTION: f64 = 1.0;

/// One relationship whose target lives in a different mem than the
/// one being validated/exported — i.e. an edge that cannot travel
/// inside a single-mem archive. `install` refuses on these
/// (`ARCHIVE_VALIDATION_FAILED`); `export` warns on them
/// (`DANGLING_CROSS_MEM_EDGE_IN_EXPORT`) so the operator sees the
/// install-time failure before sharing — one predicate, two postures.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DanglingCrossMemEdge {
    /// Archive-relative path of the entity carrying the edge.
    pub entity_path: String,
    /// Fully-qualified target id (e.g. `other-mem--thing`).
    pub target_id: String,
    /// The target's mem (the mem that won't travel in this archive).
    pub target_mem: String,
}

/// Result of running graph checks — Store + communities, ready for
/// downstream stats + canonical re-pack.
#[derive(Debug)]
pub struct GraphCheckResult {
    pub store: Store,
    pub communities: LouvainOutput,
    /// Relationships whose target lives outside the validated mem.
    /// Empty for a self-contained archive. In `cross_mem_as_error`
    /// mode `build_and_check` returns `Err` on the first such edge and
    /// this never carries entries; in lenient mode every offending edge
    /// is collected here for the caller to surface as a warning.
    pub dangling_cross_mem_edges: Vec<DanglingCrossMemEdge>,
}

/// Build a `Store` from parse results and run community detection.
/// Rejects any parse-result relationship whose target lives in a
/// different mem (defense-in-depth: `parse_markdown` already tags
/// every relationship with the entity's own mem via
/// `wiki_link_to_id`, so this should never fire on a well-formed
/// archive — but any refactor that weakens that invariant will be
/// caught here instead of silently diverging from runtime semantics).
pub fn build_and_check(
    parse_results: Vec<ParseResult>,
    fallback_schema: &TypeDefinition,
    mem_name: &str,
    cross_mem_as_error: bool,
) -> Result<GraphCheckResult, ValidationError> {
    // One predicate (`rel.target.mem() != mem_name`), two postures.
    // `install` / archive-load pass `cross_mem_as_error: true` and
    // refuse on the first
    // offending edge — a cross-mem edge can't resolve inside a
    // single-mem archive. `export` passes `false` and collects every
    // offending edge so it can warn (`DANGLING_CROSS_MEM_EDGE_IN_EXPORT`)
    // without blocking the snapshot. Same condition either way, so the
    // two surfaces can't drift.
    if cross_mem_as_error
        && let Some(pr) = parse_results.iter().find(|pr| {
            pr.entity
                .relationships
                .iter()
                .any(|r| r.target.mem() != mem_name)
        })
    {
        let rel = pr
            .entity
            .relationships
            .iter()
            .find(|r| r.target.mem() != mem_name)
            .expect("find guaranteed a match");
        return Err(ValidationError::CrossMemRelationship {
            path: pr.entity.file_path.clone(),
            target: rel.target.as_ref().to_string(),
        });
    }
    let dangling_cross_mem_edges = dangling_cross_mem_edges_in(&parse_results, mem_name);

    let mut store = Store::new();
    // Validator operates on isolated input; no roster, no drift detection —
    // nested-prefix warnings would be noise in a schema-check run.
    push_entities_into_store(&mut store, parse_results, fallback_schema, None);

    let communities = detect_communities(
        &store,
        VALIDATOR_RESOLUTION,
        VALIDATOR_LOUVAIN_SEED,
        |rel_type| {
            // Weight by the fallback schema's edge_weight lookup —
            // every entity in this archive passed strict validation
            // against a schema resolvable by type_by_name, so the
            // weights that land in Louvain match what the runtime
            // would use for the same bytes.
            fallback_schema.edge_weight(rel_type) as f64
        },
    );

    Ok(GraphCheckResult {
        store,
        communities,
        dangling_cross_mem_edges,
    })
}

/// The shared cross-mem predicate: every relationship in
/// `parse_results` whose target lives in a mem other than
/// `mem_name`. `build_and_check` (install/load) refuses on the first;
/// the export side warns on all of them — both go through this one
/// function so the surfaces can't drift.
pub fn dangling_cross_mem_edges_in(
    parse_results: &[ParseResult],
    mem_name: &str,
) -> Vec<DanglingCrossMemEdge> {
    let mut edges = Vec::new();
    for pr in parse_results {
        for rel in &pr.entity.relationships {
            if rel.target.mem() != mem_name {
                edges.push(DanglingCrossMemEdge {
                    entity_path: pr.entity.file_path.clone(),
                    target_id: rel.target.as_ref().to_string(),
                    target_mem: rel.target.mem().to_string(),
                });
            }
        }
    }
    edges
}

/// Resolve the fallback type for Store construction given a config.
/// Picks the first entry of `config.types` and falls back to the
/// engine-wide fallback if unresolvable — the config checker has
/// already confirmed every listed type resolves post-validation.
pub fn resolve_fallback_type(config_types: Option<&[String]>) -> Arc<TypeDefinition> {
    if let Some(name) = config_types.and_then(|v| v.first())
        && let Some(s) = type_by_name(name)
    {
        return s;
    }
    crate::engine_fallback_type()
}

/// Walk the store and count entities and all out-edges. Used for
/// `MemStats` post-validation.
pub fn tally(store: &Store) -> (usize, usize) {
    let entity_count = store.len();
    let edge_count: usize = store.all_ids().map(|id| store.outgoing(id).len()).sum();
    (entity_count, edge_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::id::file_path_to_id;
    use crate::entity::parser::parse_markdown;
    use memstead_schema::type_by_name;

    const MINIMAL_SPEC: &str = "\
---
type: spec
created_date: 2026-01-15
last_modified: 2026-01-15
level: M0
---
# Alpha

## Identity

A

## Purpose

B

## Specifies

C

## Constraints

D

## Rationale

E

## Relationships

- **USES**: [[beta]]
";

    fn spec_type() -> Arc<TypeDefinition> {
        type_by_name("spec").unwrap()
    }

    fn parse(path: &str, mem: &str, content: &str) -> ParseResult {
        parse_markdown(content, path, &spec_type(), mem).unwrap()
    }

    #[test]
    fn accepts_single_entity_archive() {
        let result = build_and_check(
            vec![parse("alpha.md", "v", MINIMAL_SPEC)],
            &spec_type(),
            "v",
            true,
        )
        .unwrap();
        assert!(result.store.contains(&file_path_to_id("alpha.md", "v")));
    }

    #[test]
    fn materializes_stub_for_unresolved_wiki_link() {
        let result = build_and_check(
            vec![parse("alpha.md", "v", MINIMAL_SPEC)],
            &spec_type(),
            "v",
            true,
        )
        .unwrap();
        let stub_id = file_path_to_id("beta", "v");
        let stub = result.store.get(&stub_id).expect("stub materialized");
        assert!(stub.stub);
    }

    #[test]
    fn empty_archive_yields_zero_communities() {
        let result = build_and_check(vec![], &spec_type(), "v", true).unwrap();
        assert_eq!(result.communities.count, 0);
    }

    #[test]
    fn rejects_cross_mem_relationship() {
        // Craft a parse result with a handcrafted cross-mem
        // relationship. `parse_markdown` won't produce this (it always
        // tags rels with the entity's mem), so build it by hand.
        let cross_mem_pr = || {
            let mut pr = parse("alpha.md", "v", MINIMAL_SPEC);
            pr.entity.relationships.push(crate::entity::Relationship {
                rel_type: "DEPENDS_ON".to_string(),
                target: crate::entity::EntityId("other-mem--thing".to_string()),
                description: None,
            });
            pr
        };
        let err = build_and_check(vec![cross_mem_pr()], &spec_type(), "v", true).unwrap_err();
        assert!(matches!(err, ValidationError::CrossMemRelationship { .. }));

        // Lenient mode: the same cross-mem edge is collected as data
        // (no error), so the export side can warn without blocking.
        let result = build_and_check(vec![cross_mem_pr()], &spec_type(), "v", false).unwrap();
        assert_eq!(result.dangling_cross_mem_edges.len(), 1);
        let edge = &result.dangling_cross_mem_edges[0];
        assert_eq!(edge.target_id, "other-mem--thing");
        assert_eq!(edge.target_mem, "other-mem");
    }

    #[test]
    fn resolve_fallback_type_picks_first_entry() {
        let s = resolve_fallback_type(Some(&["concept".to_string(), "spec".to_string()]));
        assert_eq!(s.name.as_str(), "concept");
    }

    #[test]
    fn resolve_fallback_type_falls_back_on_unknown() {
        let s = resolve_fallback_type(Some(&["bogus".to_string()]));
        // Engine-wide fallback is `spec`.
        assert_eq!(s.name.as_str(), "spec");
    }

    #[test]
    fn resolve_fallback_type_falls_back_on_empty() {
        let s = resolve_fallback_type(None);
        assert_eq!(s.name.as_str(), "spec");
    }
}
