//! Refusals and previews: required fields without defaults, inline
//! relation gates, dry-run, read-only mounts, unknown mems and types,
//! duplicate ids, invalid titles, unknown section keys, persistence
//! across restart, and the auto-stub warnings body links raise.

use super::*;

/// A
/// required metadata field the schema does not auto-fill
/// (`default_value` / `init_timestamp` / `auto_timestamp` all
/// absent) now triggers `REQUIRED_FIELD_UNSET` refusal on the
/// create path. Pre-fix this surfaced as a `MissingRequiredField`
/// warning and the generator silently wrote placeholder values
/// that the install-time strict validator could later refuse,
/// breaking the export-then-install round-trip.
#[test]
fn create_entity_refuses_unsupplied_no_default_required_field() {
    // The `planning.decision` schema declares `decided_on`
    // (Date, required, no default_value, no init_timestamp) and
    // `deciders` (String csv_array, required, no default).
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_planning_schema(&tmp);
    let (actor, client) = cli_actor();

    let mut args = CreateEntityArgs {
        anchors: Vec::new(),
        mem: "planning".to_string(),
        title: "Skip Postgres".to_string(),
        entity_type: "decision".to_string(),
        sections: IndexMap::from_iter([
            ("decision".to_string(), "Use SQLite locally.".to_string()),
            ("context".to_string(), "Single-user dev.".to_string()),
            ("consequences".to_string(), "Lose multi-writer.".to_string()),
        ]),
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    };

    // Real-write path: refuse on the first missing field
    // (declaration order).
    let err = engine
        .create_entity(args.clone(), actor, Some(&client), None)
        .unwrap_err();
    match err {
        EngineError::RequiredFieldUnset {
            field, entity_type, ..
        } => {
            assert!(
                field == "decided_on" || field == "deciders",
                "expected first missing field, got {field:?}"
            );
            assert_eq!(entity_type, "decision");
        }
        other => panic!("expected RequiredFieldUnset, got {other:?}"),
    }

    // Dry-run path on the same shape (different title to avoid the
    // already-exists check). Must surface the same refusal — the
    // create dry-run is the agent's preview surface.
    args.title = "Different Title".to_string();
    args.dry_run = true;
    let dry_err = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap_err();
    assert!(
        matches!(dry_err, EngineError::RequiredFieldUnset { .. }),
        "dry_run must surface the same refusal envelope, got {dry_err:?}"
    );
}

/// A follow-up call with all required-no-default
/// fields supplied succeeds. The refusal recovery is a single
/// round-trip.
#[test]
fn create_entity_succeeds_when_all_required_no_default_fields_supplied() {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_with_planning_schema(&tmp);
    let (actor, client) = cli_actor();

    let mut metadata = IndexMap::new();
    metadata.insert("decided_on".to_string(), "2026-05-13".to_string());
    metadata.insert("deciders".to_string(), "alice, bob".to_string());

    let outcome = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "planning".to_string(),
                title: "Complete Decision".to_string(),
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
        .expect("complete decision create succeeds");
    // No MissingRequiredField warnings on the success path —
    // refusal swallows the case before any warning could fire.
    let missing_field_warnings: Vec<&WarningHint> = outcome
        .warnings
        .iter()
        .filter(|w| matches!(w, WarningHint::MissingRequiredField { .. }))
        .collect();
    assert!(
        missing_field_warnings.is_empty(),
        "success path must not carry MissingRequiredField warnings, got: {missing_field_warnings:?}"
    );
}

/// Item 02: `memstead_create.relations[]` runs the same target-id
/// grammar gate as `memstead_relate`. Pre-fix the create path
/// admitted malformed ids (auto-stub at `bad@chars$here`) even
/// though `memstead_relate` rejected them.
#[test]
fn create_entity_rejects_inline_relation_with_malformed_target_id() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let mut args = empty_create_args("specs", "Source");
    args.relations = vec![crate::ops::RelateArg {
        target: crate::EntityId("specs--bad target with spaces!!".to_string()),
        rel_type: "USES".to_string(),
        description: None,
    }];
    let err = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap_err();
    assert!(
        matches!(err, EngineError::InvalidEntityId { .. }),
        "malformed target id must trip INVALID_ENTITY_ID on the create path; got {err:?}",
    );
}

/// Item 02: `memstead_create.relations[]` runs the same schema-shape
/// gate as `memstead_relate`. The relate-path shape gate is already
/// pinned by `memstead-mcp::tool_surface::INVALID_REL_SHAPE` and the
/// schema-loader tests; the cross-path lock here exercises the
/// `software` schema's `VIOLATES` rel-type, which declares
/// `source_types: [incident]` — an inline create from a `spec`
/// must trip the shape gate even though the rel-type itself is
/// valid vocabulary.
#[test]
fn create_entity_rejects_inline_relation_with_shape_violation() {
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mount = Mount {
        mem: "code".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "software",
            semver::Version::new(0, 1, 0),
        )),
        storage: MountStorage::Folder { path: mem_dir },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut engine =
        Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
    let (actor, client) = cli_actor();

    // Seed an existing target so the shape gate evaluates the
    // real target type (not `None`, which the gate admits as the
    // stub-bound case). The `requirement` type requires `statement` +
    // `rationale` sections plus `verified_on` + `source` metadata
    // (the schema lists these without `default_value` or
    // `optional: true`, so the strict-on-create gate refuses
    // unless supplied).
    let target = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "code".to_string(),
                title: "Target Requirement".to_string(),
                entity_type: "requirement".to_string(),
                sections: IndexMap::from_iter([
                    ("statement".to_string(), "MUST hold.".to_string()),
                    ("rationale".to_string(), "Because tests.".to_string()),
                ]),
                metadata: IndexMap::from_iter([
                    ("verified_on".to_string(), "2026-05-19".to_string()),
                    ("source".to_string(), "test fixture".to_string()),
                ]),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // `VIOLATES` declares `source_types: [incident]`. A `spec`
    // create with `VIOLATES` violates the shape. The `spec` type
    // in the software schema requires `identity` + `purpose`;
    // supply both so the shape gate (not the missing-sections
    // gate) is what fires.
    let args = CreateEntityArgs {
        anchors: Vec::new(),
        mem: "code".to_string(),
        title: "Misshape Source".to_string(),
        entity_type: "spec".to_string(),
        sections: IndexMap::from_iter([
            ("identity".to_string(), "this spec".to_string()),
            (
                "purpose".to_string(),
                "exercising the shape gate".to_string(),
            ),
        ]),
        metadata: IndexMap::new(),
        relations: vec![crate::ops::RelateArg {
            target: target.id.clone(),
            rel_type: "VIOLATES".to_string(),
            description: None,
        }],
        dry_run: false,
    };
    let err = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap_err();
    assert!(
        matches!(err, EngineError::Validation(_)),
        "shape violation must trip Validation(InvalidRelationshipShape); got {err:?}",
    );
}

#[test]
fn create_entity_canonicalises_inline_relation_rel_types_to_upper_snake_case() {
    // Wire-level contract: rel_type on inline relations is
    // case-insensitive. The engine stores the relationship as
    // UPPER_SNAKE_CASE regardless of input case.
    let tmp = TempDir::new().unwrap();
    let (mut engine, existing) = engine_with_seed(&tmp, "Existing Target");
    let (actor, client) = cli_actor();

    let mut args = empty_create_args("specs", "Source With Mixed Case Rel");
    args.relations = vec![crate::ops::RelateArg {
        target: existing.id.clone(),
        rel_type: "uses".to_string(),
        description: None,
    }];

    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();

    let source = engine
        .store()
        .get(&outcome.id)
        .expect("source must be in store");
    assert_eq!(source.relationships.len(), 1);
    assert_eq!(
        source.relationships[0].rel_type, "USES",
        "inline relation rel_type must be stored UPPER_SNAKE_CASE",
    );
}

#[test]
fn create_entity_dry_run_skips_disk_and_store_yet_returns_hash() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let mut args = empty_create_args("specs", "Preview Only");
    args.dry_run = true;

    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();

    // Wire shape: content_hash = prospective hash; write_id empty.
    assert_eq!(outcome.id.to_string(), "specs--preview-only");
    assert!(
        !outcome.content_hash.is_empty(),
        "prospective hash populated"
    );
    assert!(outcome.write_id.is_empty(), "no commit on dry_run");
    // No store entry — the engine didn't push.
    assert!(
        engine.store().get(&outcome.id).is_none(),
        "dry_run must not mutate the store",
    );
    // No file on disk.
    assert!(
        !mem_dir.join("preview-only.md").exists(),
        "dry_run must not touch disk",
    );
    // No provenance line.
    let log = mem_dir.join(".memstead").join("changes.jsonl");
    assert!(
        !log.exists()
            || !std::fs::read_to_string(&log)
                .unwrap()
                .contains("preview-only"),
        "dry_run must not append provenance",
    );
}

#[test]
fn create_entity_rejects_read_only_mount_before_backend() {
    let tmp = TempDir::new().unwrap();
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"# a")]);
    let mut engine = Engine::from_mounts(vec![(
        archive_mount("external", archive_path.clone()),
        Box::new(ArchiveBackend::new(archive_path)),
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let err = engine
        .create_entity(
            empty_create_args("external", "Should Fail"),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::ReadOnlyMount(v) => assert_eq!(v, "external"),
        other => panic!("expected ReadOnlyMount, got {other:?}"),
    }
    // Capability gating runs before the backend → the typed
    // BackendError::Sealed variant never surfaces here. That's
    // the intended ordering.
}

#[test]
fn create_entity_rejects_unknown_mem() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", tmp.path().to_path_buf()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let err = engine
        .create_entity(
            empty_create_args("does-not-exist", "Anything"),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownMem(v) if v == "does-not-exist"));
}

#[test]
fn create_entity_rejects_unknown_type_against_pinned_schema() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let mut args = empty_create_args("specs", "Anything");
    args.entity_type = "definitely-not-a-real-type".to_string();
    let err = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap_err();
    match err {
        EngineError::UnknownType { name, declared, .. } => {
            assert_eq!(name, "definitely-not-a-real-type");
            assert!(!declared.is_empty(), "declared types must be listed");
        }
        other => panic!("expected UnknownType, got {other:?}"),
    }
}

#[test]
fn create_entity_rejects_duplicate_id() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    engine
        .create_entity(
            empty_create_args("specs", "Same Slug"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let err = engine
        .create_entity(
            empty_create_args("specs", "Same Slug"),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::AlreadyExists {
            id,
            existing_title,
            existing_is_stub,
        } => {
            assert_eq!(id, "specs--same-slug");
            // The refusal names the occupying title so the caller
            // sees which existing title derived the colliding slug.
            assert!(!existing_title.is_empty());
            assert!(!existing_is_stub);
        }
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
}

#[test]
fn create_entity_rejects_invalid_title() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    // F4: empty/whitespace-only titles now refuse with
    // `INVALID_TITLE` / reason `empty`. The earlier hash-fallback
    // behaviour applies only to the loader path (pre-gate
    // entities); the strict mutation gate rejects so the
    // structured-content envelope can carry actionable details.
    let err = engine
        .create_entity(empty_create_args("specs", "  "), actor, Some(&client), None)
        .unwrap_err();
    match err {
        EngineError::InvalidTitle(slug_err) => {
            assert_eq!(slug_err.reason(), "empty", "expected empty reason");
        }
        other => panic!("expected InvalidTitle/TitleEmpty, got {other:?}"),
    }

    // Widened grammar: char-drop titles land, with the divergence
    // reported as the typed warning naming the dropped characters
    // and the derived slug.
    let outcome = engine
        .create_entity(
            empty_create_args("specs", "Hello, World!"),
            actor,
            Some(&client),
            None,
        )
        .expect("char-drop title lands under the widened grammar");
    assert_eq!(outcome.id.as_ref(), "specs--hello-world");
    let dropped = outcome
        .warnings
        .iter()
        .find_map(|w| match w {
            WarningHint::TitleCharsDroppedFromSlug {
                dropped_chars,
                slug,
                ..
            } => Some((dropped_chars.clone(), slug.clone())),
            _ => None,
        })
        .expect("divergence warning rides the outcome");
    assert!(dropped.0.contains(&',') && dropped.0.contains(&'!'));
    assert_eq!(dropped.1, "hello-world");

    // Path-traversal-shaped titles are display text too — the
    // dropped `/` and `.` never reach the slug, so the id stays
    // sanitised (no traversal), and the divergence is reported.
    let outcome = engine
        .create_entity(
            empty_create_args("specs", "../etc/passwd"),
            actor,
            Some(&client),
            None,
        )
        .expect("traversal-shaped title lands with a sanitised slug");
    assert_eq!(outcome.id.as_ref(), "specs--etcpasswd");
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::TitleCharsDroppedFromSlug { .. }))
    );
}

#[test]
fn create_entity_rejects_unknown_section_key() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let mut args = empty_create_args("specs", "Bad Sections");
    args.sections
        .insert("not-a-real-section-key".to_string(), "body".to_string());
    let err = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap_err();
    assert!(matches!(err, EngineError::Validation(_)));
}

#[test]
fn create_entity_persists_across_engine_restart() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    {
        let writer = FilesystemBackend::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let (actor, client) = cli_actor();
        engine
            .create_entity(
                empty_create_args("specs", "Survives Restart"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
    }
    // New engine reading the same mem must see the entity.
    let writer2 = FilesystemBackend::new(mem_dir.clone());
    let engine2 = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer2) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let entity = engine2
        .get_entity(&crate::EntityId::new("specs", "survives-restart"))
        .expect("entity must persist across engine restart");
    assert_eq!(entity.title, "Survives Restart");
}

/// Create with
/// a body wiki-link to a non-existent target emits
/// `INLINE_WIKI_LINK_AUTO_STUBBED` with the stubbed target id in
/// `details.stubs`. Pre-fix the warning never fired because the
/// emission walked `parse_markdown(generated_markdown).inline_links`,
/// which the parser-side coverage filter had already emptied for
/// the alias-synthesised body link.
#[test]
fn create_entity_emits_inline_wiki_link_auto_stubbed_for_new_stub_target() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let ghost = crate::EntityId::new("specs", "ghost-target");
    assert!(!engine.store().contains(&ghost), "ghost must not pre-exist");

    let mut args = empty_create_args("specs", "Source With Body Link");
    args.sections.insert(
        "identity".to_string(),
        "ref [[ghost-target]] for context".to_string(),
    );

    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();
    let stubbed: Vec<&crate::EntityId> = outcome
        .warnings
        .iter()
        .filter_map(|w| match w {
            WarningHint::InlineWikiLinkAutoStubbed { stubs, .. } => Some(stubs),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(
        stubbed.contains(&&ghost),
        "INLINE_WIKI_LINK_AUTO_STUBBED warning must name the ghost target; got: {:?}",
        outcome.warnings,
    );
    // The stub also lands in the store and the REFERENCES edge exists.
    assert!(
        engine.store().contains(&ghost),
        "ghost stub must materialise"
    );
}

/// CLI F11: a body wiki-link to the entity's own slug is dropped (no
/// vacuous self-edge) with a `SELF_LINK_IGNORED` warning, while a body
/// link to a *different* target in the same entity still synthesises
/// its REFERENCES edge normally — only the self-target is dropped.
#[test]
fn create_entity_drops_self_link_keeps_other_links_and_warns() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    // Title "Selfie" → slug "selfie" → id "specs--selfie". The body
    // links its own slug AND a different target.
    let mut args = empty_create_args("specs", "Selfie");
    args.sections.insert(
        "identity".to_string(),
        "see [[selfie]] itself and also [[other-ref]]".to_string(),
    );
    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();
    let self_id = outcome.id.clone();
    assert_eq!(self_id.to_string(), "specs--selfie");
    let other_id = crate::EntityId::new("specs", "other-ref");

    // SELF_LINK_IGNORED warning names the self-linking entity.
    assert!(
        outcome.warnings.iter().any(|w| matches!(
            w, WarningHint::SelfLinkIgnored { id } if *id == self_id
        )),
        "self-link must emit SELF_LINK_IGNORED; got: {:?}",
        outcome.warnings,
    );

    // No self-edge: not in relationships, not Outgoing, not Incoming.
    let ent = engine.get_entity(&self_id).unwrap();
    assert!(
        ent.relationships.iter().all(|r| r.target != self_id),
        "no self-relation may be synthesised; got: {:?}",
        ent.relationships,
    );
    assert!(
        engine
            .store()
            .outgoing(&self_id)
            .iter()
            .all(|e| e.target != self_id),
        "self must not be its own Outgoing neighbour",
    );
    assert!(
        engine
            .store()
            .incoming(&self_id)
            .iter()
            .all(|e| e.from != self_id),
        "self must not be its own Incoming neighbour",
    );

    // Complement: the link to a *different* target synthesised its
    // REFERENCES edge normally.
    assert!(
        ent.relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == other_id),
        "non-self body link must still synthesise its edge; got: {:?}",
        ent.relationships,
    );
}

/// dry_run preview matches real-write outcome.
#[test]
fn create_entity_dry_run_emits_same_auto_stub_warning() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let mut args = empty_create_args("specs", "Dry Run Body Link");
    args.dry_run = true;
    args.sections
        .insert("identity".to_string(), "see [[dry-run-ghost]]".to_string());

    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();
    let has_warning = outcome.warnings.iter().any(|w| {
        matches!(
            w,
            WarningHint::InlineWikiLinkAutoStubbed { stubs, .. }
                if stubs.iter().any(|t| t.to_string() == "specs--dry-run-ghost")
        )
    });
    assert!(
        has_warning,
        "dry_run must emit the same warning as real write: {:?}",
        outcome.warnings
    );
}

/// Body wiki-link to a target that already exists
/// in the store does NOT fire the warning — no stub was created.
#[test]
fn create_entity_no_auto_stub_warning_when_target_exists() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, existing) = engine_with_seed(&tmp, "Existing Target");
    let (actor, client) = cli_actor();

    let mut args = empty_create_args("specs", "Source Linking Existing");
    let body = format!("ref [[{}]]", existing.id.path());
    args.sections.insert("identity".to_string(), body);

    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();
    let has_warning = outcome
        .warnings
        .iter()
        .any(|w| matches!(w, WarningHint::InlineWikiLinkAutoStubbed { .. }));
    assert!(
        !has_warning,
        "no auto-stub warning when target pre-exists; got: {:?}",
        outcome.warnings
    );
}
