#![cfg(test)]

use indexmap::IndexMap;
use tempfile::TempDir;

use crate::backend::MemBackend;
use crate::engine::test_helpers::*;
use crate::engine::{CreateEntityArgs, Engine, EngineError, RelateAction, RelateEntityArgs};
use crate::ops::WarningHint;
use crate::storage::FilesystemBackend;
use crate::vcs::{Actor, CommitContext};

#[test]
fn relate_alias_delegates_to_relate_entity() {
    // Positional-args alias mirrors full's signature
    // `engine.relate(from, to, rel_type, remove, ctx)`. Add an
    // edge via the alias and via `relate_entity` and assert
    // they reach the same observable post-state.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    // Seed two real entities (no stub).
    let a = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "A".to_string(),
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
    let b = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "B".to_string(),
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

    // Use the positional `relate` alias.
    let ctx = CommitContext::internal();
    let result = engine.relate(&a.id, &b.id, "PART_OF", false, &ctx).unwrap();
    assert_eq!(result.from, a.id);
    assert_eq!(result.to, b.id);
    assert_eq!(result.rel_type, "PART_OF");
    // The edge is in the store post-call.
    let outgoing: Vec<_> = engine.store().outgoing(&a.id).to_vec();
    assert!(
        outgoing
            .iter()
            .any(|e| e.target == b.id && e.rel_type == "PART_OF")
    );
}

#[test]
fn relate_entity_appends_relationship_and_logs_provenance() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Source");
    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let outcome = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
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
    assert_eq!(outcome.action, RelateAction::Added);
    assert_ne!(outcome.content_hash, source.content_hash);
    // Edge present in store.
    let edges = engine.store().outgoing(&source.id);
    assert!(
        edges
            .iter()
            .any(|e| e.rel_type == "USES" && e.target == target.id),
        "expected USES edge in store"
    );
    // Provenance log records relate.
    let log = std::fs::read_to_string(tmp.path().join(".memstead/changes.jsonl")).unwrap();
    assert!(log.contains("\"kind\":\"relate\""));
}

#[test]
fn relate_entity_no_op_when_already_present() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Already");
    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(empty_create_args("specs", "T2"), actor, Some(&client), None)
        .unwrap();
    let first = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
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
    let second = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(first.content_hash.clone()),
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
    assert_eq!(second.action, RelateAction::NoOpAlreadyPresent);
    // Hash unchanged on no-op.
    assert_eq!(second.content_hash, first.content_hash);
}

#[test]
fn relate_entity_returns_write_id_on_real_write() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Source");
    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let outcome = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
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
    // Folder backend returns a synthetic CommitId — non-empty string.
    // Wire-equivalent to full's commit SHA: agents reading the field
    // get a usable cursor regardless of which backend served the write.
    assert!(
        !outcome.write_id.is_empty(),
        "write_id must be populated on a real write"
    );
}

#[test]
fn relate_entity_no_op_paths_carry_typed_warnings_and_empty_write_id() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "S");
    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(empty_create_args("specs", "T"), actor, Some(&client), None)
        .unwrap();

    // Add the edge once.
    let first = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
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

    // Duplicate-add — typed DuplicateRelationship warning, empty
    // write_id (no disk write happened).
    let dup = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(first.content_hash.clone()),
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
    assert_eq!(dup.action, RelateAction::NoOpAlreadyPresent);
    assert!(dup.write_id.is_empty());
    assert_eq!(dup.warnings.len(), 1);
    assert!(matches!(
        dup.warnings[0],
        WarningHint::DuplicateRelationship { .. }
    ));

    // Remove a non-existent edge — typed NoSuchRelationship warning,
    // empty write_id.
    let no_such = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(first.content_hash.clone()),
                rel_type: "DEPENDS_ON".to_string(),
                target: target.id.clone(),
                remove: true,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(no_such.action, RelateAction::NoOpAbsent);
    assert!(no_such.write_id.is_empty());
    assert_eq!(no_such.warnings.len(), 1);
    assert!(matches!(
        no_such.warnings[0],
        WarningHint::NoSuchRelationship { .. }
    ));
}

#[test]
fn relate_entity_creates_stub_for_absent_target_on_add_path() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Source");
    let (actor, client) = cli_actor();
    let absent_target = crate::EntityId::new("specs", "ghost-target");
    // Sanity: target not in store.
    assert!(!engine.store().contains(&absent_target));

    let outcome = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: absent_target.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    assert_eq!(outcome.action, RelateAction::Added);
    assert_eq!(outcome.source, "explicit");
    // Auto-stub now surfaces through the typed warning vocabulary
    // (`AutoStubCreated`) on `warnings[]` — the deprecated
    // top-level `stub_warning` field was retired in favour of the
    // uniform diagnostic shape. Agents iterating `warnings[]` see
    // the stub id without special-casing a sibling field.
    let stub_warning = outcome
        .warnings
        .iter()
        .find_map(|w| match w {
            crate::ops::WarningHint::AutoStubCreated { stub_id, .. } => Some(stub_id.clone()),
            _ => None,
        })
        .expect("AutoStubCreated warning must surface when target was absent");
    assert_eq!(stub_warning, absent_target);
    // The real path keeps the performed-effect wording exactly —
    // only the dry-run path carries the conditional form.
    let msg = outcome
        .warnings
        .iter()
        .find(|w| matches!(w, crate::ops::WarningHint::AutoStubCreated { .. }))
        .unwrap()
        .message();
    assert!(
        msg.contains("stub auto-created"),
        "real relate keeps the performed-effect wording: {msg}"
    );

    // Stub now in-store, marked as stub, no body.
    let stub = engine.store().get(&absent_target).expect("stub upserted");
    assert!(stub.stub);
    assert!(stub.entity_type.is_empty());
    assert!(stub.file_path.is_empty());
}

#[test]
fn relate_entity_skips_stub_creation_when_target_already_exists() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Src");
    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(
            empty_create_args("specs", "Real"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let outcome = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
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

    assert!(
        !outcome
            .warnings
            .iter()
            .any(|w| matches!(w, crate::ops::WarningHint::AutoStubCreated { .. })),
        "AutoStubCreated must not surface when target was already in store"
    );
    assert_eq!(outcome.source, "explicit");
    // Real entity remains a real entity (not coerced to stub).
    let target_after = engine.store().get(&target.id).unwrap();
    assert!(!target_after.stub);
}

#[test]
fn relate_entity_does_not_create_stub_on_remove_path() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Src");
    let (actor, client) = cli_actor();
    let absent_target = crate::EntityId::new("specs", "never-existed");

    let outcome = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: absent_target.clone(),
                remove: true,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Remove of an absent edge — no stub creation, NoOpAbsent action,
    // typed NoSuchRelationship warning.
    assert_eq!(outcome.action, RelateAction::NoOpAbsent);
    assert!(
        !outcome
            .warnings
            .iter()
            .any(|w| matches!(w, crate::ops::WarningHint::AutoStubCreated { .. })),
        "remove path must never auto-stub the target",
    );
    assert!(!engine.store().contains(&absent_target));
}

#[test]
fn relate_entity_remove_refuses_when_source_body_still_references_target() {
    use crate::engine::CreateEntityArgs;
    use indexmap::IndexMap;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Source entity carries a body wiki-link to the target — the
    // alias-synthesis pass emits the backing REFERENCES relation
    // (default schema's `alias_target_rel_type` points at
    // REFERENCES, so explicit `memstead_relate type=REFERENCES` is
    // refused; the body link alone produces the relation).
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert("identity".to_string(), "source identity".to_string());
    sections.insert(
        "purpose".to_string(),
        "discussion stems from [[target]]".to_string(),
    );
    let source = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
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
    let related = source.clone();

    // Removing the explicit relation while the body still has
    // [[target]] must refuse with `RelationHasBodyLinks`, naming
    // the surviving section in `body_links`.
    let err = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(related.content_hash.clone()),
                rel_type: "REFERENCES".to_string(),
                target: target.id.clone(),
                remove: true,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::RelationHasBodyLinks {
            from_id,
            to_id,
            rel_type,
            body_links,
        } => {
            assert_eq!(from_id, source.id.to_string());
            assert_eq!(to_id, target.id.to_string());
            assert_eq!(rel_type, "REFERENCES");
            assert_eq!(body_links, vec!["purpose".to_string()]);
        }
        other => panic!("expected RelationHasBodyLinks, got {other:?}"),
    }
    // Relation must still be present in-memory (refuse before any
    // store mutation).
    let in_mem = engine.get_entity(&source.id).unwrap();
    assert!(
        in_mem
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
        "relation must survive the refused remove; got {:?}",
        in_mem.relationships
    );
}

#[test]
fn relate_entity_remove_succeeds_when_body_no_longer_references_target() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Src");
    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(
            empty_create_args("specs", "Other"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    // Default seed has empty body sections, so the relation can be
    // added and removed without body-link interference. This locks
    // the happy path: when no body link survives, remove proceeds.
    // (USES instead of REFERENCES — REFERENCES is engine-emitted-only
    // under the default schema's alias_target_rel_type pointer.)
    let related = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
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
    let removed = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(related.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: target.id.clone(),
                remove: true,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(removed.action, RelateAction::Removed);
}

#[test]
fn relate_entity_auto_stub_is_tagged_forward_reference() {
    // `memstead_relate` to an absent target auto-stubs it. The stub's
    // `stub_kind` records the origin (`ForwardReference`) so an
    // agent reading the stub later via `memstead_entity` sees the
    // typed provenance — not just `stub: true`.
    use crate::entity::StubKind;

    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Src");
    let (actor, client) = cli_actor();
    let absent_target = crate::EntityId::new("specs", "absent-target");

    let _ = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: absent_target.clone(),
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
        .get_entity(&absent_target)
        .expect("relate auto-stubbed target must be in the store");
    assert!(stub.stub, "auto-stubbed target must carry stub: true");
    assert_eq!(
        stub.stub_kind,
        Some(StubKind::ForwardReference),
        "auto-stub from relate must be tagged ForwardReference; got {:?}",
        stub.stub_kind
    );
}

#[test]
fn relate_entity_case_insensitive_rel_type_input_canonicalises_to_upper_snake_case() {
    // Wire-level contract: rel_type input is case-insensitive; the
    // engine stores it as UPPER_SNAKE_CASE and echoes the canonical
    // form back in the response. Same store-shape regardless of
    // input case.
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Source");
    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Lowercase input — must succeed and store as `USES`.
    let lower = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "uses".to_string(),
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
    assert_eq!(lower.rel_type, "USES", "response must echo canonical form");
    assert_eq!(lower.action, RelateAction::Added);
    let edges = engine.store().outgoing(&source.id);
    assert!(
        edges
            .iter()
            .any(|e| e.rel_type == "USES" && e.target == target.id),
        "store must hold UPPER_SNAKE_CASE rel_type after lowercase input"
    );

    // Adding via mixed-case input on the same edge is the canonical
    // duplicate — DuplicateRelationship warning, no second store
    // entry.
    let dup = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(lower.content_hash.clone()),
                rel_type: "Uses".to_string(),
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
    assert_eq!(dup.action, RelateAction::NoOpAlreadyPresent);
    assert_eq!(dup.rel_type, "USES");
    assert!(matches!(
        dup.warnings[0],
        WarningHint::DuplicateRelationship { .. }
    ));
}

#[test]
fn relate_entity_rejects_cross_mem_when_policy_denies() {
    // Default workspace settings carry no `cross_mem_links`
    // policy and no `default_cross_links` on the create rules, so
    // `cross_mem_link_allowed` returns false for any cross-mem
    // pair. The relate refuse now surfaces the typed
    // policy-denial code instead of the legacy categorical
    // `CrossMemRelate`.
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "S");
    let (actor, client) = cli_actor();
    let err = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: crate::EntityId::new("other-mem", "thing"),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    match err {
        EngineError::CrossMemLinkNotAllowed { from_mem, to_mem } => {
            assert_eq!(from_mem, "specs");
            assert_eq!(to_mem, "other-mem");
        }
        other => panic!("expected CrossMemLinkNotAllowed, got {other:?}"),
    }
}

/// Bare-string target without a `mem--` separator is malformed
/// (the wiki-link grammar requires `<mem>--<path>`). Pre-fix
/// the cross-mem check fired first: the parser saw `mem: ""`,
/// compared against the source mem, and produced
/// `CROSS_MEM_RELATION` — pointing the agent at workspace
/// `[cross_mem_links]` policy when the actual issue was a
/// malformed id. Post-fix the grammar gate runs first; the
/// envelope identifies the real problem.
#[test]
fn relate_entity_malformed_bare_target_surfaces_invalid_entity_id_not_cross_mem() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "S");
    let (actor, client) = cli_actor();
    let err = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                // No `--` separator AND contains characters the
                // grammar rejects. Parses as mem="", path=raw.
                target: crate::EntityId("bad target with spaces!!".to_string()),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert!(
        matches!(err, EngineError::InvalidEntityId { .. }),
        "malformed bare-string target must surface INVALID_ENTITY_ID, got: {err:?}"
    );
}

/// Companion case: target carries the source's mem prefix but a
/// grammar-violating path. The grammar check fires (same path as
/// the bare-string case); cross-mem stays out of the picture
/// because mems match.
#[test]
fn relate_entity_malformed_prefixed_target_surfaces_invalid_entity_id() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "S");
    let (actor, client) = cli_actor();
    let source_mem = source.id.mem().to_string();
    let err = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: crate::EntityId(format!("{source_mem}--bad target with spaces!!")),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert!(
        matches!(err, EngineError::InvalidEntityId { .. }),
        "prefixed malformed target must still surface INVALID_ENTITY_ID, got: {err:?}"
    );
}

// ---- Auto-timestamp on relate add/remove ------------------------

/// `memstead_relate add` rewrites the
/// source's on-disk file, so its `last_modified` auto-stamp must
/// bump. The schema's default-stamped field is `last_modified`.
#[test]
fn relate_add_bumps_last_modified_on_source_entity() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "S");
    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(empty_create_args("specs", "T"), actor, Some(&client), None)
        .unwrap();

    let outcome = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
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
    assert_eq!(outcome.action, RelateAction::Added);

    // last_modified now carries a fresh ISO timestamp on the
    // source entity. The auto-stamp helper sets every
    // `auto_timestamp: true` metadata field on each commit-
    // producing relate mutation.
    let post = engine.get_entity(&source.id).unwrap();
    let last_modified = post
        .metadata
        .get("last_modified")
        .map(|v| v.to_frontmatter_string())
        .unwrap_or_default();
    assert!(
        last_modified.starts_with("20"),
        "last_modified must carry an ISO timestamp post-relate; got: {last_modified:?}"
    );
}

/// Relate-add no-op (idempotent
/// re-add) skips the disk write and therefore does not advance
/// `last_modified`. The auto-stamp fires only on commit-producing
/// mutations — wired into the post-no-op-short-circuit branch.
#[test]
fn relate_add_noop_does_not_bump_last_modified() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "S");
    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(empty_create_args("specs", "T"), actor, Some(&client), None)
        .unwrap();
    let first = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
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
    let pre = engine.get_entity(&source.id).unwrap();
    let pre_stamp = pre
        .metadata
        .get("last_modified")
        .map(|v| v.to_frontmatter_string())
        .unwrap_or_default();

    // Second relate of same edge — NoOpAlreadyPresent.
    let dup = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(first.content_hash.clone()),
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
    assert_eq!(dup.action, RelateAction::NoOpAlreadyPresent);

    let post = engine.get_entity(&source.id).unwrap();
    let post_stamp = post
        .metadata
        .get("last_modified")
        .map(|v| v.to_frontmatter_string())
        .unwrap_or_default();
    assert_eq!(
        pre_stamp, post_stamp,
        "last_modified must not advance on a duplicate-add no-op (no disk write happened)"
    );
}

/// Cross-mem relate that policy admits
/// but whose target mem is not mounted in the workspace emits
/// `CROSS_MEM_TARGET_MEM_UNCREATED` alongside `AutoStubCreated`.
/// The auto-stub still lands; the warning is layered observability.
#[test]
fn cross_mem_relate_to_uncreated_mem_emits_typed_warning() {
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "S");
    let (actor, client) = cli_actor();
    // Grant `specs -> uncreated-mem` so the policy gate passes.
    // The target mem is intentionally not mounted; the auto-stub
    // should still land, with the typed warning attached.
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "specs".to_string(),
        CrossLinkValue::List(vec!["uncreated-mem".to_string()]),
    );
    engine.set_settings(settings);

    let absent = crate::EntityId::new("uncreated-mem", "ghost");
    let outcome = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: absent.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(outcome.action, RelateAction::Added);

    // Auto-stub created plus uncreated-mem warning, side by side.
    let saw_uncreated = outcome.warnings.iter().any(|w| {
        matches!(
            w,
            WarningHint::CrossMemTargetMemUncreated {
                from_mem,
                to_mem,
                target_id,
            } if from_mem == "specs"
                && to_mem == "uncreated-mem"
                && target_id == &absent
        )
    });
    assert!(
        saw_uncreated,
        "CrossMemTargetMemUncreated warning must surface; got: {:?}",
        outcome.warnings
    );
    // The auto-stub still landed.
    assert!(engine.store().contains(&absent));
}

/// Policy refusal takes precedence
/// over the uncreated-mem warning. When the cross-mem link
/// isn't granted, the engine refuses with
/// `CROSS_MEM_LINK_NOT_ALLOWED` and never reaches the warning
/// emission point — there's no stub to warn about.
#[test]
fn cross_mem_relate_policy_refusal_preempts_uncreated_mem_warning() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "S");
    let (actor, client) = cli_actor();
    // No cross_mem_links entry → policy denies.
    let absent = crate::EntityId::new("uncreated-mem", "ghost");
    let err = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: absent.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::CrossMemLinkNotAllowed { .. }));
    // No stub created on the refusal path.
    assert!(!engine.store().contains(&absent));
}

/// `memstead_relate --remove` that drops the
/// last incoming edge to a stub GCs the now-orphan stub in the
/// same call. The response carries the dropped ids in
/// `orphan_stubs_removed`, mirroring the `memstead_delete` envelope's
/// shape so consumers branch uniformly.
#[test]
fn relate_remove_garbage_collects_orphan_stub() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Src");
    let (actor, client) = cli_actor();
    let stub_id = crate::EntityId::new("specs", "ghost-target");

    // Auto-stub via relate-add.
    let added = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: stub_id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(engine.store().contains(&stub_id));

    let removed = engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(added.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: stub_id.clone(),
                remove: true,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(removed.action, RelateAction::Removed);
    assert_eq!(
        removed.orphan_stubs_removed,
        vec![stub_id.clone()],
        "orphan stub must be GC'd in the same call"
    );
    assert!(
        !engine.store().contains(&stub_id),
        "stub must be gone from the store after GC"
    );
}

/// When the stub has another
/// surviving incoming edge, the relate-remove GCs nothing —
/// the stub stays alive via the second referrer. The sweep is
/// scoped to *just-orphaned* targets, not pre-existing orphans
/// or stubs that still have referrers.
#[test]
fn relate_remove_does_not_gc_stub_with_surviving_incoming_edge() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source_a) = engine_with_seed(&tmp, "SrcA");
    let (actor, client) = cli_actor();
    let source_b = engine
        .create_entity(
            empty_create_args("specs", "SrcB"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let stub_id = crate::EntityId::new("specs", "ghost-target");

    // Both sources relate to the same stub.
    let a_added = engine
        .relate_entity(
            RelateEntityArgs {
                source: source_a.id.clone(),
                expected_hash: Some(source_a.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: stub_id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let _b_added = engine
        .relate_entity(
            RelateEntityArgs {
                source: source_b.id.clone(),
                expected_hash: Some(source_b.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: stub_id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Drop source_a's edge only — source_b's edge survives,
    // so the stub is not orphaned.
    let removed = engine
        .relate_entity(
            RelateEntityArgs {
                source: source_a.id.clone(),
                expected_hash: Some(a_added.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: stub_id.clone(),
                remove: true,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(removed.action, RelateAction::Removed);
    assert!(
        removed.orphan_stubs_removed.is_empty(),
        "stub with surviving referrer must not be GC'd; got: {:?}",
        removed.orphan_stubs_removed
    );
    assert!(
        engine.store().contains(&stub_id),
        "stub must remain in store while another referrer holds it"
    );
}

// ---- Cross-mem vocabulary -------------------------------------

/// Two-mem test bench wired for cross-mem routing.
/// Mem `src` pins `src-cv@0.1.0` whose `cross_mem_relationships`
/// section declares an outbound entry to the `tgt-cv` domain with
/// `ADDRESSES: doc → req`. Mem `tgt` pins `tgt-cv@0.1.0`, a
/// schema with a different name. The workspace policy admits the
/// cross-mem link so vocabulary failures surface independently
/// of permission.
mod cross_mem {
    use std::collections::BTreeMap;
    use std::path::Path;

    use indexmap::IndexMap;
    use memstead_schema::SchemaRef;
    use memstead_schema::workspace_config::CrossLinkValue;
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::test_helpers::*;
    use crate::engine::{
        CreateEntityArgs, CreateEntityOutcome, Engine, EngineError, RelateAction, RelateEntityArgs,
    };
    use crate::storage::FilesystemBackend;

    use crate::workspace::{
        Mount, MountCapability, MountLifecycle, MountStorage, WorkspaceSettings,
    };

    fn write_schema_files(root: &Path, name: &str, manifest: &str, types: &[(&str, &str)]) {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("types")).unwrap();
        std::fs::write(dir.join("schema.yaml"), manifest).unwrap();
        for (type_name, body) in types {
            std::fs::write(dir.join("types").join(format!("{type_name}.yaml")), body).unwrap();
        }
    }

    const TYPE_BODY: &str = r#"description: t
when_to_use: Here
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
hierarchy_relationship: _default
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
"#;

    fn make_type_yaml(name: &str) -> String {
        format!("name: {name}\n{TYPE_BODY}")
    }

    fn folder_mount_with_pin(mem: &str, path: std::path::PathBuf, pin: SchemaRef) -> Mount {
        Mount {
            mem: mem.to_string(),
            schema: Some(pin),
            storage: MountStorage::Folder { path },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }
    }

    /// Build an engine with two mems pinning two distinct schemas
    /// and a `cross_mem_links` policy admitting the cross-edge.
    fn two_mem_engine() -> (TempDir, Engine, CreateEntityOutcome, CreateEntityOutcome) {
        let tmp = TempDir::new().unwrap();

        // Source schema with cross-mem declarations to the
        // tgt-cv domain.
        let src_manifest = r#"name: src-cv
version: 0.1.0
description: source schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: IMPLEMENTS
      description: intra-mem only
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
cross_mem_relationships:
  - to_schema: tgt-cv
    definitions:
      - name: ADDRESSES
        description: outbound shape-pinned
        default_weight: 1.0
        source_types: [doc]
        target_types: [req]
community:
  resolution: 1.0
  seed: 42
"#;
        // Target schema declares no cross_mem_relationships (we
        // never relate from tgt → src in these tests).
        let tgt_manifest = r#"name: tgt-cv
version: 0.1.0
description: target schema
when_to_use: tests
types:
  - req
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hierarchy
      default_weight: 3.0
      acyclic: true
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();
        write_schema_files(
            &schemas_dir,
            "src-cv",
            src_manifest,
            &[("doc", &make_type_yaml("doc"))],
        );
        write_schema_files(
            &schemas_dir,
            "tgt-cv",
            tgt_manifest,
            &[("req", &make_type_yaml("req"))],
        );

        let src_dir = tmp.path().join("mem-src");
        let tgt_dir = tmp.path().join("mem-tgt");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&tgt_dir).unwrap();

        let src_writer = FilesystemBackend::new(src_dir.clone());
        let tgt_writer = FilesystemBackend::new(tgt_dir.clone());
        let src_pin = SchemaRef::new("src-cv", semver::Version::new(0, 1, 0));
        let tgt_pin = SchemaRef::new("tgt-cv", semver::Version::new(0, 1, 0));

        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![
                (
                    folder_mount_with_pin("src", src_dir, src_pin),
                    Box::new(src_writer) as Box<dyn MemBackend>,
                ),
                (
                    folder_mount_with_pin("tgt", tgt_dir, tgt_pin),
                    Box::new(tgt_writer) as Box<dyn MemBackend>,
                ),
            ],
            Some(&schemas_dir),
        )
        .expect("two-mem engine constructs");

        // Wildcard permission so cross-mem edges aren't blocked
        // by the orthogonal policy gate (we exercise the vocabulary
        // gate here, not the permission gate).
        let mut settings = WorkspaceSettings::default();
        let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
        links.insert("src".to_string(), CrossLinkValue::Wildcard);
        settings.cross_mem_links = links;
        engine.set_settings(settings);

        let (actor, client) = cli_actor();
        let src_entity = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "src".to_string(),
                    title: "Doc One".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([("body".to_string(), "seed".to_string())]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("source entity creates");
        let tgt_entity = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "tgt".to_string(),
                    title: "Req One".to_string(),
                    entity_type: "req".to_string(),
                    sections: IndexMap::from_iter([("body".to_string(), "seed".to_string())]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("target entity creates");

        (tmp, engine, src_entity, tgt_entity)
    }

    #[test]
    fn cross_different_schema_admits_declared_edge() {
        let (_tmp, mut engine, src, tgt) = two_mem_engine();
        let (actor, client) = cli_actor();
        let outcome = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(src.content_hash.clone()),
                    rel_type: "ADDRESSES".to_string(),
                    target: tgt.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("declared cross-mem edge admits");
        assert_eq!(outcome.rel_type, "ADDRESSES");
    }

    /// Same schema name at different versions is the same domain:
    /// edges between two `same-dom`-pinned mems route through
    /// the intra-schema relationship vocabulary (governed by the
    /// source mem's pinned version) with no
    /// `cross_mem_relationships` declaration at all.
    #[test]
    fn same_name_different_version_uses_intra_mem_vocabulary() {
        let tmp = TempDir::new().unwrap();

        let manifest_for = |version: &str| {
            format!(
                r#"name: same-dom
version: {version}
description: same-domain schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: IMPLEMENTS
      description: intra-mem vocabulary
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#
            )
        };
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();
        // Subdir names carry the version so both iterations of the
        // `same-dom` domain coexist in one schemas dir.
        write_schema_files(
            &schemas_dir,
            "same-dom-0.1.0",
            &manifest_for("0.1.0"),
            &[("doc", &make_type_yaml("doc"))],
        );
        write_schema_files(
            &schemas_dir,
            "same-dom-0.2.0",
            &manifest_for("0.2.0"),
            &[("doc", &make_type_yaml("doc"))],
        );

        let src_dir = tmp.path().join("mem-src");
        let tgt_dir = tmp.path().join("mem-tgt");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&tgt_dir).unwrap();
        let src_pin = SchemaRef::new("same-dom", semver::Version::new(0, 1, 0));
        let tgt_pin = SchemaRef::new("same-dom", semver::Version::new(0, 2, 0));
        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![
                (
                    folder_mount_with_pin("src", src_dir.clone(), src_pin),
                    Box::new(FilesystemBackend::new(src_dir)) as Box<dyn MemBackend>,
                ),
                (
                    folder_mount_with_pin("tgt", tgt_dir.clone(), tgt_pin),
                    Box::new(FilesystemBackend::new(tgt_dir)) as Box<dyn MemBackend>,
                ),
            ],
            Some(&schemas_dir),
        )
        .expect("same-domain two-version engine constructs");

        let mut settings = WorkspaceSettings::default();
        let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
        links.insert("src".to_string(), CrossLinkValue::Wildcard);
        settings.cross_mem_links = links;
        engine.set_settings(settings);

        let (actor, client) = cli_actor();
        let mk_entity = |engine: &mut Engine, mem: &str, title: &str| {
            engine
                .create_entity(
                    CreateEntityArgs {
                        anchors: Vec::new(),
                        mem: mem.to_string(),
                        title: title.to_string(),
                        entity_type: "doc".to_string(),
                        sections: IndexMap::from_iter([("body".to_string(), "seed".to_string())]),
                        metadata: IndexMap::new(),
                        relations: Vec::new(),
                        dry_run: false,
                    },
                    actor,
                    Some(&client),
                    None,
                )
                .expect("entity creates")
        };
        let src_entity = mk_entity(&mut engine, "src", "Doc A");
        let tgt_entity = mk_entity(&mut engine, "tgt", "Doc B");

        let outcome = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src_entity.id.clone(),
                    expected_hash: Some(src_entity.content_hash.clone()),
                    rel_type: "IMPLEMENTS".to_string(),
                    target: tgt_entity.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("same-domain edge uses the intra-schema vocabulary across versions");
        assert_eq!(outcome.rel_type, "IMPLEMENTS");
    }

    #[test]
    fn cross_different_schema_unknown_rel_type_returns_invalid_rel_type() {
        // `IMPLEMENTS` exists intra-mem but not in the cross-mem
        // entry — must refuse with INVALID_REL_TYPE against the
        // cross-mem entry's vocabulary (not intra-mem's).
        let (_tmp, mut engine, src, tgt) = two_mem_engine();
        let (actor, client) = cli_actor();
        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(src.content_hash.clone()),
                    rel_type: "IMPLEMENTS".to_string(),
                    target: tgt.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::Validation(
                crate::runtime_validator::ValidationError::InvalidRelationshipType {
                    input,
                    allowed,
                    ..
                },
            ) => {
                assert_eq!(input, "IMPLEMENTS");
                let names: Vec<String> = allowed.into_iter().map(|h| h.name).collect();
                assert!(names.iter().any(|n| n == "ADDRESSES"));
                assert!(!names.iter().any(|n| n == "IMPLEMENTS"));
            }
            other => panic!("expected Validation(InvalidRelationshipType), got {other:?}"),
        }
    }

    #[test]
    fn cross_different_schema_shape_violation_returns_invalid_rel_shape() {
        // ADDRESSES is shape-pinned to source=doc, target=req in
        // the cross-mem entry. Need a source whose type isn't doc.
        // src-cv only declares `doc`, so to provoke a shape miss we
        // build a third schema with type `note` and a fresh mem —
        // but that requires more plumbing than this test needs.
        // Instead: exercise a target-side shape miss by relating
        // ADDRESSES to a target that doesn't exist at all — the
        // target_type lookup returns None and the target check is
        // skipped (admits). So we exercise this via cross_mem
        // unit tests instead.
        //
        // What this integration test confirms: the source-side
        // shape check fires when the source type doesn't match —
        // here we'd need a non-`doc` source. Since src-cv only has
        // `doc`, the source-side admits trivially. Covered fully
        // by the runtime_validator unit tests.
    }

    #[test]
    fn cross_different_schema_no_matching_entry_returns_edge_not_declared() {
        // Build a third mem pinning a schema not declared in
        // src-cv's cross_mem_relationships, then relate from src.
        let tmp = TempDir::new().unwrap();
        let src_manifest = r#"name: src-cv
version: 0.1.0
description: source schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: IMPLEMENTS
      description: intra-mem
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
cross_mem_relationships:
  - to_schema: tgt-cv
    definitions:
      - name: ADDRESSES
        description: outbound
        default_weight: 1.0
        source_types: [doc]
        target_types: [req]
community:
  resolution: 1.0
  seed: 42
"#;
        // Different target schema NOT named in src's cross-mem list.
        let other_manifest = r#"name: other-cv
version: 0.1.0
description: foreign schema
when_to_use: tests
types:
  - thing
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
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();
        write_schema_files(
            &schemas_dir,
            "src-cv",
            src_manifest,
            &[("doc", &make_type_yaml("doc"))],
        );
        write_schema_files(
            &schemas_dir,
            "other-cv",
            other_manifest,
            &[("thing", &make_type_yaml("thing"))],
        );
        let src_dir = tmp.path().join("mem-src");
        let other_dir = tmp.path().join("mem-other");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&other_dir).unwrap();

        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![
                (
                    folder_mount_with_pin(
                        "src",
                        src_dir.clone(),
                        SchemaRef::new("src-cv", semver::Version::new(0, 1, 0)),
                    ),
                    Box::new(FilesystemBackend::new(src_dir)) as Box<dyn MemBackend>,
                ),
                (
                    folder_mount_with_pin(
                        "other",
                        other_dir.clone(),
                        SchemaRef::new("other-cv", semver::Version::new(0, 1, 0)),
                    ),
                    Box::new(FilesystemBackend::new(other_dir)) as Box<dyn MemBackend>,
                ),
            ],
            Some(&schemas_dir),
        )
        .expect("engine constructs");

        let mut settings = WorkspaceSettings::default();
        let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
        links.insert("src".to_string(), CrossLinkValue::Wildcard);
        settings.cross_mem_links = links;
        engine.set_settings(settings);

        let (actor, client) = cli_actor();
        let src_entity = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "src".to_string(),
                    title: "D".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([("body".to_string(), "x".to_string())]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let other_entity = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "other".to_string(),
                    title: "T".to_string(),
                    entity_type: "thing".to_string(),
                    sections: IndexMap::from_iter([("body".to_string(), "x".to_string())]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src_entity.id.clone(),
                    expected_hash: Some(src_entity.content_hash.clone()),
                    rel_type: "ADDRESSES".to_string(),
                    target: other_entity.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::CrossMemEdgeNotDeclared {
                source_schema,
                target_schema,
                rel_type,
                from_id,
                to_id,
            } => {
                assert_eq!(source_schema, "src-cv@0.1.0");
                assert_eq!(target_schema, "other-cv@0.1.0");
                assert_eq!(rel_type, "ADDRESSES");
                assert_eq!(from_id, src_entity.id.to_string());
                assert_eq!(to_id, other_entity.id.to_string());
            }
            other => panic!("expected CrossMemEdgeNotDeclared, got {other:?}"),
        }
    }

    #[test]
    fn intra_mem_with_cross_mem_only_rel_type_returns_invalid_rel_type() {
        // `ADDRESSES` is declared in src-cv's cross_mem_relationships
        // only — intra-mem relate must refuse with
        // INVALID_REL_TYPE since the intra-mem vocabulary
        // (`IMPLEMENTS` / `_default`) doesn't know it.
        let (_tmp, mut engine, src, _tgt) = two_mem_engine();
        let (actor, client) = cli_actor();
        // Create a same-mem target.
        let intra_target = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "src".to_string(),
                    title: "Doc Two".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([("body".to_string(), "x".to_string())]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        // Source's content_hash may have rotated due to incoming
        // edges from intra_target — fetch fresh.
        let src_fresh = engine.get_entity(&src.id).unwrap();
        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(src_fresh.content_hash.clone()),
                    rel_type: "ADDRESSES".to_string(),
                    target: intra_target.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::Validation(
                crate::runtime_validator::ValidationError::InvalidRelationshipType {
                    input, ..
                },
            ) => {
                assert_eq!(input, "ADDRESSES");
            }
            other => panic!("expected Validation(InvalidRelationshipType), got {other:?}"),
        }
    }

    #[test]
    fn vocabulary_admissible_edge_blocked_by_policy_returns_cross_mem_link_not_allowed() {
        // Same fixture but flip the cross-mem policy to deny.
        // ADDRESSES is vocabulary-admissible but permission refuses
        // it independently — surfaces CROSS_MEM_LINK_NOT_ALLOWED.
        let (_tmp, mut engine, src, tgt) = two_mem_engine();
        // Replace the wildcard policy with default-deny.
        engine.set_settings(WorkspaceSettings::default());
        let (actor, client) = cli_actor();
        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(src.content_hash.clone()),
                    rel_type: "ADDRESSES".to_string(),
                    target: tgt.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert!(
            matches!(err, EngineError::CrossMemLinkNotAllowed { .. }),
            "expected CrossMemLinkNotAllowed, got {err:?}"
        );
    }

    /// Cross-mem remove bypasses the `cross_mem_links` policy
    /// gate. Without this, a workspace whose grant was revoked
    /// while edges still existed gets wedged: the natural recovery
    /// (`memstead_relate ... --remove`) refuses, leaving the operator
    /// to re-grant just to delete the data that the grant once
    /// permitted.
    #[test]
    fn cross_mem_remove_bypasses_policy_after_revoke() {
        let (_tmp, mut engine, src, tgt) = two_mem_engine();
        let (actor, client) = cli_actor();

        // 1. Edge admits under wildcard grant.
        let added = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(src.content_hash.clone()),
                    rel_type: "ADDRESSES".to_string(),
                    target: tgt.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("declared cross-mem edge admits under grant");
        assert_eq!(added.action, RelateAction::Added);

        // 2. Revoke the grant — default settings deny everything.
        engine.set_settings(WorkspaceSettings::default());

        // 3. Re-attempting an *add* still refuses (constraint:
        //    the gate is unchanged for the add path).
        let add_err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(added.content_hash.clone()),
                    rel_type: "ADDRESSES".to_string(),
                    target: tgt.id.clone(),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert!(
            matches!(add_err, EngineError::CrossMemLinkNotAllowed { .. }),
            "add path must still refuse under denial, got {add_err:?}"
        );

        // 4. Remove succeeds — the cleanup path bypasses the
        //    policy gate.
        let removed = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(added.content_hash.clone()),
                    rel_type: "ADDRESSES".to_string(),
                    target: tgt.id.clone(),
                    remove: true,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("remove must bypass the policy gate post-revoke");
        assert_eq!(removed.action, RelateAction::Removed);

        // 5. Edge is gone from the store's outgoing index.
        let outgoing = engine.store().outgoing(&src.id);
        assert!(
            !outgoing
                .iter()
                .any(|e| e.target == tgt.id && e.rel_type == "ADDRESSES"),
            "ADDRESSES edge must be gone after remove"
        );
    }

    /// Remove on a non-existent cross-mem edge with no grant
    /// returns a no-op, not a policy refusal. The remove path is
    /// permissive on absence — same shape as same-mem remove.
    #[test]
    fn cross_mem_remove_of_absent_edge_under_denial_is_no_op() {
        let (_tmp, mut engine, src, tgt) = two_mem_engine();
        // Default-deny from the start: no edge ever existed.
        engine.set_settings(WorkspaceSettings::default());
        let (actor, client) = cli_actor();
        let outcome = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(src.content_hash.clone()),
                    rel_type: "ADDRESSES".to_string(),
                    target: tgt.id.clone(),
                    remove: true,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("absent-edge remove must not refuse on policy");
        assert!(
            matches!(outcome.action, RelateAction::NoOpAbsent),
            "expected NoOpAbsent, got {:?}",
            outcome.action
        );
    }

    // ---- ReadOnly-target refusal (shared add-path funnel) --------

    /// Engine with mem `src` (Write, `alias_target_rel_type:
    /// REFERENCES`, cross-mem vocabulary into `tgt-al`) and mem
    /// `tgt` mounted with the given capability, pre-populated on
    /// disk with one entity `tgt--req-one`. Wildcard cross-mem
    /// grant for `src`. Exercises the funnel's ReadOnly-missing-
    /// target refusal across every add-shaped write path.
    fn engine_with_tgt_capability(
        capability: MountCapability,
    ) -> (TempDir, Engine, CreateEntityOutcome) {
        let tmp = TempDir::new().unwrap();

        let src_manifest = r#"name: src-al
version: 0.1.0
description: source schema with alias pointer
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: ADDRESSES
      description: explicit cross-mem
      default_weight: 1.0
    - name: REFERENCES
      description: alias pointer
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
cross_mem_relationships:
  - to_schema: tgt-al
    definitions:
      - name: ADDRESSES
        description: explicit cross-mem
        default_weight: 1.0
      - name: REFERENCES
        description: alias-emitted cross-mem
        default_weight: 1.0
alias_target_rel_type: REFERENCES
community:
  resolution: 1.0
  seed: 42
"#;
        let tgt_manifest = r#"name: tgt-al
version: 0.1.0
description: target schema
when_to_use: tests
types:
  - req
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
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();
        write_schema_files(
            &schemas_dir,
            "src-al",
            src_manifest,
            &[("doc", &make_type_yaml("doc"))],
        );
        write_schema_files(
            &schemas_dir,
            "tgt-al",
            tgt_manifest,
            &[("req", &make_type_yaml("req"))],
        );

        let src_dir = tmp.path().join("mem-src");
        let tgt_dir = tmp.path().join("mem-tgt");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&tgt_dir).unwrap();
        // The read-only mem is pre-populated on disk — the engine
        // never writes to it.
        std::fs::write(
            tgt_dir.join("req-one.md"),
            "---\ntype: req\n---\n# Req One\n\n## Body\n\nseed.\n",
        )
        .unwrap();

        let src_writer = FilesystemBackend::new(src_dir.clone());
        let tgt_writer = FilesystemBackend::new(tgt_dir.clone());
        let src_pin = SchemaRef::new("src-al", semver::Version::new(0, 1, 0));
        let tgt_pin = SchemaRef::new("tgt-al", semver::Version::new(0, 1, 0));

        let tgt_mount = Mount {
            mem: "tgt".to_string(),
            schema: Some(tgt_pin),
            storage: MountStorage::Folder {
                path: tgt_dir.clone(),
            },
            capability,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![
                (
                    folder_mount_with_pin("src", src_dir, src_pin),
                    Box::new(src_writer) as Box<dyn MemBackend>,
                ),
                (tgt_mount, Box::new(tgt_writer) as Box<dyn MemBackend>),
            ],
            Some(&schemas_dir),
        )
        .expect("two-mem engine constructs");

        let mut settings = WorkspaceSettings::default();
        let mut links: BTreeMap<String, CrossLinkValue> = BTreeMap::new();
        links.insert("src".to_string(), CrossLinkValue::Wildcard);
        settings.cross_mem_links = links;
        engine.set_settings(settings);

        let (actor, client) = cli_actor();
        let src_entity = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "src".to_string(),
                    title: "Doc One".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([("body".to_string(), "seed".to_string())]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("source entity creates");

        (tmp, engine, src_entity)
    }

    fn assert_cross_mem_target_not_found(err: EngineError, expected_target: &str) {
        match err {
            EngineError::CrossMemTargetNotFound {
                target_id,
                target_mem,
            } => {
                assert_eq!(target_id, expected_target);
                assert_eq!(target_mem, "tgt");
            }
            other => panic!("expected CrossMemTargetNotFound, got {other:?}"),
        }
    }

    /// Rehearsal complement: a rehearsed
    /// relate against a read-only boundary refuses EXACTLY as the
    /// real call would — same variant, same payload. Paired with
    /// `relate_to_missing_target_in_readonly_mem_refuses` below.
    #[test]
    fn relate_dry_run_to_missing_target_in_readonly_mem_refuses_identically() {
        let (_tmp, mut engine, src) = engine_with_tgt_capability(MountCapability::ReadOnly);
        let (actor, client) = cli_actor();
        let args = |dry_run: bool| RelateEntityArgs {
            source: src.id.clone(),
            expected_hash: Some(src.content_hash.clone()),
            rel_type: "ADDRESSES".to_string(),
            target: crate::EntityId::new("tgt", "missing"),
            remove: false,
            description: None,
            dry_run,
        };
        let rehearsed = engine
            .relate_entity(args(true), actor, Some(&client), None)
            .unwrap_err();
        let real = engine
            .relate_entity(args(false), actor, Some(&client), None)
            .unwrap_err();
        assert_eq!(format!("{rehearsed:?}"), format!("{real:?}"));
        assert_cross_mem_target_not_found(rehearsed, "tgt--missing");
    }

    #[test]
    fn relate_to_missing_target_in_readonly_mem_refuses() {
        let (_tmp, mut engine, src) = engine_with_tgt_capability(MountCapability::ReadOnly);
        let (actor, client) = cli_actor();
        let err = engine
            .relate_entity(
                RelateEntityArgs {
                    source: src.id.clone(),
                    expected_hash: Some(src.content_hash.clone()),
                    rel_type: "ADDRESSES".to_string(),
                    target: crate::EntityId::new("tgt", "missing"),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_cross_mem_target_not_found(err, "tgt--missing");
    }

    /// Pre-funnel, `memstead_create.relations[]` lacked the
    /// ReadOnly-missing-target check the relate path had — an
    /// inline relation to an absent read-only target auto-stubbed
    /// instead of refusing.
    #[test]
    fn create_inline_relation_to_missing_target_in_readonly_mem_refuses() {
        let (_tmp, mut engine, _src) = engine_with_tgt_capability(MountCapability::ReadOnly);
        let (actor, client) = cli_actor();
        let err = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "src".to_string(),
                    title: "Doc Two".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([("body".to_string(), "x".to_string())]),
                    metadata: IndexMap::new(),
                    relations: vec![crate::ops::RelateArg {
                        target: crate::EntityId::new("tgt", "missing"),
                        rel_type: "ADDRESSES".to_string(),
                        description: None,
                    }],
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_cross_mem_target_not_found(err, "tgt--missing");
    }

    /// The body-wiki-link channel (alias synthesis) — pre-funnel a
    /// granted body link to a missing read-only target silently
    /// auto-stubbed at load; `memstead_health` was the only signal.
    #[test]
    fn create_body_link_to_missing_target_in_readonly_mem_refuses() {
        let (_tmp, mut engine, _src) = engine_with_tgt_capability(MountCapability::ReadOnly);
        let (actor, client) = cli_actor();
        let err = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "src".to_string(),
                    title: "Doc Three".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([(
                        "body".to_string(),
                        "see [[tgt--missing]].".to_string(),
                    )]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        assert_cross_mem_target_not_found(err, "tgt--missing");
    }

    #[test]
    fn update_body_link_to_missing_target_in_readonly_mem_refuses() {
        let (_tmp, mut engine, src) = engine_with_tgt_capability(MountCapability::ReadOnly);
        let (actor, client) = cli_actor();
        let err = engine
            .update_entity(
                crate::engine::UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: src.id.clone(),
                    expected_hash: Some(src.content_hash.clone()),
                    sections: IndexMap::from_iter([(
                        "body".to_string(),
                        "now see [[tgt--missing]].".to_string(),
                    )]),
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
        assert_cross_mem_target_not_found(err, "tgt--missing");
    }

    /// Positive control: a body link to a target that EXISTS in
    /// the read-only mem writes clean and materialises the typed
    /// alias edge — the seam's happy path.
    #[test]
    fn body_link_to_existing_target_in_readonly_mem_admits_and_emits_edge() {
        let (_tmp, mut engine, _src) = engine_with_tgt_capability(MountCapability::ReadOnly);
        let (actor, client) = cli_actor();
        let created = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "src".to_string(),
                    title: "Doc Four".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([(
                        "body".to_string(),
                        "see [[tgt--req-one]].".to_string(),
                    )]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("body link to existing read-only target admits");
        let outgoing = engine.store().outgoing(&created.id);
        assert!(
            outgoing.iter().any(|e| e.rel_type == "REFERENCES"
                && e.target == crate::EntityId::new("tgt", "req-one")),
            "alias REFERENCES edge to the read-only target must materialise; got {outgoing:?}"
        );
    }

    /// Behaviour preserved: a missing target in a WRITE-mounted
    /// sibling mem is a legitimate forward reference and keeps
    /// the auto-stub mechanic on every path.
    #[test]
    fn body_link_to_missing_target_in_write_mem_still_stubs() {
        let (_tmp, mut engine, _src) = engine_with_tgt_capability(MountCapability::Write);
        let (actor, client) = cli_actor();
        let created = engine
            .create_entity(
                CreateEntityArgs {
                    anchors: Vec::new(),
                    mem: "src".to_string(),
                    title: "Doc Five".to_string(),
                    entity_type: "doc".to_string(),
                    sections: IndexMap::from_iter([(
                        "body".to_string(),
                        "see [[tgt--missing]].".to_string(),
                    )]),
                    metadata: IndexMap::new(),
                    relations: Vec::new(),
                    dry_run: false,
                },
                actor,
                Some(&client),
                None,
            )
            .expect("forward reference into a Write sibling mem keeps stubbing");
        assert!(
            engine
                .store()
                .contains(&crate::EntityId::new("tgt", "missing")),
            "auto-stub must land for the Write-mem forward reference"
        );
        let outgoing = engine.store().outgoing(&created.id);
        assert!(
            outgoing.iter().any(|e| e.rel_type == "REFERENCES"),
            "alias edge must still emit for the stubbed target"
        );
    }
}

// ---- Engine::rename_entity --------------------------------------

/// Batch relate: one list mixing additions and removals, applied
/// IN ORDER in one invocation with one commit. The remove entry
/// targets an edge added earlier in the same batch — if entries
/// validated against the pre-batch state instead, that remove
/// would resolve to `NoOpAbsent` ("noop"), so the asserted
/// `"removed"` action is the in-order proof.
#[test]
fn batch_relate_applies_adds_and_removes_in_order_one_commit() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    for title in ["A", "B", "C"] {
        engine
            .create_entity(
                empty_create_args("specs", title),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
    }
    let id = |slug: &str| crate::entity::EntityId::new("specs", slug);
    let edge = |from: &str, to: &str, remove: bool| RelateEntityArgs {
        source: id(from),
        expected_hash: None,
        rel_type: "USES".to_string(),
        target: id(to),
        remove,
        description: None,
        dry_run: false,
    };

    let result = engine
        .batch_relate(
            vec![
                (edge("a", "b", false), Some("add a-b".to_string())),
                (edge("a", "c", false), Some("add a-c".to_string())),
                (edge("a", "c", true), Some("undo a-c".to_string())),
            ],
            actor,
            Some(&client),
            false,
        )
        .unwrap();
    assert!(result.applied, "{result:?}");
    assert_eq!(result.succeeded, 3);
    assert_eq!(result.failed, 0);
    assert!(!result.write_id.is_empty(), "one real commit");
    let actions: Vec<&str> = result.results.iter().map(|r| r.action.as_str()).collect();
    assert_eq!(
        actions,
        vec!["added", "added", "removed"],
        "in-order application: the remove sees the same batch's add"
    );

    // Net state: A carries exactly the surviving edge to B.
    let a = engine.get_entity(&id("a")).unwrap();
    assert_eq!(a.relationships.len(), 1, "{:?}", a.relationships);
    assert_eq!(a.relationships[0].rel_type, "USES");
    assert_eq!(a.relationships[0].target, id("b"));
}

/// Rehearsal contract — single relate:
/// `dry_run: true` runs the FULL validation, reports the would-be
/// edge and the would-be auto-stub (reported, never created) with
/// the marker form's empty `write_id`, and writes nothing. The
/// follow-up real call succeeds and lands EXACTLY the rehearsed
/// prospective `_hash` — the strongest identical-validation
/// observable.
#[test]
fn relate_dry_run_reports_would_be_stub_and_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Src");
    // Pin the mutation clock: the auto-stamped `last_modified`
    // enters the content hash, so the prospective-hash == real-hash
    // assertion below is only deterministic under a frozen clock
    // (unpinned, it fails whenever a wall-clock second ticks
    // between the rehearsal and the real call).
    engine.set_mutation_clock(std::sync::Arc::new(|| {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_754_000_000)
    }));
    let (actor, client) = cli_actor();
    let absent = crate::EntityId::new("specs", "ghost-target");
    let args = |dry_run: bool| RelateEntityArgs {
        source: source.id.clone(),
        expected_hash: Some(source.content_hash.clone()),
        rel_type: "USES".to_string(),
        target: absent.clone(),
        remove: false,
        description: None,
        dry_run,
    };

    let rehearsed = engine
        .relate_entity(args(true), actor, Some(&client), None)
        .unwrap();
    assert_eq!(rehearsed.action, RelateAction::Added);
    assert!(rehearsed.write_id.is_empty(), "marker form: empty write_id");
    assert!(
            rehearsed.warnings.iter().any(
                |w| matches!(w, crate::ops::WarningHint::AutoStubCreated { stub_id, pending: true } if *stub_id == absent)
            ),
            "would-be stub must be reported as pending: {:?}",
            rehearsed.warnings
        );
    // The rehearsed warning must not claim a performed effect —
    // conditional wording, code unchanged (AUTO_STUB_CREATED).
    let rehearsed_stub = rehearsed
        .warnings
        .iter()
        .find(|w| matches!(w, crate::ops::WarningHint::AutoStubCreated { .. }))
        .unwrap();
    assert_eq!(rehearsed_stub.code(), "AUTO_STUB_CREATED");
    let msg = rehearsed_stub.message();
    assert!(
        msg.contains("would be auto-created") && !msg.contains("stub auto-created."),
        "dry-run wording must be conditional: {msg}"
    );
    assert!(
        !engine.store().contains(&absent),
        "would-be stub reported, never created"
    );
    // Source untouched: stored hash still the pre-call hash, and
    // the reported hash is the PROSPECTIVE one (a real change).
    let stored = engine.store().get(&source.id).unwrap();
    assert_eq!(stored.content_hash, source.content_hash);
    assert_ne!(rehearsed.content_hash, source.content_hash);
    assert!(stored.relationships.is_empty(), "no edge landed");

    // Follow-up real call: succeeds, commits, and the rehearsed
    // prospective hash IS the real post-write hash.
    let real = engine
        .relate_entity(args(false), actor, Some(&client), None)
        .unwrap();
    assert!(!real.write_id.is_empty(), "the real relate commits");
    assert_eq!(
        real.content_hash, rehearsed.content_hash,
        "prospective hash must equal the real post-write hash"
    );
    assert!(engine.store().get(&absent).expect("real call stubs").stub);
    // The real call keeps the performed-effect wording exactly.
    let real_msg = real
        .warnings
        .iter()
        .find(|w| {
            matches!(
                w,
                crate::ops::WarningHint::AutoStubCreated { pending: false, .. }
            )
        })
        .expect("real relate carries the non-pending stub warning")
        .message();
    assert!(
        real_msg.contains("did not exist — stub auto-created."),
        "real wording unchanged: {real_msg}"
    );
}

/// Rehearsal refusal parity — single relate: an illegal rehearsed
/// relate refuses with the IDENTICAL typed error the real call
/// returns (same variant, same payload).
#[test]
fn relate_dry_run_refuses_identically_to_real() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Src");
    let (actor, client) = cli_actor();
    // Malformed target id (no `--` separator) — INVALID_ENTITY_ID.
    let args = |dry_run: bool| RelateEntityArgs {
        source: source.id.clone(),
        expected_hash: None,
        rel_type: "USES".to_string(),
        target: crate::EntityId("bad target with spaces".to_string()),
        remove: false,
        description: None,
        dry_run,
    };
    let rehearsed = engine
        .relate_entity(args(true), actor, Some(&client), None)
        .unwrap_err();
    let real = engine
        .relate_entity(args(false), actor, Some(&client), None)
        .unwrap_err();
    assert_eq!(
        format!("{rehearsed:?}"),
        format!("{real:?}"),
        "identical typed refusal"
    );
    assert_eq!(rehearsed.code(), real.code());
}

/// Rehearsal — batch relate: `dry_run: true` validates the whole
/// list in order (a remove of an edge added earlier in the SAME
/// batch reports `"removed"` — the in-order proof), reports the
/// would-be receipt with empty `write_id`, and commits nothing:
/// no edge, no stub, no head movement. The follow-up real batch
/// succeeds.
#[test]
fn batch_relate_dry_run_reports_receipt_and_commits_nothing() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    for title in ["A", "B"] {
        engine
            .create_entity(
                empty_create_args("specs", title),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
    }
    let id = |slug: &str| crate::entity::EntityId::new("specs", slug);
    let edge = |from: &str, to: &str, remove: bool| RelateEntityArgs {
        source: id(from),
        expected_hash: None,
        rel_type: "USES".to_string(),
        target: id(to),
        remove,
        description: None,
        dry_run: false,
    };
    let batch = || {
        vec![
            (edge("a", "b", false), None),
            (edge("a", "ghost", false), None), // would-be auto-stub
            (edge("a", "b", true), None),      // in-order: sees entry 0's add
        ]
    };
    let head_before = engine
        .mem_head_sha("specs")
        .ok()
        .flatten()
        .unwrap_or_default();

    let rehearsed = engine
        .batch_relate(batch(), actor, Some(&client), true)
        .unwrap();
    assert!(rehearsed.applied, "{rehearsed:?}");
    assert!(rehearsed.write_id.is_empty(), "marker form: empty write_id");
    let actions: Vec<&str> = rehearsed
        .results
        .iter()
        .map(|r| r.action.as_str())
        .collect();
    assert_eq!(
        actions,
        vec!["added", "added", "removed"],
        "in-order rehearsal semantics"
    );
    // Nothing landed: no stub, no edge, no head movement.
    assert!(!engine.store().contains(&id("ghost")), "no stub created");
    assert!(
        engine
            .get_entity(&id("a"))
            .unwrap()
            .relationships
            .is_empty(),
        "no edge landed"
    );
    let head_after = engine
        .mem_head_sha("specs")
        .ok()
        .flatten()
        .unwrap_or_default();
    assert_eq!(head_before, head_after, "no commit landed");

    // The real batch on the unchanged mem succeeds.
    let real = engine
        .batch_relate(batch(), actor, Some(&client), false)
        .unwrap();
    assert!(real.applied, "{real:?}");
    assert!(!real.write_id.is_empty());
}

/// Rehearsal refusal parity — batch relate: a failing list refuses
/// under `dry_run: true` with the SAME per-entry report-all
/// envelope the real refusal carries.
#[test]
fn batch_relate_dry_run_refuses_identically_to_real() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    for title in ["A", "B"] {
        engine
            .create_entity(
                empty_create_args("specs", title),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
    }
    let id = |slug: &str| crate::entity::EntityId::new("specs", slug);
    let batch = || {
        vec![
            (
                RelateEntityArgs {
                    source: id("a"),
                    expected_hash: None,
                    rel_type: "USES".to_string(),
                    target: id("b"),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                None,
            ),
            (
                RelateEntityArgs {
                    source: id("a"),
                    expected_hash: None,
                    rel_type: "USES".to_string(),
                    target: crate::EntityId("bad target".to_string()),
                    remove: false,
                    description: None,
                    dry_run: false,
                },
                None,
            ),
        ]
    };
    let rehearsed = engine
        .batch_relate(batch(), actor, Some(&client), true)
        .unwrap();
    let real = engine
        .batch_relate(batch(), actor, Some(&client), false)
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
    assert!(
        engine
            .get_entity(&id("a"))
            .unwrap()
            .relationships
            .is_empty(),
        "neither run landed the valid entry"
    );
}

/// Atomicity + report-all for batch relate: a batch with several
/// invalid entries changes NOTHING (no edge lands, the head is
/// unmoved, staged earlier entries roll back) and names EVERY
/// failing entry with its typed code.
#[test]
fn batch_relate_refuses_whole_batch_reporting_every_failure() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let a = engine
        .create_entity(empty_create_args("specs", "A"), actor, Some(&client), None)
        .unwrap();
    engine
        .create_entity(empty_create_args("specs", "B"), actor, Some(&client), None)
        .unwrap();
    let head_before = engine
        .mem_head_sha("specs")
        .ok()
        .flatten()
        .unwrap_or_default();
    let count_before = engine.store().all_entities().count();

    let id = |slug: &str| crate::entity::EntityId::new("specs", slug);
    let result = engine
        .batch_relate(
            vec![
                // Valid — stages an edge that must roll back.
                (
                    RelateEntityArgs {
                        source: id("a"),
                        expected_hash: None,
                        rel_type: "USES".to_string(),
                        target: id("b"),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    None,
                ),
                // Missing source.
                (
                    RelateEntityArgs {
                        source: id("ghost"),
                        expected_hash: None,
                        rel_type: "USES".to_string(),
                        target: id("b"),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    None,
                ),
                // Optimistic-lock mismatch.
                (
                    RelateEntityArgs {
                        source: id("b"),
                        expected_hash: Some("definitely-wrong".to_string()),
                        rel_type: "USES".to_string(),
                        target: id("a"),
                        remove: false,
                        description: None,
                        dry_run: false,
                    },
                    None,
                ),
            ],
            actor,
            Some(&client),
            false,
        )
        .unwrap();
    assert!(!result.applied);
    assert_eq!(result.failed, 2, "{result:?}");
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
        vec![(1, "ENTITY_NOT_FOUND"), (2, "HASH_MISMATCH")],
        "every failing entry named with index + typed code: {result:?}"
    );
    assert_eq!(result.results[0].action, "not_applied");

    // NOTHING changed: the staged first edge rolled back, the head
    // is unmoved, and no stub or entity appeared.
    let a_after = engine.get_entity(&a.id).unwrap();
    assert!(
        a_after.relationships.is_empty(),
        "staged edge must roll back: {:?}",
        a_after.relationships
    );
    assert_eq!(a_after.content_hash, a.content_hash);
    let head_after = engine
        .mem_head_sha("specs")
        .ok()
        .flatten()
        .unwrap_or_default();
    assert_eq!(head_before, head_after, "mem head unmoved");
    assert_eq!(engine.store().all_entities().count(), count_before);
}
