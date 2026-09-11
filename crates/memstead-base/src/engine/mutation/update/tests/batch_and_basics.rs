//! Short ids, heading divergence, the atomic batch (refusal, receipt,
//! rehearsal, rollback), and the basic verbs: replace with provenance,
//! hash mismatch, unknown id, read-only mount, patch.

use super::*;

/// A bare slug resolves when exactly one mounted mem carries it,
/// announced as `SHORT_ID_RESOLVED`; refuses
/// `ENTITY_ID_MISSING_MEM` naming every carrier when two do, and
/// naming none when none does; a full id takes the path it always
/// took (B7 grader finding, 2026-09-02: a bare slug reached the
/// verbs as an id with an empty mem, and the caller was told a mem
/// called "" did not exist).
#[test]
fn short_id_resolves_when_unique_and_refuses_with_candidates_otherwise() {
    let tmp_a = TempDir::new().unwrap();
    let tmp_b = TempDir::new().unwrap();
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("a", tmp_a.path().to_path_buf()),
            Box::new(FilesystemBackend::new(tmp_a.path().to_path_buf())) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("b", tmp_b.path().to_path_buf()),
            Box::new(FilesystemBackend::new(tmp_b.path().to_path_buf())) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    for (mem, title) in [("a", "Foo"), ("b", "Foo"), ("a", "Only")] {
        engine
            .create_entity(empty_create_args(mem, title), Actor::Cli, None, None)
            .unwrap();
    }
    let update_args = |id: &str, tag: &str| UpdateEntityArgs {
        anchors: Vec::new(),
        id: EntityId(id.into()),
        expected_hash: None,
        sections: IndexMap::new(),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::from_iter([("tags".to_string(), tag.to_string())]),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };

    // Unique: resolved and announced.
    let out = engine
        .update_entity(update_args("only", "x"), Actor::Cli, None, None)
        .expect("unique short id resolves");
    assert_eq!(out.id.0, "a--only");
    assert_eq!(
        out.warnings.first().map(|w| w.code()),
        Some("SHORT_ID_RESOLVED"),
        "{:?}",
        out.warnings
    );

    // Ambiguous: refused, both carriers named.
    let err = engine
        .update_entity(update_args("foo", "x"), Actor::Cli, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "ENTITY_ID_MISSING_MEM");
    assert_eq!(
        err.details()["candidates"],
        serde_json::json!(["a--foo", "b--foo"])
    );

    // No carrier: refused, no candidates.
    let err = engine
        .update_entity(update_args("nothing", "x"), Actor::Cli, None, None)
        .unwrap_err();
    assert_eq!(err.code(), "ENTITY_ID_MISSING_MEM");
    assert_eq!(err.details()["candidates"], serde_json::json!([]));

    // The same rule on the other verbs.
    let err = engine
        .delete_entity(
            crate::engine::DeleteEntityArgs {
                id: EntityId("foo".into()),
                expected_hash: None,
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "ENTITY_ID_MISSING_MEM");

    // A full id: no announcement.
    let out = engine
        .update_entity(update_args("a--only", "y"), Actor::Cli, None, None)
        .unwrap();
    assert!(
        out.warnings.iter().all(|w| w.code() != "SHORT_ID_RESOLVED"),
        "{:?}",
        out.warnings
    );
}

/// A mutation writing a section whose declared heading differs
/// from a heading the file already carried for the same key warns
/// (`SECTION_HEADING_DIVERGENCE`, naming both headings) and still
/// commits. Refusal complement: once the file carries the matching
/// heading, the same update emits no such warning.
#[test]
fn update_warns_on_section_heading_divergence_and_still_commits() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    // Pre-existing file whose heading derives to `identity` but is
    // not the schema's declared `Identity`.
    std::fs::write(
        mem_dir.join("diverged.md"),
        "---\ntype: spec\n---\n# Diverged\n\n## IDENTITY\n\nold text.\n",
    )
    .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let id = EntityId::new("specs", "diverged");

    let update_identity = |engine: &mut Engine, body: &str| {
        let current = engine.get_entity(&id).unwrap().content_hash.clone();
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), body.to_string());
        engine
            .update_entity(
                UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: id.clone(),
                    expected_hash: Some(current),
                    sections,
                    append_sections: IndexMap::new(),
                    patch_sections: IndexMap::new(),
                    sections_unset: Vec::new(),
                    metadata: IndexMap::new(),
                    metadata_unset: Vec::new(),
                    declare_relations: Vec::new(),
                    dry_run: false,
                    relations_unset: Vec::new(),
                    anchors_unset: Vec::new(),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap()
    };

    let outcome = update_identity(&mut engine, "new text.");
    assert!(!outcome.write_id.is_empty(), "the mutation still commits");
    let divergences: Vec<_> = outcome
        .warnings
        .iter()
        .filter_map(|w| match w {
            crate::ops::WarningHint::SectionHeadingDivergence {
                section_key,
                writing_heading,
                existing_heading,
                ..
            } => Some((
                section_key.clone(),
                writing_heading.clone(),
                existing_heading.clone(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        divergences,
        vec![(
            "identity".to_string(),
            "Identity".to_string(),
            "IDENTITY".to_string()
        )],
        "warning names both headings; all warnings = {:?}",
        outcome.warnings
    );

    // The regenerated file now carries the declared heading — a
    // second update to the same section must not warn.
    let outcome2 = update_identity(&mut engine, "third text.");
    assert!(
        !outcome2
            .warnings
            .iter()
            .any(|w| matches!(w, crate::ops::WarningHint::SectionHeadingDivergence { .. })),
        "matching heading emits no divergence warning: {:?}",
        outcome2.warnings
    );
}

#[test]
fn batch_update_empty_batch_returns_zero_counts() {
    // No updates → BatchResult with zero counts + empty
    // write_id. No engine mutation happens.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let result = engine
        .batch_update(Vec::new(), Actor::Cli, None, false)
        .unwrap();
    assert!(result.applied, "empty batch is a vacuous success");
    assert_eq!(result.results.len(), 0);
    assert_eq!(result.succeeded, 0);
    assert_eq!(result.failed, 0);
    assert_eq!(result.write_id, "");
}

#[test]
fn batch_update_refuses_whole_batch_when_one_item_fails() {
    // Atomic semantics: a 2-item batch where item 1 is valid and
    // item 2 targets a missing id refuses the WHOLE batch. Nothing
    // is committed — item 1 is NOT applied (its section change does
    // not land), `applied` is false, `write_id` is empty, the
    // missing item carries the typed ENTITY_NOT_FOUND envelope, and
    // the valid item is marked `not_applied`.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    // Seed: create an entity.
    let create_args = CreateEntityArgs {
        anchors: Vec::new(),
        mem: "specs".to_string(),
        title: "Seed".to_string(),
        entity_type: "spec".to_string(),
        sections: IndexMap::from_iter([
            ("identity".to_string(), "seed identity".to_string()),
            ("purpose".to_string(), "seed purpose".to_string()),
        ]),
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    };
    let created = engine
        .create_entity(create_args, Actor::Cli, None, None)
        .unwrap();

    // Batch: update the seed entity AND a missing id.
    let valid_update = UpdateEntityArgs {
        anchors: Vec::new(),
        id: created.id.clone(),
        expected_hash: Some(created.content_hash.clone()),
        sections: IndexMap::from_iter([("identity".to_string(), "updated body".to_string())]),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };
    let missing_update = UpdateEntityArgs {
        anchors: Vec::new(),
        id: EntityId("specs--nonexistent".to_string()),
        expected_hash: None,
        sections: IndexMap::new(),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };

    let result = engine
        .batch_update(
            vec![(valid_update, None), (missing_update, None)],
            Actor::Cli,
            None,
            false,
        )
        .unwrap();
    // Whole batch refused: nothing applied, no commit.
    assert!(!result.applied, "a failing item must refuse the batch");
    assert_eq!(result.results.len(), 2);
    assert_eq!(result.succeeded, 0);
    assert_eq!(result.failed, 1);
    assert_eq!(result.write_id, "", "refused batch must not commit");
    // First entry: the valid item, marked not_applied (the batch
    // was refused before it could land).
    assert_eq!(result.results[0].action, "not_applied");
    assert!(result.results[0].error.is_none());
    // Second entry: the failing item carries the typed envelope.
    assert_eq!(result.results[1].action, "error");
    let err = result.results[1]
        .error
        .as_ref()
        .expect("failed entry must carry a structured error envelope");
    assert_eq!(err.code, "ENTITY_NOT_FOUND");
    assert!(err.message.contains("not found"), "got: {}", err.message);

    // The valid item's section change must NOT have landed — the
    // store is byte-identical to pre-call.
    let seed = engine.get_entity(&created.id).unwrap();
    assert_eq!(
        seed.sections.get("identity").map(String::as_str),
        Some("seed identity"),
        "refused batch must leave the in-memory store untouched",
    );
    assert_eq!(
        seed.content_hash, created.content_hash,
        "refused batch must not change the entity's content hash",
    );
}

#[test]
fn batch_update_applies_all_valid_items_as_one_commit() {
    // A 2-item batch where both items are valid
    // applies both and produces exactly one commit; the response's
    // write_id names it and both entries report "updated".
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let mk = |title: &str| CreateEntityArgs {
        anchors: Vec::new(),
        mem: "specs".to_string(),
        title: title.to_string(),
        entity_type: "spec".to_string(),
        sections: IndexMap::from_iter([
            ("identity".to_string(), "id".to_string()),
            ("purpose".to_string(), "purp".to_string()),
        ]),
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    };
    let a = engine
        .create_entity(mk("A"), Actor::Cli, None, None)
        .unwrap();
    let b = engine
        .create_entity(mk("B"), Actor::Cli, None, None)
        .unwrap();

    let upd = |id: EntityId, hash: String, body: &str| UpdateEntityArgs {
        anchors: Vec::new(),
        id,
        expected_hash: Some(hash),
        sections: IndexMap::from_iter([("identity".to_string(), body.to_string())]),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };

    let result = engine
        .batch_update(
            vec![
                (upd(a.id.clone(), a.content_hash.clone(), "A body"), None),
                (upd(b.id.clone(), b.content_hash.clone(), "B body"), None),
            ],
            Actor::Cli,
            None,
            false,
        )
        .unwrap();
    assert!(result.applied);
    assert_eq!(result.succeeded, 2);
    assert_eq!(result.failed, 0);
    assert!(
        !result.write_id.is_empty(),
        "applied batch carries the commit"
    );
    assert!(result.results.iter().all(|e| e.action == "updated"));
    // Both section changes landed.
    assert_eq!(
        engine
            .get_entity(&a.id)
            .unwrap()
            .sections
            .get("identity")
            .map(String::as_str),
        Some("A body"),
    );
    assert_eq!(
        engine
            .get_entity(&b.id)
            .unwrap()
            .sections
            .get("identity")
            .map(String::as_str),
        Some("B body"),
    );
}

/// Rehearsal contract: `batch_update` with
/// `dry_run: true` runs the full per-item validation, reports the
/// would-be receipt with the marker form's empty `write_id`, and
/// persists NOTHING — on-disk bodies and hashes stay untouched.
/// The follow-up real call with the SAME expected hashes succeeds,
/// proving both the identical-validation contract and the
/// side-effect-freeness (a persisted rehearsal would have moved
/// the hashes and refused the real call).
#[test]
fn batch_update_dry_run_reports_receipt_and_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let mk = |title: &str| CreateEntityArgs {
        anchors: Vec::new(),
        mem: "specs".to_string(),
        title: title.to_string(),
        entity_type: "spec".to_string(),
        sections: IndexMap::from_iter([
            ("identity".to_string(), "id".to_string()),
            ("purpose".to_string(), "purp".to_string()),
        ]),
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    };
    let a = engine
        .create_entity(mk("A"), Actor::Cli, None, None)
        .unwrap();
    let b = engine
        .create_entity(mk("B"), Actor::Cli, None, None)
        .unwrap();

    let upd = |id: EntityId, hash: String, body: &str| UpdateEntityArgs {
        anchors: Vec::new(),
        id,
        expected_hash: Some(hash),
        sections: IndexMap::from_iter([("identity".to_string(), body.to_string())]),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };
    let batch = || {
        vec![
            (upd(a.id.clone(), a.content_hash.clone(), "A body"), None),
            (upd(b.id.clone(), b.content_hash.clone(), "B body"), None),
        ]
    };

    let rehearsed = engine
        .batch_update(batch(), Actor::Cli, None, true)
        .unwrap();
    assert!(rehearsed.applied, "{rehearsed:?}");
    assert_eq!(rehearsed.succeeded, 2);
    assert!(rehearsed.write_id.is_empty(), "marker form: empty write_id");
    assert!(rehearsed.results.iter().all(|e| e.action == "updated"));
    // Nothing persisted: body and hash unchanged.
    let a_now = engine.get_entity(&a.id).unwrap();
    assert_eq!(
        a_now.sections.get("identity").map(String::as_str),
        Some("id")
    );
    assert_eq!(a_now.content_hash, a.content_hash);

    // The real call with the same (pre-rehearsal) hashes lands.
    let real = engine
        .batch_update(batch(), Actor::Cli, None, false)
        .unwrap();
    assert!(real.applied, "{real:?}");
    assert!(!real.write_id.is_empty());
    assert_eq!(
        engine
            .get_entity(&a.id)
            .unwrap()
            .sections
            .get("identity")
            .map(String::as_str),
        Some("A body"),
    );
}

/// Rehearsal refusal parity: a failing batch refuses under
/// `dry_run: true` with the SAME per-entry envelope (code,
/// message, details) the real refusal carries.
#[test]
fn batch_update_dry_run_refuses_identically_to_real() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let created = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "Valid".to_string(),
                entity_type: "spec".to_string(),
                sections: IndexMap::from_iter([
                    ("identity".to_string(), "x".to_string()),
                    ("purpose".to_string(), "p".to_string()),
                ]),
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap();
    let upd = |id: EntityId, hash: Option<String>| UpdateEntityArgs {
        anchors: Vec::new(),
        id,
        expected_hash: hash,
        sections: IndexMap::from_iter([("identity".to_string(), "new".to_string())]),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };
    let batch = || {
        vec![
            (
                upd(created.id.clone(), Some("wrong-hash".to_string())),
                None,
            ),
            (upd(EntityId("specs--missing".to_string()), None), None),
        ]
    };

    let rehearsed = engine
        .batch_update(batch(), Actor::Cli, None, true)
        .unwrap();
    let real = engine
        .batch_update(batch(), Actor::Cli, None, false)
        .unwrap();
    assert!(!rehearsed.applied && !real.applied);
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
    // Neither run persisted anything.
    assert_eq!(
        engine
            .get_entity(&created.id)
            .unwrap()
            .sections
            .get("identity")
            .map(String::as_str),
        Some("x"),
    );
}

/// Report-all (the family's upgraded contract): a batch with TWO
/// failing items names both with their typed codes — a failing
/// item no longer stops preparation at the first error.
#[test]
fn batch_update_reports_every_failing_item() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let created = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "Seed".to_string(),
                entity_type: "spec".to_string(),
                sections: IndexMap::from_iter([
                    ("identity".to_string(), "seed identity".to_string()),
                    ("purpose".to_string(), "seed purpose".to_string()),
                ]),
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap();

    let upd = |id: EntityId, hash: Option<String>| UpdateEntityArgs {
        anchors: Vec::new(),
        id,
        expected_hash: hash,
        sections: IndexMap::from_iter([("identity".to_string(), "new body".to_string())]),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };
    let result = engine
        .batch_update(
            vec![
                (upd(created.id.clone(), None), None),
                (upd(EntityId("specs--missing-one".to_string()), None), None),
                (upd(EntityId("specs--missing-two".to_string()), None), None),
            ],
            Actor::Cli,
            None,
            false,
        )
        .unwrap();
    assert!(!result.applied);
    assert_eq!(result.failed, 2, "{result:?}");
    assert_eq!(result.write_id, "");
    let codes: Vec<(usize, &str)> = result
        .results
        .iter()
        .enumerate()
        .filter(|(_, r)| r.action == "error")
        .map(|(i, r)| (i, r.error.as_ref().map(|e| e.code.as_str()).unwrap_or("")))
        .collect();
    assert_eq!(
        codes,
        vec![(1, "ENTITY_NOT_FOUND"), (2, "ENTITY_NOT_FOUND")],
        "BOTH failing items named, not just the first: {result:?}"
    );
    assert_eq!(result.results[0].action, "not_applied");
    // The valid item's change did not land.
    assert_eq!(
        engine
            .get_entity(&created.id)
            .unwrap()
            .sections
            .get("identity")
            .map(String::as_str),
        Some("seed identity"),
    );
}

#[test]
fn batch_update_rolls_back_in_memory_store_auto_stub_on_refusal() {
    // The subtle invariant: an earlier item that auto-stubs a
    // relation target during preparation must have that stub rolled
    // OUT of the in-memory store when a later item refuses the
    // batch. Item 1 declares a relation to an absent target (which
    // upserts a forward-reference stub during prepare); item 2
    // targets a missing entity and fails. The refusal must leave no
    // trace of the stub.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir);
    let (actor, client) = cli_actor();

    let a = engine
        .create_entity(
            empty_create_args("specs", "Anchor"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let stub_target = EntityId::new("specs", "would-be-stub");
    let item1 = UpdateEntityArgs {
        anchors: Vec::new(),
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
        id: a.id.clone(),
        expected_hash: Some(a.content_hash.clone()),
        sections: IndexMap::new(),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: vec![crate::ops::RelateArg {
            rel_type: "USES".to_string(),
            target: stub_target.clone(),
            description: None,
        }],
        dry_run: false,
    };
    let item2 = UpdateEntityArgs {
        anchors: Vec::new(),
        id: EntityId::new("specs", "nonexistent"),
        expected_hash: None,
        sections: IndexMap::from_iter([("identity".to_string(), "x".to_string())]),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };

    // Sanity: the would-be stub does not exist before the batch.
    assert!(engine.get_entity(&stub_target).is_none());

    let result = engine
        .batch_update(
            vec![(item1, None), (item2, None)],
            actor,
            Some(&client),
            false,
        )
        .unwrap();
    assert!(!result.applied, "missing item 2 must refuse the batch");

    // The auto-stub item 1 created during preparation was rolled
    // back with the store snapshot — no orphaned stub survives.
    assert!(
        engine.get_entity(&stub_target).is_none(),
        "refused batch must roll the in-memory auto-stub back out of the store",
    );
    // The anchor's relation set is unchanged too.
    let anchor = engine.get_entity(&a.id).unwrap();
    assert!(
        !anchor.relationships.iter().any(|r| r.target == stub_target),
        "refused batch must not leave the declared relation on the anchor",
    );
}

#[test]
fn update_entity_replaces_a_section_and_logs_provenance() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Updatable");
    let (actor, client) = cli_actor();

    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "Updated body.".to_string());

    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                sections,
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            Some("section update"),
        )
        .unwrap();

    assert_eq!(
        outcome.modified_sections.replaced,
        vec!["identity".to_string()]
    );
    assert_ne!(
        outcome.content_hash, seeded.content_hash,
        "hash must change"
    );
    // Store carries the new content.
    let entity = engine.get_entity(&seeded.id).unwrap();
    assert!(
        entity
            .sections
            .get("identity")
            .unwrap()
            .contains("Updated body.")
    );
    // Provenance log records the update.
    let log = std::fs::read_to_string(tmp.path().join(".memstead/changes.jsonl")).unwrap();
    assert!(log.contains("\"kind\":\"update\""));
    assert!(log.contains("\"note\":\"section update\""));
}

#[test]
fn update_entity_rejects_hash_mismatch() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Hash Guarded");
    let (actor, client) = cli_actor();
    let err = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some("wrong-hash".to_string()),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::HashMismatch {
            id,
            current,
            is_stub,
        } => {
            assert_eq!(id, seeded.id.to_string());
            assert_eq!(current, seeded.content_hash);
            assert!(!is_stub, "real entity must not flag as stub");
        }
        other => panic!("expected HashMismatch, got {other:?}"),
    }
}

#[test]
fn update_entity_rejects_unknown_id() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, _) = engine_with_seed(&tmp, "Anchor");
    let (actor, client) = cli_actor();
    let err = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: crate::EntityId::new("specs", "ghost"),
                expected_hash: None,
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotFound { .. }));
}

#[test]
fn update_entity_rejects_read_only_mount() {
    let tmp = TempDir::new().unwrap();
    let archive_path = build_archive(
        tmp.path(),
        "ext",
        &[(
            "a.md",
            b"---\ntype: spec\n---\n# A\n\n## Identity\n\nbody.\n",
        )],
    );
    let mut engine = Engine::from_mounts(vec![(
        archive_mount("external", archive_path.clone()),
        Box::new(ArchiveBackend::new(archive_path)),
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let id = crate::EntityId::new("external", "a");
    let err = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id,
                expected_hash: None,
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::ReadOnlyMount(v) if v == "external"));
}

#[test]
fn update_entity_patches_section_with_find_and_replace() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Patch Subject");
    let (actor, client) = cli_actor();

    // Pre-write a known body via the replace path so the patch
    // test has a deterministic substring to target.
    let mut replace = IndexMap::new();
    replace.insert("identity".to_string(), "hello world hello".to_string());
    let replaced = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                sections: replace,
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // First-occurrence patch (all = false).
    let mut patches = IndexMap::new();
    patches.insert(
        "identity".to_string(),
        vec![crate::ops::PatchArg {
            old: "hello".to_string(),
            new: "HI".to_string(),
            all: false,
        }],
    );
    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(replaced.content_hash.clone()),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: patches,
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(outcome.modified_sections.patched, vec!["identity"]);
    let body = engine
        .get_entity(&seeded.id)
        .unwrap()
        .sections
        .get("identity")
        .unwrap()
        .clone();
    assert!(body.contains("HI world hello"), "first-only: {body:?}");
}

#[test]
fn update_entity_patch_rejects_missing_old_substring() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Patch Miss");
    let (actor, client) = cli_actor();
    let mut patches = IndexMap::new();
    patches.insert(
        "identity".to_string(),
        vec![crate::ops::PatchArg {
            old: "this-substring-does-not-exist".to_string(),
            new: "nope".to_string(),
            all: false,
        }],
    );
    let err = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: patches,
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: false,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::PatchOldNotFound { section, .. } => {
            assert_eq!(section, "identity");
        }
        other => panic!("expected PatchOldNotFound, got {other:?}"),
    }
}
