#![cfg(test)]

use super::*;
use crate::{Entity, EntityId, ListResult, SearchResult};
use indexmap::IndexMap;
use std::collections::HashMap;

fn make_hit(id: &str, title: &str, entity_type: &str, sections: &[(&str, &str)]) -> SearchHit {
    SearchHit {
        id: EntityId(id.to_string()),
        last_modified: None,
        title: title.to_string(),
        mem: id.split("--").next().unwrap_or("").to_string(),
        entity_type: entity_type.to_string(),
        stub: false,
        score: 1.0,
        tokens: 10,
        snippet: None,
        sections: sections
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        score_breakdown: None,
        matched_terms: None,
        expansion: None,
        // Test fixtures exercise the render-time fallback (default-schema
        // lookup); the engine-precomputed path is set in the search op.
        summary: None,
    }
}

fn search_result(hits: Vec<SearchHit>) -> SearchResult {
    let returned = hits.len();
    let total_tokens = hits.iter().map(|h| h.tokens).sum();
    SearchResult {
        total: returned,
        returned,
        offset: 0,
        total_tokens,
        hits,
        facets: None,
        warnings: vec![],
    }
}

fn list_result(hits: Vec<SearchHit>) -> ListResult {
    let returned = hits.len();
    ListResult {
        total: returned,
        returned,
        offset: 0,
        total_tokens: hits.iter().map(|h| h.tokens).sum(),
        hits,
        warnings: vec![],
    }
}

fn test_entity() -> Entity {
    Entity {
            id: EntityId("specs--test-entity".to_string()),
            title: "Test Entity".to_string(),
            entity_type: "spec".to_string(),
            mem: "specs".to_string(),
            file_path: "test-entity.md".to_string(),
            metadata: IndexMap::new(),
            sections: IndexMap::from([
                ("identity".to_string(), "A test entity for unit tests.".to_string()),
                ("purpose".to_string(), "Validates render logic.".to_string()),
                ("specifies".to_string(), "Long section content that adds significant token weight to the full entity estimate.".to_string()),
            ]),
            relationships: vec![],
            content_hash: "abc123".to_string(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
            raw_section_headings: Vec::new(),
        }
}

/// The lite skeleton carries a field's `value_pattern` (wire key
/// `pattern`, as in the full reply) and a type's `last_resort`
/// declaration: both are legality-relevant facts the server
/// instructions promise the skeleton shows, and an agent that
/// planned a write from a skeleton without them was refused on a
/// shape it never saw (B6 grader finding, 2026-09-02). A type
/// declaring neither renders without either key, so schemas that
/// do not use them keep their bytes.
#[test]
fn lite_skeleton_carries_value_pattern_and_last_resort() {
    let manifest = r#"name: flagged
version: 1.0.0
description: legality-flag render fixture
when_to_use: render tests
types:
  - ticket
  - misc
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let ticket = r#"name: ticket
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: ticket_key
    description: The tracker key.
    field_type: string
    value_pattern: "[A-Z]+-[0-9]+"
  - key: owner
    description: Who holds it.
    field_type: string
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
    let misc = r#"name: misc
description: the fallback
when_to_use: tests
last_resort: true
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
    let schema = Arc::new(
        memstead_schema::loader::load_schema_from_memory(
            manifest,
            &[
                ("ticket".to_string(), ticket.to_string()),
                ("misc".to_string(), misc.to_string()),
            ],
        )
        .expect("flag fixture loads"),
    );
    for verbosity in [SchemaVerbosity::Full, SchemaVerbosity::Lite] {
        let payload = build_schema_payload(&schema, vec![], verbosity, OriginClass::FirstParty);
        let types = match verbosity {
            SchemaVerbosity::Full => &payload["types"],
            SchemaVerbosity::Lite => &payload["types_summary"],
        };
        let types = types.as_array().unwrap();
        let ticket = types.iter().find(|t| t["name"] == "ticket").unwrap();
        let misc = types.iter().find(|t| t["name"] == "misc").unwrap();
        let fields = ticket["fields"].as_array().unwrap();
        let keyed = fields.iter().find(|f| f["name"] == "ticket_key").unwrap();
        assert_eq!(
            keyed["pattern"], "[A-Z]+-[0-9]+",
            "{verbosity:?} carries the declared value_pattern"
        );
        let owner = fields.iter().find(|f| f["name"] == "owner").unwrap();
        assert!(
            owner.get("pattern").is_none(),
            "{verbosity:?}: a field without a pattern renders no key"
        );
        assert_eq!(
            misc["last_resort"],
            serde_json::json!(true),
            "{verbosity:?} carries the last_resort declaration"
        );
        assert!(
            ticket.get("last_resort").is_none(),
            "{verbosity:?}: a type not declaring last_resort renders no key"
        );
    }
    // The CLI `type` markdown says it too.
    let md = render_type_info_markdown(&schema.types["misc"]);
    assert!(md.contains("Last resort:"), "{md}");
    let md = render_type_info_markdown(&schema.types["ticket"]);
    assert!(!md.contains("Last resort:"), "{md}");
    assert!(md.contains("pattern: `[A-Z]+-[0-9]+`"), "{md}");
}

#[test]
fn markdown_frontmatter_filters_computed_and_reserved_metadata_keys() {
    // A stored `_hash` metadata key (frontmatter copied out of a read
    // response and written back) must not render as a second `_hash:`
    // line beside the computed one, and the reserved triple stays
    // structural — same predicate as the JSON envelope's metadata map.
    use crate::entity::MetadataValue;
    let mut entity = test_entity();
    entity.metadata.insert(
        "_hash".to_string(),
        MetadataValue::String("stale".to_string()),
    );
    entity.metadata.insert(
        "type".to_string(),
        MetadataValue::String("spec".to_string()),
    );
    entity
        .metadata
        .insert("level".to_string(), MetadataValue::String("M0".to_string()));

    let md = render_entity_markdown(&entity, None);
    assert_eq!(
        md.matches("_hash:").count(),
        1,
        "one computed _hash line, no stored copy"
    );
    assert!(md.contains("_hash: abc123"), "the computed hash wins");
    assert!(
        !md.contains("stale"),
        "the stored _hash value never renders"
    );
    assert!(
        !md.contains("\ntype: "),
        "the reserved triple stays structural"
    );
    assert!(md.contains("level: M0"), "declared metadata still renders");
}

#[test]
fn section_key_to_heading_basic() {
    assert_eq!(section_key_to_heading("identity"), "Identity");
    assert_eq!(section_key_to_heading("current_state"), "Current state");
}

#[test]
fn render_uses_schema_declared_heading_for_non_trivial_casing() {
    // The `ingest.inconsistency` schema declares `claim_a` with
    // heading "Claim A" — the simple key-derivation would produce
    // "Claim a", which would disagree with the on-disk markdown
    // emitted by the generator. The renderer must echo the
    // schema's declared heading verbatim.
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("claim_a".to_string(), "Body A.".to_string());
    sections.insert("claim_b".to_string(), "Body B.".to_string());

    let entity = Entity {
        id: EntityId("ingest--example".to_string()),
        title: "Example".to_string(),
        entity_type: "inconsistency".to_string(),
        mem: "ingest".to_string(),
        file_path: "example.md".to_string(),
        metadata: IndexMap::new(),
        sections,
        relationships: vec![],
        content_hash: "h".to_string(),
        stub: false,
        stub_kind: None,
        heading_spans: std::collections::HashMap::new(),
        raw_section_headings: Vec::new(),
    };

    let md = render_entity_markdown(&entity, None);
    assert!(
        md.contains("## Claim A"),
        "expected schema-declared `## Claim A` heading; got:\n{md}"
    );
    assert!(
        md.contains("## Claim B"),
        "expected schema-declared `## Claim B` heading; got:\n{md}"
    );
    // The naive derivation would have produced lower-case `a`/`b`.
    assert!(
        !md.contains("## Claim a"),
        "renderer must not fall back to key-derivation when the \
             schema declares a heading; got:\n{md}"
    );
}

#[test]
fn render_falls_back_to_key_derivation_for_unknown_types() {
    // When the entity_type is not in any built-in schema (custom
    // workspace schemas, legacy entities), the renderer falls back
    // to the simple key→heading derivation.
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("identity".to_string(), "body".to_string());

    let entity = Entity {
        id: EntityId("custom--example".to_string()),
        title: "Example".to_string(),
        entity_type: "not-a-builtin-type".to_string(),
        mem: "custom".to_string(),
        file_path: "example.md".to_string(),
        metadata: IndexMap::new(),
        sections,
        relationships: vec![],
        content_hash: "h".to_string(),
        stub: false,
        stub_kind: None,
        heading_spans: std::collections::HashMap::new(),
        raw_section_headings: Vec::new(),
    };

    let md = render_entity_markdown(&entity, None);
    assert!(
        md.contains("## Identity"),
        "fallback derivation must produce `## Identity`; got:\n{md}"
    );
}

// Regression lock for deterministic section order. The invariant:
// render_entity_body walks `entity.sections` in IndexMap insertion order,
// so whatever order the parser/caller inserts is what ships. The parser
// inserts in schema-declared order; this test deliberately inserts in
// REVERSE schema order to prove the renderer honors insertion order
// (not the schema's declared order directly).
#[test]
fn render_entity_sections_follow_indexmap_insertion_order() {
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("specifies".to_string(), "S content.".to_string());
    sections.insert("purpose".to_string(), "P content.".to_string());
    sections.insert("identity".to_string(), "I content.".to_string());

    let entity = Entity {
        id: EntityId("specs--order-test".to_string()),
        title: "Order Test".to_string(),
        entity_type: "spec".to_string(),
        mem: "specs".to_string(),
        file_path: "order-test.md".to_string(),
        metadata: IndexMap::new(),
        sections,
        relationships: vec![],
        content_hash: "abc123".to_string(),
        stub: false,
        stub_kind: None,
        heading_spans: std::collections::HashMap::new(),
        raw_section_headings: Vec::new(),
    };

    let md = render_entity_markdown(&entity, None);
    let specifies_pos = md.find("## Specifies").expect("## Specifies must appear");
    let purpose_pos = md.find("## Purpose").expect("## Purpose must appear");
    let identity_pos = md.find("## Identity").expect("## Identity must appear");

    assert!(
        specifies_pos < purpose_pos,
        "Specifies (inserted first) must render before Purpose; got:\n{md}"
    );
    assert!(
        purpose_pos < identity_pos,
        "Purpose (inserted second) must render before Identity; got:\n{md}"
    );
}

/// `_tokens_unfiltered_body` rides only when a section filter
/// narrows the rendered output; it carries the unfiltered-base
/// cost so agents can predict the cost of dropping the filter. The
/// name avoids a monotonic-relationship implication
/// that the opt-in path could invert.
#[test]
fn tokens_reflect_filtered_output() {
    let entity = test_entity();

    // Full render — no filter
    let full = render_entity_markdown(&entity, None);
    assert!(full.contains("_tokens:"), "should have _tokens");
    assert!(
        !full.contains("_tokens_unfiltered_body:"),
        "should NOT have _tokens_unfiltered_body when unfiltered"
    );
    assert!(
        !full.contains("_tokens_full:"),
        "old _tokens_full name must not survive — rename is one-way"
    );

    // Filtered render — request only "identity"
    let filtered = render_entity_markdown(&entity, Some(&["identity".to_string()]));
    assert!(filtered.contains("_tokens:"), "should have _tokens");
    assert!(
        filtered.contains("_tokens_unfiltered_body:"),
        "should have _tokens_unfiltered_body when filtered"
    );
    assert!(
        !filtered.contains("_tokens_full:"),
        "old _tokens_full name must not survive — rename is one-way"
    );

    // Extract token values
    let full_tokens: usize = full
        .lines()
        .find(|l| l.starts_with("_tokens:"))
        .unwrap()
        .trim_start_matches("_tokens: ")
        .parse()
        .unwrap();
    let filtered_tokens: usize = filtered
        .lines()
        .find(|l| l.starts_with("_tokens:"))
        .unwrap()
        .trim_start_matches("_tokens: ")
        .parse()
        .unwrap();
    let tokens_unfiltered_body: usize = filtered
        .lines()
        .find(|l| l.starts_with("_tokens_unfiltered_body:"))
        .unwrap()
        .trim_start_matches("_tokens_unfiltered_body: ")
        .parse()
        .unwrap();

    assert!(
        filtered_tokens < full_tokens,
        "filtered _tokens ({filtered_tokens}) should be less than full _tokens ({full_tokens})"
    );
    assert!(
        tokens_unfiltered_body >= full_tokens,
        "_tokens_unfiltered_body ({tokens_unfiltered_body}) should be >= full render _tokens ({full_tokens})"
    );
}

// -----------------------------------------------------------------------
// Summary line — search rendering
// -----------------------------------------------------------------------

#[test]
fn render_search_uses_first_required_section_for_spec() {
    let hit = make_hit(
        "specs--demo",
        "Demo Spec",
        "spec",
        &[
            ("identity", "A demo spec."),
            ("purpose", "Verifies rendering."),
        ],
    );
    let out = render_search_markdown(&search_result(vec![hit]), 0);
    assert!(
        out.contains("**Identity**: A demo spec."),
        "expected Identity line for spec hit, got:\n{out}"
    );
}

#[test]
fn render_search_uses_first_required_section_for_memo() {
    let hit = make_hit(
        "memos--d1",
        "Memo One",
        "memo",
        &[("claim", "Some claim."), ("context", "Some context.")],
    );
    let out = render_search_markdown(&search_result(vec![hit]), 0);
    assert!(
        out.contains("**Claim**: Some claim."),
        "expected Claim line for memo hit, got:\n{out}"
    );
    assert!(
        !out.contains("**Identity**"),
        "memo hit must not render Identity label"
    );
    assert!(
        !out.contains("**Purpose**"),
        "memo hit must not render Purpose label"
    );
}

#[test]
fn render_search_uses_first_required_section_for_concept() {
    let hit = make_hit(
        "concepts--thing",
        "Thing",
        "concept",
        &[("definition", "A thing."), ("explanation", "Details.")],
    );
    let out = render_search_markdown(&search_result(vec![hit]), 0);
    assert!(
        out.contains("**Definition**: A thing."),
        "expected Definition line for concept hit, got:\n{out}"
    );
}

#[test]
fn render_search_missing_summary_section_shows_dash() {
    // Memo hit with no "claim" section — renderer falls back to em-dash.
    let hit = make_hit("memos--empty", "Empty Memo", "memo", &[]);
    let out = render_search_markdown(&search_result(vec![hit]), 0);
    assert!(
        out.contains("**Claim**: —"),
        "expected Claim dash fallback, got:\n{out}"
    );
}

#[test]
fn render_search_mixes_schemas_in_one_result() {
    let spec_hit = make_hit(
        "specs--s1",
        "Spec One",
        "spec",
        &[("identity", "Spec body.")],
    );
    let memo_hit = make_hit("memos--m1", "Memo One", "memo", &[("claim", "Memo claim.")]);
    let out = render_search_markdown(&search_result(vec![spec_hit, memo_hit]), 0);
    assert!(
        out.contains("**Identity**: Spec body."),
        "spec hit should still render Identity, got:\n{out}"
    );
    assert!(
        out.contains("**Claim**: Memo claim."),
        "memo hit should render Claim in the same output, got:\n{out}"
    );
}

#[test]
fn render_search_unknown_schema_shows_summary_dash() {
    let hit = make_hit("bogus--x", "Bogus", "bogus", &[]);
    let out = render_search_markdown(&search_result(vec![hit]), 0);
    assert!(
        out.contains("**Summary**: —"),
        "unknown schema should render Summary dash, got:\n{out}"
    );
}

#[test]
fn summary_pair_falls_back_when_schema_has_no_required_sections() {
    use memstead_schema::{SectionDef, TypeDefinition};

    let schema = TypeDefinition {
        name: "spec".to_string(),
        description: "test".to_string(),
        when_to_use: "test".to_string(),
        boundaries: vec![],
        exemplar: None,
        legacy_examples: None,
        system_message: None,
        sections: vec![SectionDef {
            key: "note".to_string(),
            heading: "Note".to_string(),
            required: false,
            load_bearing: None,
            search_weight: 1.0,
            catch_all: false,
            write_rules: vec![],
            description: None,
            content: None,
            item_pattern: None,
            table: None,
            example: None,
            format_severity: memstead_schema::ConstraintSeverity::Block,
            compiled_content: None,
            format_problems: Vec::new(),
        }],
        metadata_fields: vec![],
        title_weight: 1.0,
        text_fields: vec![],
        hierarchy_relationship: "PART_OF".to_string(),
        last_resort: false,
        edge_weight_overrides: indexmap::IndexMap::new(),
        edge_weights: indexmap::IndexMap::new(),
        no_self_loop_relationships: vec![],
        legacy_propagating_relationships: None,
        due: None,
        resolution: None,
        leaf: false,
        updatable_fields: vec![],
        health_required_fields: vec![],
        staleness_threshold_days: 90,
        write_rules: vec![],
        required_outgoing: vec![],
        must_reach: vec![],
        signals: vec![],
        constraints: vec![],
        declared_metadata_keys: vec![],
    };

    let mut sections = HashMap::new();
    sections.insert("note".to_string(), "a note".to_string());
    assert_eq!(
        summary_pair(Some(&schema), &sections),
        ("Note".to_string(), "a note".to_string()),
    );

    assert_eq!(
        summary_pair(Some(&schema), &HashMap::new()),
        ("Note".to_string(), "—".to_string()),
    );
}

// -----------------------------------------------------------------------
// Summary line — list rendering (symmetric)
// -----------------------------------------------------------------------

#[test]
fn render_list_uses_first_required_section_for_spec() {
    let hit = make_hit(
        "specs--demo",
        "Demo Spec",
        "spec",
        &[
            ("identity", "A demo spec."),
            ("purpose", "Verifies rendering."),
        ],
    );
    let out = render_list_markdown(&list_result(vec![hit]));
    assert!(
        out.contains("**Identity**: A demo spec."),
        "expected Identity line for spec hit, got:\n{out}"
    );
}

#[test]
fn render_list_uses_first_required_section_for_memo() {
    let hit = make_hit("memos--d1", "Memo One", "memo", &[("claim", "Some claim.")]);
    let out = render_list_markdown(&list_result(vec![hit]));
    assert!(
        out.contains("**Claim**: Some claim."),
        "expected Claim line for memo hit, got:\n{out}"
    );
    assert!(
        !out.contains("**Identity**"),
        "memo hit must not render Identity label in list output"
    );
}

#[test]
fn render_list_uses_first_required_section_for_concept() {
    let hit = make_hit(
        "concepts--thing",
        "Thing",
        "concept",
        &[("definition", "A thing.")],
    );
    let out = render_list_markdown(&list_result(vec![hit]));
    assert!(
        out.contains("**Definition**: A thing."),
        "expected Definition line for concept hit, got:\n{out}"
    );
}

#[test]
fn render_list_missing_summary_section_shows_dash() {
    let hit = make_hit("memos--empty", "Empty Memo", "memo", &[]);
    let out = render_list_markdown(&list_result(vec![hit]));
    assert!(
        out.contains("**Claim**: —"),
        "expected Claim dash fallback in list output, got:\n{out}"
    );
}

#[test]
fn render_list_mixes_schemas_in_one_result() {
    let spec_hit = make_hit(
        "specs--s1",
        "Spec One",
        "spec",
        &[("identity", "Spec body.")],
    );
    let memo_hit = make_hit("memos--m1", "Memo One", "memo", &[("claim", "Memo claim.")]);
    let out = render_list_markdown(&list_result(vec![spec_hit, memo_hit]));
    assert!(
        out.contains("**Identity**: Spec body."),
        "spec hit should still render Identity in list output, got:\n{out}"
    );
    assert!(
        out.contains("**Claim**: Memo claim."),
        "memo hit should render Claim in list output, got:\n{out}"
    );
}

#[test]
fn render_list_unknown_schema_shows_summary_dash() {
    let hit = make_hit("bogus--x", "Bogus", "bogus", &[]);
    let out = render_list_markdown(&list_result(vec![hit]));
    assert!(
        out.contains("**Summary**: —"),
        "unknown schema should render Summary dash in list output, got:\n{out}"
    );
}

// -----------------------------------------------------------------------
// summary_pair — structured-content source of truth
// -----------------------------------------------------------------------

#[test]
fn summary_pair_for_spec_returns_identity() {
    let schema = type_by_name("spec");
    let mut sections = HashMap::new();
    sections.insert("identity".to_string(), "A demo spec.".to_string());
    assert_eq!(
        summary_pair(schema.as_deref(), &sections),
        ("Identity".to_string(), "A demo spec.".to_string()),
    );
}

#[test]
fn summary_pair_for_memo_returns_claim() {
    let schema = type_by_name("memo");
    let mut sections = HashMap::new();
    sections.insert("claim".to_string(), "Memos matter.".to_string());
    assert_eq!(
        summary_pair(schema.as_deref(), &sections),
        ("Claim".to_string(), "Memos matter.".to_string()),
    );
}

#[test]
fn summary_pair_missing_section_returns_dash() {
    let schema = type_by_name("memo");
    assert_eq!(
        summary_pair(schema.as_deref(), &HashMap::new()),
        ("Claim".to_string(), "—".to_string()),
    );
}

#[test]
fn summary_pair_unknown_schema_returns_summary_dash() {
    assert_eq!(
        summary_pair(None, &HashMap::new()),
        ("Summary".to_string(), "—".to_string()),
    );
}

// -----------------------------------------------------------------------
// Envelope serialization — structured-content sidecar
// -----------------------------------------------------------------------

#[test]
fn envelope_serializes_summary_fields() {
    let hit = make_hit(
        "memos--d1",
        "Memo One",
        "memo",
        &[("claim", "Memos matter.")],
    );
    let result = search_result(vec![hit]);
    let envelope = build_search_envelope(&result, 0, &|_| OriginClass::FirstParty);
    let value = serde_json::to_value(&envelope).expect("envelope must serialize");

    // The top-level counters use the `_-prefixed` engine-emitted
    // shape so the wire signals "engine-authored metadata, not
    // user data".
    assert_eq!(value["_total"], 1);
    assert_eq!(value["_returned"], 1);
    assert_eq!(value["_offset"], 0);
    // The envelope is one stable shape: `warnings` is on the wire
    // as `[]` when nothing warned, never elided.
    assert_eq!(
        value["warnings"],
        serde_json::json!([]),
        "empty warnings must serialise as [], got: {value}"
    );

    let hit0 = &value["hits"][0];
    assert_eq!(hit0["summary_heading"], "Claim");
    assert_eq!(hit0["summary_value"], "Memos matter.");
    // Flattened SearchHit fields present.
    assert_eq!(hit0["id"], "memos--d1");
    assert_eq!(hit0["title"], "Memo One");
    assert_eq!(hit0["entity_type"], "memo");
    assert_eq!(hit0["mem"], "memos");
    assert_eq!(hit0["stub"], false);
    assert_eq!(hit0["tokens"], 10);
    assert!(hit0["sections"].is_object());
}

#[test]
fn envelope_roundtrips_through_structured_content() {
    // Mixed-schema result: one spec hit, one memo hit. Both summary pairs
    // must match what summary_pair produces for each schema.
    let spec_hit = make_hit(
        "specs--s1",
        "Spec One",
        "spec",
        &[("identity", "Spec body.")],
    );
    let memo_hit = make_hit("memos--m1", "Memo One", "memo", &[("claim", "Memo claim.")]);
    let result = search_result(vec![spec_hit, memo_hit]);
    let envelope = build_search_envelope(&result, 0, &|_| OriginClass::FirstParty);
    let value = serde_json::to_value(&envelope).expect("envelope must serialize");

    let hits = value["hits"].as_array().expect("hits must be array");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0]["summary_heading"], "Identity");
    assert_eq!(hits[0]["summary_value"], "Spec body.");
    assert_eq!(hits[1]["summary_heading"], "Claim");
    assert_eq!(hits[1]["summary_value"], "Memo claim.");
}

#[test]
fn list_envelope_includes_total_tokens() {
    let hit = make_hit(
        "concepts--c1",
        "Thing",
        "concept",
        &[("definition", "A thing.")],
    );
    let result = list_result(vec![hit]);
    let envelope = build_list_envelope(&result, &|_| OriginClass::FirstParty);
    let value = serde_json::to_value(&envelope).expect("envelope must serialize");

    // `_`-prefixed engine-meta keys, matching the search envelope.
    assert_eq!(value["_total"], 1);
    assert_eq!(value["_total_tokens"], 10);
    assert!(value.get("total").is_none(), "unprefixed keys retired");
    assert_eq!(value["hits"][0]["summary_heading"], "Definition");
    assert_eq!(value["hits"][0]["summary_value"], "A thing.");
}

#[test]
fn envelope_emits_warnings_when_present() {
    let mut result = search_result(vec![]);
    // Search warnings ship as typed `WarningHint` entries (same
    // `{code, details, message}` envelope every other tool uses).
    result.warnings = vec![crate::ops::WarningHint::FieldNotFilterable {
        field: "foo".to_string(),
    }];
    let envelope = build_search_envelope(&result, 0, &|_| OriginClass::FirstParty);
    let value = serde_json::to_value(&envelope).expect("envelope must serialize");
    assert_eq!(value["warnings"][0]["code"], "FIELD_NOT_FILTERABLE");
    assert_eq!(value["warnings"][0]["details"]["field"], "foo");
    assert!(
        value["warnings"][0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("not filterable"))
    );
}

// -----------------------------------------------------------------------
// Per-hit and per-result fields that must appear in the Markdown body.
// -----------------------------------------------------------------------

fn tm(field: &str, snippet: &str, heading_path: Option<&[&str]>) -> TermMatch {
    TermMatch {
        field: field.to_string(),
        snippet: snippet.to_string(),
        heading_path: heading_path.map(|p| p.iter().map(|s| s.to_string()).collect()),
    }
}

fn sample_facets() -> Facets {
    use crate::ops::SubsectionFacet;
    Facets {
        by_type: HashMap::from([
            ("spec".to_string(), 7),
            ("memo".to_string(), 3),
            ("decision".to_string(), 2),
        ]),
        by_mem: HashMap::from([("specs".to_string(), 10), ("memos".to_string(), 2)]),
        by_level: HashMap::from([("high".to_string(), 4)]),
        by_status: HashMap::from([("active".to_string(), 6)]),
        by_confidence: HashMap::from([("medium".to_string(), 3)]),
        by_subsection: vec![
            SubsectionFacet {
                path: vec!["specifies".to_string(), "Response Shapes".to_string()],
                count: 4,
            },
            SubsectionFacet {
                path: vec!["purpose".to_string(), "Rationale".to_string()],
                count: 2,
            },
        ],
        by_expansion: HashMap::from([("primary".to_string(), 8), ("expanded".to_string(), 4)]),
    }
}

#[test]
fn render_search_emits_matched_terms_line() {
    let mut hit = make_hit(
        "specs--e1",
        "Entity One",
        "spec",
        &[("identity", "Body text.")],
    );
    hit.matched_terms = Some(HashMap::from([
        (
            "entity".to_string(),
            vec![
                tm("title", "...entity...", None),
                tm("purpose", "...entity...", None),
                tm("purpose", "...entity two...", None),
            ],
        ),
        ("one".to_string(), vec![tm("title", "...one...", None)]),
    ]));
    let out = render_search_markdown(&search_result(vec![hit]), 0);
    assert!(
        out.contains("**Matched terms:**"),
        "missing Matched terms line; got:\n{out}"
    );
    assert!(
        out.contains("`entity` (purpose×2, title×1)"),
        "entity term grouping wrong; got:\n{out}"
    );
    assert!(
        out.contains("`one` (title×1)"),
        "one term grouping wrong; got:\n{out}"
    );
}

#[test]
fn render_search_emits_score_breakdown_line() {
    let mut hit = make_hit("specs--e1", "Entity", "spec", &[("identity", "b")]);
    hit.score_breakdown = Some(ScoreBreakdown {
        bm25: 2.5,
        title_boost: 2.0,
        field_weights: HashMap::from([("body".to_string(), 0.8), ("purpose".to_string(), 0.3)]),
        expansion_decay: Some(0.5),
    });
    let out = render_search_markdown(&search_result(vec![hit]), 0);
    assert!(
        out.contains(
            "**Score:** bm25 2.5 + title 2.0 + body 0.8 + purpose 0.3 + expansion_decay ×0.5"
        ),
        "score breakdown line wrong; got:\n{out}"
    );
}

#[test]
fn render_search_omits_expansion_decay_when_none() {
    let mut hit = make_hit("specs--e1", "Entity", "spec", &[("identity", "b")]);
    hit.score_breakdown = Some(ScoreBreakdown {
        bm25: 1.5,
        title_boost: 1.0,
        field_weights: HashMap::new(),
        expansion_decay: None,
    });
    let out = render_search_markdown(&search_result(vec![hit]), 0);
    assert!(
        out.contains("**Score:** bm25 1.5 + title 1.0"),
        "base score wrong; got:\n{out}"
    );
    assert!(
        !out.contains("expansion_decay"),
        "expansion_decay must be absent when None; got:\n{out}"
    );
}

#[test]
fn render_search_emits_heading_path_line() {
    let mut hit = make_hit("specs--e1", "Entity", "spec", &[("identity", "b")]);
    hit.matched_terms = Some(HashMap::from([(
        "x".to_string(),
        vec![
            tm("purpose", "...x...", Some(&["Purpose", "Rationale"])),
            tm("purpose", "...x...", Some(&["Purpose", "Rationale"])), // duplicate, dedupe
            tm("specifies", "...x...", Some(&["Specifies", "Responses"])),
        ],
    )]));
    let out = render_search_markdown(&search_result(vec![hit]), 0);
    assert!(
        out.contains("**Heading path:** Purpose › Rationale; Specifies › Responses"),
        "heading path line wrong; got:\n{out}"
    );
}

#[test]
fn render_search_emits_expansion_line() {
    let mut hit = make_hit("specs--e2", "Entity Two", "spec", &[("identity", "b")]);
    hit.expansion = Some(ExpansionInfo {
        of: EntityId("specs--seed".to_string()),
        via_edge: "refines".to_string(),
        via_direction: crate::graph::query::TraversalDirection::Out,
        depth: 1,
    });
    let out = render_search_markdown(&search_result(vec![hit]), 0);
    assert!(
        out.contains("**Expansion:** from `specs--seed` via `refines` [out] (depth 1)"),
        "expansion line reports the traversal direction beside the label; got:\n{out}"
    );
}

#[test]
fn render_search_emits_facets_block() {
    let mut result = search_result(vec![]);
    result.facets = Some(sample_facets());
    let out = render_search_markdown(&result, 0);
    assert!(
        out.contains("## Facets"),
        "facets header missing; got:\n{out}"
    );
    assert!(
        out.contains("- **by_type:** spec=7, memo=3, decision=2"),
        "by_type bucket wrong; got:\n{out}"
    );
    assert!(
        out.contains("- **by_mem:** specs=10, memos=2"),
        "by_mem bucket wrong; got:\n{out}"
    );
    assert!(
        out.contains("- **by_level:** high=4"),
        "by_level bucket wrong; got:\n{out}"
    );
    assert!(
        out.contains("- **by_status:** active=6"),
        "by_status bucket wrong; got:\n{out}"
    );
    assert!(
        out.contains("- **by_confidence:** medium=3"),
        "by_confidence bucket wrong; got:\n{out}"
    );
    assert!(
        out.contains("- **by_expansion:** primary=8, expanded=4"),
        "by_expansion bucket wrong; got:\n{out}"
    );
    assert!(
        out.contains("- **by_subsection:**"),
        "by_subsection header missing; got:\n{out}"
    );
    assert!(
        out.contains("`specifies › Response Shapes`: 4"),
        "subsection facet wrong; got:\n{out}"
    );
}

#[test]
fn render_search_omits_facets_block_when_all_empty() {
    let mut result = search_result(vec![]);
    result.facets = Some(Facets::default());
    let out = render_search_markdown(&result, 0);
    assert!(
        !out.contains("## Facets"),
        "empty facets must not emit header; got:\n{out}"
    );
}

/// Every field the search-tool description promises must be rendered
/// in Markdown. This test exercises all of them in one result and
/// asserts they all appear.
#[test]
fn search_markdown_covers_every_sidecar_field() {
    let mut hit = make_hit(
        "specs--e1",
        "Entity One",
        "spec",
        &[("identity", "Body text.")],
    );
    hit.matched_terms = Some(HashMap::from([(
        "entity".to_string(),
        vec![tm("title", "...entity...", Some(&["Purpose", "Rationale"]))],
    )]));
    hit.score_breakdown = Some(ScoreBreakdown {
        bm25: 1.5,
        title_boost: 1.0,
        field_weights: HashMap::from([("body".to_string(), 0.4)]),
        expansion_decay: Some(0.5),
    });
    hit.expansion = Some(ExpansionInfo {
        of: EntityId("specs--seed".to_string()),
        via_edge: "refines".to_string(),
        via_direction: crate::graph::query::TraversalDirection::Out,
        depth: 2,
    });

    let mut result = search_result(vec![hit]);
    result.facets = Some(sample_facets());

    let out = render_search_markdown(&result, 0);
    for marker in [
        "## Facets",
        "- **by_type:**",
        "- **by_mem:**",
        "- **by_level:**",
        "- **by_status:**",
        "- **by_confidence:**",
        "- **by_expansion:**",
        "- **by_subsection:**",
        "**Matched terms:**",
        "**Score:**",
        "**Heading path:**",
        "**Expansion:**",
    ] {
        assert!(
            out.contains(marker),
            "lockstep marker `{marker}` missing from search markdown; \
                 update render_search_markdown when adding sidecar fields. got:\n{out}"
        );
    }
}

/// The envelope's `relationships[].source` field reads the store's
/// `EdgeSource` discriminator rather than a hardcoded `"explicit"`,
/// which would disagree with the stub-adoption
/// response for alias-synthesised edges (and would be
/// misleading because REFERENCES carries `manual_authoring:
/// forbidden`).
#[test]
fn build_entity_envelope_source_field_reads_edge_source() {
    let mut entity = test_entity();
    let body_link_target = EntityId("specs--body-link-target".to_string());
    let explicit_target = EntityId("specs--explicit-target".to_string());
    entity.relationships = vec![
        crate::entity::Relationship::new("REFERENCES".to_string(), body_link_target.clone()),
        crate::entity::Relationship::new("USES".to_string(), explicit_target.clone()),
    ];

    let edges = vec![
        crate::store::Edge {
            rel_type: "REFERENCES".to_string(),
            target: body_link_target.clone(),
            source: crate::store::EdgeSource::BodyLink,
        },
        crate::store::Edge {
            rel_type: "USES".to_string(),
            target: explicit_target.clone(),
            source: crate::store::EdgeSource::Explicit,
        },
    ];

    let env = build_entity_envelope(
        &entity,
        0,
        None,
        None,
        None,
        OriginClass::FirstParty,
        &edges,
        None,
        None,
        None,
    );
    let relationships = env["relationships"].as_array().expect("array");
    let refs = relationships
        .iter()
        .find(|r| r["rel_type"] == "REFERENCES")
        .expect("REFERENCES present");
    assert_eq!(
        refs["source"], "body_link",
        "alias-synthesised edge must label body_link"
    );
    let uses = relationships
        .iter()
        .find(|r| r["rel_type"] == "USES")
        .expect("USES present");
    assert_eq!(
        uses["source"], "explicit",
        "explicit-authored edge must label explicit"
    );
}

/// The envelope's read contract is structural (cold-start 0-8-0,
/// F9/F13/F15): `origin` is present on every envelope, every
/// relationship entry declares its `direction`, and incoming edges
/// — when the caller passes them — appear as `direction: "in"`
/// entries carrying the other endpoint under `from`. A consumer
/// can therefore always tell whether the block is one-directional.
#[test]
fn build_entity_envelope_carries_origin_direction_and_incoming() {
    let mut entity = test_entity();
    let out_target = EntityId("specs--downstream".to_string());
    entity.relationships = vec![crate::entity::Relationship::new(
        "USES".to_string(),
        out_target.clone(),
    )];
    let edges = vec![crate::store::Edge {
        rel_type: "USES".to_string(),
        target: out_target,
        source: crate::store::EdgeSource::Explicit,
    }];
    let incoming = vec![crate::store::InEdge {
        rel_type: "MANAGES".to_string(),
        from: EntityId("specs--upstream".to_string()),
        source: crate::store::EdgeSource::Explicit,
    }];

    // Without incoming: outgoing entries are direction-labelled.
    let env = build_entity_envelope(
        &entity,
        0,
        None,
        None,
        None,
        OriginClass::ThirdParty,
        &edges,
        None,
        None,
        None,
    );
    assert_eq!(env["origin"], "third-party", "origin is envelope-level");
    let rels = env["relationships"].as_array().expect("array");
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0]["direction"], "out");

    // With incoming: the other half of the neighbourhood appears,
    // direction-labelled, endpoint under `from`.
    let env = build_entity_envelope(
        &entity,
        0,
        None,
        None,
        None,
        OriginClass::FirstParty,
        &edges,
        Some(&incoming),
        None,
        None,
    );
    assert_eq!(env["origin"], "first-party");
    let rels = env["relationships"].as_array().expect("array");
    assert_eq!(rels.len(), 2);
    let inc = rels
        .iter()
        .find(|r| r["direction"] == "in")
        .expect("incoming entry present");
    assert_eq!(inc["rel_type"], "MANAGES");
    assert_eq!(inc["from"], "specs--upstream");
    assert!(
        inc.get("target").is_none(),
        "incoming carries from, not target"
    );
}

/// A relationship whose store edge is missing
/// (transitional drift, store-rebuild lag) falls back to
/// `"explicit"` so the envelope doesn't crash. The fallback is
/// the conservative label — agents already branch on it.
#[test]
fn build_entity_envelope_source_field_falls_back_to_explicit_when_edge_missing() {
    let mut entity = test_entity();
    let target = EntityId("specs--unmapped".to_string());
    entity.relationships = vec![crate::entity::Relationship::new("USES".to_string(), target)];
    let edges: Vec<crate::store::Edge> = Vec::new();
    let env = build_entity_envelope(
        &entity,
        0,
        None,
        None,
        None,
        OriginClass::FirstParty,
        &edges,
        None,
        None,
        None,
    );
    let relationships = env["relationships"].as_array().expect("array");
    assert_eq!(relationships[0]["source"], "explicit");
}

/// Every schema-declared frontmatter key surfaces under the nested
/// `metadata` map — its single home. The four
/// formerly-hoisted scalars are not at the top level; the
/// read-only identity triple (mem/id/type) and underscore-prefixed
/// internal keys are excluded from the nested map.
#[test]
fn build_entity_envelope_nested_metadata_carries_every_schema_field() {
    use crate::entity::MetadataValue;
    let mut entity = test_entity();
    entity.entity_type = "contract".to_string();
    // Pre-fix the envelope dropped every non-promoted key.
    entity.metadata = IndexMap::from([
        ("level".to_string(), MetadataValue::String("M0".to_string())),
        (
            "stability".to_string(),
            MetadataValue::String("stable".to_string()),
        ),
        (
            "created_date".to_string(),
            MetadataValue::String("2026-01-01".to_string()),
        ),
        (
            "last_modified".to_string(),
            MetadataValue::String("2026-05-19".to_string()),
        ),
        (
            "protocol".to_string(),
            MetadataValue::String("https".to_string()),
        ),
        (
            "version".to_string(),
            MetadataValue::String("0.1.0".to_string()),
        ),
        (
            "deprecation_status".to_string(),
            MetadataValue::String("none".to_string()),
        ),
    ]);

    let env = build_entity_envelope(
        &entity,
        0,
        None,
        None,
        None,
        OriginClass::FirstParty,
        &[],
        None,
        None,
        None,
    );

    // Metadata scalars are NOT hoisted to the top level — the
    // nested map is their single home.
    assert!(
        env.get("level").is_none(),
        "level must not be hoisted top-level"
    );
    assert!(
        env.get("stability").is_none(),
        "stability must not be hoisted"
    );
    assert!(
        env.get("created_date").is_none(),
        "created_date must not be hoisted"
    );
    assert!(
        env.get("last_modified").is_none(),
        "last_modified must not be hoisted"
    );
    // The entity's type stays top-level as identity, spelled
    // `entity_type` on the wire (2026-08-28 batch); the retired `type`
    // key is gone, not aliased.
    assert_eq!(env["entity_type"], "contract");
    assert!(
        env.get("type").is_none(),
        "the retired wire key must not survive"
    );

    // Nested map carries every non-internal, non-identity frontmatter key.
    let metadata = env["metadata"].as_object().expect("metadata map");
    assert_eq!(metadata["level"], "M0");
    assert_eq!(metadata["stability"], "stable");
    assert_eq!(metadata["created_date"], "2026-01-01");
    assert_eq!(metadata["last_modified"], "2026-05-19");
    assert_eq!(metadata["protocol"], "https");
    assert_eq!(metadata["version"], "0.1.0");
    assert_eq!(metadata["deprecation_status"], "none");

    // Internal underscore-prefixed keys and the read-only identity
    // triple (mem/id/type) do NOT appear inside the nested map.
    for k in metadata.keys() {
        assert!(
            !k.starts_with('_'),
            "metadata map must not carry underscore-prefixed key `{k}`"
        );
        assert!(
            !["mem", "id", "type"].contains(&k.as_str()),
            "metadata map must not carry identity key `{k}` (it lives top-level)"
        );
    }
}

/// Stub envelopes carry an
/// empty `metadata: {}` map so consumers don't branch on the
/// map's presence.
#[test]
fn build_entity_envelope_stub_carries_empty_metadata_map() {
    let mut entity = test_entity();
    entity.stub = true;
    entity.stub_kind = Some(crate::entity::StubKind::ForwardReference);
    entity.metadata = IndexMap::new();
    let env = build_entity_envelope(
        &entity,
        0,
        None,
        None,
        None,
        OriginClass::FirstParty,
        &[],
        None,
        None,
        None,
    );
    let metadata = env["metadata"]
        .as_object()
        .expect("metadata key present even on stubs");
    assert!(metadata.is_empty(), "stub metadata map must be empty");
}

/// A user-defined schema names a
/// metadata field colliding with structured envelope slots
/// (`sections`, `relationships`). The colliding name surfaces
/// under `metadata.sections` / `metadata.relationships` without
/// disturbing the top-level structured arrays — the nested map
/// decouples user namespace from engine namespace.
#[test]
fn build_entity_envelope_user_field_collisions_isolated_to_nested_map() {
    use crate::entity::MetadataValue;
    let mut entity = test_entity();
    entity.metadata = IndexMap::from([
        (
            "sections".to_string(),
            MetadataValue::String("user-supplied-shadow".to_string()),
        ),
        (
            "relationships".to_string(),
            MetadataValue::String("also-shadowed".to_string()),
        ),
    ]);
    let env = build_entity_envelope(
        &entity,
        0,
        None,
        None,
        None,
        OriginClass::FirstParty,
        &[],
        None,
        None,
        None,
    );
    // Top-level structured slots stay structured.
    assert!(
        env["sections"].is_object(),
        "top-level sections stays a map"
    );
    assert!(
        env["relationships"].is_array(),
        "top-level relationships stays an array"
    );
    // User-supplied collisions land inside the nested map.
    let metadata = env["metadata"].as_object().expect("metadata map");
    assert_eq!(metadata["sections"], "user-supplied-shadow");
    assert_eq!(metadata["relationships"], "also-shadowed");
}

/// `_tokens_unfiltered_body` on the structured envelope rides only
/// when `full_tokens` is supplied (a section filter was active);
/// the legacy `_tokens_full` name is not present as an alias.
#[test]
fn build_entity_envelope_unfiltered_body_token_field_name() {
    let entity = test_entity();
    // Filter-active path — field present under new name.
    let env_filtered = build_entity_envelope(
        &entity,
        10,
        Some(42),
        None,
        None,
        OriginClass::FirstParty,
        &[],
        None,
        None,
        None,
    );
    assert_eq!(env_filtered["_tokens_unfiltered_body"], 42);
    assert!(
        env_filtered.get("_tokens_full").is_none(),
        "_tokens_full must not survive — rename is one-way"
    );
    // No-filter path — field absent under both names.
    let env_unfiltered = build_entity_envelope(
        &entity,
        10,
        None,
        None,
        None,
        OriginClass::FirstParty,
        &[],
        None,
        None,
        None,
    );
    assert!(env_unfiltered.get("_tokens_unfiltered_body").is_none());
    assert!(env_unfiltered.get("_tokens_full").is_none());
}

// ------------------------------------------------------------------
// Schema verbosity (lite vs. full) — Plan 01.
// ------------------------------------------------------------------

/// Load the embedded `software` schema (~42 rel-types, 9 entity
/// types, `alias_target_rel_type: REFERENCES`) — the heaviest builtin,
/// so the lite cut has something to bite into.
fn software_schema() -> Arc<Schema> {
    memstead_schema::builtins::load_builtin_schemas()
        .expect("builtins load")
        .into_iter()
        .find(|s| s.manifest.name == "software")
        .expect("software schema is a builtin")
}

#[test]
fn schema_verbosity_wire_round_trips() {
    assert_eq!(
        SchemaVerbosity::from_wire("full"),
        Some(SchemaVerbosity::Full)
    );
    assert_eq!(
        SchemaVerbosity::from_wire("lite"),
        Some(SchemaVerbosity::Lite)
    );
    assert_eq!(SchemaVerbosity::from_wire("brief"), None);
    assert_eq!(SchemaVerbosity::from_wire(""), None);
    assert_eq!(SchemaVerbosity::Full.as_wire(), "full");
    assert_eq!(SchemaVerbosity::Lite.as_wire(), "lite");
    assert_eq!(SchemaVerbosity::default(), SchemaVerbosity::Full);
}

/// Exemplar serving: `verbosity: full`
/// carries each type's exemplar (title, metadata, sections,
/// relations with placeholder targets); the lite skeleton is
/// BYTE-unchanged between the same schema with and without an
/// exemplar — the per-session lite fetch never grows.
#[test]
fn exemplar_serves_at_full_and_lite_stays_byte_unchanged() {
    let manifest = r#"name: servefix
version: 1.0.0
description: serving fixture
when_to_use: tests
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let base_type = r#"name: sample
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: status
    description: state
    field_type: string
    enum_values: [draft, final]
    optional: true
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
    let with_exemplar = format!(
        "{base_type}exemplar:\n  title: A Conforming Sample\n  metadata:\n    status: draft\n  sections:\n    body: \"One canonical body paragraph.\"\n  relations:\n    - to: parent-placeholder\n      type: PART_OF\n"
    );

    let plain = Arc::new(
        memstead_schema::loader::load_schema_from_memory(
            manifest,
            &[("sample".to_string(), base_type.to_string())],
        )
        .expect("fixture loads"),
    );
    let exemplary = Arc::new(
        memstead_schema::loader::load_schema_from_memory(
            manifest,
            &[("sample".to_string(), with_exemplar)],
        )
        .expect("fixture loads"),
    );

    // FULL serves the exemplar with the type.
    let full = build_schema_payload(
        &exemplary,
        vec![],
        SchemaVerbosity::Full,
        OriginClass::FirstParty,
    );
    let ex = &full["types"][0]["exemplar"];
    assert_eq!(ex["title"], "A Conforming Sample", "{full}");
    assert_eq!(ex["metadata"]["status"], "draft");
    assert_eq!(ex["sections"]["body"], "One canonical body paragraph.");
    assert_eq!(ex["relations"][0]["target"], "parent-placeholder");
    assert_eq!(ex["relations"][0]["rel_type"], "PART_OF");

    // FULL without an exemplar: no key (absent, not null).
    let full_plain = build_schema_payload(
        &plain,
        vec![],
        SchemaVerbosity::Full,
        OriginClass::FirstParty,
    );
    assert!(full_plain["types"][0].get("exemplar").is_none());

    // LITE is byte-identical with and without the exemplar — the
    // skeleton every session fetches does not grow.
    let lite_with = build_schema_payload(
        &exemplary,
        vec![],
        SchemaVerbosity::Lite,
        OriginClass::FirstParty,
    );
    let lite_without = build_schema_payload(
        &plain,
        vec![],
        SchemaVerbosity::Lite,
        OriginClass::FirstParty,
    );
    assert_eq!(
        serde_json::to_string(&lite_with).unwrap(),
        serde_json::to_string(&lite_without).unwrap(),
        "lite must not change when an exemplar exists"
    );
    assert!(
        !serde_json::to_string(&lite_with)
            .unwrap()
            .contains("exemplar"),
        "lite must not mention exemplars at all"
    );
}

/// A first-party schema labels its origin and serves its full prose
/// under `full`. The origin field is additive and present in both
/// verbosities so a consuming host can always read it.
#[test]
fn first_party_origin_is_labelled_and_keeps_prose() {
    let schema = software_schema();
    let full = build_schema_payload(
        &schema,
        vec!["v".into()],
        SchemaVerbosity::Full,
        OriginClass::FirstParty,
    );
    assert_eq!(full["origin"], "first-party");
    // First-party full keeps the prose-instruction fields.
    assert!(full["description"].is_string());
    let t = &full["types"].as_array().unwrap()[0];
    assert!(t.get("system_context").is_some());
    assert!(t.get("writing_guidance").is_some());

    // The origin label rides the lite skeleton too.
    let lite = build_schema_payload(
        &schema,
        vec!["v".into()],
        SchemaVerbosity::Lite,
        OriginClass::FirstParty,
    );
    assert_eq!(lite["origin"], "first-party");
}

/// Declared constraints and `required_outgoing` severities are
/// visible at BOTH verbosity levels — no legality condition may
/// exist that the schema response omits. Complement: a type
/// declaring none renders `constraints: []`, never an absent key.
#[test]
fn constraints_and_severity_render_at_both_verbosities() {
    let manifest = r#"name: constrained
version: 1.0.0
description: constraint render fixture
when_to_use: render tests
types:
  - sample
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let type_yaml = r#"name: sample
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: status
    description: state
    field_type: string
    enum_values: [open, checked]
    optional: true
  - key: checked_by
    description: who
    field_type: string
    optional: true
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
required_outgoing:
  - relationships: [PART_OF]
    cardinality: at_least_one
    severity: block
constraints:
  - kind: requires_when
    field: checked_by
    when_field: status
    when_value: checked
  - kind: unique
    fields: [status, checked_by]
  - kind: enum_from_neighbour
    field: status
    rel_type: PART_OF
    section: body
  - kind: status_propagation
    field: status
    value: checked
    rel_type: PART_OF
    direction: incoming
write_rules: []
"#;
    let schema = Arc::new(
        memstead_schema::loader::load_schema_from_memory(
            manifest,
            &[("sample".to_string(), type_yaml.to_string())],
        )
        .expect("fixture loads"),
    );

    // All five constraint forms (requires_when, unique,
    // enum_from_neighbour, status_propagation here; form 4 is the
    // required_outgoing severity) must be visible with their
    // severity at both verbosity levels.
    let expected_constraints = serde_json::json!([
        {
            "kind": "requires_when",
            "field": "checked_by",
            "when_field": "status",
            "when_value": "checked",
            "severity": "warn",
        },
        {
            "kind": "unique",
            "fields": ["status", "checked_by"],
            "severity": "block",
        },
        {
            "kind": "enum_from_neighbour",
            "field": "status",
            "rel_type": "PART_OF",
            "section": "body",
            "severity": "warn",
        },
        {
            "kind": "status_propagation",
            "field": "status",
            "value": "checked",
            "rel_type": "PART_OF",
            "direction": "incoming",
            "severity": "warn",
        },
    ]);

    let full = build_schema_payload(
        &schema,
        vec![],
        SchemaVerbosity::Full,
        OriginClass::FirstParty,
    );
    let t = &full["types"].as_array().unwrap()[0];
    assert_eq!(t["constraints"], expected_constraints);
    assert_eq!(t["required_outgoing"][0]["severity"], "block");

    let lite = build_schema_payload(
        &schema,
        vec![],
        SchemaVerbosity::Lite,
        OriginClass::FirstParty,
    );
    let ts = &lite["types_summary"].as_array().unwrap()[0];
    assert_eq!(ts["constraints"], expected_constraints);
    assert_eq!(ts["required_outgoing"][0]["severity"], "block");

    // Section-format declarations render at BOTH verbosity
    // levels (plan 08 shares plan 07's no-hidden-legality rule).
    let fmt_manifest = r#"name: formatted
version: 1.0.0
description: format render fixture
when_to_use: render tests
types:
  - plan
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let fmt_type = r#"name: plan
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
  - key: meilensteine
    heading: Meilensteine
    required: false
    search_weight: 5.0
    catch_all: false
    write_rules: []
    content: "(heading(3) list(bullet))+"
    item_pattern: '\*\*(?<name>[^*]+)\*\*'
    example: |
      ### Phase 1
      - **Kickoff**
    format_severity: warn
  - key: tabelle
    heading: Tabelle
    required: false
    search_weight: 5.0
    catch_all: false
    write_rules: []
    content: "table"
    table:
      columns: [Name, Datum]
      column_patterns:
        Datum: '\d{4}-\d{2}-\d{2}'
  - key: belege
    heading: Belege
    required: false
    search_weight: 5.0
    catch_all: false
    write_rules: []
    content: "paragraph+"
    item_pattern: '(?<quelle>\S[^|]*?) \| (?<aussage>.+)'
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
    let fmt_schema = Arc::new(
        memstead_schema::loader::load_schema_from_memory(
            fmt_manifest,
            &[("plan".to_string(), fmt_type.to_string())],
        )
        .expect("format fixture loads"),
    );
    for verbosity in [SchemaVerbosity::Full, SchemaVerbosity::Lite] {
        let payload = build_schema_payload(&fmt_schema, vec![], verbosity, OriginClass::FirstParty);
        let sections_key = match verbosity {
            SchemaVerbosity::Full => &payload["types"][0]["sections"],
            SchemaVerbosity::Lite => &payload["types_summary"][0]["sections"],
        };
        let secs = sections_key.as_array().unwrap();
        let meilensteine = secs
            .iter()
            .find(|s| s["key"] == "meilensteine")
            .expect("declared section present");
        assert_eq!(
            meilensteine["content"], "(heading(3) list(bullet))+",
            "{verbosity:?} carries content"
        );
        assert!(
            meilensteine["item_pattern"]
                .as_str()
                .unwrap()
                .contains("name")
        );
        assert!(
            meilensteine["example"]
                .as_str()
                .unwrap()
                .contains("Kickoff")
        );
        assert_eq!(meilensteine["format_severity"], "warn");
        let tabelle = secs.iter().find(|s| s["key"] == "tabelle").unwrap();
        assert_eq!(tabelle["format_severity"], "block", "default renders");
        assert_eq!(tabelle["table"]["columns"][0], "Name");
        assert!(
            tabelle["table"]["column_patterns"]["Datum"]
                .as_str()
                .is_some()
        );
        let belege = secs.iter().find(|s| s["key"] == "belege").unwrap();
        assert_eq!(belege["content"], "paragraph+");
        assert!(belege["item_pattern"].as_str().unwrap().contains("quelle"));
        let body = secs.iter().find(|s| s["key"] == "body").unwrap();
        assert!(
            body.get("content").is_none() && body.get("format_severity").is_none(),
            "undeclared section keeps its pre-plan shape"
        );
    }

    // Complement: a constraint-free builtin renders the
    // always-present empty list at both levels.
    let plain_full = build_schema_payload(
        &software_schema(),
        vec![],
        SchemaVerbosity::Full,
        OriginClass::FirstParty,
    );
    let pt = &plain_full["types"].as_array().unwrap()[0];
    assert_eq!(pt["constraints"], serde_json::json!([]));
    let plain_lite = build_schema_payload(
        &software_schema(),
        vec![],
        SchemaVerbosity::Lite,
        OriginClass::FirstParty,
    );
    let pts = &plain_lite["types_summary"].as_array().unwrap()[0];
    assert_eq!(pts["constraints"], serde_json::json!([]));
}

/// A third-party schema is de-framed: a `full`-verbosity request is
/// overridden to the structural-only skeleton, so NONE of the
/// prose-instruction fields (`system_context`, `writing_guidance`,
/// section `write_rules`, schema `description` / `when_to_use`,
/// `default_writing_guidance`, rel `description` / `when_to_use`)
/// reach a consuming agent — even though `full` was asked for. The
/// structural skeleton (type/section/field/rel shape) survives so the
/// mem stays understandable and queryable. This is the refusal
/// complement: a `full` request cannot re-admit the prose.
#[test]
fn third_party_origin_forces_structural_only_even_under_full() {
    let schema = software_schema();
    let full_requested = build_schema_payload(
        &schema,
        vec!["v".into()],
        SchemaVerbosity::Full,
        OriginClass::ThirdParty,
    );

    // Origin label.
    assert_eq!(full_requested["origin"], "third-party");

    // Prose-bearing rich arrays are GONE despite the full request;
    // the structural-only summaries are present instead.
    assert!(
        full_requested.get("types").is_none(),
        "third-party omits the rich `types` array even under full"
    );
    assert!(
        full_requested.get("relationships").is_none(),
        "third-party omits the rich `relationships` array even under full"
    );
    assert!(
        full_requested["types_summary"].is_array(),
        "third-party serves the structural `types_summary` skeleton"
    );
    assert!(
        full_requested["relationships_summary"].is_array(),
        "third-party serves the structural `relationships_summary` skeleton"
    );

    // Schema-level prose-instruction fields dropped.
    assert!(
        full_requested.get("description").is_none(),
        "third-party drops schema description prose"
    );
    assert!(
        full_requested.get("when_to_use").is_none(),
        "third-party drops schema when_to_use prose"
    );
    assert!(
        full_requested.get("default_writing_guidance").is_none(),
        "third-party drops default_writing_guidance prose"
    );

    // Per-type prose-instruction fields dropped.
    for t in full_requested["types_summary"].as_array().unwrap() {
        assert!(
            t.get("system_context").is_none(),
            "third-party drops system_context"
        );
        assert!(
            t.get("writing_guidance").is_none(),
            "third-party drops writing_guidance"
        );
        assert!(
            t.get("description").is_none(),
            "third-party drops type description"
        );
        for s in t["sections"].as_array().unwrap() {
            assert!(
                s.get("write_rules").is_none(),
                "third-party drops section write_rules"
            );
        }
    }
    // Per-rel prose dropped.
    for r in full_requested["relationships_summary"].as_array().unwrap() {
        assert!(
            r.get("description").is_none(),
            "third-party drops rel description"
        );
        assert!(
            r.get("when_to_use").is_none(),
            "third-party drops rel when_to_use"
        );
    }

    // A third-party schema served under `full` is byte-identical to
    // the same schema served under `lite` (modulo the origin label,
    // which is identical here) — the override fully collapses to Lite.
    let lite_requested = build_schema_payload(
        &schema,
        vec!["v".into()],
        SchemaVerbosity::Lite,
        OriginClass::ThirdParty,
    );
    assert_eq!(
        full_requested, lite_requested,
        "third-party full must collapse to the lite skeleton"
    );
}

#[test]
fn full_payload_carries_the_rich_arrays_and_prose() {
    let schema = software_schema();
    let full = build_schema_payload(
        &schema,
        vec!["v".into()],
        SchemaVerbosity::Full,
        OriginClass::FirstParty,
    );

    // Full keeps today's contract: rich arrays + schema-level prose.
    assert!(full["types"].is_array(), "full has `types`");
    assert!(full["relationships"].is_array(), "full has `relationships`");
    assert!(
        full.get("types_summary").is_none(),
        "full omits `types_summary`"
    );
    assert!(
        full.get("relationships_summary").is_none(),
        "full omits `relationships_summary`"
    );
    assert!(
        full["description"].is_string(),
        "full keeps schema description"
    );
    assert!(
        full["when_to_use"].is_string(),
        "full keeps schema when_to_use"
    );
    assert_eq!(full["alias_target_rel_type"], "REFERENCES");

    // A full type entry keeps the prose the lite cut drops.
    let t = &full["types"].as_array().unwrap()[0];
    assert!(t["description"].is_string());
    assert!(t.get("writing_guidance").is_some());
    assert!(t.get("system_context").is_some());
    // A full rel entry keeps its prose.
    let r = &full["relationships"].as_array().unwrap()[0];
    assert!(r["description"].is_string());
    assert!(r.get("when_to_use").is_some());
    assert!(r.get("default_weight").is_some());
}

/// The declared `required_outgoing` blocks appear per type — with
/// their relationship lists and cardinality, in declaration order —
/// at BOTH verbosity levels, and a type declaring none reports an
/// empty list (never a missing key). The `project` built-in is the
/// live fixture: `evidence` declares one block, `decision` (among
/// others) declares none. The `no_self_loop_relationships_effect`
/// note ships at both levels and claims nothing beyond the
/// self-loop refusal.
#[test]
fn required_outgoing_reported_with_cardinality_at_both_levels() {
    let reg = memstead_schema::SchemaRegistry::builtin();
    let project = reg
        .get("project", &semver::Version::new(0, 2, 0))
        .expect("project is a built-in");

    for verbosity in [SchemaVerbosity::Full, SchemaVerbosity::Lite] {
        let payload = build_schema_payload(&project, vec![], verbosity, OriginClass::FirstParty);
        let types_key = if verbosity == SchemaVerbosity::Full {
            "types"
        } else {
            "types_summary"
        };
        let types = payload[types_key].as_array().expect("types array");

        let mut saw_evidence = false;
        let mut saw_memo = false;
        for t in types {
            let ro = t
                .get("required_outgoing")
                .unwrap_or_else(|| panic!("type {} omits required_outgoing", t["name"]))
                .as_array()
                .expect("required_outgoing is an array for every type");
            if t["name"] == "evidence" {
                saw_evidence = true;
                assert_eq!(ro.len(), 1, "evidence declares one block");
                assert_eq!(
                    ro[0]["relationships"],
                    serde_json::json!(["STRENGTHENS", "WEAKENS", "VALIDATES", "CONTRADICTS"]),
                    "relationship alternatives in declaration order"
                );
                assert_eq!(
                    ro[0]["cardinality"], "at_least_one",
                    "cardinality rendered as declared — the open upper bound \
                         stays open, never a finite number"
                );
            } else if t["name"] == "memo" {
                // A type declaring no blocks reports the empty
                // list, not a missing key.
                saw_memo = true;
                assert!(ro.is_empty(), "memo declares no blocks → empty list");
            }
        }
        assert!(saw_evidence, "project schema carries the evidence type");
        assert!(saw_memo, "project schema carries the memo type");

        // The effect note for no_self_loop_relationships ships at both
        // levels and states the single real effect.
        let note = payload["no_self_loop_relationships_effect"]
            .as_str()
            .expect("effect note present at both verbosity levels");
        assert!(note.contains("self-loop"), "names the actual effect");
        assert!(
            !note.contains("propagates impact") || note.contains("does not propagate"),
            "claims no propagation behaviour beyond the self-loop refusal"
        );
        assert!(
            note.contains("status_propagation"),
            "deprecation pointer names the real propagation declaration"
        );
    }
}

/// A conditional `required_outgoing` block's trigger (`when_field`
/// / `when_value`) is visible at BOTH verbosity levels — no
/// legality condition the schema response omits — while an
/// unconditional block keeps its byte-identical three-key shape
/// (no `when_*` keys at all).
#[test]
fn conditional_required_outgoing_trigger_visible_at_both_levels() {
    let manifest = r#"name: condro-render
version: 0.1.0
description: conditional required_outgoing render fixture
when_to_use: tests
types:
  - task
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let task_yaml = "name: task\ndescription: t\nwhen_to_use: tests\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: status\n    description: workflow state\n    field_type: string\n    enum_values: [open, checked]\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\n  - status\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\nrequired_outgoing:\n  - relationships: [PART_OF]\n    cardinality: at_least_one\n  - relationships: [PART_OF]\n    cardinality: at_least_one\n    severity: block\n    when_field: status\n    when_value: checked\n";
    let schema = Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest,
            &[("task".to_string(), task_yaml.to_string())],
        )
        .expect("render fixture schema must parse"),
    );

    for verbosity in [SchemaVerbosity::Full, SchemaVerbosity::Lite] {
        let payload = build_schema_payload(&schema, vec![], verbosity, OriginClass::FirstParty);
        let types_key = if verbosity == SchemaVerbosity::Full {
            "types"
        } else {
            "types_summary"
        };
        let task = &payload[types_key].as_array().expect("types array")[0];
        let ro = task["required_outgoing"].as_array().expect("blocks array");
        assert_eq!(ro.len(), 2);
        assert!(
            ro[0].get("when_field").is_none() && ro[0].get("when_value").is_none(),
            "unconditional block carries no when_* keys: {:?}",
            ro[0]
        );
        assert_eq!(ro[1]["when_field"], "status");
        assert_eq!(ro[1]["when_value"], "checked");
    }
}

/// Declared `acyclic_sets` and a `status_propagation` relation
/// set are visible at BOTH verbosity levels; a single-name
/// propagation declaration keeps its `rel_type` key with no
/// `rel_types`, and a schema without sets carries no
/// `acyclic_sets` key at all.
#[test]
fn acyclic_sets_and_propagation_rel_types_visible_at_both_levels() {
    let manifest = r#"name: relsets-render
version: 0.1.0
description: relation-set render fixture
when_to_use: tests
types:
  - claim
relationships:
  mode: strict
  acyclic_sets:
    - [GROUNDS, CONCLUDES]
  definitions:
    - name: GROUNDS
      description: g
      default_weight: 3.0
    - name: CONCLUDES
      description: c
      default_weight: 3.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let claim = "name: claim\ndescription: t\nwhen_to_use: tests\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: standing\n    description: s\n    field_type: string\n    enum_values: [active, withdrawn]\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\n  - standing\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\nconstraints:\n  - kind: status_propagation\n    field: standing\n    value: withdrawn\n    rel_types: [GROUNDS, CONCLUDES]\n    direction: incoming\n  - kind: status_propagation\n    field: standing\n    value: withdrawn\n    rel_type: PART_OF\n    direction: outgoing\n";
    let schema = Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest,
            &[("claim".to_string(), claim.to_string())],
        )
        .expect("render fixture schema must parse"),
    );

    for verbosity in [SchemaVerbosity::Full, SchemaVerbosity::Lite] {
        let payload = build_schema_payload(&schema, vec![], verbosity, OriginClass::FirstParty);
        assert_eq!(
            payload["acyclic_sets"],
            serde_json::json!([["GROUNDS", "CONCLUDES"]]),
            "acyclic_sets present at {verbosity:?}"
        );
        let types_key = if verbosity == SchemaVerbosity::Full {
            "types"
        } else {
            "types_summary"
        };
        let claim = &payload[types_key].as_array().expect("types array")[0];
        let constraints = claim["constraints"].as_array().expect("constraints array");
        assert_eq!(
            constraints[0]["rel_types"],
            serde_json::json!(["GROUNDS", "CONCLUDES"])
        );
        assert!(
            constraints[0].get("rel_type").is_none(),
            "set declaration carries no single-name key: {:?}",
            constraints[0]
        );
        assert_eq!(constraints[1]["rel_type"], "PART_OF");
        assert!(
            constraints[1].get("rel_types").is_none(),
            "single-name declaration stays byte-identical: {:?}",
            constraints[1]
        );
    }

    // A schema without sets carries no `acyclic_sets` key.
    let plain = software_schema();
    for verbosity in [SchemaVerbosity::Full, SchemaVerbosity::Lite] {
        let payload = build_schema_payload(&plain, vec![], verbosity, OriginClass::FirstParty);
        assert!(
            payload.get("acyclic_sets").is_none(),
            "undeclared schema carries no acyclic_sets key"
        );
    }
}

/// The labelling declaration is visible at BOTH verbosity levels
/// with attack set and support walk echoed whole; a schema
/// declaring none carries no `labelling` key at all.
#[test]
fn labelling_declaration_visible_at_both_levels_and_absent_when_undeclared() {
    let manifest = r#"name: labelling-render
version: 0.1.0
description: labelling render fixture
when_to_use: tests
types:
  - claim
relationships:
  mode: strict
  labelling:
    attack: [REBUTS]
    support:
      relationships: [GROUNDS]
      direction: out
      terminal_types: [claim]
  definitions:
    - name: REBUTS
      description: attack
      default_weight: 3.0
    - name: GROUNDS
      description: support
      default_weight: 3.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let claim = "name: claim\ndescription: t\nwhen_to_use: tests\nmetadata_fields: []\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";
    let schema = Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest,
            &[("claim".to_string(), claim.to_string())],
        )
        .expect("render fixture schema must parse"),
    );

    for verbosity in [SchemaVerbosity::Full, SchemaVerbosity::Lite] {
        let payload = build_schema_payload(&schema, vec![], verbosity, OriginClass::FirstParty);
        assert_eq!(
            payload["labelling"]["attack"],
            serde_json::json!(["REBUTS"]),
            "attack set present at {verbosity:?}"
        );
        assert_eq!(
            payload["labelling"]["support"]["relationships"],
            serde_json::json!(["GROUNDS"])
        );
        assert_eq!(payload["labelling"]["support"]["direction"], "out");
    }

    let plain = software_schema();
    for verbosity in [SchemaVerbosity::Full, SchemaVerbosity::Lite] {
        let payload = build_schema_payload(&plain, vec![], verbosity, OriginClass::FirstParty);
        assert!(
            payload.get("labelling").is_none(),
            "undeclared schema carries no labelling key"
        );
    }
}

/// Declared signals are visible at BOTH verbosity levels with the
/// declaration echoed whole; a type declaring none carries no
/// `signals` key at all.
#[test]
fn signal_declarations_visible_at_both_levels_and_absent_when_undeclared() {
    let manifest = r#"name: signals-render
version: 0.1.0
description: signal render fixture
when_to_use: tests
types:
  - claim
  - objection
relationships:
  mode: strict
  definitions:
    - name: REBUTS
      description: r
      default_weight: 3.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let body = "sections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";
    let claim = format!(
        "name: claim\ndescription: t\nwhen_to_use: tests\nmetadata_fields: []\n{body}signals:\n  - name: attack_load\n    kind: edge_load\n    relationships: [REBUTS]\n    direction: in\n    thresholds:\n      - at_least: 1\n        level: notice\n      - at_least: 3\n        level: warn\n"
    );
    let objection = format!(
        "name: objection\ndescription: t\nwhen_to_use: tests\nmetadata_fields:\n  - key: state\n    description: s\n    field_type: string\n    enum_values: [open, closed]\n{body}"
    );
    let schema = Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest,
            &[
                ("claim".to_string(), claim),
                ("objection".to_string(), objection),
            ],
        )
        .expect("render fixture schema must parse"),
    );

    for verbosity in [SchemaVerbosity::Full, SchemaVerbosity::Lite] {
        let payload = build_schema_payload(&schema, vec![], verbosity, OriginClass::FirstParty);
        let types_key = if verbosity == SchemaVerbosity::Full {
            "types"
        } else {
            "types_summary"
        };
        let types = payload[types_key].as_array().expect("types array");
        let claim = types
            .iter()
            .find(|t| t["name"] == "claim")
            .expect("claim type present");
        let sigs = claim["signals"].as_array().expect("signals array");
        assert_eq!(sigs[0]["name"], "attack_load");
        assert_eq!(sigs[0]["kind"], "edge_load");
        assert_eq!(sigs[0]["direction"], "in");
        assert_eq!(sigs[0]["thresholds"][1]["at_least"], 3);
        assert_eq!(sigs[0]["thresholds"][1]["level"], "warn");
        let objection = types
            .iter()
            .find(|t| t["name"] == "objection")
            .expect("objection type present");
        assert!(
            objection.get("signals").is_none(),
            "undeclared type carries no signals key"
        );
    }
}

/// A declared `must_reach` obligation is visible at BOTH verbosity
/// levels with the declaration echoed (relation set, direction,
/// terminal types, depth); a type declaring none carries no
/// `must_reach` key at all (undeclared schemas keep their payload
/// bytes unchanged).
#[test]
fn must_reach_visible_at_both_levels_and_absent_when_undeclared() {
    let manifest = r#"name: mustreach-render
version: 0.1.0
description: must_reach render fixture
when_to_use: tests
types:
  - claim
  - evidence
relationships:
  mode: strict
  definitions:
    - name: GROUNDS
      description: g
      default_weight: 3.0
    - name: PART_OF
      description: hier
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let body = "sections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";
    let claim = format!(
        "name: claim\ndescription: t\nwhen_to_use: tests\n{body}must_reach:\n  - relationships: [GROUNDS]\n    direction: out\n    terminal_types: [evidence]\n    max_depth: 12\n"
    );
    let evidence = format!("name: evidence\ndescription: t\nwhen_to_use: tests\n{body}");
    let schema = Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest,
            &[
                ("claim".to_string(), claim),
                ("evidence".to_string(), evidence),
            ],
        )
        .expect("render fixture schema must parse"),
    );

    for verbosity in [SchemaVerbosity::Full, SchemaVerbosity::Lite] {
        let payload = build_schema_payload(&schema, vec![], verbosity, OriginClass::FirstParty);
        let types_key = if verbosity == SchemaVerbosity::Full {
            "types"
        } else {
            "types_summary"
        };
        let types = payload[types_key].as_array().expect("types array");
        let claim = types
            .iter()
            .find(|t| t["name"] == "claim")
            .expect("claim type present");
        let mr = claim["must_reach"].as_array().expect("obligations array");
        assert_eq!(mr.len(), 1);
        assert_eq!(mr[0]["relationships"], serde_json::json!(["GROUNDS"]));
        assert_eq!(mr[0]["direction"], "out");
        assert_eq!(mr[0]["terminal_types"], serde_json::json!(["evidence"]));
        assert_eq!(mr[0]["max_depth"], 12);
        let evidence = types
            .iter()
            .find(|t| t["name"] == "evidence")
            .expect("evidence type present");
        assert!(
            evidence.get("must_reach").is_none(),
            "undeclared type carries no must_reach key: {evidence:?}"
        );
    }
}

#[test]
fn lite_payload_is_the_structural_skeleton_without_prose() {
    let schema = software_schema();
    let lite = build_schema_payload(
        &schema,
        vec!["v".into()],
        SchemaVerbosity::Lite,
        OriginClass::FirstParty,
    );

    // Heavy arrays under the distinct lite keys; rich keys absent.
    let types = lite["types_summary"]
        .as_array()
        .expect("lite has `types_summary`");
    let rels = lite["relationships_summary"]
        .as_array()
        .expect("lite has `relationships_summary`");
    assert!(lite.get("types").is_none(), "lite omits rich `types`");
    assert!(
        lite.get("relationships").is_none(),
        "lite omits rich `relationships`"
    );

    // Alias pointer + endpoint constraints survive the cut — every
    // flag an agent needs to author a legal write.
    assert_eq!(lite["alias_target_rel_type"], "REFERENCES");

    // Schema-level prose dropped.
    assert!(
        lite.get("description").is_none(),
        "lite drops schema description"
    );
    assert!(
        lite.get("when_to_use").is_none(),
        "lite drops schema when_to_use"
    );
    assert!(
        lite.get("default_writing_guidance").is_none(),
        "lite drops default_writing_guidance"
    );

    // Every entity-type name carries its section keys (with `required`)
    // and field shapes — and NO type/section prose.
    for t in types {
        assert!(t["name"].is_string());
        let sections = t["sections"].as_array().expect("lite type has sections");
        for s in sections {
            assert!(s["key"].is_string(), "section carries its key");
            assert!(s["required"].is_boolean(), "section carries required flag");
            assert!(
                s.get("write_rules").is_none(),
                "lite section drops write_rules prose"
            );
            assert!(s.get("heading").is_none(), "lite section drops heading");
        }
        assert!(
            t.get("description").is_none(),
            "lite type drops description"
        );
        assert!(
            t.get("writing_guidance").is_none(),
            "lite type drops writing_guidance"
        );
        assert!(
            t.get("system_context").is_none(),
            "lite type drops system_context"
        );
        // `no_self_loop_relationships` rides along — it governs the
        // self-loop relate refusal, a write-time refusal lite must let
        // an agent avoid.
        assert!(
            t.get("no_self_loop_relationships").is_some(),
            "lite type keeps no_self_loop_relationships"
        );
        // `required_outgoing` rides along — the only declared
        // legality condition on outgoing edges. Always an array,
        // never an absent key (absence would read as "unknown").
        assert!(
            t.get("required_outgoing").is_some_and(|v| v.is_array()),
            "lite type keeps required_outgoing as an array"
        );
        // Field shapes present (name + required), prose absent.
        if let Some(fields) = t["fields"].as_array() {
            for f in fields {
                assert!(f["name"].is_string());
                assert!(f["required"].is_boolean());
                assert!(
                    f.get("description").is_none(),
                    "lite field drops description"
                );
            }
        }
    }

    // Every relationship name carries its allowed endpoints and the
    // refusal-governing flags — and NO description/when_to_use prose.
    for r in rels {
        assert!(r["name"].is_string());
        assert!(
            r.get("allowed_sources").is_some(),
            "lite rel has allowed_sources"
        );
        assert!(
            r.get("allowed_targets").is_some(),
            "lite rel has allowed_targets"
        );
        assert!(
            r.get("manual_authoring").is_some(),
            "lite rel keeps manual_authoring"
        );
        assert!(r.get("acyclic").is_some(), "lite rel keeps acyclic");
        assert!(
            r.get("per_edge_description").is_some(),
            "lite rel keeps per_edge_description"
        );
        assert!(r.get("description").is_none(), "lite rel drops description");
        assert!(r.get("when_to_use").is_none(), "lite rel drops when_to_use");
        assert!(
            r.get("default_weight").is_none(),
            "lite rel drops default_weight"
        );
    }
}

#[test]
fn lite_is_measurably_smaller_than_full() {
    let schema = software_schema();
    let full = build_schema_payload(
        &schema,
        vec!["v".into()],
        SchemaVerbosity::Full,
        OriginClass::FirstParty,
    );
    let lite = build_schema_payload(
        &schema,
        vec!["v".into()],
        SchemaVerbosity::Lite,
        OriginClass::FirstParty,
    );
    let full_len = serde_json::to_string(&full).unwrap().len();
    let lite_len = serde_json::to_string(&lite).unwrap().len();
    assert!(
        lite_len * 2 < full_len,
        "lite ({lite_len} B) must be well under half of full ({full_len} B)"
    );
}

#[test]
fn lite_full_carry_the_same_type_and_rel_names() {
    // The cut drops prose, never an entity type or a rel-type — an
    // agent orienting on lite sees the full vocabulary.
    let schema = software_schema();
    let full = build_schema_payload(
        &schema,
        vec!["v".into()],
        SchemaVerbosity::Full,
        OriginClass::FirstParty,
    );
    let lite = build_schema_payload(
        &schema,
        vec!["v".into()],
        SchemaVerbosity::Lite,
        OriginClass::FirstParty,
    );

    let names = |arr: &serde_json::Value| -> Vec<String> {
        arr.as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names(&full["types"]), names(&lite["types_summary"]));
    assert_eq!(
        names(&full["relationships"]),
        names(&lite["relationships_summary"])
    );
}

/// Criterion 4 of 04/02, on the surface an agent actually reads. The
/// conformance axis knew, but it is opt-in; a plain `memstead_entity` /
/// `memstead entity --json` returned the swallowed sections as `""` with
/// nothing to distinguish them from sections the author left blank.
#[test]
fn swallowed_sections_carry_a_marker_on_the_plain_read() {
    let mut e = test_entity();
    e.sections.insert(
        "identity".to_string(),
        "intro\n\n```rust\nfn main() {}".to_string(),
    );
    e.sections.insert("purpose".to_string(), String::new());
    let env = build_entity_envelope(
        &e,
        10,
        None,
        None,
        None,
        OriginClass::FirstParty,
        &[],
        None,
        None,
        None,
    );
    let marker = &env["_unread_sections"];
    assert_eq!(marker["reason"], "UNTERMINATED_FENCE");
    assert_eq!(marker["absorbed_into"], "identity");
    assert_eq!(marker["sections"], serde_json::json!(["purpose"]));
}

#[test]
fn an_ordinary_entity_carries_no_unread_marker() {
    // Including one whose empty section is genuinely just empty, and one
    // carrying a CLOSED fence: a marker that fired on either would make
    // every blank section look like data loss.
    for body in ["plain prose", "```rust\nfn main() {}\n```"] {
        let mut e = test_entity();
        e.sections.insert("identity".to_string(), body.to_string());
        e.sections.insert("purpose".to_string(), String::new());
        let env = build_entity_envelope(
            &e,
            10,
            None,
            None,
            None,
            OriginClass::FirstParty,
            &[],
            None,
            None,
            None,
        );
        assert!(
            env.get("_unread_sections").is_none(),
            "body {body:?} produced a marker"
        );
    }
}
