//! The single and batch create paths: anchors against a binding,
//! batch semantics, the folder backend, required-section refusals and
//! their pre-announced metadata gate, stub adoption, timestamps, and
//! inline relations.

use super::*;

/// The source-vs-binding check at the engine seam: an anchor naming
/// BOTH a producing binding (by hash) and a `source` refuses when
/// the binding resolves in this workspace but does not declare the
/// name — with the declared names in the recovery payload. A
/// declared name is accepted; an unresolvable binding hash accepts
/// any non-empty name (validation never requires resolution).
#[test]
fn anchor_source_validated_against_resolvable_binding() {
    use crate::binding::{
        BINDING_VERSION, Binding, BuildMode, BuildOperation, Operations, hash_binding,
    };
    use crate::pipeline::{IngestTrigger, PatternEntry, PatternMode, Source};

    let tmp = TempDir::new().unwrap();
    let (mut engine, _seed) = engine_with_seed(&tmp, "Seed");
    let (actor, client) = cli_actor();

    // A workspace root carrying one binding with two declared sources.
    let ws = TempDir::new().unwrap();
    let binding = Binding {
        version: BINDING_VERSION,
        intent: None,
        sources: ["api-docs", "guides"]
            .into_iter()
            .map(|n| Source {
                name: n.to_string(),
                medium_type: crate::pipeline::MediumType::Codebase,
                pointer: "../src".to_string(),
                change_detection: None,
                scope: vec![PatternEntry {
                    path: "**/*".to_string(),
                    mode: PatternMode::Allow,
                }],
                engagement: None,
                preparation: None,
            })
            .collect(),
        reference_mems: Vec::new(),
        destination_mem: "specs".to_string(),
        deny_paths: Vec::new(),
        coverage_semantics: None,
        rules: None,
        prune: None,
        operations: Operations {
            build: Some(BuildOperation {
                mode: BuildMode::Discovery,
                trigger: IngestTrigger::Loop,
                batch_size: 20,
                post_actions: None,
            }),
            sync: None,
            verify: None,
        },
    };
    let dir = ws
        .path()
        .join(".memstead")
        .join("projections")
        .join("specs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("docs.json"),
        serde_json::to_string_pretty(&binding).unwrap(),
    )
    .unwrap();
    engine.set_workspace_root(ws.path().to_path_buf());
    let binding_hash = hash_binding(&binding);

    // The artifact must resolve (workspace-relative fallback) — the
    // write gate refuses dead references; this test's subject is the
    // source-NAME validation, not the path join.
    std::fs::create_dir_all(ws.path().join("src")).unwrap();
    std::fs::write(ws.path().join("src").join("x.rs"), "fn x() {}").unwrap();

    let anchor = |source: &str, binding: &str| crate::anchor::AnchorInput {
        artifact: Some("src/x.rs".into()),
        grain: Some("file".into()),
        class: Some("anchored".into()),
        binding: Some(binding.into()),
        source: Some(source.into()),
        ..Default::default()
    };
    let make_args = |title: &str, a: crate::anchor::AnchorInput| {
        let mut args = empty_create_args("specs", title);
        args.anchors = vec![a];
        args
    };

    // Undeclared name against the RESOLVING binding: refuses with the
    // declared names in the payload.
    let err = engine
        .create_entity(
            make_args("Bad Source", anchor("front-page", &binding_hash)),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_ANCHOR", "got {err:?}");
    let details = err.details();
    assert_eq!(details["field"], "source");
    assert_eq!(details["got"], "front-page");
    assert_eq!(
        details["declared"],
        serde_json::json!(["api-docs", "guides"])
    );

    // A declared name is accepted, and the anchor round-trips with it.
    let ok = engine
        .create_entity(
            make_args("Good Source", anchor("api-docs", &binding_hash)),
            actor,
            Some(&client),
            None,
        )
        .expect("declared source name accepted");
    let anchors = engine.mem_anchors_resolved("specs");
    let stored = anchors
        .iter()
        .find(|(id, _)| id == &ok.id)
        .map(|(_, a)| &a.anchor)
        .expect("anchor stored for the new entity");
    assert_eq!(stored.source.as_deref(), Some("api-docs"));

    // An unresolvable binding hash accepts any non-empty name.
    engine
        .create_entity(
            make_args("Orphaned Binding", anchor("whatever", "deadbeef")),
            actor,
            Some(&client),
            None,
        )
        .expect("unresolvable binding accepts any non-empty name");
}

/// Batch create: N mutually-referencing entities (cycle included —
/// USES is not acyclic in the default schema) land in ONE
/// invocation with every reference resolving to a REAL typed
/// entity, never a stub, and no stub warnings.
#[test]
fn batch_create_intra_batch_references_resolve_real() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let with_rel = |title: &str, to: &str| {
        let mut args = empty_create_args("specs", title);
        args.relations = vec![crate::ops::RelateArg {
            target: crate::entity::EntityId::new("specs", to),
            rel_type: "USES".to_string(),
            description: None,
        }];
        (args, Some(format!("note for {title}")))
    };
    // A → B → C → A: a cycle the schema permits.
    let result = engine
        .batch_create(
            vec![
                with_rel("Alpha", "beta"),
                with_rel("Beta", "gamma"),
                with_rel("Gamma", "alpha"),
            ],
            actor,
            Some(&client),
            false,
        )
        .unwrap();
    assert!(result.applied, "{result:?}");
    assert_eq!(result.succeeded, 3);
    assert!(!result.write_id.is_empty(), "one real commit");
    assert!(
        result.results.iter().all(|r| r.action == "created"),
        "{result:?}"
    );

    // Every reference resolves to a REAL entity of the right type.
    for name in ["alpha", "beta", "gamma"] {
        let e = engine
            .get_entity(&crate::entity::EntityId::new("specs", name))
            .unwrap();
        assert!(!e.stub, "{name} must be real, not a stub");
        assert_eq!(e.entity_type, "spec");
        assert_eq!(e.relationships.len(), 1, "{name} carries its edge");
    }
    // No stub warnings anywhere in the outcome (in-batch targets
    // never transit through the stub machinery).
    // (BatchResult carries no warnings channel; absence of stubs in
    // the store is the observable.)
}

/// Rehearsal contract: `batch_create` with
/// `dry_run: true` validates the whole batch — intra-batch
/// references included — and reports the would-be receipt with the
/// marker form's empty `write_id`, writing NOTHING. The
/// follow-up real call on the unchanged mem succeeds.
#[test]
fn batch_create_dry_run_reports_receipt_and_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let with_rel = |title: &str, to: &str| {
        let mut args = empty_create_args("specs", title);
        args.relations = vec![crate::ops::RelateArg {
            target: crate::entity::EntityId::new("specs", to),
            rel_type: "USES".to_string(),
            description: None,
        }];
        (args, None)
    };
    let batch = || {
        vec![
            with_rel("Alpha", "beta"),
            with_rel("Beta", "gamma"),
            with_rel("Gamma", "alpha"),
        ]
    };

    let rehearsed = engine
        .batch_create(batch(), actor, Some(&client), true)
        .unwrap();
    assert!(rehearsed.applied, "{rehearsed:?}");
    assert_eq!(rehearsed.succeeded, 3);
    assert!(rehearsed.write_id.is_empty(), "marker form: empty write_id");
    assert!(rehearsed.results.iter().all(|r| r.action == "created"));
    // The receipt names the prospective ids; nothing landed.
    for name in ["alpha", "beta", "gamma"] {
        let id = crate::entity::EntityId::new("specs", name);
        assert!(
            rehearsed.results.iter().any(|r| r.id == id),
            "receipt must name {id}: {rehearsed:?}"
        );
        assert!(
            !engine.store().contains(&id),
            "rehearsal must create nothing"
        );
    }
    assert_eq!(engine.store().all_entities().count(), 0);

    // Identical validation: the real call on the unchanged mem lands.
    let real = engine
        .batch_create(batch(), actor, Some(&client), false)
        .unwrap();
    assert!(real.applied, "{real:?}");
    assert!(!real.write_id.is_empty(), "the real batch commits");
    assert_eq!(real.succeeded, 3);
}

/// Rehearsal refusal parity: a batch with failing entries refuses
/// under `dry_run: true` with the SAME per-entry report-all
/// envelope the real call returns — and both perform nothing, so
/// the paired invocations are directly comparable.
#[test]
fn batch_create_dry_run_refuses_identically_to_real() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, _seeded) = engine_with_seed(&tmp, "Existing");
    let (actor, client) = cli_actor();
    let plain = |title: &str| (empty_create_args("specs", title), None);
    let batch = || {
        vec![
            plain("Fine One"),
            plain("Existing"),  // duplicate vs pre-batch store
            plain("Bad/Title"), // invalid title character
        ]
    };

    let rehearsed = engine
        .batch_create(batch(), actor, Some(&client), true)
        .unwrap();
    let real = engine
        .batch_create(batch(), actor, Some(&client), false)
        .unwrap();
    assert!(!rehearsed.applied && !real.applied);
    assert_eq!(rehearsed.failed, real.failed);
    assert_eq!(rehearsed.errors_suppressed, real.errors_suppressed);
    let envelope = |r: &crate::ops::BatchResult| {
        r.results
            .iter()
            .map(|e| {
                (
                    e.id.to_string(),
                    e.action.clone(),
                    e.error
                        .as_ref()
                        .map(|err| (err.code.clone(), err.message.clone(), err.details.clone())),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(envelope(&rehearsed), envelope(&real), "identical refusals");
    assert!(
        !engine
            .store()
            .contains(&crate::entity::EntityId::new("specs", "fine-one"))
    );
}

/// Atomicity + report-all: a batch with several invalid entries
/// writes NOTHING (no entity, no head movement) and names EVERY
/// failing entry with its typed code — not only the first.
#[test]
fn batch_create_refuses_whole_batch_reporting_every_failure() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Existing");
    let (actor, client) = cli_actor();
    let head_before = engine
        .mem_head_sha("specs")
        .ok()
        .flatten()
        .unwrap_or_default();
    let count_before = engine.store().all_entities().count();

    let plain = |title: &str| (empty_create_args("specs", title), None);
    let result = engine
        .batch_create(
            vec![
                plain("Fine One"),
                plain("Existing"),   // duplicate vs pre-batch store
                plain("Bad\nTitle"), // control character in title
                plain("Fine Two"),
                plain("Fine Two"), // duplicate WITHIN the batch
            ],
            actor,
            Some(&client),
            false,
        )
        .unwrap();
    assert!(!result.applied);
    assert_eq!(result.failed, 3, "{result:?}");
    assert!(result.write_id.is_empty());
    let codes: Vec<(usize, &str)> = result
        .results
        .iter()
        .enumerate()
        .filter(|(_, r)| r.action == "error")
        .map(|(i, r)| (i, r.error.as_ref().map(|e| e.code.as_str()).unwrap_or("")))
        .collect();
    assert_eq!(
        codes,
        vec![
            (1, "ENTITY_ALREADY_EXISTS"),
            (2, "INVALID_TITLE"),
            (4, "ENTITY_ALREADY_EXISTS"),
        ],
        "every failing entry named with index + typed code: {result:?}"
    );
    // Valid entries are marked not_applied, and NOTHING was written.
    assert_eq!(result.results[0].action, "not_applied");
    assert_eq!(result.results[3].action, "not_applied");
    let head_after = engine
        .mem_head_sha("specs")
        .ok()
        .flatten()
        .unwrap_or_default();
    assert_eq!(head_before, head_after, "mem head unmoved");
    assert_eq!(
        engine.store().all_entities().count(),
        count_before,
        "no entity created, no skeleton left behind"
    );
    let _ = seeded;
}

/// Bounded reporting: with more failing entries than the cap, the
/// report carries the cap's worth of detailed envelopes and counts
/// the suppressed remainder — never a silent truncation.
#[test]
fn batch_create_bounds_the_failure_report() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let n = Engine::BATCH_ERROR_REPORT_CAP + 10;
    let batch: Vec<_> = (0..n)
        .map(|i| (empty_create_args("specs", &format!("Bad\nTitle {i}")), None))
        .collect();
    let result = engine
        .batch_create(batch, actor, Some(&client), false)
        .unwrap();
    assert!(!result.applied);
    assert_eq!(result.failed, n);
    let detailed = result
        .results
        .iter()
        .filter(|r| r.action == "error" && r.error.is_some())
        .count();
    let bare = result
        .results
        .iter()
        .filter(|r| r.action == "error" && r.error.is_none())
        .count();
    assert_eq!(detailed, Engine::BATCH_ERROR_REPORT_CAP);
    assert_eq!(bare, 10);
    assert_eq!(
        result.errors_suppressed, 10,
        "suppression is counted, never silent"
    );
}

#[test]
fn create_entity_writes_through_folder_backend_and_updates_store() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let outcome = engine
        .create_entity(
            empty_create_args("specs", "Hello World"),
            actor,
            Some(&client),
            Some("first draft"),
        )
        .unwrap();

    // Outcome reports a real id, real file path, real hash.
    assert_eq!(outcome.id.to_string(), "specs--hello-world");
    assert_eq!(outcome.file_path, "hello-world.md");
    assert!(!outcome.content_hash.is_empty());

    // Store has the new entity.
    let entity = engine
        .get_entity(&crate::EntityId::new("specs", "hello-world"))
        .expect("entity must be in the store after create");
    assert_eq!(entity.title, "Hello World");
    assert_eq!(entity.entity_type, "spec");
    assert_eq!(entity.content_hash, outcome.content_hash);

    // On-disk markdown exists at the expected path.
    let on_disk = std::fs::read_to_string(mem_dir.join("hello-world.md")).unwrap();
    assert!(on_disk.contains("# Hello World"));
    assert!(on_disk.contains("type: spec"));

    // Provenance log has the create record.
    let log_path = mem_dir.join(".memstead").join("changes.jsonl");
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(log.contains("\"kind\":\"create\""));
    assert!(log.contains("\"entity\":\"specs--hello-world\""));
    assert!(log.contains("\"actor\":\"cli\""));
    assert!(log.contains("\"note\":\"first draft\""));
}

/// Supplying a
/// value for an auto-managed field (`created_date`) on create no
/// longer silently discards it — the response carries an
/// `IGNORED_READONLY_FIELD` warning, and the stored value is the
/// engine-stamped one, not the supplied `2020-01-01`.
#[test]
fn create_entity_warns_on_supplied_auto_managed_field() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let mut args = empty_create_args("specs", "Dated Entity");
    args.metadata
        .insert("created_date".to_string(), "2020-01-01".to_string());

    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();

    let warned = outcome.warnings.iter().any(|w| {
        w.code() == "IGNORED_READONLY_FIELD"
            && matches!(w, WarningHint::IgnoredReadonlyField { field, supplied }
                    if field == "created_date" && supplied == "2020-01-01")
    });
    assert!(
        warned,
        "expected IGNORED_READONLY_FIELD; got {:?}",
        outcome.warnings
    );

    // The engine value was stamped, not the supplied 2020 date.
    assert_ne!(outcome.created_date, "2020-01-01");
}

/// Complement: a create with no auto-managed field supplied emits no
/// `IGNORED_READONLY_FIELD` warning.
#[test]
fn create_entity_no_warning_when_auto_managed_field_absent() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let outcome = engine
        .create_entity(
            empty_create_args("specs", "Plain Entity"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(
        !outcome
            .warnings
            .iter()
            .any(|w| w.code() == "IGNORED_READONLY_FIELD"),
        "no auto-managed field supplied — no warning expected; got {:?}",
        outcome.warnings
    );
}

#[test]
fn create_entity_returns_write_id_title_mem_on_real_write() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let outcome = engine
        .create_entity(
            empty_create_args("specs", "Rich Shape"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Folder backend produces a synthetic CommitId — wire-equiv
    // to full's commit SHA.
    assert!(
        !outcome.write_id.is_empty(),
        "write_id must be populated on a real create"
    );
    // title + mem echoed from args (full CreateResult parity).
    assert_eq!(outcome.title, "Rich Shape");
    assert_eq!(outcome.mem, "specs");
    // The create path refuses on missing required sections, so
    // `empty_create_args` seeds identity + purpose and the
    // success path's warnings vec carries no
    // `MissingRequiredSection` entries. The dedicated refusal
    // tests below exercise the gate directly.
    assert!(
        !outcome
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::MissingRequiredSection { .. })),
        "success path must not carry MissingRequiredSection warnings — those refuse on create now",
    );
}

/// Missing
/// required sections refuse on create. The error envelope names
/// every missing key (in schema-declaration order), carries each
/// section's `write_rules`, and surfaces the type-level
/// `type_guidance` map keyed by `entity_type`.
#[test]
fn create_entity_refuses_missing_required_sections_with_typed_envelope() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    // `spec` requires `identity` + `purpose`. Supply neither.
    let args = CreateEntityArgs {
        anchors: Vec::new(),
        mem: "specs".to_string(),
        title: "Half Done".to_string(),
        entity_type: "spec".to_string(),
        sections: IndexMap::new(),
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    };
    let err = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap_err();
    match err {
        EngineError::MissingRequiredSection {
            entity_type,
            missing_count,
            sections,
            type_guidance,
            pre_announced_missing_fields: _,
        } => {
            assert_eq!(entity_type, "spec");
            assert_eq!(missing_count, sections.len());
            assert!(
                missing_count >= 2,
                "expected ≥2 missing sections, got {missing_count}"
            );
            let keys: Vec<String> = sections.iter().map(|s| s.key.clone()).collect();
            assert!(
                keys.contains(&"identity".to_string()),
                "missing keys: {keys:?}"
            );
            assert!(
                keys.contains(&"purpose".to_string()),
                "missing keys: {keys:?}"
            );
            assert!(
                type_guidance.contains_key("spec"),
                "type_guidance must include `spec` entry, got: {type_guidance:?}",
            );
        }
        other => panic!("expected MissingRequiredSection, got {other:?}"),
    }

    // No entity landed in the store.
    let id = crate::EntityId::new("specs", "half-done");
    assert!(
        engine.store().get(&id).is_none(),
        "refused create must not persist any entity"
    );
}

/// Cross-gate pre-announcement: a first write
/// failing BOTH the section gate and the metadata gate learns both
/// demands in the one `MISSING_REQUIRED_SECTION` refusal — the
/// pre-announced set names exactly what `REQUIRED_FIELD_UNSET`
/// would demand next — and a second submission fixing everything
/// announced succeeds. The cold write that took three round-trips
/// takes two.
#[test]
fn create_refusing_sections_pre_announces_metadata_gate_and_fixed_retry_succeeds() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_planning_schema(&tmp);
    let (actor, client) = cli_actor();

    // Round-trip 1: neither gate satisfied — no sections, no
    // metadata. `planning.decision` requires sections decision/
    // context/consequences and no-default fields decided_on +
    // deciders.
    let bare = CreateEntityArgs {
        anchors: Vec::new(),
        mem: "planning".to_string(),
        title: "Cold Write".to_string(),
        entity_type: "decision".to_string(),
        sections: IndexMap::new(),
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    };
    let err = engine
        .create_entity(bare, actor, Some(&client), None)
        .unwrap_err();
    let (sections, announced) = match err {
        EngineError::MissingRequiredSection {
            sections,
            pre_announced_missing_fields,
            ..
        } => (sections, pre_announced_missing_fields),
        other => panic!("expected MissingRequiredSection, got {other:?}"),
    };
    let section_keys: Vec<&str> = sections.iter().map(|s| s.key.as_str()).collect();
    for key in ["decision", "context", "consequences"] {
        assert!(section_keys.contains(&key), "sections: {section_keys:?}");
    }
    // The announcement is exactly the metadata gate's demand set —
    // nothing speculative, nothing withheld that is knowable.
    let announced_keys: Vec<&str> = announced.iter().map(|m| m.key.as_str()).collect();
    assert_eq!(
        announced_keys,
        vec!["decided_on", "deciders"],
        "pre-announced set must equal the metadata gate's demand in declaration order"
    );
    // The wire payload carries the block under `pre_announced`, in
    // REQUIRED_FIELD_UNSET's established `missing[]` element shape.
    let rebuilt = EngineError::MissingRequiredSection {
        entity_type: "decision".to_string(),
        missing_count: sections.len(),
        sections,
        type_guidance: Default::default(),
        pre_announced_missing_fields: announced,
    };
    let details = rebuilt.details();
    let wire_missing = details["pre_announced"]["required_field_unset"]["missing"]
        .as_array()
        .expect("pre_announced.required_field_unset.missing[] present");
    assert_eq!(wire_missing[0]["field"], "decided_on");
    assert!(wire_missing[0].get("description").is_some());
    assert!(wire_missing[0].get("enum_values").is_some());

    // Round-trip 2: fix everything the one refusal announced —
    // and nothing else. Succeeds: no third round-trip exists.
    let mut metadata = IndexMap::new();
    metadata.insert("decided_on".to_string(), "2026-08-19".to_string());
    metadata.insert("deciders".to_string(), "alice".to_string());
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "planning".to_string(),
                title: "Cold Write".to_string(),
                entity_type: "decision".to_string(),
                sections: IndexMap::from_iter([
                    ("decision".to_string(), "x".to_string()),
                    ("context".to_string(), "y".to_string()),
                    ("consequences".to_string(), "z".to_string()),
                ]),
                metadata,
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .expect("fixing everything announced must succeed in the second round-trip");
}

/// Complement: a body failing ONLY
/// the section gate — metadata complete — refuses with an empty
/// pre-announcement, and its `details` payload carries no
/// `pre_announced` key at all: byte-compatible with the
/// pre-announcement-free shape.
#[test]
fn section_only_refusal_omits_the_pre_announced_block() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_planning_schema(&tmp);
    let (actor, client) = cli_actor();

    let mut metadata = IndexMap::new();
    metadata.insert("decided_on".to_string(), "2026-08-19".to_string());
    metadata.insert("deciders".to_string(), "alice".to_string());
    let err = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "planning".to_string(),
                title: "Sections Only".to_string(),
                entity_type: "decision".to_string(),
                sections: IndexMap::new(),
                metadata,
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match &err {
        EngineError::MissingRequiredSection {
            pre_announced_missing_fields,
            ..
        } => assert!(
            pre_announced_missing_fields.is_empty(),
            "metadata gate is satisfied — nothing to pre-announce"
        ),
        other => panic!("expected MissingRequiredSection, got {other:?}"),
    }
    assert!(
        err.details().get("pre_announced").is_none(),
        "single-gate refusal must stay byte-compatible: no pre_announced key"
    );
}

/// `dry_run: true` returns the same refusal envelope
/// the real call would. The preview surface doesn't admit content
/// the real call would refuse.
#[test]
fn create_entity_dry_run_returns_same_refusal_envelope_as_real_call() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let args = CreateEntityArgs {
        anchors: Vec::new(),
        mem: "specs".to_string(),
        title: "Half Done Dry".to_string(),
        entity_type: "spec".to_string(),
        sections: IndexMap::new(),
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: true,
    };
    let err = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap_err();
    assert!(
        matches!(err, EngineError::MissingRequiredSection { .. }),
        "dry_run must surface the same refusal envelope, got {err:?}"
    );
}

/// A follow-up call with the missing sections filled
/// in succeeds. The refusal carries enough recovery information
/// that the agent's next attempt resolves in one round-trip.
#[test]
fn create_entity_succeeds_after_filling_in_required_sections() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "the identity body".to_string());
    sections.insert("purpose".to_string(), "the purpose body".to_string());
    let args = CreateEntityArgs {
        anchors: Vec::new(),
        mem: "specs".to_string(),
        title: "Complete".to_string(),
        entity_type: "spec".to_string(),
        sections,
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    };
    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .expect("complete create succeeds");
    assert_eq!(outcome.title, "Complete");
}

#[test]
fn create_entity_promotes_existing_stub_and_preserves_incoming_edges() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Source");
    let (actor, client) = cli_actor();

    // Step 1: relate source → "ghost-target" — creates a stub
    // entity at `specs--ghost-target` with one incoming edge.
    let stub_target = crate::EntityId::new("specs", "ghost-target");
    engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: stub_target.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let stub = engine
        .store()
        .get(&stub_target)
        .expect("stub must be in store");
    assert!(stub.stub);
    assert_eq!(engine.store().incoming(&stub_target).len(), 1);

    // Step 2: create a real entity with the same title — should
    // promote the stub and preserve the incoming edge.
    let outcome = engine
        .create_entity(
            empty_create_args("specs", "Ghost Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // No error: stub adoption proceeded.
    assert_eq!(outcome.id, stub_target);
    // Entity is now a real entity, not a stub.
    let real = engine
        .store()
        .get(&stub_target)
        .expect("entity must still be in store");
    assert!(!real.stub);
    // Incoming edge survived the upsert.
    assert_eq!(engine.store().incoming(&stub_target).len(), 1);
    // Outcome surfaces stub adoption.
    assert_eq!(outcome.incoming_count, Some(1));
    assert_eq!(outcome.incoming.len(), 1);
    assert_eq!(outcome.incoming[0].from, source.id);
    assert_eq!(outcome.incoming[0].rel_type, "USES");
}

#[test]
fn create_entity_reports_no_incoming_on_greenfield_create() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let outcome = engine
        .create_entity(
            empty_create_args("specs", "Greenfield"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    // No pre-existing stub → incoming_count is None, incoming vec
    // is empty. Full's wire shape skip-serialises both.
    assert!(outcome.incoming_count.is_none());
    assert!(outcome.incoming.is_empty());
}

#[test]
fn create_entity_populates_created_date_from_schema_auto_stamp() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let outcome = engine
        .create_entity(
            empty_create_args("specs", "Has Date"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    // The default `spec` schema declares `created_date` with
    // an init_timestamp default. The parsed entity carries the
    // auto-stamped value; the outcome surfaces it for callers
    // who need it without a follow-up read.
    assert!(
        !outcome.created_date.is_empty(),
        "created_date must be populated when the schema auto-stamps it"
    );
}

#[test]
fn create_overrides_user_supplied_timestamps_update_rejects_them() {
    // Schema-declared `init_timestamp` (set on create) and
    // `auto_timestamp` (re-stamped on every update) fields are
    // engine-managed. On create the engine still silently
    // overrides any caller-supplied value (the entity must be
    // stampable in one shot from the user's perspective). On
    // update the writable-metadata validator rejects the write
    // up-front with `READ_ONLY_FIELD` — the agent gets a
    // structured rejection instead of a "set" response whose
    // value the auto-stamp pass silently discards (per the F13
    // / F14 contract).
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    // Pin the mutation clock. The auto-stamp is second-resolution,
    // and the assertions below compare it against a separately
    // computed "now" — so an unpinned run fails whenever a second
    // ticks between the create and the comparison. That is a real
    // flake, not a theoretical one: it fired on a suite run that
    // straddled midnight. The engine's injectable clock exists for
    // exactly this, and pinning it here also makes the expected
    // string a constant rather than a second read of the wall clock.
    const FROZEN_SECS: u64 = 1_754_000_000;
    let frozen = std::time::UNIX_EPOCH + std::time::Duration::from_secs(FROZEN_SECS);
    engine.set_mutation_clock(std::sync::Arc::new(move || frozen));
    let (actor, client) = cli_actor();

    // Caller supplies a past value for the init_timestamp field
    // and the auto_timestamp field. The engine ignores both on
    // create.
    let mut args = empty_create_args("specs", "Stamped Today");
    args.metadata
        .insert("created_date".to_string(), "2020-01-01".to_string());
    args.metadata
        .insert("last_modified".to_string(), "2020-01-01".to_string());

    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();

    // Both timestamps should reflect the engine's own clock, not
    // the caller's `2020-01-01`.
    let today = crate::engine::mutation::iso_from_system_time(frozen);
    assert_eq!(outcome.created_date, today);
    let entity = engine
        .get_entity(&outcome.id)
        .expect("entity must be in store after create");
    assert_eq!(
        entity
            .metadata
            .get("created_date")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
        today,
        "init_timestamp field must be engine-determined on create, not user-supplied"
    );
    assert_eq!(
        entity
            .metadata
            .get("last_modified")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
        today,
        "auto_timestamp field must be engine-determined on create, not user-supplied"
    );

    // F13/F14: update rejects a user-supplied value for either
    // init_timestamp or auto_timestamp metadata fields with
    // `READ_ONLY_FIELD`. Test both fields in turn.
    let attempt_update = |key: &str, value: &str| {
        let mut metadata = IndexMap::new();
        metadata.insert(key.to_string(), value.to_string());
        crate::engine::UpdateEntityArgs {
            anchors: Vec::new(),
            id: outcome.id.clone(),
            metadata,
            metadata_unset: Vec::new(),
            sections: IndexMap::new(),
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            sections_unset: Vec::new(),
            expected_hash: Some(outcome.content_hash.clone()),
            dry_run: false,
            declare_relations: Vec::new(),
            relations_unset: Vec::new(),
            anchors_unset: Vec::new(),
        }
    };
    for key in ["created_date", "last_modified"] {
        let err = engine
            .update_entity(
                attempt_update(key, "2019-12-31"),
                actor,
                Some(&client),
                None,
            )
            .expect_err("schema-managed timestamp must be rejected on update");
        assert_eq!(err.code(), "READ_ONLY_FIELD", "got: {err:?}");
    }
    // Stored value is unchanged after a rejected attempt.
    let entity = engine
        .get_entity(&outcome.id)
        .expect("entity must remain in store after rejected update");
    assert_eq!(
        entity
            .metadata
            .get("last_modified")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
        today,
        "rejected update must not mutate the auto_timestamp field"
    );
}

#[test]
fn create_entity_wires_inline_relations_and_stubs_absent_targets() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, existing) = engine_with_seed(&tmp, "Existing Target");
    let (actor, client) = cli_actor();
    let absent = crate::EntityId::new("specs", "future-target");
    assert!(!engine.store().contains(&absent));

    let mut args = empty_create_args("specs", "Source With Relations");
    args.relations = vec![
        crate::ops::RelateArg {
            target: existing.id.clone(),
            rel_type: "USES".to_string(),
            description: None,
        },
        crate::ops::RelateArg {
            target: absent.clone(),
            rel_type: "USES".to_string(),
            description: None,
        },
    ];

    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();

    // New entity in store with both edges materialised.
    let source = engine
        .store()
        .get(&outcome.id)
        .expect("source must be in store");
    assert_eq!(source.relationships.len(), 2);
    assert!(
        source
            .relationships
            .iter()
            .any(|r| r.target == existing.id && r.rel_type == "USES")
    );
    assert!(
        source
            .relationships
            .iter()
            .any(|r| r.target == absent && r.rel_type == "USES")
    );

    // Absent target was auto-stubbed (mirrors the relate path's
    // ensure_target).
    let stub = engine
        .store()
        .get(&absent)
        .expect("absent relation target must be auto-stubbed");
    assert!(stub.stub);
    // Existing target unchanged.
    let existing_after = engine.store().get(&existing.id).unwrap();
    assert!(!existing_after.stub);
}
