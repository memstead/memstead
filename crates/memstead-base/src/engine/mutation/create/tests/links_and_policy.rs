//! Body links across schemas and mems: wildcard obligations,
//! undeclared cross-schema links, the cross-mem policy on inline
//! relations and body links, and the `require_notes` nudge.

use super::*;

/// Obligation-schema counterpart of the ingest wildcard:
/// an obligation mem
/// body-links into a NON-SOFTWARE user-schema destination; the
/// wildcard alias grant admits the auto-emitted REFERENCES edge.
#[test]
fn obligation_wildcard_links_into_arbitrary_destination_schema() {
    use crate::engine::test_helpers::write_schema_files_with_default_type;
    use memstead_schema::workspace_config::CrossLinkValue;

    let tmp = TempDir::new().unwrap();
    let dest_dir = tmp.path().join("dest");
    let duties_dir = tmp.path().join("duties");
    std::fs::create_dir_all(&dest_dir).unwrap();
    std::fs::create_dir_all(&duties_dir).unwrap();
    let schemas_dir = tmp.path().join("schemas");
    let user_manifest = r#"name: casefiles
version: 0.1.0
description: a user-written, non-software destination schema
when_to_use: tests
types:
  - doc
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
    write_schema_files_with_default_type(&schemas_dir, "casefiles@0.1.0", user_manifest, &["doc"]);

    let mount = |mem: &str, dir: &std::path::Path, schema: &str| crate::workspace::Mount {
        mem: mem.to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            schema,
            semver::Version::new(0, 1, 0),
        )),
        storage: crate::workspace::MountStorage::Folder {
            path: dir.to_path_buf(),
        },
        capability: crate::workspace::MountCapability::Write,
        lifecycle: crate::workspace::MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mounts = vec![
        (
            mount("dest", &dest_dir, "casefiles"),
            Box::new(FilesystemBackend::new(dest_dir.clone())) as Box<dyn MemBackend>,
        ),
        (
            mount("duties", &duties_dir, "obligation"),
            Box::new(FilesystemBackend::new(duties_dir.clone())) as Box<dyn MemBackend>,
        ),
    ];
    let mut engine = Engine::from_mounts_with_schemas_dir(mounts, Some(schemas_dir.as_path()))
        .expect("obligation + user schema boot");
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "duties".to_string(),
        CrossLinkValue::List(vec!["dest".to_string()]),
    );
    engine.set_settings(settings);
    let (actor, client) = cli_actor();

    let target = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "dest".to_string(),
                title: "Case File 17".to_string(),
                entity_type: "doc".to_string(),
                sections: IndexMap::from_iter([(
                    "body".to_string(),
                    "destination content".to_string(),
                )]),
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let entry = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "duties".to_string(),
                title: "File Annual Report & Notice".to_string(),
                entity_type: "obligation".to_string(),
                sections: IndexMap::from_iter([
                    (
                        "duty".to_string(),
                        "File the report cited in [[dest--case-file-17]].".to_string(),
                    ),
                    (
                        "consequence".to_string(),
                        "Standing lapses at the deadline.".to_string(),
                    ),
                ]),
                metadata: IndexMap::from_iter([
                    ("due_date".to_string(), "2026-12-31".to_string()),
                    ("status".to_string(), "open".to_string()),
                ]),
                relations: vec![crate::ops::RelateArg {
                    target: crate::entity::EntityId::new("duties", "subject"),
                    rel_type: "CONCERNS".to_string(),
                    description: None,
                }],
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .expect("wildcard admits the alias link into the non-software destination");
    let stored = engine.get_entity(&entry.id).unwrap();
    assert!(
        stored
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
        "alias REFERENCES edge must emit cross-mem: {:?}",
        stored.relationships
    );
}

/// End to end: an `ingest`-schema process mem body-links
/// into a destination pinning an ARBITRARY user-written schema.
/// The wildcard (bound to `alias_target_rel_type: REFERENCES`)
/// admits the auto-emitted alias edge; the edge survives a fresh
/// boot (the load path routes through the same matcher); explicit
/// authoring of the alias type still refuses
/// RELATION_MANUAL_AUTHORING_FORBIDDEN; a structural rel-type into
/// the undeclared destination still refuses
/// A body wiki-link into a destination schema the source schema
/// declares NO cross-mem entry for (and no wildcard): the schema
/// legitimately declines the edge — but the author must be TOLD.
/// default@1.3.0 declares REFERENCES only toward `software`, so a
/// default-mem entity citing a planning-mem entity got inert prose
/// where the author expects a citation edge, silently (graph-plans
/// 02 grading, 2026-08-28). The write knows it dropped the link, so
/// it warns, typed, naming the target and the declaration gap.
#[test]
fn undeclared_cross_schema_body_link_warns_instead_of_vanishing() {
    use memstead_schema::workspace_config::CrossLinkValue;

    let tmp = TempDir::new().unwrap();
    let scratch_dir = tmp.path().join("scratch");
    let plans_dir = tmp.path().join("plans");
    std::fs::create_dir_all(&scratch_dir).unwrap();
    std::fs::create_dir_all(&plans_dir).unwrap();

    let mount = |mem: &str, dir: &std::path::Path, schema: &str, version: (u64, u64, u64)| {
        crate::workspace::Mount {
            mem: mem.to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                schema,
                semver::Version::new(version.0, version.1, version.2),
            )),
            storage: crate::workspace::MountStorage::Folder {
                path: dir.to_path_buf(),
            },
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }
    };
    let mut engine = Engine::from_mounts(vec![
        (
            mount("scratch", &scratch_dir, "default", (1, 3, 0)),
            Box::new(FilesystemBackend::new(scratch_dir.clone())) as Box<dyn MemBackend>,
        ),
        (
            mount("plans", &plans_dir, "planning", (0, 4, 0)),
            Box::new(FilesystemBackend::new(plans_dir.clone())) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "scratch".to_string(),
        CrossLinkValue::List(vec!["plans".to_string()]),
    );
    engine.set_settings(settings);
    let (actor, client) = cli_actor();

    let target = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "plans".to_string(),
                title: "Target Plan Note".to_string(),
                entity_type: "goal".to_string(),
                sections: IndexMap::from_iter([
                    (
                        "statement".to_string(),
                        "the plan goal being cited".to_string(),
                    ),
                    (
                        "rationale".to_string(),
                        "cited from a scratch mem in the repro".to_string(),
                    ),
                    (
                        "success_criteria".to_string(),
                        "the citation edge exists or the drop is warned".to_string(),
                    ),
                ]),
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let entry = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "scratch".to_string(),
                title: "Session Scratch".to_string(),
                entity_type: "memo".to_string(),
                sections: IndexMap::from_iter([
                    (
                        "claim".to_string(),
                        "the pilot follows [[plans--target-plan-note]] step by step".to_string(),
                    ),
                    ("context".to_string(), "exec session scratch".to_string()),
                ]),
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .expect("prose citing an undeclared destination schema must not refuse the write");

    // The schema declines the edge — that stays.
    let stored = engine.get_entity(&entry.id).unwrap();
    assert!(
        !stored.relationships.iter().any(|r| r.target == target.id),
        "default→planning declares no entry, so no edge is emitted: {:?}",
        stored.relationships
    );
    // But the drop is TOLD, typed, naming target and gap.
    assert!(
        entry.warnings.iter().any(|w| matches!(
            w,
            WarningHint::CrossSchemaLinkUndeclared { target, .. }
                if target.as_ref() == "plans--target-plan-note"
        )),
        "the dropped body link must surface as a typed warning: {:?}",
        entry.warnings
    );
}

/// CROSS_MEM_EDGE_NOT_DECLARED; and the workspace policy gate
/// still fires when the direction is not granted.
#[test]
fn ingest_wildcard_links_into_arbitrary_destination_schema() {
    use crate::engine::test_helpers::write_schema_files_with_default_type;
    use memstead_schema::workspace_config::CrossLinkValue;

    let tmp = TempDir::new().unwrap();
    let dest_dir = tmp.path().join("dest");
    let proc_dir = tmp.path().join("proc");
    std::fs::create_dir_all(&dest_dir).unwrap();
    std::fs::create_dir_all(&proc_dir).unwrap();

    // A user-written schema the engine has never shipped.
    let schemas_dir = tmp.path().join("schemas");
    let user_manifest = r#"name: debate
version: 0.1.0
description: a user-written destination schema
when_to_use: tests
types:
  - doc
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
    write_schema_files_with_default_type(&schemas_dir, "debate@0.1.0", user_manifest, &["doc"]);

    let mount = |mem: &str, dir: &std::path::Path, schema: &str, version: (u64, u64, u64)| {
        crate::workspace::Mount {
            mem: mem.to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                schema,
                semver::Version::new(version.0, version.1, version.2),
            )),
            storage: crate::workspace::MountStorage::Folder {
                path: dir.to_path_buf(),
            },
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }
    };
    let boot = |grant: bool| -> Engine {
        let mounts = vec![
            (
                mount("dest", &dest_dir, "debate", (0, 1, 0)),
                Box::new(FilesystemBackend::new(dest_dir.clone())) as Box<dyn MemBackend>,
            ),
            (
                mount("proc", &proc_dir, "ingest", (0, 2, 0)),
                Box::new(FilesystemBackend::new(proc_dir.clone())) as Box<dyn MemBackend>,
            ),
        ];
        let mut engine = Engine::from_mounts_with_schemas_dir(mounts, Some(schemas_dir.as_path()))
            .expect("ingest + user schema boot");
        let mut settings = crate::workspace::WorkspaceSettings::default();
        if grant {
            settings.cross_mem_links.insert(
                "proc".to_string(),
                CrossLinkValue::List(vec!["dest".to_string()]),
            );
        }
        engine.set_settings(settings);
        engine
    };
    let (actor, client) = cli_actor();

    let mut engine = boot(true);
    // Destination entity in the user-schema mem.
    let target = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "dest".to_string(),
                title: "Target Doc".to_string(),
                entity_type: "doc".to_string(),
                sections: IndexMap::from_iter([(
                    "body".to_string(),
                    "destination content".to_string(),
                )]),
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Process-mem entry body-linking the destination entity.
    let entry = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "proc".to_string(),
                title: "Check The Claim".to_string(),
                entity_type: "verification_target".to_string(),
                sections: IndexMap::from_iter([
                    (
                        "claim".to_string(),
                        "the claim under suspicion lives in [[dest--target-doc]]".to_string(),
                    ),
                    ("source_to_check".to_string(), "dest mem".to_string()),
                    (
                        "verifiable_when".to_string(),
                        "the linked entity still says so".to_string(),
                    ),
                ]),
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .expect("wildcard admits the alias link into the user-schema destination");
    let stored = engine.get_entity(&entry.id).unwrap();
    assert!(
        stored
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
        "alias REFERENCES edge must emit: {:?}",
        stored.relationships
    );

    // Explicit authoring of the alias rel-type: still forbidden.
    let err = engine
        .relate_entity(
            RelateEntityArgs {
                source: entry.id.clone(),
                expected_hash: None,
                rel_type: "REFERENCES".to_string(),
                target: target.id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "RELATION_MANUAL_AUTHORING_FORBIDDEN", "{err:?}");

    // Structural rel-type into the undeclared destination: the
    // historical refusal, wildcard notwithstanding.
    let err = engine
        .relate_entity(
            RelateEntityArgs {
                source: entry.id.clone(),
                expected_hash: None,
                rel_type: "PART_OF".to_string(),
                target: target.id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "CROSS_MEM_EDGE_NOT_DECLARED", "{err:?}");

    // Load-path survival: a FRESH boot over the same folders (the
    // store-builder path that previously dropped undeclared
    // cross-mem edges) keeps the alias edge.
    drop(engine);
    let rebooted = boot(true);
    let reloaded = rebooted.get_entity(&entry.id).unwrap();
    assert!(
        reloaded
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
        "alias edge must survive reload: {:?}",
        reloaded.relationships
    );

    // Policy gate intact: without the grant, the same wildcarded
    // link refuses CROSS_MEM_LINK_NOT_ALLOWED.
    let mut denied = boot(false);
    let err = denied
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "proc".to_string(),
                title: "Denied Entry".to_string(),
                entity_type: "verification_target".to_string(),
                sections: IndexMap::from_iter([
                    (
                        "claim".to_string(),
                        "points at [[dest--target-doc]]".to_string(),
                    ),
                    ("source_to_check".to_string(), "dest mem".to_string()),
                    ("verifiable_when".to_string(), "never".to_string()),
                ]),
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "CROSS_MEM_LINK_NOT_ALLOWED", "{err:?}");
}

/// Two-mem Write-Write scaffold —
/// `test` and `other` both pin the default schema, no
/// `cross_mem_links` policy set yet (default deny-all). The
/// caller installs the policy that matches each scenario.
fn engine_with_two_default_mems() -> (TempDir, TempDir, Engine) {
    let tmp_test = TempDir::new().unwrap();
    let tmp_other = TempDir::new().unwrap();
    let test_dir = tmp_test.path().to_path_buf();
    let other_dir = tmp_other.path().to_path_buf();
    let writer_test = FilesystemBackend::new(test_dir.clone());
    let writer_other = FilesystemBackend::new(other_dir.clone());
    let engine = Engine::from_mounts(vec![
        (
            folder_mount("test", test_dir),
            Box::new(writer_test) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("other", other_dir),
            Box::new(writer_other) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    (tmp_test, tmp_other, engine)
}

/// `memstead_create` with an inline cross-mem relation refuses
/// with `CROSS_MEM_LINK_NOT_ALLOWED` when policy denies the
/// direction. The entity does not persist; the would-be id reads
/// as `NotFound`.
#[test]
fn create_entity_refuses_inline_cross_mem_relation_when_policy_denies() {
    use crate::entity::EntityId;
    use crate::ops::RelateArg;
    use memstead_schema::workspace_config::CrossLinkValue;

    let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
    let (actor, client) = cli_actor();

    // Policy: `test → other` granted only. The inline create
    // request below is `other → test`, which must refuse.
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "test".to_string(),
        CrossLinkValue::List(vec!["other".to_string()]),
    );
    engine.set_settings(settings);

    // Seed a target in the `test` mem so the inline relation
    // names a real id (the policy gate fires before target
    // resolution regardless, but a real target removes any
    // ambiguity from the assertion).
    let target = engine
        .create_entity(
            empty_create_args("test", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let mut args = empty_create_args("other", "Source");
    args.relations = vec![RelateArg {
        rel_type: "IMPLEMENTS".to_string(),
        target: target.id.clone(),
        description: None,
    }];
    let err = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap_err();
    match err {
        EngineError::CrossMemLinkNotAllowed { from_mem, to_mem } => {
            assert_eq!(from_mem, "other");
            assert_eq!(to_mem, "test");
        }
        other => panic!("expected CROSS_MEM_LINK_NOT_ALLOWED, got {other:?}"),
    }

    // No entity landed: the would-be id is absent.
    let would_be = EntityId::new("other", "source");
    assert!(
        engine.get_entity(&would_be).is_none(),
        "entity must not persist when inline relation refuses"
    );
}

/// With the granted direction, the
/// inline cross-mem relation succeeds and the edge persists.
#[test]
fn create_entity_allows_inline_cross_mem_relation_when_policy_grants() {
    use crate::ops::RelateArg;
    use memstead_schema::workspace_config::CrossLinkValue;

    let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
    let (actor, client) = cli_actor();

    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "other".to_string(),
        CrossLinkValue::List(vec!["test".to_string()]),
    );
    engine.set_settings(settings);

    let target = engine
        .create_entity(
            empty_create_args("test", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let mut args = empty_create_args("other", "Source");
    args.relations = vec![RelateArg {
        rel_type: "IMPLEMENTS".to_string(),
        target: target.id.clone(),
        description: None,
    }];
    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();
    let stored = engine.get_entity(&outcome.id).expect("entity persists");
    assert!(
        stored
            .relationships
            .iter()
            .any(|r| r.rel_type == "IMPLEMENTS" && r.target == target.id),
        "IMPLEMENTS edge must persist on the source's relationships",
    );
}

/// A same-mem inline relation
/// bypasses the policy gate entirely. Even with an empty policy
/// (default deny-all for cross-mem), the create succeeds.
#[test]
fn create_entity_admits_same_mem_inline_relation_regardless_of_policy() {
    use crate::ops::RelateArg;

    let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
    let (actor, client) = cli_actor();
    // No cross_mem_links set; same-mem writes must still work.

    let target = engine
        .create_entity(
            empty_create_args("test", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let mut args = empty_create_args("test", "Source");
    args.relations = vec![RelateArg {
        rel_type: "USES".to_string(),
        target: target.id.clone(),
        description: None,
    }];
    let outcome = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();
    let stored = engine.get_entity(&outcome.id).expect("entity persists");
    assert!(
        stored
            .relationships
            .iter()
            .any(|r| r.rel_type == "USES" && r.target == target.id),
        "same-mem USES edge must persist",
    );
}

/// The existing `memstead_relate` path
/// refuses the same scenario with the same typed code and
/// payload shape — the two surfaces' refusals are
/// indistinguishable to an agent.
#[test]
fn relate_and_create_refuse_cross_mem_policy_with_identical_envelope() {
    use crate::ops::RelateArg;
    use memstead_schema::workspace_config::CrossLinkValue;

    let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
    let (actor, client) = cli_actor();

    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "test".to_string(),
        CrossLinkValue::List(vec!["other".to_string()]),
    );
    engine.set_settings(settings);

    let target = engine
        .create_entity(
            empty_create_args("test", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let src = engine
        .create_entity(
            empty_create_args("other", "Source"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // memstead_relate refusal.
    let relate_err = engine
        .relate_entity(
            RelateEntityArgs {
                source: src.id.clone(),
                rel_type: "IMPLEMENTS".to_string(),
                target: target.id.clone(),
                expected_hash: Some(src.content_hash.clone()),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();

    // memstead_create.relations[] refusal — fresh title so the create
    // attempt hasn't already landed.
    let mut create_args = empty_create_args("other", "Source Two");
    create_args.relations = vec![RelateArg {
        rel_type: "IMPLEMENTS".to_string(),
        target: target.id.clone(),
        description: None,
    }];
    let create_err = engine
        .create_entity(create_args, actor, Some(&client), None)
        .unwrap_err();

    // Both refusals share the typed code, the payload shape, and
    // the (from_mem, to_mem) values.
    match (relate_err, create_err) {
        (
            EngineError::CrossMemLinkNotAllowed {
                from_mem: rfv,
                to_mem: rtv,
            },
            EngineError::CrossMemLinkNotAllowed {
                from_mem: cfv,
                to_mem: ctv,
            },
        ) => {
            assert_eq!(rfv, "other");
            assert_eq!(rtv, "test");
            assert_eq!(cfv, "other");
            assert_eq!(ctv, "test");
        }
        (a, b) => panic!(
            "expected matching CROSS_MEM_LINK_NOT_ALLOWED on both surfaces; got relate={a:?}, create={b:?}"
        ),
    }
}

/// Body wiki-link `[[other--target]]` in mem `test` (with
/// `test → other` granted) creates the entity, auto-stubs at
/// `other--target` (NOT `test--other--target` — that was the
/// pre-fix phantom-stub bug), and emits one REFERENCES edge via
/// the alias-synthesis path.
#[test]
fn create_entity_body_link_cross_mem_dash_form_routes_correctly() {
    use crate::entity::EntityId;
    use indexmap::IndexMap;
    use memstead_schema::workspace_config::CrossLinkValue;

    let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
    let (actor, client) = cli_actor();

    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "test".to_string(),
        CrossLinkValue::List(vec!["other".to_string()]),
    );
    engine.set_settings(settings);

    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert(
        "identity".to_string(),
        "see [[other--target]] for details".to_string(),
    );
    sections.insert("purpose".to_string(), "source purpose".to_string());
    let outcome = engine
        .create_entity(
            crate::engine::CreateEntityArgs {
                anchors: Vec::new(),
                mem: "test".to_string(),
                title: "Source".to_string(),
                entity_type: "spec".to_string(),
                sections,
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Auto-stub landed at `other--target`, NOT `test--other--target`.
    let canonical = EntityId::new("other", "target");
    assert!(
        engine.get_entity(&canonical).is_some(),
        "auto-stub must land at the canonical cross-mem id"
    );
    let phantom = EntityId::new("test", "other--target");
    assert!(
        engine.get_entity(&phantom).is_none(),
        "no double-prefixed phantom stub"
    );

    // Exactly one REFERENCES edge to the cross-mem target.
    let source = engine.get_entity(&outcome.id).unwrap();
    let references_count = source
        .relationships
        .iter()
        .filter(|r| r.rel_type == "REFERENCES" && r.target == canonical)
        .count();
    assert_eq!(
        references_count, 1,
        "alias-synthesis must emit exactly one REFERENCES edge per cross-mem body link",
    );
}

/// Complement: body wiki-link cross-mem refusal when policy
/// denies the direction. The auto-stub never lands, the entity
/// never persists.
#[test]
fn create_entity_body_link_cross_mem_refused_when_policy_denies() {
    use indexmap::IndexMap;

    let (_tmp_test, _tmp_other, mut engine) = engine_with_two_default_mems();
    let (actor, client) = cli_actor();
    // Empty cross-link policy — `test → other` denied.

    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert(
        "identity".to_string(),
        "see [[other--target]] for details".to_string(),
    );
    sections.insert("purpose".to_string(), "source purpose".to_string());
    let err = engine
        .create_entity(
            crate::engine::CreateEntityArgs {
                anchors: Vec::new(),
                mem: "test".to_string(),
                title: "Source".to_string(),
                entity_type: "spec".to_string(),
                sections,
                metadata: IndexMap::new(),
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::CrossMemLinkNotAllowed { from_mem, to_mem } => {
            assert_eq!(from_mem, "test");
            assert_eq!(to_mem, "other");
        }
        other => panic!("expected CROSS_MEM_LINK_NOT_ALLOWED, got {other:?}"),
    }
}

/// `[mutations].require_notes = true` drives a single `NOTE_MISSING`
/// warning out of the engine mutation pipeline on every noteless
/// mutation — the single enforcement point both the CLI and the MCP
/// transport inherit. The mutation still commits (the policy nudges,
/// it never blocks). Supplying a note suppresses it; turning the
/// policy off silences it entirely. Covers create / update / relate
/// in one engine instance.
#[test]
fn require_notes_drives_single_note_missing_warning_per_noteless_mutation() {
    use crate::engine::UpdateEntityArgs;
    use crate::workspace::{MutationsSection, WorkspaceSettings};
    use indexmap::IndexMap;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    engine.set_settings(WorkspaceSettings {
        mutations: MutationsSection {
            require_notes: Some(true),
        },
        ..Default::default()
    });
    let (actor, client) = cli_actor();

    let note_missing = |ws: &[WarningHint]| -> usize {
        ws.iter()
            .filter(|w| matches!(w, WarningHint::NoteMissing { tool: _ }))
            .count()
    };

    // --- create, no note: exactly one NOTE_MISSING, commit landed ---
    let created = engine
        .create_entity(
            empty_create_args("specs", "Noteless"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(
        note_missing(&created.warnings),
        1,
        "create under require_notes must emit exactly one NOTE_MISSING; got {:?}",
        created.warnings,
    );
    assert!(
        matches!(
            created.warnings.iter().find(|w| matches!(w, WarningHint::NoteMissing { .. })),
            Some(WarningHint::NoteMissing { tool }) if tool == "create_entity"
        ),
        "the warning names the engine-level verb",
    );
    assert!(
        !created.write_id.is_empty(),
        "create still commits (nudge, not block)"
    );

    // --- update, no note: NOTE_MISSING + commit landed ---
    let mut edit: IndexMap<String, String> = IndexMap::new();
    edit.insert("identity".to_string(), "revised".to_string());
    let updated = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: created.id.clone(),
                expected_hash: Some(created.content_hash.clone()),
                sections: edit,
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
    assert_eq!(
        note_missing(&updated.warnings),
        1,
        "update emits NOTE_MISSING"
    );
    assert!(!updated.write_id.is_empty(), "update still commits");

    // --- relate, no note: NOTE_MISSING + commit landed ---
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            Some("seed"),
        )
        .unwrap();
    let related = engine
        .relate_entity(
            RelateEntityArgs {
                source: updated.id.clone(),
                expected_hash: Some(updated.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: target.id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(
        note_missing(&related.warnings),
        1,
        "relate emits NOTE_MISSING"
    );
    assert!(!related.write_id.is_empty(), "relate still commits");

    // --- relate rehearsal, no note: nothing lands, nothing to attribute ---
    let rehearsed = engine
        .relate_entity(
            RelateEntityArgs {
                source: updated.id.clone(),
                expected_hash: Some(related.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: target.id.clone(),
                remove: true,
                description: None,
                dry_run: true,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(rehearsed.write_id.is_empty(), "a rehearsal commits nothing");
    assert_eq!(
        note_missing(&rehearsed.warnings),
        0,
        "a relate rehearsal never demands a note: {:?}",
        rehearsed.warnings
    );

    // --- with a note: suppressed ---
    let with_note = engine
        .create_entity(
            empty_create_args("specs", "Documented"),
            actor,
            Some(&client),
            Some("a real provenance note"),
        )
        .unwrap();
    assert_eq!(
        note_missing(&with_note.warnings),
        0,
        "a supplied note suppresses the warning",
    );

    // --- policy off: silent even without a note ---
    engine.set_settings(WorkspaceSettings::default());
    let after_off = engine
        .create_entity(
            empty_create_args("specs", "Quiet"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(
        note_missing(&after_off.warnings),
        0,
        "no NOTE_MISSING when require_notes is unset",
    );
}
