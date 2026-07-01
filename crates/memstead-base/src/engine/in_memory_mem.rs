//! Engine-level lifecycle coverage for the in-memory backend
//! ([`crate::storage::InMemoryBackend`]).
//!
//! These tests boot an [`Engine`] from a single
//! [`MountStorage::InMemory`](crate::workspace::MountStorage::InMemory)
//! mount and drive the full mutation surface — create → read → update →
//! relate → rename → delete — entirely through the public engine API.
//! No `TempDir`, no path, no git: the absence of any filesystem fixture
//! in these tests *is* the proof that a full lifecycle round-trips with
//! nothing on disk to clean up. Backend-internal facts (the synthetic
//! commit-id shape, `current_head` reporting `None`, config/provenance
//! round-trip) are covered directly in
//! [`crate::storage::in_memory`]'s own unit tests.

use indexmap::IndexMap;

use super::test_helpers::*;
use crate::backend::MemBackend;
use crate::engine::{
    CreateEntityArgs, DeleteEntityArgs, Engine, EngineError, RelateEntityArgs, RenameEntityArgs,
    UpdateEntityArgs,
};
use crate::entity::EntityId;
use crate::storage::InMemoryBackend;
use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

/// Build a section map seeding the two `spec`-required sections
/// (`identity`, `purpose`), with `purpose` carrying `body`.
fn spec_sections(identity: &str, purpose: &str) -> IndexMap<String, String> {
    let mut s = IndexMap::new();
    s.insert("identity".to_string(), identity.to_string());
    s.insert("purpose".to_string(), purpose.to_string());
    s
}

fn spec_create(mem: &str, title: &str, sections: IndexMap<String, String>) -> CreateEntityArgs {
    CreateEntityArgs {
        mem: mem.to_string(),
        title: title.to_string(),
        entity_type: "spec".to_string(),
        sections,
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    }
}

fn empty_update(id: EntityId, expected_hash: Option<String>) -> UpdateEntityArgs {
    UpdateEntityArgs {
        id,
        expected_hash,
        sections: IndexMap::new(),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
    }
}

/// AC1 + AC2: one engine, one in-memory mount, the full entity
/// lifecycle round-trips entirely in RAM with the same engine-level
/// outcomes the folder backend produces — created entity reads back, an
/// update bumps the hash, a relate adds an edge, a rename rewrites
/// referrers, a delete removes the entity. No filesystem fixture is
/// constructed anywhere in this test.
#[test]
fn full_lifecycle_round_trips_in_memory() {
    let mut engine = Engine::from_mounts(vec![(
        in_memory_mount("specs"),
        Box::new(InMemoryBackend::new()) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    // Mem boots empty — nothing was provisioned.
    assert!(engine.get_entity(&EntityId::new("specs", "target-spec")).is_none());

    // --- create: target, then a referrer whose body wiki-links it ---
    let target = engine
        .create_entity(
            spec_create("specs", "Target Spec", spec_sections("target identity", "target purpose")),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(target.id.to_string(), "specs--target-spec");

    let referrer = engine
        .create_entity(
            spec_create(
                "specs",
                "Referrer Spec",
                // Body wiki-link auto-emits a REFERENCES edge via the
                // alias-synthesis pass — the same contract the folder
                // backend honours.
                spec_sections("referrer identity", "relies on [[target-spec]] for context"),
            ),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // --- read: the created entity reads back from the in-RAM store ---
    let read_back = engine.get_entity(&referrer.id).expect("referrer reads back");
    assert_eq!(read_back.title, "Referrer Spec");
    assert_eq!(read_back.content_hash, referrer.content_hash);
    assert!(
        read_back
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
        "body wiki-link must have produced a REFERENCES edge, got {:?}",
        read_back.relationships
    );

    // --- update: replacing a section bumps the content hash ---
    let mut update = empty_update(referrer.id.clone(), Some(referrer.content_hash.clone()));
    update.sections.insert("identity".to_string(), "revised identity body".to_string());
    let updated = engine.update_entity(update, actor, Some(&client), None).unwrap();
    assert_ne!(
        updated.content_hash, referrer.content_hash,
        "an update must bump the content hash"
    );

    // --- optimistic-lock refusal (AC1 complement): the now-stale
    //     pre-update hash trips HASH_MISMATCH, exactly as on the folder
    //     backend — the guard is enforced above the backend, so the
    //     in-memory backend can neither skip nor weaken it. ---
    let stale = empty_update(referrer.id.clone(), Some(referrer.content_hash.clone()));
    let err = engine.update_entity(stale, actor, Some(&client), None).unwrap_err();
    assert!(
        matches!(err, EngineError::HashMismatch { .. }),
        "stale expected-hash must refuse with HASH_MISMATCH, got {err:?}"
    );

    // --- relate: an explicit edge is added ---
    let relate = engine
        .relate_entity(
            RelateEntityArgs {
                source: referrer.id.clone(),
                expected_hash: Some(updated.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: target.id.clone(),
                remove: false,
                description: None,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let after_relate = engine.get_entity(&referrer.id).unwrap();
    assert!(
        after_relate
            .relationships
            .iter()
            .any(|r| r.rel_type == "USES" && r.target == target.id),
        "relate must add the USES edge, got {:?}",
        after_relate.relationships
    );

    // --- rename: referrers are rewritten to the new slug ---
    let renamed = engine
        .rename_entity(
            RenameEntityArgs {
                id: target.id.clone(),
                expected_hash: Some(target.content_hash.clone()),
                new_title: "Renamed Spec".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let renamed_target = EntityId::new("specs", "renamed-spec");
    assert_eq!(renamed.new_id, renamed_target);
    assert!(engine.get_entity(&target.id).is_none(), "old id must be gone after rename");

    let after_rename = engine.get_entity(&referrer.id).unwrap();
    // Both the auto-emitted REFERENCES edge and the explicit USES edge
    // now point at the new id.
    assert!(
        after_rename
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == renamed_target),
        "REFERENCES edge must be rewritten to renamed-spec, got {:?}",
        after_rename.relationships
    );
    assert!(
        after_rename
            .relationships
            .iter()
            .any(|r| r.rel_type == "USES" && r.target == renamed_target),
        "USES edge must be rewritten to renamed-spec, got {:?}",
        after_rename.relationships
    );
    // Body wiki-link rewritten too.
    assert!(
        after_rename
            .sections
            .get("purpose")
            .map(|s| s.contains("[[renamed-spec]]") && !s.contains("[[target-spec]]"))
            .unwrap_or(false),
        "referrer body wiki-link must be rewritten to the new slug, got {:?}",
        after_rename.sections.get("purpose")
    );

    // --- delete: removes the entity (referrer first so the renamed
    //     target loses its last incoming ref, then the target). ---
    engine
        .delete_entity(
            DeleteEntityArgs { id: referrer.id.clone(), expected_hash: None },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(engine.get_entity(&referrer.id).is_none(), "delete must remove the referrer");

    engine
        .delete_entity(
            DeleteEntityArgs { id: renamed_target.clone(), expected_hash: None },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(engine.get_entity(&renamed_target).is_none(), "delete must remove the target");

    // Edge actually committed (a real relate, not a no-op).
    assert!(!relate.commit_sha.is_empty(), "relate must produce a commit id");
}

/// An in-memory mem exports to a self-describing `.mem` archive that
/// mounts standalone in a fresh engine — with no filesystem, no network,
/// and no other inputs — yielding the same entities and relationships the
/// agent built. This is the funnel-exit guarantee: the visitor can take
/// the graph home.
#[test]
fn in_memory_mem_exports_to_mem_archive_that_mounts_standalone() {
    // A session-style in-memory mem is self-describing: a config (with
    // a version, which export requires) is written to the backend before
    // boot, so `mem_config_for` resolves it and export can project it.
    let backend = InMemoryBackend::new();
    backend
        .write_mem_config(br#"{"version":"0.1.0","schema":"default@1.0.0"}"#)
        .expect("in-memory backend accepts a config write");
    let mount = Mount {
        mem: "playground".to_string(),
        schema: Some(pin("default")),
        storage: MountStorage::InMemory,
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut engine =
        Engine::from_mounts(vec![(mount, Box::new(backend) as Box<dyn MemBackend>)]).unwrap();
    let (actor, client) = cli_actor();

    // Build a small graph: a target, plus a referrer that both
    // body-wiki-links it (auto REFERENCES) and explicitly USES it.
    let target = engine
        .create_entity(
            spec_create("playground", "Target Spec", spec_sections("target id", "target purpose")),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let referrer = engine
        .create_entity(
            spec_create(
                "playground",
                "Referrer Spec",
                spec_sections("referrer id", "relies on [[target-spec]] for context"),
            ),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    engine
        .relate_entity(
            RelateEntityArgs {
                source: referrer.id.clone(),
                expected_hash: Some(referrer.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: target.id.clone(),
                remove: false,
                description: None,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Seal the live mem to `.mem` bytes, then mount those bytes
    // standalone in a brand-new engine — no shared state with the source.
    let bytes = engine.export_mem_to_bytes("playground").expect("in-memory export succeeds");
    let standalone = Engine::from_archive_bytes(bytes).expect("exported .mem mounts standalone");

    // The standalone engine yields the same graph the agent built.
    let target_id = EntityId::new("playground", "target-spec");
    let referrer_id = EntityId::new("playground", "referrer-spec");
    assert!(standalone.get_entity(&target_id).is_some(), "target survives the round-trip");
    let r = standalone.get_entity(&referrer_id).expect("referrer survives the round-trip");
    assert!(
        r.relationships
            .iter()
            .any(|rel| rel.rel_type == "REFERENCES" && rel.target == target_id),
        "the body-wiki-link REFERENCES edge round-trips: {:?}",
        r.relationships
    );
    assert!(
        r.relationships
            .iter()
            .any(|rel| rel.rel_type == "USES" && rel.target == target_id),
        "the explicit USES edge round-trips: {:?}",
        r.relationships
    );
}
