//! Anchors on create, reserved metadata keys, cycle and self-loop
//! refusals on inline relations, and declared section formats.

use super::*;

// ---- Provenance anchors: create/persist/reload/isolation -----------

fn file_anchor(artifact: &str, hash: &str) -> crate::anchor::AnchorInput {
    crate::anchor::AnchorInput {
        artifact: Some(artifact.to_string()),
        grain: Some("file".to_string()),
        class: Some("anchored".to_string()),
        hash: Some(hash.to_string()),
        hash_stability: Some("stable".to_string()),
        ..Default::default()
    }
}

fn folder_engine(mem: &str) -> (Engine, TempDir) {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount(mem, dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    (engine, tmp)
}

#[test]
fn create_with_anchors_persists_and_survives_reload() {
    let (mut engine, tmp) = folder_engine("specs");
    let dir = tmp.path().to_path_buf();
    let (actor, client) = cli_actor();
    let mut args = empty_create_args("specs", "Anchored Entity");
    args.anchors = vec![file_anchor("src/lib.rs", "h1")];
    engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();

    let id = crate::EntityId::new("specs", "anchored-entity");
    let anchors = engine.entity_anchors(&id);
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].artifact, "src/lib.rs");
    assert_eq!(
        anchors[0].class,
        crate::anchor::AnchorProvenanceClass::Anchored
    );

    // Survives a fresh boot from the same on-disk mem.
    let writer = FilesystemBackend::new(dir.clone());
    let reloaded = Engine::from_mounts(vec![(
        folder_mount("specs", dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    assert_eq!(reloaded.entity_anchors(&id).len(), 1);
    // Reverse lookup finds it by artifact path.
    assert_eq!(reloaded.anchors_referencing_artifact("src/lib.rs").len(), 1);
}

#[test]
fn malformed_anchor_refuses_and_entity_not_written() {
    let (mut engine, tmp) = folder_engine("specs");
    let (actor, client) = cli_actor();
    let mut args = empty_create_args("specs", "Bad Anchor");
    args.anchors = vec![crate::anchor::AnchorInput {
        artifact: Some("x".into()),
        grain: Some("paragraph".into()), // unknown grain
        class: Some("anchored".into()),
        ..Default::default()
    }];
    let err = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap_err();
    assert_eq!(err.code(), crate::anchor::INVALID_ANCHOR_CODE);
    // Entity was not written (refusal fires before the disk write).
    assert!(
        engine
            .get_entity(&crate::EntityId::new("specs", "bad-anchor"))
            .is_none()
    );
    assert!(!tmp.path().join("bad-anchor.md").exists());
}

#[test]
fn anchors_are_not_folded_into_content_hash() {
    // Two identical creates — one anchored, one not — produce the same
    // `_hash`: the anchors sidecar lives under `.memstead/` and never
    // enters content hashing.
    //
    // Both engines run on ONE frozen clock. The schema auto-stamps
    // `created_date` / `last_modified` at second granularity, so without
    // this the assertion also silently depended on both creates landing
    // inside the same second — true on an idle machine, false under a
    // loaded one, where the two entities differ in frontmatter and the
    // hashes diverge for a reason that has nothing to do with anchors.
    let (mut anchored, _t1) = folder_engine("specs");
    let (mut plain, _t2) = folder_engine("specs");
    let frozen = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_754_000_000);
    anchored.set_mutation_clock(std::sync::Arc::new(move || frozen));
    plain.set_mutation_clock(std::sync::Arc::new(move || frozen));
    let (actor, client) = cli_actor();

    let mut a = empty_create_args("specs", "Same Title");
    a.anchors = vec![file_anchor("src/lib.rs", "h1")];
    let with = anchored
        .create_entity(a, actor, Some(&client), None)
        .unwrap();

    let p = empty_create_args("specs", "Same Title");
    let without = plain.create_entity(p, actor, Some(&client), None).unwrap();

    assert_eq!(
        with.content_hash, without.content_hash,
        "anchors must not change the entity content hash"
    );
}

#[test]
fn anchorless_create_writes_no_sidecar() {
    let (mut engine, _tmp) = folder_engine("specs");
    let (actor, client) = cli_actor();
    engine
        .create_entity(
            empty_create_args("specs", "No Anchors"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(
        engine
            .entity_anchors(&crate::EntityId::new("specs", "no-anchors"))
            .is_empty()
    );
}

// ---- reserved metadata keys on create --------------------------------

/// A create carrying a reserved identity/discriminator metadata key
/// (`type` / `mem` / `id`) refuses with the same deliberate
/// `READ_ONLY_FIELD` the update path uses — not the incidental
/// `UNKNOWN_METADATA_FIELD` — and the entity is not written.
/// Refusal complement: a create with only declared, non-reserved
/// keys lands exactly as today (covered pervasively by every other
/// create test; the explicit control below re-asserts it beside
/// the refusals).
#[test]
fn create_refuses_reserved_metadata_keys_deliberately() {
    let (mut engine, _tmp) = folder_engine("specs");
    let (actor, client) = cli_actor();
    for reserved in ["type", "mem", "id"] {
        let mut args = empty_create_args("specs", "Smuggler");
        args.metadata
            .insert(reserved.to_string(), "bogus".to_string());
        let err = engine
            .create_entity(args, actor, Some(&client), None)
            .expect_err("reserved key must refuse on create");
        assert_eq!(err.code(), "READ_ONLY_FIELD", "key '{reserved}': {err:?}");
        assert!(
            engine
                .get_entity(&crate::EntityId::new("specs", "smuggler"))
                .is_none(),
            "entity must not be written after the '{reserved}' refusal"
        );
    }
    // Control: the same create without the smuggled key lands.
    engine
        .create_entity(
            empty_create_args("specs", "Smuggler"),
            actor,
            Some(&client),
            None,
        )
        .expect("a clean create is untouched by the reserved-key gate");
}

// ---- cycle family on the create paths --------------------------------

fn create_with_relation(mem: &str, title: &str, rel_type: &str, to: &str) -> CreateEntityArgs {
    let mut args = empty_create_args(mem, title);
    args.relations = vec![crate::ops::RelateArg {
        target: crate::EntityId(to.to_string()),
        rel_type: rel_type.to_string(),
        description: None,
    }];
    args
}

/// `create.relations[]` runs the same cycle family as
/// `memstead_relate`: an edge closing a cycle through a promoted
/// stub refuses `RELATIONSHIP_CYCLE` (acyclic rel-type), a
/// self-loop on a listed no-self-loop rel-type refuses
/// identically, and —
/// refusal complement — a non-cycle edge on the acyclic type lands
/// exactly as today.
#[test]
fn create_relations_refuse_cycle_and_self_loop_like_relate() {
    let (mut engine, _tmp) = folder_engine("specs");
    let (actor, client) = cli_actor();

    // A PART_OF→ghost auto-stubs `ghost` with an incoming edge.
    engine
        .create_entity(
            create_with_relation("specs", "Alpha", "PART_OF", "specs--ghost"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Promoting the stub with a back-edge closes alpha→ghost→alpha.
    let err = engine
        .create_entity(
            create_with_relation("specs", "Ghost", "PART_OF", "specs--alpha"),
            actor,
            Some(&client),
            None,
        )
        .expect_err("cycle-closing create.relations[] must refuse");
    assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");
    // Recovery detail matches the relate path's shape.
    let details = err.details();
    assert_eq!(details["rel_type"], "PART_OF");
    assert!(details["existing_path"].is_array());
    assert!(
        engine
            .get_entity(&crate::EntityId::new("specs", "ghost"))
            .is_none_or(|e| e.stub),
        "the refused entity must not be written"
    );

    // Self-loop on a listed no-self-loop rel-type (spec lists USES).
    let err = engine
        .create_entity(
            create_with_relation("specs", "Selfy", "USES", "specs--selfy"),
            actor,
            Some(&client),
            None,
        )
        .expect_err("self-loop create.relations[] must refuse");
    assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");

    // Refusal complement: a non-cycle edge on the acyclic type
    // lands (fresh chain link, no back-path).
    engine
        .create_entity(
            create_with_relation("specs", "Beta", "PART_OF", "specs--alpha"),
            actor,
            Some(&client),
            None,
        )
        .expect("a non-cycle PART_OF edge must land as today");
}

/// An intra-batch cycle on an acyclic rel-type refuses the whole
/// batch — the staged state IS the graph state the batch validates
/// against. Refusal complement: an acyclic intra-batch chain lands.
#[test]
fn batch_create_refuses_intra_batch_cycle() {
    let (mut engine, _tmp) = folder_engine("specs");
    let (actor, client) = cli_actor();

    let result = engine
        .batch_create(
            vec![
                (
                    create_with_relation("specs", "Ping", "PART_OF", "specs--pong"),
                    None,
                ),
                (
                    create_with_relation("specs", "Pong", "PART_OF", "specs--ping"),
                    None,
                ),
            ],
            actor,
            Some(&client),
            false,
        )
        .expect("batch returns a result envelope");
    assert!(!result.applied, "intra-batch cycle must refuse the batch");
    assert!(
        result.results.iter().any(|r| r
            .error
            .as_ref()
            .is_some_and(|e| e.code == "RELATIONSHIP_CYCLE")),
        "the refusal must carry RELATIONSHIP_CYCLE: {:?}",
        result.results
    );
    assert!(
        engine
            .get_entity(&crate::EntityId::new("specs", "ping"))
            .is_none(),
        "nothing lands from a refused batch"
    );

    // Refusal complement: an acyclic intra-batch chain lands.
    let result = engine
        .batch_create(
            vec![
                (
                    create_with_relation("specs", "Chain One", "PART_OF", "specs--chain-two"),
                    None,
                ),
                (empty_create_args("specs", "Chain Two"), None),
            ],
            actor,
            Some(&client),
            false,
        )
        .expect("acyclic batch lands");
    assert!(result.applied, "{:?}", result.results);
    assert_eq!(result.succeeded, 2);
}

/// Refusal complement at depth: a deep-but-acyclic PART_OF chain
/// past the cycle path cap is accepted on the create path — the cap
/// bounds the *reported* path on refusal, never the legality of a
/// long acyclic chain — and one closing edge at the far end still
/// refuses.
#[test]
fn deep_acyclic_chain_near_path_cap_is_accepted() {
    let (mut engine, _tmp) = folder_engine("specs");
    let (actor, client) = cli_actor();
    let depth = crate::engine::mutation::RELATIONSHIP_CYCLE_PATH_CAP + 2;

    // link-0 ← link-1 ← … each new entity PART_OF the previous.
    engine
        .create_entity(
            empty_create_args("specs", "Link 0"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    for i in 1..depth {
        engine
            .create_entity(
                create_with_relation(
                    "specs",
                    &format!("Link {i}"),
                    "PART_OF",
                    &format!("specs--link-{}", i - 1),
                ),
                actor,
                Some(&client),
                None,
            )
            .unwrap_or_else(|e| panic!("deep acyclic link {i} must land: {e:?}"));
    }

    // Closing the loop end-to-end still refuses, with the reported
    // path truncated at the cap.
    let last = depth - 1;
    let err = engine
        .update_entity(
            {
                let id = crate::EntityId::new("specs", "link-0");
                let hash = engine.get_entity(&id).unwrap().content_hash.clone();
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    anchors_unset: Vec::new(),
                    id,
                    expected_hash: Some(hash),
                    sections: IndexMap::new(),
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: vec![crate::ops::RelateArg {
                        target: crate::EntityId::new("specs", &format!("link-{last}")),
                        rel_type: "PART_OF".to_string(),
                        description: None,
                    }],
                    dry_run: false,
                    relations_unset: Vec::new(),
                }
            },
            actor,
            Some(&client),
            None,
        )
        .expect_err("closing the deep chain must refuse");
    assert_eq!(err.code(), "RELATIONSHIP_CYCLE");
    let details = err.details();
    assert_eq!(details["path_truncated"], true);
    assert_eq!(
        details["existing_path"].as_array().unwrap().len(),
        crate::engine::mutation::RELATIONSHIP_CYCLE_PATH_CAP
    );
}

const FORMAT_MANIFEST: &str = r#"name: formatproof
version: 0.1.0
description: section-format proof schema
when_to_use: format tests
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

const FORMAT_PLAN_TYPE: &str = r#"name: plan
description: a plan with formatted milestones
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
    item_pattern: '\*\*(?<name>[^*]+)\*\* — (?<datum>\d{4}-\d{2}-\d{2})'
    example: |
      ### Phase 1
      - **Kickoff** — 2026-09-01
  - key: notizen
    heading: Notizen
    required: false
    search_weight: 5.0
    catch_all: false
    write_rules: []
    content: "list(bullet)"
    format_severity: warn
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
  - meilensteine
  - notizen
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;

fn format_engine(tmp: &TempDir) -> Engine {
    engine_with_proof_schema(
        tmp,
        "formatproof",
        FORMAT_MANIFEST,
        &[("plan", FORMAT_PLAN_TYPE)],
    )
}

fn plan_create_args(
    title: &str,
    meilensteine: Option<&str>,
    notizen: Option<&str>,
) -> CreateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("body".to_string(), "a plan body.".to_string());
    if let Some(m) = meilensteine {
        sections.insert("meilensteine".to_string(), m.to_string());
    }
    if let Some(n) = notizen {
        sections.insert("notizen".to_string(), n.to_string());
    }
    CreateEntityArgs {
        anchors: Vec::new(),
        mem: "proof".to_string(),
        title: title.to_string(),
        entity_type: "plan".to_string(),
        sections,
        metadata: IndexMap::new(),
        relations: vec![],
        dry_run: false,
    }
}

/// Block-tier format enforcement on create: a nonconforming
/// section refuses with the format code and the echoed example;
/// the conforming write passes; a warn-tier section never refuses.
#[test]
fn create_enforces_declared_section_format() {
    let tmp = TempDir::new().unwrap();
    let mut engine = format_engine(&tmp);
    let (actor, client) = cli_actor();

    let err = engine
        .create_entity(
            plan_create_args("Plan A", Some("### Phase 1\n\nprose statt liste\n"), None),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "SECTION_CONTENT_MISMATCH");
    let details = err.details();
    assert_eq!(details["section"], "meilensteine");
    assert!(
        details["example"].as_str().unwrap().contains("Kickoff"),
        "the conforming example is echoed: {details}"
    );
    assert_eq!(details["expected_next"][0], "list(bullet)");

    // Item-pattern violation gets its own code.
    let err = engine
        .create_entity(
            plan_create_args("Plan B", Some("### Phase 1\n- kein format\n"), None),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "SECTION_ITEM_PATTERN_MISMATCH");

    // Conforming write passes.
    engine
        .create_entity(
            plan_create_args(
                "Plan C",
                Some("### Phase 1\n- **Kickoff** — 2026-09-01\n"),
                None,
            ),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Warn-tier section: nonconforming content commits.
    let outcome = engine
        .create_entity(
            plan_create_args(
                "Plan D",
                Some("### Phase 1\n- **Kickoff** — 2026-09-01\n"),
                Some("kein listenpunkt\n"),
            ),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(!outcome.write_id.is_empty(), "warn tier never refuses");

    // Absent-as-empty: omitting the block-tier section refuses
    // exactly like an explicit empty body — the generator renders
    // the empty heading either way, and write path and health
    // must agree about that on-disk state. `+` does not admit the
    // empty sequence, so the section is effectively required.
    let err = engine
        .create_entity(
            plan_create_args("Plan E", None, None),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "SECTION_CONTENT_MISMATCH");
}

/// Composed-body rule on update: an append whose delta is
/// harmless refuses when the COMPOSED body violates; the
/// conforming replacement passes.
#[test]
fn update_judges_format_on_composed_body() {
    let tmp = TempDir::new().unwrap();
    let mut engine = format_engine(&tmp);
    let (actor, client) = cli_actor();
    let created = engine
        .create_entity(
            plan_create_args(
                "Plan A",
                Some("### Phase 1\n- **Kickoff** — 2026-09-01\n"),
                None,
            ),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Append a trailing paragraph: the delta alone is legal
    // markdown, the composed body no longer matches the shape.
    let current = engine.get_entity(&created.id).unwrap().content_hash.clone();
    let mut append = IndexMap::new();
    append.insert(
        "meilensteine".to_string(),
        "\n\nnachtrag als absatz\n".to_string(),
    );
    let err = engine
        .update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: created.id.clone(),
                expected_hash: Some(current.clone()),
                sections: IndexMap::new(),
                append_sections: append,
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: vec![],
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "SECTION_CONTENT_MISMATCH");

    // A conforming append (another phase) passes.
    let mut append = IndexMap::new();
    append.insert(
        "meilensteine".to_string(),
        "\n\n### Phase 2\n- **Go-Live** — 2026-10-01\n".to_string(),
    );
    engine
        .update_entity(
            crate::engine::UpdateEntityArgs {
                anchors: Vec::new(),
                id: created.id.clone(),
                expected_hash: Some(current),
                sections: IndexMap::new(),
                append_sections: append,
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: vec![],
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
}

/// Reserved-heading extension: `^# ` now refuses in
/// any section body, exactly like `^## ` — free-form sections
/// included, via the byte-class line guard.
#[test]
fn embedded_h1_refuses_in_any_section() {
    let tmp = TempDir::new().unwrap();
    let mut engine = format_engine(&tmp);
    let (actor, client) = cli_actor();
    let mut args = plan_create_args(
        "Plan H",
        Some("### Phase 1\n- **Kickoff** — 2026-09-01\n"),
        None,
    );
    args.sections.insert(
        "body".to_string(),
        "intro\n# Injected Title\ntail".to_string(),
    );
    let err = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap_err();
    assert_eq!(err.code(), "SECTION_CONTENT_INVALID");
}
