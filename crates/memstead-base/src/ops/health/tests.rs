#![cfg(test)]

use super::*;
use crate::entity::{Entity, EntityId, MetadataValue};
use crate::ops::DanglingLinkKind;
use crate::store::Store;
use indexmap::IndexMap;
use memstead_schema::type_by_name;

/// A bullet inside a code block is an example of the list, not a
/// member of it. `enum_from_neighbour` harvests legal values from a
/// neighbour's section body, so an unmasked scan would accept any
/// value someone documented in a fenced sample.
#[test]
fn bullet_entries_ignores_code() {
    let body = "- real-one\n- real-two\n\n```\n- fenced-ghost\n```\n\n    - indented-ghost\n\nA `- span-ghost` sample.\n";
    let entries = bullet_entries(body);
    assert_eq!(
        entries,
        vec!["real-one".to_string(), "real-two".to_string()],
        "only prose bullets are legal values: {entries:?}"
    );
}

/// Complement: indentation, `*` markers and inline formatting in a
/// prose bullet all still read exactly as before — the entry text
/// comes from the original, not the mask.
#[test]
fn bullet_entries_still_reads_prose_bullets_verbatim() {
    let entries = bullet_entries("- alpha\n  * beta\n* `gamma`\n");
    assert_eq!(
        entries,
        vec![
            "alpha".to_string(),
            "beta".to_string(),
            "`gamma`".to_string()
        ]
    );
}

/// A destination config
/// declaring its process mem resolves the pairing regardless of
/// naming — with no binding at all — and a declaration naming a
/// missing mem surfaces as the typed finding, never a silent
/// fallback.
#[test]
fn declared_process_mem_pairs_and_missing_declaration_is_typed() {
    use crate::engine::test_helpers::folder_mount;
    let tmp = tempfile::TempDir::new().unwrap();
    let dest_dir = tmp.path().join("dest");
    let proc_dir = tmp.path().join("oddly-named-process");
    std::fs::create_dir_all(dest_dir.join(".memstead")).unwrap();
    std::fs::create_dir_all(&proc_dir).unwrap();
    // Declaration: the destination pairs with a mem whose name no
    // convention would derive.
    std::fs::write(
        dest_dir.join(".memstead").join("config.json"),
        r#"{ "schema": "default@1.0.0", "processMem": "oddly-named-process" }"#,
    )
    .unwrap();
    let engine = crate::Engine::from_mounts(vec![
        (
            folder_mount("dest", dest_dir.clone()),
            Box::new(crate::storage::FilesystemBackend::new(dest_dir.clone()))
                as Box<dyn crate::backend::MemBackend>,
        ),
        (
            folder_mount("oddly-named-process", proc_dir.clone()),
            Box::new(crate::storage::FilesystemBackend::new(proc_dir))
                as Box<dyn crate::backend::MemBackend>,
        ),
    ])
    .unwrap();

    // The one resolution function: declaration wins.
    let r = crate::binding_run::resolve_process_mem(&engine, "dest", "dest-derived");
    assert!(r.declared && r.mounted);
    assert_eq!(r.mem, "oddly-named-process");
    // No declaration → derivation fallback, byte-identical to the
    // pre-declaration behaviour.
    let r = crate::binding_run::resolve_process_mem(&engine, "oddly-named-process", "whatever");
    assert!(!r.declared && !r.mounted);
    assert_eq!(r.mem, "whatever");

    // The axis pairs the declared mem with no binding present.
    let axis = health_open_questions_axis(&engine, Some("dest"));
    let process = &axis["dest"]["process"];
    assert_eq!(process[0]["process_mem"], "oddly-named-process", "{axis}");
    assert_eq!(process[0]["declared"], true, "{axis}");
    assert_eq!(process[0]["resolvable"], true, "{axis}");

    // Declaration naming a missing mem: typed finding.
    std::fs::write(
        dest_dir.join(".memstead").join("config.json"),
        r#"{ "schema": "default@1.0.0", "processMem": "nowhere" }"#,
    )
    .unwrap();
    let engine2 = crate::Engine::from_mounts(vec![(
        folder_mount("dest", dest_dir.clone()),
        Box::new(crate::storage::FilesystemBackend::new(dest_dir))
            as Box<dyn crate::backend::MemBackend>,
    )])
    .unwrap();
    let axis = health_open_questions_axis(&engine2, Some("dest"));
    let process = &axis["dest"]["process"];
    assert_eq!(
        process[0]["finding"], "DECLARED_PROCESS_MEM_MISSING",
        "{axis}"
    );
    assert_eq!(process[0]["resolvable"], false, "{axis}");
}

/// The independence gate compares
/// caller-declared identities and nothing else (with
/// the transport complement): equal identities read
/// `self_checked` even across DIFFERING `(actor, client)` pairs,
/// differing identities read `confirmed_independent` even on the
/// SAME pair, a missing identity on either side reads
/// `unconfirmable` — the pair provably does not participate.
#[test]
fn independence_gate_compares_identities_only() {
    use crate::engine::test_helpers::folder_mount;
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("gate");
    std::fs::create_dir_all(&dir).unwrap();
    let mut engine = crate::Engine::from_mounts(vec![(
        folder_mount("gate", dir.clone()),
        Box::new(crate::storage::FilesystemBackend::new(dir))
            as Box<dyn crate::backend::MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(tmp.path().to_path_buf());

    let create = |engine: &mut crate::Engine, title: &str, identity: Option<&str>| {
        engine.set_identity(identity.map(str::to_string));
        engine
            .create_entity(
                crate::CreateEntityArgs {
                    mem: "gate".to_string(),
                    title: title.to_string(),
                    entity_type: "spec".to_string(),
                    sections: [
                        ("identity".to_string(), "x".to_string()),
                        ("purpose".to_string(), "y".to_string()),
                    ]
                    .into_iter()
                    .collect(),
                    metadata: Default::default(),
                    relations: Vec::new(),
                    anchors: Vec::new(),
                    dry_run: false,
                },
                crate::vcs::Actor::Cli,
                None,
                None,
            )
            .unwrap()
            .id
            .0
    };
    let a = create(&mut engine, "Self Checked", Some("alice"));
    let b = create(&mut engine, "Independent", Some("alice"));
    let c = create(&mut engine, "No Author Identity", None);

    let check = |engine: &mut crate::Engine,
                 id: &str,
                 identity: Option<&str>,
                 actor: crate::vcs::Actor,
                 client: Option<&crate::vcs::ClientId>| {
        engine.set_identity(identity.map(str::to_string));
        engine
            .record_check(
                "gate",
                id,
                crate::check::Verdict::Ok,
                crate::check::CheckKind::Verification,
                None,
                actor,
                client,
            )
            .unwrap();
    };
    let other_client = crate::vcs::ClientId {
        name: "claude-code".into(),
        version: "9.9".into(),
    };
    // a: authored (cli, no client), checked (agent, claude-code) —
    // DIFFERING pairs, equal identities → self_checked.
    check(
        &mut engine,
        &a,
        Some("alice"),
        crate::vcs::Actor::Agent,
        Some(&other_client),
    );
    // b: SAME pair as its author record, differing identities →
    // confirmed_independent.
    check(&mut engine, &b, Some("bob"), crate::vcs::Actor::Cli, None);
    // c: checker declared, author never did → unconfirmable.
    check(&mut engine, &c, Some("carol"), crate::vcs::Actor::Cli, None);

    let axis = health_checks_axis(&engine, Some("gate"));
    let gate = &axis["gate"]["independence"];
    assert_eq!(
        gate["self_checked"]["items"],
        serde_json::json!([a]),
        "{axis}"
    );
    assert_eq!(
        gate["confirmed_independent"]["items"],
        serde_json::json!([b]),
        "{axis}"
    );
    assert_eq!(
        gate["unconfirmable"]["items"],
        serde_json::json!([c]),
        "{axis}"
    );

    // The recorded identity is served by the provenance block and
    // the check record (the engine half of the gate).
    let prov = engine.entity_provenance("gate", &a).unwrap();
    assert_eq!(
        prov.created_by.as_ref().and_then(|r| r.identity.as_deref()),
        Some("alice"),
        "created-by serves the declared identity"
    );
    assert_eq!(
        prov.last_check.as_ref().and_then(|r| r.identity.as_deref()),
        Some("alice"),
        "the check record serves the declared identity"
    );
}

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
        raw_section_headings: Vec::new(),
    }
}

/// A sealed-violator type: section key `answers` with heading
/// `Answers argued` (derives to `answers_argued`) — the plenum
/// finding's exact shape. Loads fine; only new installs refuse.
fn violating_type() -> std::sync::Arc<TypeDefinition> {
    let manifest = r#"name: debate
version: 0.1.0
description: sealed-violator fixture
when_to_use: health tests
types:
  - question
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
    let type_yaml = r#"name: question
description: t
when_to_use: tests
sections:
  - key: answers
    heading: Answers argued
    required: true
    search_weight: 10.0
    write_rules: []
  - key: notes
    heading: Notes
    required: false
    search_weight: 3.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - answers
  - notes
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - answers
  - notes
health_required_fields:
  - answers
staleness_threshold_days: 90
write_rules: []
"#;
    memstead_schema::load_schema_from_memory(
        manifest,
        &[("question".to_string(), type_yaml.to_string())],
    )
    .expect("violating schema still loads")
    .get_type("question")
    .expect("question type")
}

/// Health must report the distinct SECTION_HEADING_MISMATCH finding
/// — naming both headings and the catch-all the content landed in —
/// for content sitting under a non-deriving heading, and must NOT
/// report that section as missing. A genuinely absent section keeps
/// the missing report; a conforming entity gets neither.
#[test]
fn health_distinguishes_heading_mismatch_from_missing_section() {
    let schema = violating_type();

    // Content present under the declared (non-deriving) heading.
    let md = "---\ntype: question\n---\n# Q\n\n## Answers argued\n\nTwo answers.\n";
    let parsed = crate::entity::parser::parse_markdown(md, "q.md", &schema, "debate")
        .expect("parses")
        .entity;
    let report = entity_health(&parsed, &schema);
    let mismatch: Vec<_> = report
        .issues
        .iter()
        .filter(|i| i.code == super::super::HealthIssueCode::SectionHeadingMismatch)
        .collect();
    assert_eq!(mismatch.len(), 1, "issues: {:?}", report.issues);
    let msg = &mismatch[0].message;
    assert!(
        msg.contains("'Answers argued'") && msg.contains("'answers_argued'"),
        "names found heading and derived key: {msg}"
    );
    assert!(
        msg.contains("'notes'"),
        "names the catch-all landing: {msg}"
    );
    assert!(
        !report.issues.iter().any(|i| i.message.contains("is empty")),
        "must not also report the section as missing: {:?}",
        report.issues
    );

    // Genuinely missing section: missing report exactly as today.
    let md_missing = "---\ntype: question\n---\n# Q2\n";
    let parsed_missing =
        crate::entity::parser::parse_markdown(md_missing, "q2.md", &schema, "debate")
            .expect("parses")
            .entity;
    let report_missing = entity_health(&parsed_missing, &schema);
    assert!(
        report_missing
            .issues
            .iter()
            .any(|i| i.code == super::super::HealthIssueCode::Missing
                && i.message == "required section 'answers' is empty"),
        "absent section keeps the missing report (structured MISSING code): {:?}",
        report_missing.issues
    );
    assert!(
        !report_missing
            .issues
            .iter()
            .any(|i| i.code == super::super::HealthIssueCode::SectionHeadingMismatch),
        "no mismatch finding when the heading is not in the file"
    );

    // Conforming entity (content under a heading deriving to the
    // key would need a deriving heading — for this violating
    // schema no heading can reach `answers`, so use the conforming
    // catch-all only): neither finding for a section with content.
    let ok_type = crate::entity::parser::parse_markdown(
        "---\ntype: question\n---\n# Q3\n\n## Answers\n\nfree.\n",
        "q3.md",
        &schema,
        "debate",
    )
    .expect("parses")
    .entity;
    let report_ok = entity_health(&ok_type, &schema);
    assert!(
        !report_ok
            .issues
            .iter()
            .any(|i| i.code == super::super::HealthIssueCode::SectionHeadingMismatch),
        "mismatch fires only when the declared heading is present: {:?}",
        report_ok.issues
    );
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
        raw_section_headings: Vec::new(),
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
    let summary = compute_health(&store, schema, &HashMap::new(), None);
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
        .get("software", &semver::Version::new(0, 2, 0))
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
    let summary = compute_health(&store, schema, &mem_schemas, None);
    let report = summary
        .missing_fields
        .iter()
        .find(|r| r.id.as_ref() == "specs--bad-owns-source")
        .expect("shape-violating entity must surface");
    let issue = report
        .issues
        .iter()
        .find(|i| i.field == "relationships" && i.message.contains("INVALID_REL_SHAPE"))
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
        .get("software", &semver::Version::new(0, 2, 0))
        .expect("software schema ships as a builtin");

    let mut store = Store::new();
    let mut owner = make_entity("owner", true);
    owner.entity_type = "actor".into();
    owner
        .metadata
        .insert("kind".into(), MetadataValue::String("team".into()));
    owner
        .metadata
        .insert("active".into(), MetadataValue::Bool(true));
    owner
        .metadata
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
    let summary = compute_health(&store, schema, &mem_schemas, None);
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
    let summary = compute_health(&store, schema, &mem_schemas, None);
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
    entity.sections.insert(section_key.into(), body.to_string());
    entity
}

#[test]
fn dangling_link_detected_after_delete() {
    use crate::entity::store_builder::make_stub;

    let mut store = Store::new();
    let a = make_entity_with_body("a", "purpose", "Refers to [[b]] in prose.");
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
    assert_eq!(d.kind, DanglingLinkKind::LinkTargetMissing);
}

/// 04/06: the three conditions the fused `DANGLING_LINK` name used
/// to cover are discriminated at the producer, and each carries its
/// own repair. Two of them (a body link to a written entity with no
/// relationship, and a relationship row whose target is absent from
/// the store) were not separable at all before — the payload
/// distinguished them only by whether `section` happened to be
/// null, which is a rendering accident, not a condition.
#[test]
fn the_three_dangling_conditions_are_discriminated() {
    use crate::entity::store_builder::make_stub;

    let mut store = Store::new();

    // (1) Body link whose target has no markdown file at all.
    let gone = make_entity_with_body("gone-link", "purpose", "See [[absent]].");
    store.upsert(gone.id.clone(), gone.clone());
    let absent = EntityId::new("specs", "absent");
    store.upsert(absent.clone(), make_stub(absent.clone()));

    // (2) Body link to a fully written entity that the source does
    // not relate to. The target exists; the edge does not.
    let written = make_entity("written", true);
    store.upsert(written.id.clone(), written.clone());
    let unrelated = make_entity_with_body("unrelated-link", "purpose", "See [[written]].");
    store.upsert(unrelated.id.clone(), unrelated.clone());

    // (3) Relationship row naming an entity that is not in the
    // store at all — not even as a stub. Auto-stubbing means this
    // only arises from out-of-band edits or historical corruption.
    let mut rel_source = make_entity("rel-source", true);
    rel_source.relationships.push(crate::entity::Relationship {
        rel_type: "DEPENDS_ON".to_string(),
        target: EntityId::new("specs", "vanished"),
        description: None,
    });
    store.upsert(rel_source.id.clone(), rel_source.clone());

    let found = super::collect_dangling_links(&store, None);
    let kind_of = |from: &str| {
        found
            .iter()
            .find(|d| d.from.path() == from)
            .unwrap_or_else(|| panic!("no dangling link from {from}: {found:?}"))
            .kind
    };
    assert_eq!(kind_of("gone-link"), DanglingLinkKind::LinkTargetMissing);
    assert_eq!(kind_of("unrelated-link"), DanglingLinkKind::LinkNotRelated);
    assert_eq!(
        kind_of("rel-source"),
        DanglingLinkKind::RelationTargetMissing
    );

    // Three conditions, three codes, three repairs: no two of them
    // collapse onto the same string.
    let codes: std::collections::BTreeSet<_> = found.iter().map(|d| d.kind.code()).collect();
    let repairs: std::collections::BTreeSet<_> = found.iter().map(|d| d.kind.repair()).collect();
    assert_eq!(codes.len(), 3, "{found:?}");
    assert_eq!(repairs.len(), 3, "{found:?}");
}

/// The relationships row keeps its tolerance.
/// A row whose target is a stub is a legitimate forward reference —
/// the alias machinery auto-stubs absent targets by design — and the
/// split must not start reporting them. The body scan treats the
/// same stub as missing, which is the whole reason the two axes
/// needed separate codes rather than one.
#[test]
fn a_relationship_row_pointing_at_a_stub_is_still_not_flagged() {
    use crate::entity::store_builder::make_stub;

    let mut store = Store::new();
    let stub_id = EntityId::new("specs", "forward");
    store.upsert(stub_id.clone(), make_stub(stub_id.clone()));

    let mut source = make_entity("forward-ref", true);
    source.relationships.push(crate::entity::Relationship {
        rel_type: "DEPENDS_ON".to_string(),
        target: stub_id.clone(),
        description: None,
    });
    store.upsert(source.id.clone(), source.clone());

    assert!(
        super::collect_dangling_links(&store, None).is_empty(),
        "a forward reference through the relationships table stays unflagged"
    );

    // The complement of the complement: the SAME stub reached from a
    // body wiki-link is the target-missing condition.
    let body = make_entity_with_body("body-ref", "purpose", "See [[forward]].");
    store.upsert(body.id.clone(), body.clone());
    let found = super::collect_dangling_links(&store, None);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].from, body.id);
    assert_eq!(found[0].kind, DanglingLinkKind::LinkTargetMissing);
}

/// Decision 18: dangling-links and stubs
/// output is deterministic — the store iterates a HashMap, so the
/// collectors sort before serving. Two independently built
/// identical stores must produce byte-identical lists, in the
/// documented (from, target, section) / id order.
#[test]
fn dangling_links_and_stubs_serve_in_deterministic_order() {
    use crate::entity::store_builder::make_stub;

    let build = || {
        let mut store = Store::new();
        // Insert in an order unrelated to the expected output order.
        for name in ["zeta", "alpha", "mid"] {
            let e = make_entity_with_body(
                name,
                "purpose",
                &format!("See [[gone-{name}]] and [[lost-{name}]]."),
            );
            store.upsert(e.id.clone(), e);
        }
        for name in ["zeta", "alpha", "mid"] {
            for pre in ["gone", "lost"] {
                let id = EntityId::new("specs", &format!("{pre}-{name}"));
                store.upsert(id.clone(), make_stub(id));
            }
        }
        store
    };

    let store_a = build();
    let store_b = build();

    let key =
        |d: &super::DanglingLink| (d.from.0.clone(), d.target_id.0.clone(), d.section.clone());
    let dangling_a: Vec<_> = super::collect_dangling_links(&store_a, None)
        .iter()
        .map(key)
        .collect();
    let dangling_b: Vec<_> = super::collect_dangling_links(&store_b, None)
        .iter()
        .map(key)
        .collect();
    assert_eq!(dangling_a, dangling_b, "identical stores, identical order");
    let mut sorted = dangling_a.clone();
    sorted.sort();
    assert_eq!(dangling_a, sorted, "served pre-sorted by (from, target)");
    assert_eq!(dangling_a.len(), 6);

    let stub_ids = |s: &Store| -> Vec<String> {
        crate::graph::query::find_stubs(s)
            .into_iter()
            .map(|(id, _)| id.0)
            .collect()
    };
    let stubs_a = stub_ids(&store_a);
    assert_eq!(stubs_a, stub_ids(&store_b), "stub order is deterministic");
    let mut sorted = stubs_a.clone();
    sorted.sort();
    assert_eq!(stubs_a, sorted, "stubs served pre-sorted by id");
    assert_eq!(stubs_a.len(), 6);
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
    let mut a = make_entity_with_body("a", "purpose", "Refers to [[b]] in prose.");
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
    assert_eq!(
        dangling.len(),
        1,
        "exactly one relationship-section dangler"
    );
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
    let mut a = make_entity_with_body("a", "purpose", "Refers to [[b]] in prose.");
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
    let a = make_entity_with_body("a", "purpose", "Refers to [[gone]] in prose.");
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
    let body_section = "sections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";
    let decision_yaml = format!(
        "name: decision\ndescription: t\nwhen_to_use: Here\n{body_section}required_outgoing:\n  - relationships: [CHOSEN]\n    cardinality: at_least_one\n  - relationships: [REJECTED]\n    cardinality: at_least_one\n",
    );
    let note_yaml = format!("name: note\ndescription: t\nwhen_to_use: Here\n{body_section}",);
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
    metadata.insert("type".into(), MetadataValue::String(entity_type.into()));
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
        raw_section_headings: Vec::new(),
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

    let reports = collect_missing_required_outgoing(&store, None, &mem_schemas);
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

    let alpha_only = collect_missing_required_outgoing(&store, Some("alpha"), &mem_schemas);
    assert_eq!(alpha_only.len(), 1);
    assert_eq!(alpha_only[0].mem, "alpha");

    let both = collect_missing_required_outgoing(&store, None, &mem_schemas);
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

/// A conditional block arms only on the trigger value: the sweep
/// reports the armed violator (with the trigger named in the
/// block entry), and skips both the other-value and the
/// edge-satisfied entities.
#[test]
fn missing_required_outgoing_conditional_blocks_arm_on_trigger() {
    use crate::entity::MetadataValue;
    let manifest = r#"name: tests-ro-cond
version: 0.1.0
description: conditional required_outgoing health test schema
when_to_use: tests
types:
  - task
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: Hier
      default_weight: 3.0
    - name: _default
      description: Fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let task_yaml = "name: task\ndescription: t\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: status\n    description: workflow state\n    field_type: string\n    enum_values: [open, checked]\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\n  - status\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\nrequired_outgoing:\n  - relationships: [PART_OF]\n    cardinality: at_least_one\n    when_field: status\n    when_value: checked\n";
    let schema = std::sync::Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest,
            &[("task".to_string(), task_yaml.to_string())],
        )
        .expect("conditional ro fixture schema must parse"),
    );

    let mut store = Store::new();
    let mut armed = make_typed_entity("plan", "armed", "task");
    armed
        .metadata
        .insert("status".into(), MetadataValue::String("checked".into()));
    let mut other_value = make_typed_entity("plan", "quiet", "task");
    other_value
        .metadata
        .insert("status".into(), MetadataValue::String("open".into()));
    let unset = make_typed_entity("plan", "blank", "task");
    let parent = make_typed_entity("plan", "parent", "task");
    let mut satisfied = make_typed_entity("plan", "wired", "task");
    satisfied
        .metadata
        .insert("status".into(), MetadataValue::String("checked".into()));
    satisfied.relationships.push(crate::entity::Relationship {
        rel_type: "PART_OF".into(),
        target: parent.id.clone(),
        description: None,
    });
    for e in [armed.clone(), other_value, unset, parent, satisfied] {
        store.upsert(e.id.clone(), e);
    }

    let mut mem_schemas = HashMap::new();
    mem_schemas.insert("plan".to_string(), schema);

    let reports = collect_missing_required_outgoing(&store, None, &mem_schemas);
    assert_eq!(
        reports.len(),
        1,
        "only the armed edge-less entity is reported; got {reports:?}"
    );
    let r = &reports[0];
    assert_eq!(r.id, armed.id);
    assert_eq!(r.missing.len(), 1);
    assert_eq!(r.missing[0].when_field.as_deref(), Some("status"));
    assert_eq!(r.missing[0].when_value.as_deref(), Some("checked"));
}

// ----------------------------------------------------------------------
// must_reach reachability obligations
// ----------------------------------------------------------------------

/// Three-type argument-shaped fixture: claim / inference /
/// evidence over GROUNDS / CONCLUDES. The per-type `must_reach`
/// blocks are injected by the caller (empty string = none).
fn must_reach_schema(
    claim_extra: &str,
    inference_extra: &str,
) -> std::sync::Arc<memstead_schema::Schema> {
    let manifest = r#"name: tests-must-reach
version: 0.1.0
description: must_reach health test schema
when_to_use: tests
types:
  - claim
  - inference
  - evidence
relationships:
  mode: strict
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
    let body = "sections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";
    let claim = format!("name: claim\ndescription: t\nwhen_to_use: Here\n{body}{claim_extra}");
    let inference =
        format!("name: inference\ndescription: t\nwhen_to_use: Here\n{body}{inference_extra}");
    let evidence = format!("name: evidence\ndescription: t\nwhen_to_use: Here\n{body}");
    std::sync::Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest,
            &[
                ("claim".to_string(), claim),
                ("inference".to_string(), inference),
                ("evidence".to_string(), evidence),
            ],
        )
        .expect("must_reach fixture schema must parse"),
    )
}

fn link(from: &mut crate::entity::Entity, rel: &str, to: &crate::entity::EntityId) {
    from.relationships.push(crate::entity::Relationship {
        rel_type: rel.into(),
        target: to.clone(),
        description: None,
    });
}

fn must_reach_violations(r: &ConstraintFindingReport) -> Vec<&UnsatisfiedConstraint> {
    r.violations
        .iter()
        .filter(|v| matches!(v, UnsatisfiedConstraint::MustReach { .. }))
        .collect()
}

const CLAIM_GROUNDS_EVIDENCE: &str = "must_reach:\n  - relationships: [GROUNDS]\n    direction: out\n    terminal_types: [evidence]\n";

/// A conforming path (direct or transitive through a non-terminal)
/// is silent; an entity without one carries a finding echoing the
/// whole declaration.
#[test]
fn must_reach_conforming_path_silent_gap_reported() {
    let schema = must_reach_schema(CLAIM_GROUNDS_EVIDENCE, "");
    let mut store = Store::new();
    let ev = make_typed_entity("arg", "ev", "evidence");
    let mut direct = make_typed_entity("arg", "direct", "claim");
    link(&mut direct, "GROUNDS", &ev.id);
    let mut mid = make_typed_entity("arg", "mid", "claim");
    let mut chained = make_typed_entity("arg", "chained", "claim");
    link(&mut chained, "GROUNDS", &mid.id);
    link(&mut mid, "GROUNDS", &ev.id);
    let floating = make_typed_entity("arg", "floating", "claim");
    for e in [ev, direct, mid, chained, floating.clone()] {
        store.upsert(e.id.clone(), e);
    }
    let mut mem_schemas = HashMap::new();
    mem_schemas.insert("arg".to_string(), schema);

    let reports = collect_constraint_findings(&store, None, &mem_schemas, None);
    assert_eq!(reports.len(), 1, "only the pathless claim: {reports:?}");
    assert_eq!(reports[0].id, floating.id);
    let v = must_reach_violations(&reports[0]);
    assert_eq!(v.len(), 1);
    let UnsatisfiedConstraint::MustReach {
        relationships,
        direction,
        terminal_types,
        max_depth,
        ..
    } = v[0]
    else {
        panic!("expected must_reach finding");
    };
    assert_eq!(relationships, &vec!["GROUNDS".to_string()]);
    assert_eq!(*direction, memstead_schema::ReachDirection::Out);
    assert_eq!(terminal_types, &vec!["evidence".to_string()]);
    assert_eq!(*max_depth, None);
}

/// The floating leap: an inference no premise reaches (zero
/// incoming edges of the set) is a finding; one incoming premise
/// edge silences it. Incoming direction with depth 1 is the
/// required-incoming-edge case.
#[test]
fn must_reach_one_hop_incoming_floating_leap() {
    let schema = must_reach_schema(
        "",
        "must_reach:\n  - relationships: [GROUNDS]\n    direction: in\n    terminal_types: [claim]\n    max_depth: 1\n",
    );
    let mut store = Store::new();
    let leap = make_typed_entity("arg", "leap", "inference");
    let grounded = make_typed_entity("arg", "grounded", "inference");
    let mut premise = make_typed_entity("arg", "premise", "claim");
    link(&mut premise, "GROUNDS", &grounded.id);
    for e in [leap.clone(), grounded, premise] {
        store.upsert(e.id.clone(), e);
    }
    let mut mem_schemas = HashMap::new();
    mem_schemas.insert("arg".to_string(), schema);

    let reports = collect_constraint_findings(&store, None, &mem_schemas, None);
    assert_eq!(reports.len(), 1, "only the floating leap: {reports:?}");
    assert_eq!(reports[0].id, leap.id);
}

/// A chain ending in a stub or in non-terminal types is a finding;
/// adding one conforming path clears it on the next sweep.
#[test]
fn must_reach_stub_and_non_terminal_chains_then_cleared() {
    let schema = must_reach_schema(CLAIM_GROUNDS_EVIDENCE, "");
    let mut store = Store::new();
    let mut stub_ev = make_typed_entity("arg", "ghost", "evidence");
    stub_ev.stub = true;
    let mut to_stub = make_typed_entity("arg", "to-stub", "claim");
    link(&mut to_stub, "GROUNDS", &stub_ev.id);
    let dead_end = make_typed_entity("arg", "dead-end", "claim");
    let mut to_claim = make_typed_entity("arg", "to-claim", "claim");
    link(&mut to_claim, "GROUNDS", &dead_end.id);
    for e in [stub_ev, to_stub.clone(), dead_end, to_claim.clone()] {
        store.upsert(e.id.clone(), e);
    }
    let mut mem_schemas = HashMap::new();
    mem_schemas.insert("arg".to_string(), schema.clone());

    let reports = collect_constraint_findings(&store, None, &mem_schemas, None);
    let ids: Vec<&str> = reports.iter().map(|r| r.id.0.as_str()).collect();
    assert!(
        ids.contains(&to_stub.id.0.as_str()),
        "stub terminates no obligation: {ids:?}"
    );
    assert!(
        ids.contains(&to_claim.id.0.as_str()),
        "non-terminal chain is a finding: {ids:?}"
    );

    // One conforming edge clears the finding on the next call.
    let ev = make_typed_entity("arg", "real-ev", "evidence");
    let mut repaired = store.get(&to_stub.id).unwrap().clone();
    link(&mut repaired, "GROUNDS", &ev.id);
    store.upsert(ev.id.clone(), ev);
    store.upsert(repaired.id.clone(), repaired);
    let reports = collect_constraint_findings(&store, None, &mem_schemas, None);
    let ids: Vec<&str> = reports.iter().map(|r| r.id.0.as_str()).collect();
    assert!(
        !ids.contains(&to_stub.id.0.as_str()),
        "conforming path clears the finding: {ids:?}"
    );
}

/// A cycle along the walked set terminates (visited-set
/// discipline): the sweep returns findings for both cycle members
/// instead of hanging.
#[test]
fn must_reach_cycles_terminate() {
    let schema = must_reach_schema(CLAIM_GROUNDS_EVIDENCE, "");
    let mut store = Store::new();
    let mut a = make_typed_entity("arg", "cyc-a", "claim");
    let mut b = make_typed_entity("arg", "cyc-b", "claim");
    link(&mut a, "GROUNDS", &b.id);
    link(&mut b, "GROUNDS", &a.id);
    for e in [a, b] {
        store.upsert(e.id.clone(), e);
    }
    let mut mem_schemas = HashMap::new();
    mem_schemas.insert("arg".to_string(), schema);

    let reports = collect_constraint_findings(&store, None, &mem_schemas, None);
    assert_eq!(reports.len(), 2, "both cycle members lack evidence");
}

/// Depth bound: a conforming path within the bound is silent; a
/// graph whose only conforming path exceeds the bound is a
/// finding.
#[test]
fn must_reach_depth_bound() {
    let two_hop_store = || {
        let mut store = Store::new();
        let ev = make_typed_entity("arg", "ev", "evidence");
        let mut mid = make_typed_entity("arg", "mid", "claim");
        let mut start = make_typed_entity("arg", "start", "claim");
        link(&mut start, "GROUNDS", &mid.id);
        link(&mut mid, "GROUNDS", &ev.id);
        for e in [ev, mid, start] {
            store.upsert(e.id.clone(), e);
        }
        store
    };
    let bounded = |depth: u32| {
        must_reach_schema(
            &format!(
                "must_reach:\n  - relationships: [GROUNDS]\n    direction: out\n    terminal_types: [evidence]\n    max_depth: {depth}\n"
            ),
            "",
        )
    };

    let store = two_hop_store();
    let mut mem_schemas = HashMap::new();
    mem_schemas.insert("arg".to_string(), bounded(1));
    let reports = collect_constraint_findings(&store, None, &mem_schemas, None);
    assert_eq!(
        reports.len(),
        1,
        "the two-hop path exceeds depth 1 for the start claim: {reports:?}"
    );
    assert_eq!(reports[0].id.0, "arg--start");

    let mut mem_schemas = HashMap::new();
    mem_schemas.insert("arg".to_string(), bounded(2));
    let reports = collect_constraint_findings(&store, None, &mem_schemas, None);
    assert!(
        reports.is_empty(),
        "the same path satisfies depth 2: {reports:?}"
    );
}

/// Two obligations on one type: exactly one finding, naming the
/// unsatisfied block.
#[test]
fn must_reach_two_obligations_one_finding() {
    let schema = must_reach_schema(
        "must_reach:\n  - relationships: [GROUNDS]\n    direction: out\n    terminal_types: [evidence]\n  - relationships: [CONCLUDES]\n    direction: out\n    terminal_types: [inference]\n",
        "",
    );
    let mut store = Store::new();
    let ev = make_typed_entity("arg", "ev", "evidence");
    let mut c = make_typed_entity("arg", "half", "claim");
    link(&mut c, "GROUNDS", &ev.id);
    for e in [ev, c.clone()] {
        store.upsert(e.id.clone(), e);
    }
    let mut mem_schemas = HashMap::new();
    mem_schemas.insert("arg".to_string(), schema);

    let reports = collect_constraint_findings(&store, None, &mem_schemas, None);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].id, c.id);
    let v = must_reach_violations(&reports[0]);
    assert_eq!(v.len(), 1, "only the unsatisfied obligation: {v:?}");
    let UnsatisfiedConstraint::MustReach { relationships, .. } = v[0] else {
        panic!("expected must_reach finding");
    };
    assert_eq!(relationships, &vec!["CONCLUDES".to_string()]);
}

/// `status_propagation` with `rel_types`: the taint crosses
/// rel-type boundaries along the union subgraph (the experiment's
/// withdrawn-evidence chain in the two-rel-type modelling), and
/// the finding echoes the set (`rel_types` present, `rel_type`
/// absent).
#[test]
fn status_propagation_rel_types_taints_across_type_boundaries() {
    use crate::entity::MetadataValue;
    let manifest = r#"name: tests-prop-set
version: 0.1.0
description: propagation relation-set test schema
when_to_use: tests
types:
  - claim
relationships:
  mode: strict
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
    let claim_yaml = "name: claim\ndescription: t\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: standing\n    description: dialectical standing\n    field_type: string\n    enum_values: [active, withdrawn]\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\n  - standing\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\nconstraints:\n  - kind: status_propagation\n    field: standing\n    value: withdrawn\n    rel_types: [GROUNDS, CONCLUDES]\n    direction: incoming\n";
    let schema = std::sync::Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest,
            &[("claim".to_string(), claim_yaml.to_string())],
        )
        .expect("propagation-set fixture schema must parse"),
    );

    let mut store = Store::new();
    let mut withdrawn = make_typed_entity("arg", "withdrawn-ev", "claim");
    withdrawn
        .metadata
        .insert("standing".into(), MetadataValue::String("withdrawn".into()));
    let mut inference = make_typed_entity("arg", "inference", "claim");
    link(&mut inference, "GROUNDS", &withdrawn.id);
    let mut conclusion = make_typed_entity("arg", "conclusion", "claim");
    link(&mut conclusion, "CONCLUDES", &inference.id);
    let bystander = make_typed_entity("arg", "bystander", "claim");
    for e in [withdrawn, inference.clone(), conclusion.clone(), bystander] {
        store.upsert(e.id.clone(), e);
    }
    let mut mem_schemas = HashMap::new();
    mem_schemas.insert("arg".to_string(), schema);

    let reports = collect_constraint_findings(&store, None, &mem_schemas, None);
    let ids: Vec<&str> = reports.iter().map(|r| r.id.0.as_str()).collect();
    assert_eq!(
        ids,
        vec![conclusion.id.0.as_str(), inference.id.0.as_str()],
        "the taint crosses the CONCLUDES/GROUNDS boundary, nothing else"
    );
    let UnsatisfiedConstraint::StatusPropagation {
        rel_type,
        rel_types,
        tainted_by,
        ..
    } = &reports[0].violations[0]
    else {
        panic!("expected status_propagation finding");
    };
    assert_eq!(*rel_type, None, "set declarations echo no single name");
    assert_eq!(
        rel_types.as_deref(),
        Some(&["GROUNDS".to_string(), "CONCLUDES".to_string()][..])
    );
    assert_eq!(tainted_by, "arg--withdrawn-ev");
}

/// Cross-mem edges satisfy an obligation like any edge; a mem
/// filter reports findings only for entities of the filtered mem.
#[test]
fn must_reach_cross_mem_path_and_mem_filter() {
    let schema = must_reach_schema(CLAIM_GROUNDS_EVIDENCE, "");
    let mut store = Store::new();
    let far_ev = make_typed_entity("ground", "far-ev", "evidence");
    let mut crossing = make_typed_entity("arg", "crossing", "claim");
    link(&mut crossing, "GROUNDS", &far_ev.id);
    let floating_arg = make_typed_entity("arg", "floating", "claim");
    let floating_ground = make_typed_entity("ground", "floating", "claim");
    for e in [far_ev, crossing, floating_arg.clone(), floating_ground] {
        store.upsert(e.id.clone(), e);
    }
    let mut mem_schemas = HashMap::new();
    mem_schemas.insert("arg".to_string(), schema.clone());
    mem_schemas.insert("ground".to_string(), schema);

    let all = collect_constraint_findings(&store, None, &mem_schemas, None);
    assert_eq!(
        all.len(),
        2,
        "the crossing claim is satisfied via the cross-mem edge: {all:?}"
    );
    let filtered = collect_constraint_findings(&store, Some("arg"), &mem_schemas, None);
    assert_eq!(filtered.len(), 1, "mem filter narrows: {filtered:?}");
    assert_eq!(filtered[0].id, floating_arg.id);
}

// ----------------------------------------------------------------------
// transition_requires_checks (form 6)
// ----------------------------------------------------------------------

/// Two-type gated-transition fixture: criterion --VERIFIES--> plan,
/// plan gates `status: complete` on incoming VERIFIES checks.
fn gated_transition_schema() -> std::sync::Arc<memstead_schema::Schema> {
    let manifest = r#"name: tests-gated
version: 0.1.0
description: transition_requires_checks test schema
when_to_use: tests
types:
  - plan
  - criterion
relationships:
  mode: strict
  definitions:
    - name: VERIFIES
      description: v
      default_weight: 3.0
      acyclic: true
    - name: PART_OF
      description: hier
      default_weight: 1.0
      acyclic: true
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let plan_yaml = "name: plan\ndescription: p\nwhen_to_use: t\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: status\n    description: s\n    field_type: string\n    default_value: draft\n    enum_values: [draft, complete]\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nupdatable_fields:\n  - title\nhealth_required_fields: []\nstaleness_threshold_days: 90\nwrite_rules: []\nconstraints:\n  - kind: transition_requires_checks\n    field: status\n    to_value: complete\n    relationships: [VERIFIES]\n    direction: incoming\n    severity: block\n";
    let criterion_yaml = "name: criterion\ndescription: c\nwhen_to_use: t\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nupdatable_fields:\n  - title\nhealth_required_fields: []\nstaleness_threshold_days: 90\nwrite_rules: []\n";
    std::sync::Arc::new(
        memstead_schema::load_schema_from_memory(
            manifest,
            &[
                ("plan".to_string(), plan_yaml.to_string()),
                ("criterion".to_string(), criterion_yaml.to_string()),
            ],
        )
        .expect("gated-transition fixture schema loads"),
    )
}

/// The gate triggers only at the declared value, quantifies over
/// the declared incoming edges, requires derived `checked_ok`
/// (stale and failed do not confirm), and treats a missing
/// provider as never_checked — the no-ledger honesty posture. An
/// empty related set satisfies (universal quantification).
#[test]
fn transition_requires_checks_gates_on_derived_state() {
    use crate::check::CheckState;
    use crate::entity::MetadataValue;
    let schema = gated_transition_schema();
    let td = schema.types.get("plan").unwrap().clone();
    let mut store = Store::default();

    let mut plan = make_typed_entity("g", "the-plan", "plan");
    plan.metadata
        .insert("status".into(), MetadataValue::String("complete".into()));
    let mut ok_crit = make_typed_entity("g", "ok-crit", "criterion");
    ok_crit.relationships.push(crate::entity::Relationship {
        rel_type: "VERIFIES".into(),
        target: plan.id.clone(),
        description: None,
    });
    let mut stale_crit = make_typed_entity("g", "stale-crit", "criterion");
    stale_crit.relationships.push(crate::entity::Relationship {
        rel_type: "VERIFIES".into(),
        target: plan.id.clone(),
        description: None,
    });
    for e in [plan.clone(), ok_crit.clone(), stale_crit.clone()] {
        store.upsert(e.id.clone(), e);
    }

    let state_of = |e: &crate::entity::Entity| {
        if e.id.0.contains("ok-crit") {
            CheckState::CheckedOk
        } else {
            CheckState::CheckStale
        }
    };
    let provider = |e: &crate::entity::Entity, _kind: &str| {
        crate::engine::independence::CheckStanding::assumed_independent(state_of(e))
    };
    let violations = unsatisfied_constraints(&store, &plan, &td, None, Some(&provider));
    assert_eq!(violations.len(), 1, "{violations:?}");
    match &violations[0] {
        UnsatisfiedConstraint::TransitionRequiresChecks {
            unchecked,
            severity,
            ..
        } => {
            assert_eq!(
                unchecked.len(),
                1,
                "only the unconfirmed criterion is listed"
            );
            assert_eq!(unchecked[0].id, "g--stale-crit");
            assert_eq!(unchecked[0].state, "check_stale");
            assert_eq!(*severity, memstead_schema::ConstraintSeverity::Block);
        }
        other => panic!("expected the gated-transition violation, got {other:?}"),
    }
    assert!(
        violations[0].describe().contains("g--stale-crit")
            && violations[0].describe().contains("check_stale"),
        "describe names the offender and its state: {}",
        violations[0].describe()
    );

    // Every related entity confirmed -> satisfied.
    let all_ok = |_: &crate::entity::Entity, _kind: &str| {
        crate::engine::independence::CheckStanding::assumed_independent(CheckState::CheckedOk)
    };
    assert!(
        unsatisfied_constraints(&store, &plan, &td, None, Some(&all_ok)).is_empty(),
        "all confirmed satisfies the gate"
    );

    // Not at the gated value -> no evaluation.
    let mut draft = plan.clone();
    draft
        .metadata
        .insert("status".into(), MetadataValue::String("draft".into()));
    assert!(
        unsatisfied_constraints(&store, &draft, &td, None, Some(&provider)).is_empty(),
        "the gate triggers only at to_value"
    );

    // No provider -> every related entity derives never_checked.
    let violations = unsatisfied_constraints(&store, &plan, &td, None, None);
    assert_eq!(violations.len(), 1);
    match &violations[0] {
        UnsatisfiedConstraint::TransitionRequiresChecks { unchecked, .. } => {
            assert_eq!(unchecked.len(), 2, "no ledger access confirms nothing");
            assert!(unchecked.iter().all(|u| u.state == "never_checked"));
        }
        other => panic!("expected the gated-transition violation, got {other:?}"),
    }

    // Empty related set -> vacuously satisfied.
    let mut lone = make_typed_entity("g", "lone-plan", "plan");
    lone.metadata
        .insert("status".into(), MetadataValue::String("complete".into()));
    store.upsert(lone.id.clone(), lone.clone());
    assert!(
        unsatisfied_constraints(&store, &lone, &td, None, Some(&provider)).is_empty(),
        "an empty related set satisfies the universal quantification"
    );
}

/// The form-6 floor: with `min_related: 1` a gated entity that no
/// related entity points at is a violation naming the floor, while
/// the same declaration without the floor keeps the sealed
/// vacuous-satisfaction semantics; one confirmed related entity
/// satisfies both.
#[test]
fn transition_requires_checks_floor_refuses_the_vacuous_case() {
    use crate::check::CheckState;
    use crate::entity::MetadataValue;
    let base = gated_transition_schema();
    let floored = {
        let mut td = memstead_schema::TypeDefinition::clone(base.types.get("plan").unwrap());
        let memstead_schema::ConstraintDef::TransitionRequiresChecks { min_related, .. } =
            &mut td.constraints[0]
        else {
            panic!("form 6 declared");
        };
        *min_related = 1;
        td
    };
    let unfloored = memstead_schema::TypeDefinition::clone(base.types.get("plan").unwrap());

    let mut store = Store::default();
    let mut plan = make_typed_entity("g", "lonely-plan", "plan");
    plan.metadata
        .insert("status".into(), MetadataValue::String("complete".into()));
    store.upsert(plan.id.clone(), plan.clone());
    let provider = |_e: &crate::entity::Entity, _kind: &str| {
        crate::engine::independence::CheckStanding::assumed_independent(CheckState::CheckedOk)
    };

    let v = unsatisfied_constraints(&store, &plan, &floored, None, Some(&provider));
    assert_eq!(v.len(), 1, "{v:?}");
    match &v[0] {
        UnsatisfiedConstraint::TransitionRequiresChecks {
            related,
            min_related,
            unchecked,
            ..
        } => {
            assert_eq!((*related, *min_related), (0, 1));
            assert!(unchecked.is_empty());
            assert!(
                v[0].describe()
                    .contains("requires at least 1 related entity")
            );
        }
        other => panic!("unexpected {other:?}"),
    }
    let v = unsatisfied_constraints(&store, &plan, &unfloored, None, Some(&provider));
    assert!(v.is_empty(), "no floor keeps the vacuous case: {v:?}");

    let mut crit = make_typed_entity("g", "one-crit", "criterion");
    crit.relationships.push(crate::entity::Relationship {
        rel_type: "VERIFIES".into(),
        target: plan.id.clone(),
        description: None,
    });
    store.upsert(crit.id.clone(), crit);
    let v = unsatisfied_constraints(&store, &plan, &floored, None, Some(&provider));
    assert!(
        v.is_empty(),
        "one confirmed related entity meets the floor: {v:?}"
    );
}

/// Form 7: the entity's own check record of the declared kind
/// gates the transition. The provider is asked for THAT kind; a
/// confirming independent record satisfies, a self-checked or
/// stale one does not, and no provider derives never_checked.
#[test]
fn transition_requires_self_check_reads_the_declared_kind() {
    use crate::check::CheckState;
    use crate::engine::independence::{CheckStanding, Independence};
    use crate::entity::MetadataValue;
    let base = gated_transition_schema();
    let mut td = memstead_schema::TypeDefinition::clone(base.types.get("plan").unwrap());
    td.constraints = vec![
        memstead_schema::ConstraintDef::TransitionRequiresSelfCheck {
            field: "status".into(),
            to_value: "complete".into(),
            check_kind: "x-projection".into(),
            severity: memstead_schema::ConstraintSeverity::Block,
        },
    ];
    let store = Store::default();
    let mut bundle = make_typed_entity("g", "the-bundle", "plan");
    bundle
        .metadata
        .insert("status".into(), MetadataValue::String("complete".into()));

    let asked = std::cell::RefCell::new(Vec::<String>::new());
    let confirming = |_e: &crate::entity::Entity, kind: &str| {
        asked.borrow_mut().push(kind.to_string());
        CheckStanding::assumed_independent(CheckState::CheckedOk)
    };
    let v = unsatisfied_constraints(&store, &bundle, &td, None, Some(&confirming));
    assert!(v.is_empty(), "{v:?}");
    assert_eq!(asked.borrow().as_slice(), ["x-projection"]);

    let self_checked = |_e: &crate::entity::Entity, _k: &str| CheckStanding {
        state: CheckState::CheckedOk,
        independence: Some(Independence::SelfChecked),
    };
    let v = unsatisfied_constraints(&store, &bundle, &td, None, Some(&self_checked));
    assert_eq!(v.len(), 1, "{v:?}");
    match &v[0] {
        UnsatisfiedConstraint::TransitionRequiresSelfCheck {
            state, check_kind, ..
        } => {
            assert_eq!(state, "self_checked");
            assert_eq!(check_kind, "x-projection");
        }
        other => panic!("unexpected {other:?}"),
    }

    let stale = |_e: &crate::entity::Entity, _k: &str| {
        CheckStanding::assumed_independent(CheckState::CheckStale)
    };
    let v = unsatisfied_constraints(&store, &bundle, &td, None, Some(&stale));
    assert_eq!(v.len(), 1);

    let v = unsatisfied_constraints(&store, &bundle, &td, None, None);
    assert_eq!(v.len(), 1, "no ledger refuses rather than passes");
    match &v[0] {
        UnsatisfiedConstraint::TransitionRequiresSelfCheck { state, .. } => {
            assert_eq!(state, "never_checked")
        }
        other => panic!("unexpected {other:?}"),
    }

    bundle
        .metadata
        .insert("status".into(), MetadataValue::String("draft".into()));
    let v = unsatisfied_constraints(&store, &bundle, &td, None, None);
    assert!(v.is_empty(), "not triggered before the gated value");
}
