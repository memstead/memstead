#![cfg(test)]

use super::*;
use crate::entity::{Entity, EntityId, MetadataValue};
use crate::search_index::MemIndex;
use crate::store::Store;
use indexmap::IndexMap;
use memstead_schema::{Schema, type_by_name};

fn make_entity(name: &str, mem: &str) -> Entity {
    let mut metadata = IndexMap::new();
    metadata.insert("level".into(), MetadataValue::String("M0".into()));
    metadata.insert("type".into(), MetadataValue::String("spec".into()));
    metadata.insert("tags".into(), MetadataValue::String("backend, api".into()));

    let mut sections = IndexMap::new();
    sections.insert("identity".into(), format!("Identity of {name}."));
    sections.insert("purpose".into(), format!("Purpose of {name}."));

    Entity {
        id: EntityId::new(mem, name),
        title: name.to_string(),
        entity_type: "spec".into(),
        mem: mem.into(),
        file_path: format!("{name}.md"),
        metadata,
        sections,
        relationships: Vec::new(),
        content_hash: "abc123".into(),
        stub: false,
        stub_kind: None,
        heading_spans: std::collections::HashMap::new(),
        raw_section_headings: Vec::new(),
    }
}

/// Build per-mem tantivy indexes from a store's contents. Used by the
/// unit tests since the search path now goes through tantivy.
fn build_test_indexes(store: &Store) -> (HashMap<String, MemIndex>, HashMap<String, Arc<Schema>>) {
    let schema = Schema::builtin_default();
    let mut indexes = HashMap::new();
    let mut schemas = HashMap::new();
    let mems: HashSet<String> = store
        .all_entities()
        .filter(|e| !e.stub)
        .map(|e| e.mem.clone())
        .collect();
    for mem in mems {
        let mut idx = MemIndex::build_in_ram(mem.clone(), Some(&schema)).unwrap();
        for e in store.all_entities().filter(|e| e.mem == mem) {
            idx.index_entity(e).unwrap();
        }
        idx.commit().unwrap();
        indexes.insert(mem.clone(), idx);
        schemas.insert(mem, schema.clone());
    }
    (indexes, schemas)
}

fn run_search(store: &Store, scope: &SearchScope) -> SearchResult {
    let (indexes, schemas) = build_test_indexes(store);
    let schema = type_by_name("spec").unwrap();
    search(store, scope, &schema, &indexes, &schemas)
}

/// The direction selector threads from `SearchScope` into BOTH
/// walkers: `related_to` membership narrows per direction with the
/// per-hop transitive-closure semantics, expanded hits report the
/// traversal direction, and the default (`both`) returns the
/// historical undirected set.
#[test]
fn search_direction_narrows_related_to_and_expansion() {
    // x --USES--> seed --USES--> y --USES--> z
    let mut store = Store::new();
    for n in ["x", "seed", "y", "z"] {
        let e = make_entity(n, "specs");
        store.upsert(e.id.clone(), e);
    }
    let id = |n: &str| EntityId(format!("specs--{n}"));
    let mut edge = |f: &str, t: &str| {
        store.add_edge(
            id(f),
            crate::store::Edge {
                rel_type: "USES".into(),
                target: id(t),
                source: crate::store::EdgeSource::Explicit,
            },
        )
    };
    edge("x", "seed");
    edge("seed", "y");
    edge("y", "z");

    let titles = |r: &SearchResult| {
        let mut v: Vec<String> = r.hits.iter().map(|h| h.title.clone()).collect();
        v.sort();
        v
    };

    // related_to: both (the default) = undirected; out/in narrow.
    let base = SearchScope {
        related_to: Some(id("seed")),
        depth: Some(5),
        ..Default::default()
    };
    assert_eq!(titles(&run_search(&store, &base)), ["seed", "x", "y", "z"]);
    let out_scope = SearchScope {
        direction: crate::graph::query::TraversalDirection::Out,
        ..base.clone()
    };
    assert_eq!(
        titles(&run_search(&store, &out_scope)),
        ["seed", "y", "z"],
        "out = transitive descendants only, at every hop"
    );
    let in_scope = SearchScope {
        direction: crate::graph::query::TraversalDirection::In,
        ..base
    };
    assert_eq!(
        titles(&run_search(&store, &in_scope)),
        ["seed", "x"],
        "in = transitive ancestors only"
    );

    // expand_via: primary hit is `seed`; `out` expands to y and z
    // (via_direction Out at each), `in` expands to x only.
    let expand_base = SearchScope {
        query: Some(Query {
            any: vec!["seed".into()],
            ..Default::default()
        }),
        expand_via: Some(vec!["USES".into()]),
        expand_depth: Some(3),
        ..Default::default()
    };
    let out_result = run_search(
        &store,
        &SearchScope {
            direction: crate::graph::query::TraversalDirection::Out,
            ..expand_base.clone()
        },
    );
    let expanded: Vec<(String, String)> = out_result
        .hits
        .iter()
        .filter_map(|h| {
            h.expansion.as_ref().map(|e| {
                (
                    h.title.clone(),
                    serde_json::to_value(e.via_direction)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string(),
                )
            })
        })
        .collect();
    let mut expanded_sorted = expanded.clone();
    expanded_sorted.sort();
    assert_eq!(
        expanded_sorted,
        [
            ("y".to_string(), "out".to_string()),
            ("z".to_string(), "out".to_string())
        ],
        "out-expansion reaches descendants only and reports the direction"
    );
    let in_result = run_search(
        &store,
        &SearchScope {
            direction: crate::graph::query::TraversalDirection::In,
            ..expand_base
        },
    );
    let expanded_in: Vec<String> = in_result
        .hits
        .iter()
        .filter(|h| h.expansion.is_some())
        .map(|h| h.title.clone())
        .collect();
    assert_eq!(expanded_in, ["x"], "in-expansion reaches ancestors only");
}

/// Plan 08 (metadata searchability): a value that exists only in an
/// entity's metadata — declared filterable or not, declared at all
/// or not — is returned by a free-text search; a metadata KEY finds
/// its carriers; the hit is identifiable as a metadata match; a
/// value that exists nowhere still returns zero; and where a term
/// lives in both prose and metadata, the prose hit stays and ranks
/// above the metadata-only hit (below-prose weight).
#[test]
fn search_finds_metadata_values_and_keys() {
    let mut store = Store::new();
    // `carrier` holds the identifier-shaped value in an UNDECLARED
    // metadata field (the default schema declares no `aktenzeichen`).
    let mut carrier = make_entity("carrier", "specs");
    carrier.metadata.insert(
        "aktenzeichen".into(),
        MetadataValue::String("20/54/033".into()),
    );
    store.upsert(carrier.id.clone(), carrier);
    // `prose` carries the shared term in its prose only.
    let mut prose = make_entity("prose", "specs");
    prose.sections.insert(
        "identity".into(),
        "shared-token lives in prose here.".into(),
    );
    store.upsert(prose.id.clone(), prose);
    // `meta-only` carries the shared term in metadata only.
    let mut meta_only = make_entity("meta-only", "specs");
    meta_only.metadata.insert(
        "note".into(),
        MetadataValue::String("shared-token via metadata".into()),
    );
    store.upsert(meta_only.id.clone(), meta_only);

    let q = |term: &str| SearchScope {
        query: Some(Query {
            any: vec![term.into()],
            ..Default::default()
        }),
        ..Default::default()
    };

    // The motivating case: the identifier-shaped value is found.
    let result = run_search(&store, &q("20/54/033"));
    assert_eq!(result.hits.len(), 1, "identifier found: {result:?}");
    assert_eq!(result.hits[0].title, "carrier");
    // …and the hit is identifiable as a metadata match.
    let matched = result.hits[0]
        .matched_terms
        .as_ref()
        .expect("matched_terms present");
    assert!(
        matched.values().flatten().any(|tm| tm.field == "metadata"),
        "metadata-only hit reports field \"metadata\": {matched:?}"
    );

    // The KEY finds its carrier too.
    let result = run_search(&store, &q("aktenzeichen"));
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].title, "carrier");

    // A value that exists nowhere returns zero — no spurious matches.
    let result = run_search(&store, &q("99/99/999"));
    assert!(result.hits.is_empty(), "{result:?}");

    // Shared term: the prose hit stays present and ranks above the
    // metadata-only hit; the metadata hit is ADDED, nothing dropped.
    let result = run_search(&store, &q("shared-token"));
    let titles: Vec<&str> = result.hits.iter().map(|h| h.title.as_str()).collect();
    assert!(
        titles.contains(&"prose") && titles.contains(&"meta-only"),
        "{titles:?}"
    );
    let prose_pos = titles.iter().position(|t| *t == "prose").unwrap();
    let meta_pos = titles.iter().position(|t| *t == "meta-only").unwrap();
    assert!(
        prose_pos < meta_pos,
        "prose match ranks above the metadata-only match: {titles:?}"
    );
}

/// The metadata field is ADDITIVE only: an `--exclude` term that
/// exists solely in an entity's metadata must NOT drop that entity
/// from a prose query's results — exclusion consults prose fields
/// only, so the pre-metadata-field result set never shrinks.
/// (Grader counterexample from the plan-08 gate.)
#[test]
fn search_exclude_ignores_metadata_only_tokens() {
    let mut store = Store::new();
    let mut gamma = make_entity("gamma", "specs");
    gamma
        .sections
        .insert("identity".into(), "graphword appears here.".into());
    gamma.metadata.insert(
        "status_note".into(),
        MetadataValue::String("draftword".into()),
    );
    store.upsert(gamma.id.clone(), gamma);
    let mut delta = make_entity("delta", "specs");
    delta
        .sections
        .insert("identity".into(), "graphword also here.".into());
    store.upsert(delta.id.clone(), delta);

    let result = run_search(
        &store,
        &SearchScope {
            query: Some(Query {
                any: vec!["graphword".into()],
                not: vec!["draftword".into()],
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let mut titles: Vec<&str> = result.hits.iter().map(|h| h.title.as_str()).collect();
    titles.sort();
    assert_eq!(
        titles,
        ["delta", "gamma"],
        "a metadata-only token must not exclude gamma"
    );

    // Complement: the same token in PROSE still excludes.
    let mut store2 = Store::new();
    let mut eps = make_entity("eps", "specs");
    eps.sections.insert(
        "identity".into(),
        "graphword and draftword in prose.".into(),
    );
    store2.upsert(eps.id.clone(), eps);
    let result = run_search(
        &store2,
        &SearchScope {
            query: Some(Query {
                any: vec!["graphword".into()],
                not: vec!["draftword".into()],
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    assert!(
        result.hits.is_empty(),
        "prose exclusion unchanged: {result:?}"
    );
}

#[test]
fn search_by_title() {
    let mut store = Store::new();
    let e1 = make_entity("graph-engine", "specs");
    let e2 = make_entity("mcp-server", "specs");
    store.upsert(e1.id.clone(), e1);
    store.upsert(e2.id.clone(), e2);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["graph".into()],
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    assert_eq!(result.hits[0].id.name(), "graph-engine");
}

#[test]
fn search_by_section_content() {
    let mut store = Store::new();
    let mut e = make_entity("test-entity", "specs");
    e.sections.insert(
        "identity".into(),
        "Uses the graph database for queries.".into(),
    );
    store.upsert(e.id.clone(), e);

    let scope = SearchScope {
        query: Some(Query {
            phrase: Some("graph database".into()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
}

#[test]
fn search_with_mem_filter() {
    let mut store = Store::new();
    store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
    store.upsert(EntityId::new("memos", "b"), make_entity("b", "memos"));

    let scope = SearchScope {
        mem: Some("specs".into()),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    assert_eq!(result.hits[0].mem, "specs");
}

#[test]
fn search_with_equality_filter() {
    let mut store = Store::new();
    let mut e1 = make_entity("m0-entity", "specs");
    e1.metadata
        .insert("level".into(), MetadataValue::String("M0".into()));
    let mut e2 = make_entity("m1-entity", "specs");
    e2.metadata
        .insert("level".into(), MetadataValue::String("M1".into()));
    store.upsert(e1.id.clone(), e1);
    store.upsert(e2.id.clone(), e2);

    let scope = SearchScope {
        filters: HashMap::from([("level".into(), "M0".into())]),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    assert_eq!(result.hits[0].id.name(), "m0-entity");
    assert!(result.warnings.is_empty(), "no warnings for valid filter");
}

#[test]
fn search_unknown_filter_key_warns_and_keeps_hits() {
    let mut store = Store::new();
    let e1 = make_entity("m0-entity", "specs");
    let e2 = make_entity("m1-entity", "specs");
    store.upsert(e1.id.clone(), e1);
    store.upsert(e2.id.clone(), e2);

    let scope = SearchScope {
        filters: HashMap::from([("stauts".into(), "active".into())]),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(
        result.total, 2,
        "unknown filter should be skipped, not reject all entities"
    );
    assert_eq!(result.warnings.len(), 1);
    assert!(
        result.warnings[0].to_string().contains("stauts")
            && result.warnings[0].to_string().contains("unknown"),
        "warning mentions unknown key: {:?}",
        result.warnings
    );
}

/// F7: a search scoped to entity_type=T with an unknown filter
/// key must name `T` in the warning, not the schema's default
/// type. Pre-fix the warning generator used the resolved
/// `filter_schema.name` (the default type when `T` doesn't
/// resolve), which read as if the search had been scoped to that
/// unrelated type and cost an agent round-trip while they
/// figured out the mismatch.
#[test]
fn search_unknown_filter_key_names_scoped_entity_type() {
    let mut store = Store::new();
    let e = make_entity("only", "specs");
    store.upsert(e.id.clone(), e);

    let scope = SearchScope {
        entity_type: Some("contract".into()),
        filters: HashMap::from([("confidence".into(), "verified".into())]),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    let warning = result.warnings[0].to_string();
    assert!(
        warning.contains("'contract'"),
        "warning must name the agent's scoped type: {warning}",
    );
    assert!(
        !warning.contains("'spec'"),
        "warning must not name an unrelated default type: {warning}",
    );
}

/// F7: when the caller did NOT scope the search to any
/// entity_type, the warning must omit the "for type 'X'" clause
/// rather than name the schema's default type — the user didn't
/// ask about any specific type, so naming one in the warning is
/// misleading.
#[test]
fn search_unknown_filter_key_omits_type_when_no_scope() {
    let mut store = Store::new();
    let e = make_entity("only", "specs");
    store.upsert(e.id.clone(), e);

    let scope = SearchScope {
        filters: HashMap::from([("confidence".into(), "verified".into())]),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    let warning = result.warnings[0].to_string();
    assert!(
        warning.contains("confidence"),
        "warning must name the unknown key: {warning}",
    );
    assert!(
        !warning.contains("for type"),
        "warning must omit the type-name clause when caller didn't scope: {warning}",
    );
}

/// F7 (range sibling): the range-filter warning has the same
/// scoped-type contract as its equality cousin.
#[test]
fn search_unknown_range_filter_names_scoped_entity_type() {
    let mut store = Store::new();
    let e = make_entity("only", "specs");
    store.upsert(e.id.clone(), e);

    let scope = SearchScope {
        entity_type: Some("contract".into()),
        range_filters: HashMap::from([("min_priority".into(), "0".into())]),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    let warning = result.warnings[0].to_string();
    assert!(
        warning.contains("'contract'"),
        "range warning must name the agent's scoped type: {warning}",
    );
    assert!(
        !warning.contains("'spec'"),
        "range warning must not name an unrelated default type: {warning}",
    );
}

/// Strict semantics — a filter on `level` (declared by `spec`)
/// excludes entities whose type doesn't declare the field. A
/// non-narrowing variant would pass all entities through and the
/// result would lie about what matched.
#[test]
fn search_equality_filter_excludes_types_without_declared_field() {
    let mut store = Store::new();
    // Spec entity with the filter field set — must match.
    let mut spec_match = make_entity("level-m0", "specs");
    spec_match
        .metadata
        .insert("level".into(), MetadataValue::String("M0".into()));
    // Spec entity with the field set to a different value — must
    // be excluded by the value check.
    let mut spec_other = make_entity("level-m1", "specs");
    spec_other
        .metadata
        .insert("level".into(), MetadataValue::String("M1".into()));
    // Memo-typed entity that doesn't declare `level`. It's excluded
    // because the workspace-wide schema knows `level`.
    let mut memo = make_entity("memo-no-level", "specs");
    memo.entity_type = "memo".into();
    store.upsert(spec_match.id.clone(), spec_match.clone());
    store.upsert(spec_other.id.clone(), spec_other);
    store.upsert(memo.id.clone(), memo);

    let scope = SearchScope {
        filters: HashMap::from([("level".into(), "M0".into())]),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(
        result.total,
        1,
        "strict filter must keep only the matching spec entity; got {:?}",
        result
            .hits
            .iter()
            .map(|h| h.id.to_string())
            .collect::<Vec<_>>(),
    );
    assert_eq!(result.hits[0].id, spec_match.id);
}

/// A workspace-wide-unknown filter key continues to warn and pass
/// through (no result collapse on a single typo). Companion to the
/// type-aware exclusion test
/// above — exercises the unknown-key fallback gate inside
/// `classify_filter_field` (the `Unknown` verdict passes through).
#[test]
fn search_workspace_wide_unknown_filter_passes_through() {
    let mut store = Store::new();
    let e = make_entity("only", "specs");
    store.upsert(e.id.clone(), e);

    let scope = SearchScope {
        filters: HashMap::from([("definitely-not-a-real-field".into(), "x".into())]),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(
        result.total,
        1,
        "unknown-anywhere filter key must not collapse the result set; got {:?}",
        result
            .hits
            .iter()
            .map(|h| h.id.to_string())
            .collect::<Vec<_>>(),
    );
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.to_string().contains("definitely-not-a-real-field")),
        "unknown-key warning must still surface: {:?}",
        result.warnings,
    );
}

#[test]
fn search_non_filterable_field_ignored_returns_unfiltered() {
    // MCP F2: a filter on a field declared but marked
    // `Filterable::None` (here, the
    // universal `type` base field) is truly ignored — the result
    // set equals the same search without the filter, NOT an empty
    // set. Pre-fix this branch `return false`d and emptied the set
    // under a "filter ignored" banner; the warning's word and the
    // behaviour disagreed. The `FIELD_NOT_FILTERABLE` warning still
    // fires so the agent knows the filter had no effect.
    let mut store = Store::new();
    store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
    store.upsert(EntityId::new("specs", "b"), make_entity("b", "specs"));

    let scope = SearchScope {
        filters: HashMap::from([("type".into(), "totally-different".into())]),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(
        result.total, 2,
        "non-filterable field filter must be ignored — result equals the unfiltered search, not emptied",
    );
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(
        result.warnings[0].code(),
        "FIELD_NOT_FILTERABLE",
        "non-filterable field must still warn so the agent knows the filter had no effect: {:?}",
        result.warnings,
    );
}

/// A search scoped to a type with a filter on a field that type
/// declares but marks
/// non-filterable returns the SAME hits as the same search without
/// the filter — "ignored" means unfiltered, not emptied — plus a
/// `FIELD_NOT_FILTERABLE` warning. The code/effect is coherent in
/// the scoped shape just as in the unscoped shape above.
#[test]
fn search_scoped_non_filterable_field_matches_unfiltered() {
    let mut store = Store::new();
    store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
    store.upsert(EntityId::new("specs", "b"), make_entity("b", "specs"));

    let baseline = run_search(
        &store,
        &SearchScope {
            entity_type: Some("spec".into()),
            ..Default::default()
        },
    );
    let filtered = run_search(
        &store,
        &SearchScope {
            entity_type: Some("spec".into()),
            filters: HashMap::from([("type".into(), "irrelevant".into())]),
            ..Default::default()
        },
    );
    assert_eq!(
        filtered.total, baseline.total,
        "non-filterable filter must leave the scoped result set identical to the unfiltered search",
    );
    assert_eq!(filtered.total, 2);
    assert!(
        filtered
            .warnings
            .iter()
            .any(|w| w.code() == "FIELD_NOT_FILTERABLE"),
        "scoped non-filterable filter must warn FIELD_NOT_FILTERABLE: {:?}",
        filtered.warnings,
    );
}

/// An unscoped filter on a field that IS filterable on some type
/// (here `maturity` on
/// `concept`) narrows the result to the declaring type and carries
/// `FILTER_TYPE_SCOPED` — a code distinct from the truly-unknown-key
/// code, so a consumer branching on `code` alone learns the filter
/// took effect.
#[test]
fn search_unscoped_filterable_field_narrows_with_distinct_code() {
    let mut store = Store::new();
    // Two concept entities, one matching the filter value.
    let mut c_match = make_entity("c-emerging", "specs");
    c_match.entity_type = "concept".into();
    c_match
        .metadata
        .insert("maturity".into(), MetadataValue::String("emerging".into()));
    let mut c_other = make_entity("c-stable", "specs");
    c_other.entity_type = "concept".into();
    c_other
        .metadata
        .insert("maturity".into(), MetadataValue::String("stable".into()));
    // A spec entity that doesn't declare `maturity` — narrowed away.
    let spec = make_entity("s", "specs");
    store.upsert(c_match.id.clone(), c_match.clone());
    store.upsert(c_other.id.clone(), c_other);
    store.upsert(spec.id.clone(), spec);

    let result = run_search(
        &store,
        &SearchScope {
            filters: HashMap::from([("maturity".into(), "emerging".into())]),
            ..Default::default()
        },
    );
    assert_eq!(
        result.total, 1,
        "only the matching concept survives the narrowing"
    );
    assert_eq!(result.hits[0].id, c_match.id);
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    assert_eq!(
        result.warnings[0].code(),
        "FILTER_TYPE_SCOPED",
        "applied-with-narrowing must carry a code distinct from UNKNOWN_FILTER_KEY: {:?}",
        result.warnings,
    );
}

/// MCP F3: an UNSCOPED filter on a field that is declared only as
/// **non-filterable** (`source_quality`
/// on `assertion`, `Filterable::None`) is ignored, not type-narrowed —
/// the result equals the same search without the filter (the spec
/// entities are retained, not silently dropped to the declaring type) —
/// and the warning reports `FIELD_NOT_FILTERABLE`, not the
/// `FILTER_TYPE_SCOPED` "applied-with-narrowing" code it carried pre-fix
/// (which lied: no value predicate ever ran). Filterability, not the
/// fallback type's accident of declaration, decides the outcome.
#[test]
fn search_unscoped_non_filterable_field_ignored_not_narrowed() {
    let mut store = Store::new();
    let mut assertion = make_entity("a-claim", "specs");
    assertion.entity_type = "assertion".into();
    assertion.metadata.insert(
        "source_quality".into(),
        MetadataValue::String("experimental".into()),
    );
    store.upsert(assertion.id.clone(), assertion);
    store.upsert(EntityId::new("specs", "s1"), make_entity("s1", "specs"));
    store.upsert(EntityId::new("specs", "s2"), make_entity("s2", "specs"));

    let baseline = run_search(&store, &SearchScope::default());

    // Both a wrong value and the assertion's real value must return the
    // same set as the unfiltered baseline — the filter is ignored, the
    // value is never matched. This is the discriminator that separates
    // "ignored" from "narrowed".
    for value in ["WRONG-VALUE", "experimental"] {
        let result = run_search(
            &store,
            &SearchScope {
                filters: HashMap::from([("source_quality".into(), value.into())]),
                ..Default::default()
            },
        );
        assert_eq!(
            result.total, baseline.total,
            "non-filterable filter (value={value}) must return the unfiltered set, not narrow to the declaring type",
        );
        assert_eq!(result.total, 3);
        assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
        assert_eq!(
            result.warnings[0].code(),
            "FIELD_NOT_FILTERABLE",
            "unscoped non-filterable field must report FIELD_NOT_FILTERABLE, not FILTER_TYPE_SCOPED: {:?}",
            result.warnings,
        );
    }
}

/// MCP F4 (range): an UNSCOPED range filter on a field no type
/// declares as range-filterable does
/// not drop the types that lack the field. `level` is `Filterable::
/// Equality` on `spec`; a `min_level` range filter is ignored, so a
/// `memo` entity (which doesn't declare `level`) is retained rather than
/// silently narrowed away — the warning's "ignored" word now matches the
/// result set.
#[test]
fn search_unscoped_non_range_filterable_field_not_dropped() {
    let mut store = Store::new();
    store.upsert(EntityId::new("specs", "s1"), make_entity("s1", "specs"));
    let mut memo = make_entity("m1", "specs");
    memo.entity_type = "memo".into();
    memo.metadata.shift_remove("level");
    store.upsert(memo.id.clone(), memo);

    let result = run_search(
        &store,
        &SearchScope {
            range_filters: HashMap::from([("min_level".into(), "M0".into())]),
            ..Default::default()
        },
    );
    assert_eq!(
        result.total,
        2,
        "non-range-filterable range filter must not drop the memo lacking the field; got {:?}",
        result
            .hits
            .iter()
            .map(|h| h.id.to_string())
            .collect::<Vec<_>>(),
    );
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.code() == "FIELD_NOT_RANGE_FILTERABLE"),
        "must warn FIELD_NOT_RANGE_FILTERABLE: {:?}",
        result.warnings,
    );
}

/// Range warning, fallback-type independence: an unscoped range
/// filter on a field the engine fallback type does NOT declare but
/// another type declares as
/// equality-only (`maturity` on `concept`) reports
/// `FIELD_NOT_RANGE_FILTERABLE` — keyed on workspace-wide
/// range-filterability, not on whether the fallback type happens to
/// declare it (pre-fix it emitted `RANGE_FILTER_TYPE_SCOPED`).
#[test]
fn search_unscoped_range_on_equality_only_other_type_field() {
    let mut store = Store::new();
    let mut concept = make_entity("c1", "specs");
    concept.entity_type = "concept".into();
    concept
        .metadata
        .insert("maturity".into(), MetadataValue::String("stable".into()));
    store.upsert(concept.id.clone(), concept);
    store.upsert(EntityId::new("specs", "s1"), make_entity("s1", "specs"));

    let result = run_search(
        &store,
        &SearchScope {
            range_filters: HashMap::from([("min_maturity".into(), "stable".into())]),
            ..Default::default()
        },
    );
    assert_eq!(
        result.total, 2,
        "non-range-filterable field range filter must leave the set unfiltered",
    );
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.code() == "FIELD_NOT_RANGE_FILTERABLE"),
        "must warn FIELD_NOT_RANGE_FILTERABLE (not RANGE_FILTER_TYPE_SCOPED): {:?}",
        result.warnings,
    );
}

/// A truly-unknown filter key (no reachable schema declares it) runs
/// the query unfiltered
/// and carries `UNKNOWN_FILTER_KEY` — the only "ignored" code whose
/// result set equals the unfiltered search via an unknown key.
#[test]
fn search_truly_unknown_key_ignored_with_unknown_code() {
    let mut store = Store::new();
    store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
    store.upsert(EntityId::new("specs", "b"), make_entity("b", "specs"));

    let result = run_search(
        &store,
        &SearchScope {
            filters: HashMap::from([("boguskey".into(), "x".into())]),
            ..Default::default()
        },
    );
    assert_eq!(
        result.total, 2,
        "truly-unknown key must leave the result set unfiltered"
    );
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(
        result.warnings[0].code(),
        "UNKNOWN_FILTER_KEY",
        "a key no schema declares must carry UNKNOWN_FILTER_KEY: {:?}",
        result.warnings,
    );
}

/// Range complement: a range filter on a field declared but not
/// range-filterable
/// (`level` is `filterable: equality`) is truly ignored — the
/// result equals the same search without it — and carries
/// `FIELD_NOT_RANGE_FILTERABLE`, not a silent empty.
#[test]
fn search_non_range_filterable_field_ignored_returns_unfiltered() {
    let mut store = Store::new();
    store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
    store.upsert(EntityId::new("specs", "b"), make_entity("b", "specs"));

    let result = run_search(
        &store,
        &SearchScope {
            entity_type: Some("spec".into()),
            range_filters: HashMap::from([("min_level".into(), "M0".into())]),
            ..Default::default()
        },
    );
    assert_eq!(
        result.total, 2,
        "non-range-filterable field range filter must be ignored, not empty the set",
    );
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.code() == "FIELD_NOT_RANGE_FILTERABLE"),
        "must warn FIELD_NOT_RANGE_FILTERABLE: {:?}",
        result.warnings,
    );
}

#[test]
fn search_range_filter_unknown_field_warns() {
    let mut store = Store::new();
    let e = make_entity("only", "specs");
    store.upsert(e.id.clone(), e);

    let scope = SearchScope {
        range_filters: HashMap::from([("min_nonexistent".into(), "0".into())]),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(
        result.total, 1,
        "unknown range field should be skipped, not reject all entities"
    );
    assert_eq!(result.warnings.len(), 1);
    assert!(
        result.warnings[0].to_string().contains("nonexistent"),
        "warning mentions unknown range field: {:?}",
        result.warnings
    );
}

#[test]
fn list_unknown_filter_key_warns() {
    let mut store = Store::new();
    store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));

    let schema = type_by_name("spec").unwrap();
    let scope = SearchScope {
        filters: HashMap::from([("nope".into(), "x".into())]),
        ..Default::default()
    };

    let schemas: HashMap<String, Arc<Schema>> = HashMap::new();
    let result = list(&store, &scope, &schema, &schemas);
    assert_eq!(result.total, 1);
    assert_eq!(result.warnings.len(), 1);
}

#[test]
fn token_budget_trims_overflowing_page_and_warns() {
    let mut store = Store::new();
    for i in 0..20 {
        let mut e = make_entity(&format!("entity-{i:02}"), "specs");
        e.sections.insert("identity".into(), "graph ".repeat(50));
        store.upsert(e.id.clone(), e);
    }

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["graph".into()],
            ..Default::default()
        }),
        // Tiny budget: a single hit already exceeds it, so the page must
        // trim to exactly one and warn.
        token_budget: Some(20),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(result.total, 20, "total reflects the full match count");
    assert!(result.returned >= 1, "at least one hit always returns");
    assert!(result.returned < 20, "the page was trimmed by the budget");
    assert_eq!(result.hits.len(), result.returned);
    let trunc = result
        .warnings
        .iter()
        .find(|w| w.code() == "SEARCH_RESULTS_TRUNCATED")
        .expect("budget trim emits SEARCH_RESULTS_TRUNCATED");
    assert!(trunc.message().contains("budget"));
}

#[test]
fn ample_budget_returns_all_hits_without_warning() {
    let mut store = Store::new();
    for i in 0..5 {
        let mut e = make_entity(&format!("entity-{i}"), "specs");
        e.sections.insert("identity".into(), "graph".into());
        store.upsert(e.id.clone(), e);
    }
    let scope = SearchScope {
        query: Some(Query {
            any: vec!["graph".into()],
            ..Default::default()
        }),
        token_budget: Some(1_000_000),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.returned, 5);
    assert!(
        result
            .warnings
            .iter()
            .all(|w| w.code() != "SEARCH_RESULTS_TRUNCATED"),
        "an ample budget does not trim"
    );
}

#[test]
fn search_hits_carry_no_section_bodies() {
    let mut store = Store::new();
    store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
    let scope = SearchScope {
        query: Some(Query {
            any: vec!["Identity".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    assert!(
        result.hits[0].sections.is_empty(),
        "search hits ship no section bodies — read them with memstead_entity"
    );
    // The lead-section summary is still resolved from the entity.
    assert!(result.hits[0].summary.is_some(), "summary still resolved");
}

#[test]
fn list_hits_still_carry_section_bodies() {
    let mut store = Store::new();
    store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
    let schema = type_by_name("spec").unwrap();
    let schemas: HashMap<String, Arc<Schema>> = HashMap::new();
    let result = list(&store, &SearchScope::default(), &schema, &schemas);
    assert_eq!(result.total, 1);
    assert!(
        !result.hits[0].sections.is_empty(),
        "list hits keep section bodies for human-facing roster consumers"
    );
}

#[test]
fn search_csv_array_filter() {
    let mut store = Store::new();
    let mut e = make_entity("tagged", "specs");
    e.metadata.insert(
        "tags".into(),
        MetadataValue::String("backend, api, rust".into()),
    );
    store.upsert(e.id.clone(), e);

    let scope = SearchScope {
        filters: HashMap::from([("tags".into(), "api".into())]),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
}

#[test]
fn search_pagination() {
    let mut store = Store::new();
    for i in 0..10 {
        let e = make_entity(&format!("entity-{i:02}"), "specs");
        store.upsert(e.id.clone(), e);
    }

    let scope = SearchScope {
        limit: Some(3),
        offset: Some(2),
        ..Default::default()
    };

    let result = run_search(&store, &scope);
    assert_eq!(result.total, 10);
    assert_eq!(result.returned, 3);
    assert_eq!(result.offset, 2);
}

#[test]
fn list_entities() {
    let mut store = Store::new();
    store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
    store.upsert(EntityId::new("specs", "b"), make_entity("b", "specs"));

    let schema = type_by_name("spec").unwrap();
    let scope = SearchScope::default();

    let schemas: HashMap<String, Arc<Schema>> = HashMap::new();
    let result = list(&store, &scope, &schema, &schemas);
    assert_eq!(result.total, 2);
    assert!(result.total_tokens > 0);
}

#[test]
fn build_snippet_basic() {
    let content = "The graph engine processes queries efficiently.";
    let snippet = build_snippet(content, "engine");
    assert!(snippet.contains("**engine**"));
}

// ---- Structured-query semantics ----

#[test]
fn query_any_or_semantics() {
    let mut store = Store::new();
    let mut a = make_entity("a", "specs");
    a.sections
        .insert("identity".into(), "authentication flow".into());
    let mut b = make_entity("b", "specs");
    b.sections
        .insert("identity".into(), "login pipeline".into());
    let mut c = make_entity("c", "specs");
    c.sections
        .insert("identity".into(), "unrelated subject".into());
    store.upsert(a.id.clone(), a);
    store.upsert(b.id.clone(), b);
    store.upsert(c.id.clone(), c);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["authentication".into(), "login".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let names: Vec<_> = result
        .hits
        .iter()
        .map(|h| h.id.name().to_string())
        .collect();
    assert_eq!(result.total, 2, "union of any terms: {names:?}");
    assert!(names.contains(&"a".to_string()));
    assert!(names.contains(&"b".to_string()));
}

#[test]
fn query_not_excludes() {
    let mut store = Store::new();
    let mut a = make_entity("a", "specs");
    a.sections
        .insert("identity".into(), "uses authentication".into());
    let mut b = make_entity("b", "specs");
    b.sections
        .insert("identity".into(), "uses authentication mock".into());
    store.upsert(a.id.clone(), a);
    store.upsert(b.id.clone(), b);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["authentication".into()],
            not: vec!["mock".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    assert_eq!(result.hits[0].id.name(), "a");
}

#[test]
fn query_phrase_match() {
    let mut store = Store::new();
    let mut a = make_entity("a", "specs");
    a.sections.insert(
        "identity".into(),
        "the client side agent runs locally".into(),
    );
    let mut b = make_entity("b", "specs");
    b.sections.insert(
        "identity".into(),
        "the client invokes the side channel for the agent".into(),
    );
    store.upsert(a.id.clone(), a);
    store.upsert(b.id.clone(), b);

    let scope = SearchScope {
        query: Some(Query {
            phrase: Some("client side agent".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    assert_eq!(result.hits[0].id.name(), "a");
}

#[test]
fn query_field_restricted() {
    let mut store = Store::new();
    let mut a = make_entity("a", "specs");
    a.sections.insert("identity".into(), "foo content".into());
    let mut b = make_entity("b", "specs");
    b.sections.insert("purpose".into(), "foo content".into());
    store.upsert(a.id.clone(), a);
    store.upsert(b.id.clone(), b);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["foo".into()],
            field: Some("identity".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    assert_eq!(result.hits[0].id.name(), "a");
}

#[test]
fn query_empty_is_metadata_filter() {
    let mut store = Store::new();
    let mut memo_entity = make_entity("m", "specs");
    memo_entity.entity_type = "memo".into();
    store.upsert(memo_entity.id.clone(), memo_entity);
    store.upsert(EntityId::new("specs", "s1"), make_entity("s1", "specs"));
    store.upsert(EntityId::new("specs", "s2"), make_entity("s2", "specs"));

    let scope = SearchScope {
        query: Some(Query::default()),
        entity_type: Some("spec".into()),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(
        result.total, 2,
        "empty query ⇒ metadata filter over entity_type"
    );
}

#[test]
fn query_diacritic_folding() {
    let mut store = Store::new();
    let mut a = make_entity("a", "specs");
    a.sections.insert("identity".into(), "schöne Häuser".into());
    store.upsert(a.id.clone(), a);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["hauser".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
}

#[test]
fn query_spans_all_mems_when_mem_none() {
    let mut store = Store::new();
    let mut a = make_entity("a", "specs");
    a.sections.insert("identity".into(), "foo".into());
    let mut b = make_entity("b", "memos");
    b.sections.insert("identity".into(), "foo".into());
    store.upsert(a.id.clone(), a);
    store.upsert(b.id.clone(), b);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["foo".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 2);
}

#[test]
fn query_targets_single_mem_when_named() {
    let mut store = Store::new();
    let mut a = make_entity("a", "specs");
    a.sections.insert("identity".into(), "foo".into());
    let mut b = make_entity("b", "memos");
    b.sections.insert("identity".into(), "foo".into());
    store.upsert(a.id.clone(), a);
    store.upsert(b.id.clone(), b);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["foo".into()],
            ..Default::default()
        }),
        mem: Some("memos".into()),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    assert_eq!(result.hits[0].mem, "memos");
}

// ---- matched_terms + score_breakdown + heading_path ----

use crate::entity::HeadingSpan;

#[test]
fn matched_terms_populated_for_any() {
    let mut store = Store::new();
    let mut a = make_entity("a", "specs");
    a.sections
        .insert("identity".into(), "auth flow uses oidc sessions".into());
    store.upsert(a.id.clone(), a);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["auth".into(), "oidc".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    let hit = &result.hits[0];
    let mt = hit.matched_terms.as_ref().expect("matched_terms populated");
    assert!(mt.contains_key("auth"), "auth keyed: {mt:?}");
    assert!(mt.contains_key("oidc"), "oidc keyed: {mt:?}");
}

#[test]
fn matched_terms_per_field() {
    let mut store = Store::new();
    let mut a = make_entity("graph-engine", "specs");
    a.sections.insert(
        "identity".into(),
        "graph-engine uses graph primitives".into(),
    );
    store.upsert(a.id.clone(), a);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["graph".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let hit = &result.hits[0];
    let mt = hit.matched_terms.as_ref().unwrap();
    let fields: Vec<&str> = mt["graph"].iter().map(|tm| tm.field.as_str()).collect();
    assert!(fields.contains(&"title"), "title field: {fields:?}");
    assert!(fields.contains(&"identity"), "identity field: {fields:?}");
}

#[test]
fn matched_terms_excludes_not_terms() {
    let mut store = Store::new();
    let mut a = make_entity("a", "specs");
    a.sections
        .insert("identity".into(), "uses authentication".into());
    store.upsert(a.id.clone(), a);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["authentication".into()],
            not: vec!["mock".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let hit = &result.hits[0];
    let mt = hit.matched_terms.as_ref().unwrap();
    assert!(mt.contains_key("authentication"));
    assert!(
        !mt.contains_key("mock"),
        "negative predicate must not populate matched_terms: {mt:?}"
    );
}

#[test]
fn score_breakdown_sums_to_score() {
    let mut store = Store::new();
    let mut a = make_entity("a", "specs");
    a.sections
        .insert("identity".into(), "graph engine core".into());
    store.upsert(a.id.clone(), a);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["graph".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let hit = &result.hits[0];
    let br = hit.score_breakdown.as_ref().expect("breakdown populated");
    let sum: f32 = br.bm25 + br.title_boost + br.field_weights.values().sum::<f32>();
    assert!(
        (sum - hit.score).abs() < 0.01,
        "components should sum to score: sum={sum} score={}",
        hit.score
    );
}

#[test]
fn phrase_snippet_contains_full_phrase() {
    let mut store = Store::new();
    let mut a = make_entity("a", "specs");
    a.sections.insert(
        "identity".into(),
        "the client side agent runs locally".into(),
    );
    store.upsert(a.id.clone(), a);

    let scope = SearchScope {
        query: Some(Query {
            phrase: Some("client side agent".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let hit = &result.hits[0];
    let mt = hit.matched_terms.as_ref().unwrap();
    let matches = mt
        .get("client side agent")
        .expect("phrase term keyed in matched_terms");
    let identity_snippet = matches
        .iter()
        .find(|tm| tm.field == "identity")
        .expect("phrase matched in identity");
    assert!(
        identity_snippet.snippet.contains("client side agent"),
        "snippet must contain full phrase: {}",
        identity_snippet.snippet
    );
}

fn entity_with_heading_spans(
    name: &str,
    section_key: &str,
    content: &str,
    spans: Vec<HeadingSpan>,
) -> Entity {
    let mut e = make_entity(name, "specs");
    e.sections
        .insert(section_key.to_string(), content.to_string());
    e.heading_spans.insert(section_key.to_string(), spans);
    e
}

#[test]
fn heading_path_none_when_match_above_first_subheading() {
    // H3 starts at offset 20 in the section content; match "anchor" is at offset 4 (before).
    let content = "the anchor word here\n### Later Heading\nmore text";
    let h3_offset = content.find("### Later Heading").unwrap();
    let spans = vec![HeadingSpan {
        level: 3,
        title: "Later Heading".into(),
        start_offset: h3_offset,
        end_offset: content.len(),
    }];
    let mut store = Store::new();
    store.upsert(
        EntityId::new("specs", "a"),
        entity_with_heading_spans("a", "identity", content, spans),
    );

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["anchor".into()],
            field: Some("identity".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let mt = result.hits[0].matched_terms.as_ref().unwrap();
    let tm = &mt["anchor"][0];
    assert!(
        tm.heading_path.is_none(),
        "match above first subheading ⇒ no heading_path: {:?}",
        tm.heading_path
    );
}

#[test]
fn heading_path_single_level() {
    // Match under one H3.
    let content = "### Response Shapes\nhandles unique keyword here\n";
    let spans = vec![HeadingSpan {
        level: 3,
        title: "Response Shapes".into(),
        start_offset: 0,
        end_offset: content.len(),
    }];
    let mut store = Store::new();
    store.upsert(
        EntityId::new("specs", "a"),
        entity_with_heading_spans("a", "identity", content, spans),
    );

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["unique".into()],
            field: Some("identity".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let mt = result.hits[0].matched_terms.as_ref().unwrap();
    let tm = &mt["unique"][0];
    assert_eq!(
        tm.heading_path,
        Some(vec!["Response Shapes".into()]),
        "single-level path under one H3"
    );
}

#[test]
fn heading_path_nested_h3_h4() {
    // Section content:
    //   ### Response Shapes
    //   some text
    //   #### Markdown Output
    //   match distinct-keyword here
    let mut content = String::new();
    content.push_str("### Response Shapes\n");
    content.push_str("some text\n");
    let h4_start = content.len();
    content.push_str("#### Markdown Output\n");
    let payload_start = content.len();
    content.push_str("distinct-keyword is below\n");
    let spans = vec![
        HeadingSpan {
            level: 3,
            title: "Response Shapes".into(),
            start_offset: 0,
            end_offset: content.len(),
        },
        HeadingSpan {
            level: 4,
            title: "Markdown Output".into(),
            start_offset: h4_start,
            end_offset: content.len(),
        },
    ];
    let _ = payload_start;
    let mut store = Store::new();
    store.upsert(
        EntityId::new("specs", "a"),
        entity_with_heading_spans("a", "identity", &content, spans),
    );

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["distinct-keyword".into()],
            field: Some("identity".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let mt = result.hits[0].matched_terms.as_ref().unwrap();
    let tm = &mt["distinct-keyword"][0];
    assert_eq!(
        tm.heading_path,
        Some(vec!["Response Shapes".into(), "Markdown Output".into()]),
        "nested path: outermost (H3) first, innermost (H4) last"
    );
}

#[test]
fn heading_path_survives_level_skip() {
    // H2 → H4 directly (no H3). Only the H4 span exists.
    let content = "#### Direct Subsection\nrare-match word here\n";
    let spans = vec![HeadingSpan {
        level: 4,
        title: "Direct Subsection".into(),
        start_offset: 0,
        end_offset: content.len(),
    }];
    let mut store = Store::new();
    store.upsert(
        EntityId::new("specs", "a"),
        entity_with_heading_spans("a", "identity", content, spans),
    );

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["rare-match".into()],
            field: Some("identity".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let mt = result.hits[0].matched_terms.as_ref().unwrap();
    let tm = &mt["rare-match"][0];
    assert_eq!(
        tm.heading_path,
        Some(vec!["Direct Subsection".into()]),
        "flat H4 span produces single-element path; no virtual H3 inserted"
    );
}

#[test]
fn heading_path_distinguishes_duplicate_siblings() {
    // Two `### Foo` under the same section; match in second one → path
    // carries "Foo" from the second span (same title, distinguished by
    // offset containment).
    let mut content = String::new();
    content.push_str("### Foo\nfirst body\n");
    let second_start = content.len();
    content.push_str("### Foo\nsecond body carries sentinel-word here\n");
    let spans = vec![
        HeadingSpan {
            level: 3,
            title: "Foo".into(),
            start_offset: 0,
            end_offset: second_start,
        },
        HeadingSpan {
            level: 3,
            title: "Foo".into(),
            start_offset: second_start,
            end_offset: content.len(),
        },
    ];
    let mut store = Store::new();
    store.upsert(
        EntityId::new("specs", "a"),
        entity_with_heading_spans("a", "identity", &content, spans),
    );

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["sentinel-word".into()],
            field: Some("identity".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let mt = result.hits[0].matched_terms.as_ref().unwrap();
    let tm = &mt["sentinel-word"][0];
    assert_eq!(
        tm.heading_path,
        Some(vec!["Foo".into()]),
        "second `### Foo` span contains the match (offset-based)"
    );
}

// ---- Facets ----

#[test]
fn facets_count_over_full_result_not_page() {
    // 12 matching entities; page limit 5. Facets must reflect all 12.
    let mut store = Store::new();
    for i in 0..12 {
        let mut e = make_entity(&format!("e-{i:02}"), "specs");
        e.sections
            .insert("identity".into(), "shared-keyword here".into());
        store.upsert(e.id.clone(), e);
    }

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["shared-keyword".into()],
            ..Default::default()
        }),
        limit: Some(5),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 12);
    assert_eq!(result.returned, 5);
    let facets = result.facets.as_ref().expect("facets present");
    let by_type_sum: usize = facets.by_type.values().sum();
    assert_eq!(
        by_type_sum, 12,
        "by_type must cover the full unpaginated set, not just the page"
    );
    let by_mem_sum: usize = facets.by_mem.values().sum();
    assert_eq!(by_mem_sum, 12);
}

#[test]
fn facets_by_type_and_mem_exact() {
    let mut store = Store::new();
    // 3 specs in 'specs', 2 memos in 'memos'.
    for i in 0..3 {
        let mut e = make_entity(&format!("s-{i}"), "specs");
        e.sections.insert("identity".into(), "shared anchor".into());
        store.upsert(e.id.clone(), e);
    }
    for i in 0..2 {
        let mut e = make_entity(&format!("m-{i}"), "memos");
        e.entity_type = "memo".into();
        e.sections.insert("identity".into(), "shared anchor".into());
        store.upsert(e.id.clone(), e);
    }

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["anchor".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let facets = result.facets.as_ref().unwrap();
    assert_eq!(facets.by_type.get("spec").copied(), Some(3));
    assert_eq!(facets.by_type.get("memo").copied(), Some(2));
    assert_eq!(facets.by_mem.get("specs").copied(), Some(3));
    assert_eq!(facets.by_mem.get("memos").copied(), Some(2));
    // Without graph expansion every hit is primary; no `expanded`
    // dim is populated.
    assert_eq!(facets.by_expansion.get("primary").copied(), Some(5));
    assert!(!facets.by_expansion.contains_key("expanded"));
}

#[test]
fn facets_empty_when_no_hits() {
    let mut store = Store::new();
    let e = make_entity("lonely", "specs");
    store.upsert(e.id.clone(), e);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["never-occurs-keyword".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 0);
    let facets = result
        .facets
        .as_ref()
        .expect("facets is Some(Facets::default()) even when hit set is empty");
    assert!(facets.by_type.is_empty());
    assert!(facets.by_mem.is_empty());
    assert!(facets.by_level.is_empty());
    assert!(facets.by_subsection.is_empty());
    assert!(facets.by_expansion.is_empty());
}

#[test]
fn facets_by_subsection_exact() {
    // Two hits both matching under two distinct sub-sections.
    let content_a = "### Response Shapes\nentity-a unique-anchor here\n";
    let spans_a = vec![HeadingSpan {
        level: 3,
        title: "Response Shapes".into(),
        start_offset: 0,
        end_offset: content_a.len(),
    }];
    let content_b = "### Tool Surface\nentity-b unique-anchor here\n";
    let spans_b = vec![HeadingSpan {
        level: 3,
        title: "Tool Surface".into(),
        start_offset: 0,
        end_offset: content_b.len(),
    }];
    let mut store = Store::new();
    store.upsert(
        EntityId::new("specs", "a"),
        entity_with_heading_spans("a", "identity", content_a, spans_a),
    );
    store.upsert(
        EntityId::new("specs", "b"),
        entity_with_heading_spans("b", "identity", content_b, spans_b),
    );

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["unique-anchor".into()],
            field: Some("identity".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let facets = result.facets.as_ref().unwrap();
    assert_eq!(facets.by_subsection.len(), 2);
    let paths: std::collections::HashSet<Vec<String>> = facets
        .by_subsection
        .iter()
        .map(|e| e.path.clone())
        .collect();
    assert!(paths.contains(&vec!["identity".into(), "Response Shapes".into()]));
    assert!(paths.contains(&vec!["identity".into(), "Tool Surface".into()]));
    for entry in &facets.by_subsection {
        assert_eq!(entry.count, 1);
    }
}

#[test]
fn facets_by_subsection_excludes_h2_only_matches() {
    // Match falls inside an H2 section that has no H3–H6 spans. No
    // `by_subsection` entry should appear for it.
    let mut store = Store::new();
    let mut e = make_entity("a", "specs");
    e.sections
        .insert("identity".into(), "only-here unique-keyword lives".into());
    store.upsert(e.id.clone(), e);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["unique-keyword".into()],
            field: Some("identity".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    let facets = result.facets.as_ref().unwrap();
    assert!(
        facets.by_subsection.is_empty(),
        "H2-only match must not contribute to by_subsection: {:?}",
        facets.by_subsection
    );
}

#[test]
fn facets_by_subsection_survives_punctuation_in_heading() {
    // A heading containing a slash must not be split by a delimiter.
    let content = "### Client/Server split\nword punctuation-anchor exists\n";
    let spans = vec![HeadingSpan {
        level: 3,
        title: "Client/Server split".into(),
        start_offset: 0,
        end_offset: content.len(),
    }];
    let mut store = Store::new();
    store.upsert(
        EntityId::new("specs", "a"),
        entity_with_heading_spans("a", "identity", content, spans),
    );

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["punctuation-anchor".into()],
            field: Some("identity".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let facets = result.facets.as_ref().unwrap();
    assert_eq!(facets.by_subsection.len(), 1);
    let entry = &facets.by_subsection[0];
    assert_eq!(entry.count, 1);
    assert_eq!(
        entry.path,
        vec!["identity".to_string(), "Client/Server split".to_string()],
        "punctuation in heading must remain a single path element"
    );
}

#[test]
fn facets_by_level_counts_when_present() {
    let mut store = Store::new();
    let mut e1 = make_entity("a", "specs");
    e1.metadata
        .insert("level".into(), MetadataValue::String("M0".into()));
    e1.sections
        .insert("identity".into(), "shared anchor".into());
    let mut e2 = make_entity("b", "specs");
    e2.metadata
        .insert("level".into(), MetadataValue::String("M1".into()));
    e2.sections
        .insert("identity".into(), "shared anchor".into());
    let mut e3 = make_entity("c", "specs");
    e3.metadata
        .insert("level".into(), MetadataValue::String("M1".into()));
    e3.sections
        .insert("identity".into(), "shared anchor".into());
    store.upsert(e1.id.clone(), e1);
    store.upsert(e2.id.clone(), e2);
    store.upsert(e3.id.clone(), e3);

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["anchor".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let facets = result.facets.as_ref().unwrap();
    assert_eq!(facets.by_level.get("M0").copied(), Some(1));
    assert_eq!(facets.by_level.get("M1").copied(), Some(2));
}

// ---- Graph expansion via expand_via ----

use crate::store::{Edge, EdgeSource};

fn add_edge(store: &mut Store, from: EntityId, to: EntityId, rel: &str) {
    store.add_edge(
        from,
        Edge {
            rel_type: rel.into(),
            target: to,
            source: EdgeSource::Explicit,
        },
    );
}

/// An auto-emitted mention edge (`EdgeSource::BodyLink`) — a co-mention,
/// not a typed dependency.
fn add_body_edge(store: &mut Store, from: EntityId, to: EntityId) {
    store.add_edge(
        from,
        Edge {
            rel_type: "REFERENCES".into(),
            target: to,
            source: EdgeSource::BodyLink,
        },
    );
}

/// #54: a `related_to` neighbourhood ranks by proximity — nearer hops
/// first, and a typed (dependency) link to the anchor before a
/// co-mention at the same hop. A small neighbourhood keeps full
/// membership (only ordering changes — the refusal AC).
#[test]
fn related_to_ranks_by_proximity_then_typed() {
    let mut store = Store::new();
    for n in ["hub", "dep1", "men1", "far1"] {
        let e = make_entity(n, "specs");
        store.upsert(e.id.clone(), e);
    }
    let hub = EntityId::new("specs", "hub");
    // hub —USES→ dep1 (typed, dist 1); hub —REFERENCES(mention)→ men1
    // (dist 1); dep1 —USES→ far1 (dist 2 from hub).
    add_edge(
        &mut store,
        hub.clone(),
        EntityId::new("specs", "dep1"),
        "USES",
    );
    add_body_edge(&mut store, hub.clone(), EntityId::new("specs", "men1"));
    add_edge(
        &mut store,
        EntityId::new("specs", "dep1"),
        EntityId::new("specs", "far1"),
        "USES",
    );

    let scope = SearchScope {
        related_to: Some(hub.clone()),
        depth: Some(2),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    // Membership unchanged: hub(0) + dep1,men1(1) + far1(2) — all 4.
    let order: Vec<&str> = result.hits.iter().map(|h| h.id.name()).collect();
    assert_eq!(
        result.total, 4,
        "small neighbourhood keeps full membership: {order:?}"
    );
    let pos = |n: &str| order.iter().position(|x| *x == n).unwrap();
    assert!(
        pos("dep1") < pos("far1"),
        "nearer before farther: {order:?}"
    );
    assert!(
        pos("men1") < pos("far1"),
        "nearer before farther: {order:?}"
    );
    assert!(
        pos("dep1") < pos("men1"),
        "typed link before co-mention at the same hop: {order:?}"
    );
}

/// #54: a hub neighbourhood larger than the cap is bounded to its
/// nearest N with a `NEIGHBOURHOOD_CAPPED` warning.
#[test]
fn related_to_hub_is_capped_with_warning() {
    let mut store = Store::new();
    let hub = EntityId::new("specs", "hub");
    store.upsert(hub.clone(), make_entity("hub", "specs"));
    for i in 0..150 {
        let n = format!("n{i:03}");
        let id = EntityId::new("specs", &n);
        store.upsert(id.clone(), make_entity(&n, "specs"));
        add_edge(&mut store, hub.clone(), id, "USES");
    }
    let scope = SearchScope {
        related_to: Some(hub.clone()),
        depth: Some(1),
        limit: Some(200),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(
        result.total, RELATED_TO_NEIGHBOURHOOD_CAP,
        "hub neighbourhood bounded to the cap"
    );
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.code() == "NEIGHBOURHOOD_CAPPED"),
        "capping must surface a warning; got {:?}",
        result.warnings.iter().map(|w| w.code()).collect::<Vec<_>>()
    );
}

#[test]
fn expand_via_pulls_in_direct_neighbours() {
    let mut store = Store::new();
    let mut primary = make_entity("primary", "specs");
    primary
        .sections
        .insert("identity".into(), "auth flow".into());
    let n1 = make_entity("n1", "specs");
    let n2 = make_entity("n2", "specs");
    let primary_id = primary.id.clone();
    store.upsert(primary_id.clone(), primary);
    store.upsert(n1.id.clone(), n1);
    store.upsert(n2.id.clone(), n2);
    add_edge(
        &mut store,
        primary_id.clone(),
        EntityId::new("specs", "n1"),
        "REFERENCES",
    );
    add_edge(
        &mut store,
        primary_id.clone(),
        EntityId::new("specs", "n2"),
        "REFERENCES",
    );

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["auth".into()],
            ..Default::default()
        }),
        expand_via: Some(vec!["REFERENCES".into()]),
        expand_depth: Some(1),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 3, "primary + 2 expanded");

    let expanded: Vec<&SearchHit> = result
        .hits
        .iter()
        .filter(|h| h.expansion.is_some())
        .collect();
    assert_eq!(expanded.len(), 2);
    for h in expanded {
        let exp = h.expansion.as_ref().unwrap();
        assert_eq!(exp.of, primary_id);
        assert_eq!(exp.via_edge, "REFERENCES");
        assert_eq!(exp.depth, 1);
        // Facet side check lands below — here, confirm the wire contract:
        // expanded hits carry a decayed score_breakdown, no matched_terms.
        let bd = h.score_breakdown.as_ref().unwrap();
        assert_eq!(bd.expansion_decay, Some(0.5));
        assert!(h.matched_terms.is_none());
    }
    // Facet by_expansion now carries both keys.
    let facets = result.facets.as_ref().unwrap();
    assert_eq!(facets.by_expansion.get("primary").copied(), Some(1));
    assert_eq!(facets.by_expansion.get("expanded").copied(), Some(2));
}

#[test]
fn expand_via_respects_filter() {
    // Primary is a spec; neighbour is a memo. entity_type filter drops it.
    let mut store = Store::new();
    let mut primary = make_entity("primary", "specs");
    primary
        .sections
        .insert("identity".into(), "auth flow".into());
    let mut neighbor = make_entity("neighbor", "specs");
    neighbor.entity_type = "memo".into();
    let primary_id = primary.id.clone();
    store.upsert(primary_id.clone(), primary);
    store.upsert(neighbor.id.clone(), neighbor);
    add_edge(
        &mut store,
        primary_id,
        EntityId::new("specs", "neighbor"),
        "REFERENCES",
    );

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["auth".into()],
            ..Default::default()
        }),
        entity_type: Some("spec".into()),
        expand_via: Some(vec!["REFERENCES".into()]),
        expand_depth: Some(1),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(
        result.total, 1,
        "only the primary — memo neighbour dropped by entity_type"
    );
    assert!(result.hits[0].expansion.is_none());
}

#[test]
fn expand_via_respects_depth() {
    // primary --R--> a --R--> b
    let mut store = Store::new();
    let mut primary = make_entity("primary", "specs");
    primary.sections.insert("identity".into(), "anchor".into());
    let a = make_entity("a", "specs");
    let b = make_entity("b", "specs");
    let primary_id = primary.id.clone();
    store.upsert(primary_id.clone(), primary);
    store.upsert(a.id.clone(), a);
    store.upsert(b.id.clone(), b);
    add_edge(
        &mut store,
        primary_id.clone(),
        EntityId::new("specs", "a"),
        "REFERENCES",
    );
    add_edge(
        &mut store,
        EntityId::new("specs", "a"),
        EntityId::new("specs", "b"),
        "REFERENCES",
    );

    let make_scope = |depth: usize| SearchScope {
        query: Some(Query {
            any: vec!["anchor".into()],
            ..Default::default()
        }),
        expand_via: Some(vec!["REFERENCES".into()]),
        expand_depth: Some(depth),
        ..Default::default()
    };
    let r1 = run_search(&store, &make_scope(1));
    assert_eq!(r1.total, 2, "depth 1: primary + a");

    let r2 = run_search(&store, &make_scope(2));
    assert_eq!(r2.total, 3, "depth 2: primary + a + b");
    let b_hit = r2.hits.iter().find(|h| h.id.name() == "b").unwrap();
    assert_eq!(b_hit.expansion.as_ref().unwrap().depth, 2);
}

#[test]
fn expand_via_empty_edge_types_skips() {
    let mut store = Store::new();
    let mut primary = make_entity("primary", "specs");
    primary.sections.insert("identity".into(), "anchor".into());
    let n = make_entity("n", "specs");
    let primary_id = primary.id.clone();
    store.upsert(primary_id.clone(), primary);
    store.upsert(n.id.clone(), n);
    add_edge(
        &mut store,
        primary_id,
        EntityId::new("specs", "n"),
        "REFERENCES",
    );

    let scope_empty = SearchScope {
        query: Some(Query {
            any: vec!["anchor".into()],
            ..Default::default()
        }),
        expand_via: Some(Vec::new()),
        ..Default::default()
    };
    let scope_none = SearchScope {
        query: Some(Query {
            any: vec!["anchor".into()],
            ..Default::default()
        }),
        expand_via: None,
        ..Default::default()
    };
    let r_empty = run_search(&store, &scope_empty);
    let r_none = run_search(&store, &scope_none);
    assert_eq!(r_empty.total, 1);
    assert_eq!(r_empty.total, r_none.total);
}

#[test]
fn expand_via_score_decay() {
    // primary --R--> a --R--> b, depth 2
    let mut store = Store::new();
    let mut primary = make_entity("primary", "specs");
    primary.sections.insert("identity".into(), "keyword".into());
    let a = make_entity("a", "specs");
    let b = make_entity("b", "specs");
    let primary_id = primary.id.clone();
    store.upsert(primary_id.clone(), primary);
    store.upsert(a.id.clone(), a);
    store.upsert(b.id.clone(), b);
    add_edge(
        &mut store,
        primary_id.clone(),
        EntityId::new("specs", "a"),
        "REFERENCES",
    );
    add_edge(
        &mut store,
        EntityId::new("specs", "a"),
        EntityId::new("specs", "b"),
        "REFERENCES",
    );

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["keyword".into()],
            ..Default::default()
        }),
        expand_via: Some(vec!["REFERENCES".into()]),
        expand_depth: Some(2),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let primary_hit = result
        .hits
        .iter()
        .find(|h| h.id == primary_id)
        .expect("primary present");
    let primary_score = primary_hit.score;
    assert!(primary_score > 0.0, "primary must have BM25 score");

    let a_hit = result.hits.iter().find(|h| h.id.name() == "a").unwrap();
    let b_hit = result.hits.iter().find(|h| h.id.name() == "b").unwrap();
    assert!((a_hit.score - primary_score * 0.5).abs() < 0.0001);
    assert!((b_hit.score - primary_score * 0.25).abs() < 0.0001);
    assert_eq!(
        a_hit.score_breakdown.as_ref().unwrap().expansion_decay,
        Some(0.5)
    );
    assert_eq!(
        b_hit.score_breakdown.as_ref().unwrap().expansion_decay,
        Some(0.25)
    );
}

#[test]
fn expand_via_via_edge_label_correct() {
    let mut store = Store::new();
    let mut primary = make_entity("primary", "specs");
    primary.sections.insert("identity".into(), "keyword".into());
    let realizes_n = make_entity("realizes-target", "specs");
    let references_n = make_entity("references-target", "specs");
    let primary_id = primary.id.clone();
    store.upsert(primary_id.clone(), primary);
    store.upsert(realizes_n.id.clone(), realizes_n);
    store.upsert(references_n.id.clone(), references_n);
    add_edge(
        &mut store,
        primary_id.clone(),
        EntityId::new("specs", "realizes-target"),
        "REALIZES",
    );
    add_edge(
        &mut store,
        primary_id,
        EntityId::new("specs", "references-target"),
        "REFERENCES",
    );

    let scope = SearchScope {
        query: Some(Query {
            any: vec!["keyword".into()],
            ..Default::default()
        }),
        expand_via: Some(vec!["REALIZES".into(), "REFERENCES".into()]),
        expand_depth: Some(1),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    let rt = result
        .hits
        .iter()
        .find(|h| h.id.name() == "realizes-target")
        .unwrap();
    assert_eq!(rt.expansion.as_ref().unwrap().via_edge, "REALIZES");
    let rf = result
        .hits
        .iter()
        .find(|h| h.id.name() == "references-target")
        .unwrap();
    assert_eq!(rf.expansion.as_ref().unwrap().via_edge, "REFERENCES");
}

fn make_stub_entity(name: &str, mem: &str) -> Entity {
    let mut e = make_entity(name, mem);
    e.stub = true;
    e
}

#[test]
fn search_filter_stub_none_returns_both() {
    let mut store = Store::new();
    let real = make_entity("real-a", "specs");
    let stub = make_stub_entity("stub-b", "specs");
    store.upsert(real.id.clone(), real);
    store.upsert(stub.id.clone(), stub);

    let scope = SearchScope::default();
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 2, "default returns both stubs and reals");

    let stub_hit = result
        .hits
        .iter()
        .find(|h| h.id.name() == "stub-b")
        .expect("stub must appear in default results");
    assert!(
        stub_hit.stub,
        "hit.stub reflects entity.stub (regression guard)"
    );
    let real_hit = result
        .hits
        .iter()
        .find(|h| h.id.name() == "real-a")
        .expect("real must appear");
    assert!(!real_hit.stub);
}

#[test]
fn search_filter_stub_true_returns_only_stubs() {
    let mut store = Store::new();
    let real = make_entity("real-a", "specs");
    let stub = make_stub_entity("stub-b", "specs");
    store.upsert(real.id.clone(), real);
    store.upsert(stub.id.clone(), stub);

    let scope = SearchScope {
        stub: Some(true),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    assert_eq!(result.hits[0].id.name(), "stub-b");
    assert!(result.hits[0].stub);
}

#[test]
fn search_filter_stub_false_excludes_stubs() {
    let mut store = Store::new();
    let real = make_entity("real-a", "specs");
    let stub = make_stub_entity("stub-b", "specs");
    store.upsert(real.id.clone(), real);
    store.upsert(stub.id.clone(), stub);

    let scope = SearchScope {
        stub: Some(false),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    assert_eq!(result.hits[0].id.name(), "real-a");
    assert!(!result.hits[0].stub);
}

#[test]
fn search_filter_stub_intersects_entity_type() {
    let mut store = Store::new();
    let real_spec = make_entity("real-spec", "specs");
    let stub_spec = make_stub_entity("stub-spec", "specs");
    let mut stub_memo = make_stub_entity("stub-memo", "specs");
    stub_memo.entity_type = "memo".into();
    store.upsert(real_spec.id.clone(), real_spec);
    store.upsert(stub_spec.id.clone(), stub_spec);
    store.upsert(stub_memo.id.clone(), stub_memo);

    let scope = SearchScope {
        stub: Some(true),
        entity_type: Some("spec".into()),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(result.total, 1);
    assert_eq!(result.hits[0].id.name(), "stub-spec");
    assert!(result.hits[0].stub);
}

#[test]
fn facets_by_type_omits_empty_bucket_for_stubs() {
    // Production stubs carry `entity_type: ""` (crud::make_stub). When a
    // mixed hit-set reaches compute_facets, the empty string must not
    // surface as its own `by_type` bucket — the type is semantically
    // undefined for a stub. Agents read stub counts from the `stub`
    // filter or memstead_health.stubs, not from the type facet.
    let mut store = Store::new();
    let real = make_entity("real-a", "specs");
    let mut stub = make_stub_entity("stub-b", "specs");
    stub.entity_type = String::new(); // match production make_stub
    store.upsert(real.id.clone(), real);
    store.upsert(stub.id.clone(), stub);

    let result = run_search(&store, &SearchScope::default());
    assert_eq!(result.total, 2, "both entities are in the hit set");
    let facets = result.facets.as_ref().expect("facets present");
    assert_eq!(facets.by_type.get("spec").copied(), Some(1));
    assert!(
        !facets.by_type.contains_key(""),
        "by_type must not expose an empty-string bucket for stubs: {:?}",
        facets.by_type
    );
}

/// A hit's summary is resolved against its *own* mem schema at
/// search time,
/// not the global `default` schema. A `software`-schema `requirement`
/// projects its `Statement` anchor — pre-fix `type_by_name` missed it
/// (requirement isn't a `default`-schema type) and rendered `—`.
#[test]
fn search_summary_uses_per_mem_schema_anchor_section() {
    use memstead_schema::SchemaRegistry;

    let software = SchemaRegistry::builtin()
        .get("software", &semver::Version::new(0, 2, 0))
        .expect("software builtin present");
    let req_type = software.get_type("requirement").expect("requirement type");

    let mut metadata = IndexMap::new();
    metadata.insert("type".into(), MetadataValue::String("requirement".into()));
    let mut sections = IndexMap::new();
    sections.insert(
        "statement".into(),
        "The system shall encrypt tokens at rest.".into(),
    );
    let entity = Entity {
        id: EntityId::new("reqs", "encrypt-tokens"),
        title: "Encrypt tokens".into(),
        entity_type: "requirement".into(),
        mem: "reqs".into(),
        file_path: "encrypt-tokens.md".into(),
        metadata,
        sections,
        relationships: Vec::new(),
        content_hash: "h".into(),
        stub: false,
        stub_kind: None,
        heading_spans: std::collections::HashMap::new(),
        raw_section_headings: Vec::new(),
    };
    let mut store = Store::new();
    store.upsert(entity.id.clone(), entity);

    // Index + per-mem schema map keyed to the *software* schema, so the
    // search op resolves `requirement` against it (not the default schema).
    let mut idx = MemIndex::build_in_ram("reqs".into(), Some(&software)).unwrap();
    for e in store.all_entities() {
        idx.index_entity(e).unwrap();
    }
    idx.commit().unwrap();
    let mut indexes = HashMap::new();
    indexes.insert("reqs".to_string(), idx);
    let mut schemas: HashMap<String, Arc<Schema>> = HashMap::new();
    schemas.insert("reqs".to_string(), software.clone());

    // Metadata-only scan returns the requirement.
    let result = search(
        &store,
        &SearchScope::default(),
        &req_type,
        &indexes,
        &schemas,
    );
    assert_eq!(result.total, 1);
    let summary = result.hits[0]
        .summary
        .as_ref()
        .expect("summary computed at search time");
    assert_eq!(summary.heading, "Statement");
    assert!(
        summary.value.contains("encrypt tokens at rest"),
        "got: {}",
        summary.value
    );

    // The envelope projects the anchor section, not the `—` fallback.
    let envelope = crate::render::build_search_envelope(&result, 0, &|_| {
        crate::render::OriginClass::FirstParty
    });
    assert_eq!(envelope.hits[0].summary_heading, "Statement");
    assert!(
        envelope.hits[0]
            .summary_value
            .contains("encrypt tokens at rest")
    );
}

/// The engine-stamped `created_date` is range-filterable, so the
/// canonical
/// "entities created since X" query works and returns only entities
/// past the bound — pre-fix it warned `FIELD_NOT_RANGE_FILTERABLE`.
#[test]
fn range_filter_on_created_date_works() {
    let mut store = Store::new();
    let mut old = make_entity("old", "specs");
    old.metadata.insert(
        "created_date".into(),
        MetadataValue::String("2020-01-01".into()),
    );
    let mut recent = make_entity("recent", "specs");
    recent.metadata.insert(
        "created_date".into(),
        MetadataValue::String("2026-06-01".into()),
    );
    store.upsert(old.id.clone(), old);
    store.upsert(recent.id.clone(), recent);

    let scope = SearchScope {
        range_filters: HashMap::from([("created_date_after".into(), "2025-01-01".into())]),
        ..Default::default()
    };
    let result = run_search(&store, &scope);
    assert_eq!(
        result.total, 1,
        "only the entity created after the bound matches"
    );
    assert_eq!(result.hits[0].id.name(), "recent");
    assert!(
        result.warnings.is_empty(),
        "created_date is range-filterable — no FIELD_NOT_RANGE_FILTERABLE warning; got {:?}",
        result.warnings
    );
}

/// Build a one-type schema whose `tags` field is a csv-array,
/// equality-filterable metadata field — the shape CLI F8 is about.
fn csv_tag_schema() -> std::sync::Arc<Schema> {
    let manifest = "name: tagtest\nversion: 0.1.0\ndescription: t\nwhen_to_use: t\n\
types:\n  - thing\nrelationships:\n  mode: open\n  definitions:\n    \
- name: PART_OF\n      description: parent\n      default_weight: 3.0\n    \
- name: _default\n      description: fallback\n      default_weight: 1.0\n\
community:\n  resolution: 1.0\n  seed: 42\n";
    let type_yaml = "name: thing\ndescription: t\nwhen_to_use: t\nsections:\n  \
- key: body\n    heading: Body\n    required: true\n    catch_all: true\n    \
search_weight: 1.0\n    write_rules: []\nmetadata_fields:\n  - key: labels\n    \
description: csv labels\n    field_type: string\n    serialization: csv_array\n    \
filterable: equality\n  - key: priority\n    description: prio\n    field_type: string\n    \
enum_values: [low, mid, high]\n    filterable: equality\ntitle_weight: 1.0\ntext_fields:\n  - body\n\
hierarchy_relationship: PART_OF\nno_self_loop_relationships: []\n\
updatable_fields: [title, body, labels]\nhealth_required_fields: []\n\
staleness_threshold_days: 90\nwrite_rules: []\n";
    std::sync::Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest,
            &[("thing".to_string(), type_yaml.to_string())],
        )
        .expect("csv-tag test schema must load"),
    )
}

fn codes_for(filters: &[(&str, &str)]) -> Vec<&'static str> {
    let schema = csv_tag_schema();
    let type_def = schema.get_type("thing").expect("thing type present");
    let type_def = type_def.as_ref();
    let mem_schemas: HashMap<String, Arc<Schema>> = HashMap::new();
    let filters: HashMap<String, String> = filters
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let mut warnings = Vec::new();
    super::collect_equality_filter_warnings(
        &filters,
        type_def,
        None,
        None,
        &mem_schemas,
        &mut warnings,
    );
    warnings.iter().map(|w| w.code()).collect()
}

/// CLI F8 positive: a comma-bearing value on a csv-array field warns
/// `FILTER_VALUE_MULTI_MEMBER` — the silent zero gets a recoverable
/// signal naming the single-member form.
#[test]
fn csv_filter_comma_value_warns_multi_member() {
    let codes = codes_for(&[("labels", "dedup,retry")]);
    assert!(
        codes.contains(&"FILTER_VALUE_MULTI_MEMBER"),
        "comma-bearing csv value must warn; got: {codes:?}",
    );
}

/// CLI F8 complement: a single-member value is the supported shape —
/// no multi-member warning.
#[test]
fn csv_filter_single_member_does_not_warn() {
    let codes = codes_for(&[("labels", "dedup")]);
    assert!(
        !codes.contains(&"FILTER_VALUE_MULTI_MEMBER"),
        "single-member csv value must not warn; got: {codes:?}",
    );
}

/// CLI F8 complement: a genuinely-unknown key still warns
/// `UNKNOWN_FILTER_KEY` (the new advisory is additive, not a
/// replacement).
#[test]
fn unknown_filter_key_still_warns_unknown() {
    let codes = codes_for(&[("nonexistent", "x")]);
    assert!(
        codes.contains(&"UNKNOWN_FILTER_KEY"),
        "unknown key must still warn UNKNOWN_FILTER_KEY; got: {codes:?}",
    );
    assert!(!codes.contains(&"FILTER_VALUE_MULTI_MEMBER"));
}

/// #52: filtering a valid enum-constrained field with a value outside
/// `enum_values` warns `INVALID_ENUM_VALUE`, so a 0-hit result isn't
/// mistaken for a true no-match.
#[test]
fn enum_filter_invalid_value_warns() {
    let codes = codes_for(&[("priority", "urgent")]);
    assert!(
        codes.contains(&"INVALID_ENUM_VALUE"),
        "out-of-enum filter value must warn INVALID_ENUM_VALUE; got: {codes:?}",
    );
}

/// #52 refusal: a valid enum value filters normally — no false warning.
#[test]
fn enum_filter_valid_value_does_not_warn() {
    let codes = codes_for(&[("priority", "high")]);
    assert!(
        !codes.contains(&"INVALID_ENUM_VALUE"),
        "a valid enum value must not warn; got: {codes:?}",
    );
}

/// #52 complement: an unknown field key keeps `UNKNOWN_FILTER_KEY` (the
/// enum check runs only on declared fields), not the enum warning.
#[test]
fn enum_check_does_not_fire_on_unknown_key() {
    let codes = codes_for(&[("nonexistent", "urgent")]);
    assert!(codes.contains(&"UNKNOWN_FILTER_KEY"));
    assert!(!codes.contains(&"INVALID_ENUM_VALUE"));
}
