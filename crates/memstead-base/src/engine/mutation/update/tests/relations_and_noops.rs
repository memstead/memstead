//! Declared relations and alias synthesis on update, the dry-run
//! preview, the full CRUD round trip of REFERENCES edges, the outcome
//! shape, and the no-op family.

use super::*;

#[test]
fn update_entity_pointer_schema_auto_synthesises_references_from_body_link() {
    // Under the default schema's `alias_target_rel_type: REFERENCES`
    // pointer, a body wiki-link no longer trips the strict validator
    // — the alias-synthesis pass emits the REFERENCES relation
    // first, the validator finds the link backed, the body lands.
    // (Schemas without the pointer continue to refuse with
    // `WIKILINK_WITHOUT_RELATION`; that path is covered by the
    // dedicated no-pointer fixture test elsewhere.)
    use crate::EntityId;
    use crate::engine::UpdateEntityArgs;
    use indexmap::IndexMap;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();

    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let source = engine
        .create_entity(
            empty_create_args("specs", "Source"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert(
        "purpose".to_string(),
        "see [[target]] for context".to_string(),
    );
    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
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
        .expect("auto-synthesis must satisfy the alias-existence invariant");
    // Body landed.
    assert!(
        outcome
            .modified_sections
            .replaced
            .iter()
            .any(|s| s == "purpose"),
    );
    let in_mem = engine.get_entity(&source.id).unwrap();
    assert_eq!(
        in_mem
            .sections
            .get("purpose")
            .map(String::as_str)
            .unwrap_or(""),
        "see [[target]] for context",
    );
    // REFERENCES relation synthesised from the body wiki-link.
    assert!(
        in_mem
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
        "synthesis must emit REFERENCES → target; relationships: {:?}",
        in_mem.relationships,
    );
    // Defeat unused-import warnings for the helper imports.
    let _ = EntityId::new("specs", "x");
}

#[test]
fn update_entity_declare_relations_passes_strict_validator_in_one_call() {
    // The agent declares the relation + adds the body wiki-link
    // in a single `memstead_update` call. Without
    // `declare_relations`, the strict validator would refuse
    // (no backing relation yet); with the batched declaration,
    // the relation lands *before* the strict validator runs so
    // the body link passes the gate.
    use crate::engine::UpdateEntityArgs;
    use crate::ops::RelateArg;
    use indexmap::IndexMap;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();

    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let source = engine
        .create_entity(
            empty_create_args("specs", "Source"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Atomic declare + body update. USES (not REFERENCES) — under
    // the default schema's `alias_target_rel_type: REFERENCES`
    // pointer, explicit declare_relations type=REFERENCES is
    // refused; the body wiki-link is auto-emitted via synthesis.
    // The test's intent — that declare_relations atomically lands
    // alongside body changes — holds for any rel-type that admits
    // explicit authoring.
    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert(
        "purpose".to_string(),
        "see [[target]] for context".to_string(),
    );
    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                sections,
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                dry_run: false,
                declare_relations: vec![RelateArg {
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    description: None,
                }],
            },
            actor,
            Some(&client),
            None,
        )
        .expect("declare_relations + body update must succeed in one call");

    assert_eq!(outcome.relations_declared.len(), 1);
    assert_eq!(outcome.relations_declared[0].rel_type, "USES");
    assert_eq!(outcome.relations_declared[0].target, target.id);
    assert!(
        !outcome.relations_declared[0].target_was_stubbed,
        "target was already present in store; target_was_stubbed must be false"
    );

    let in_mem = engine.get_entity(&source.id).unwrap();
    assert!(
        in_mem.relationships.iter().any(|r| r.target == target.id),
        "declared relation must land in entity.relationships; got {:?}",
        in_mem.relationships
    );
}

#[test]
fn update_entity_declare_relations_auto_stubs_absent_target() {
    // When the declared target doesn't exist yet, the engine
    // auto-stubs it (same mechanic as `memstead_relate`) and flags
    // `target_was_stubbed: true` in the outcome.
    use crate::EntityId;
    use crate::engine::UpdateEntityArgs;
    use crate::ops::RelateArg;
    use indexmap::IndexMap;

    let tmp = TempDir::new().unwrap();
    let (mut engine, source) = engine_with_seed(&tmp, "Source");
    let (actor, client) = cli_actor();
    let absent_target = EntityId::new("specs", "not-yet-existing");
    assert!(!engine.store().contains(&absent_target));

    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                dry_run: false,
                declare_relations: vec![RelateArg {
                    rel_type: "USES".to_string(),
                    target: absent_target.clone(),
                    description: None,
                }],
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    assert_eq!(outcome.relations_declared.len(), 1);
    assert!(
        outcome.relations_declared[0].target_was_stubbed,
        "absent target must be auto-stubbed; got target_was_stubbed=false"
    );
    // Stub now exists in the store.
    assert!(engine.store().contains(&absent_target));
    let stub = engine.get_entity(&absent_target).unwrap();
    assert!(stub.stub);
}

#[test]
fn update_entity_alias_synthesis_runs_unconditionally_for_pointer_schemas() {
    // Under the alias model with a pointer-set schema (default
    // schema's `alias_target_rel_type: REFERENCES`), a fresh
    // workspace's first body-wiki-link write triggers the
    // alias-synthesis pass and the mutation lands with the
    // REFERENCES relation auto-emitted.
    use crate::engine::UpdateEntityArgs;
    use indexmap::IndexMap;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(mem_dir.clone());
    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let source = engine
        .create_entity(
            empty_create_args("specs", "Source"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let mut sections: IndexMap<String, String> = IndexMap::new();
    sections.insert(
        "purpose".to_string(),
        "see [[target]] for context".to_string(),
    );
    engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
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
        .expect("synthesis must back the wiki-link and let the body land");
    let in_mem = engine.get_entity(&source.id).unwrap();
    assert!(
        in_mem
            .relationships
            .iter()
            .any(|r| r.rel_type == "REFERENCES" && r.target == target.id),
        "synthesis must emit REFERENCES → target; relationships: {:?}",
        in_mem.relationships,
    );
}

#[test]
fn update_entity_dry_run_returns_prospective_hash_without_writing() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Preview Subject");
    let (actor, client) = cli_actor();
    let original_hash = seeded.content_hash.clone();

    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "preview body".to_string());

    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                // Stale-hash recovery path — dry_run skips the
                // hash check, so a wrong expected_hash is OK.
                expected_hash: Some("wrong-hash".to_string()),
                sections,
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: Vec::new(),
                dry_run: true,
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Wire shape: content_hash = current; prospective_hash =
    // what the write would produce; write_id empty.
    assert_eq!(outcome.content_hash, original_hash);
    let prospective = outcome
        .prospective_hash
        .expect("prospective_hash populated on dry_run");
    assert_ne!(prospective, original_hash);
    assert!(outcome.write_id.is_empty());
    // Store entity unchanged.
    let store_entity = engine.get_entity(&seeded.id).unwrap();
    assert_eq!(store_entity.content_hash, original_hash);
}

/// Edge-count round-trip lock. Captures the REFERENCES-drift
/// finding:
/// a mutation cycle (create body wiki-links → relate → update
/// body → rename → delete) must return both `total_edges` and the
/// REFERENCES counter to the pre-cycle values exactly. The bug
/// was in `push_entities_into_store`: `upsert` preserved the
/// entity's pre-existing out-edges, so `add_edge` (idempotent on
/// `(from, to, rel_type)`) couldn't remove edges that the new
/// parse no longer emits. Dropping a wiki-link from a body or
/// absorbing one into an explicit relationship leaked the stale
/// REFERENCES edge.
///
/// Under the alias model the leak is structurally impossible —
/// body wiki-links no longer emit edges, so the cleanup-on-reparse
/// path the original test exercised has no premise. The test
/// keeps the CRUD cycle but routes edges through atomic
/// `relations:` declarations and explicit `memstead_relate`, which is
/// what the model now treats as the only edge source.
#[test]
fn references_edges_round_trip_across_full_crud_cycle() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    // Seed two link targets so the wiki-links inside the
    // probe entity's body resolve to real entities (not auto-
    // stubs we'd then have to GC).
    let foo = engine
        .create_entity(
            empty_create_args("specs", "Foo"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let bar = engine
        .create_entity(
            empty_create_args("specs", "Bar"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let count_references = |engine: &Engine| -> usize {
        engine
            .store()
            .all_ids()
            .flat_map(|id| engine.store().outgoing(id))
            .filter(|e| e.rel_type == "REFERENCES")
            .count()
    };

    let baseline_edges = engine.store().edge_count();
    let baseline_refs = count_references(&engine);

    // Step 1: create entity with body wiki-links — the
    // alias-synthesis pass auto-emits one REFERENCES per body
    // wiki-link (default schema's `alias_target_rel_type` →
    // REFERENCES), so the explicit `relations:` slot stays
    // empty. Net: 2 REFERENCES.
    let mut sections = IndexMap::new();
    sections.insert(
        "identity".to_string(),
        "See [[foo]] and [[bar]] inline.".to_string(),
    );
    sections.insert("purpose".to_string(), "probe purpose".to_string());
    let probe = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "Probe".to_string(),
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
    assert_eq!(count_references(&engine), baseline_refs + 2);

    // Step 2: relate INFORMED_BY → foo as a second relation to the
    // same target. The body wiki-link `[[foo]]` aliases the set of
    // relations to foo, so adding INFORMED_BY does not affect the
    // REFERENCES count — both relations coexist.
    let relate1 = engine
        .relate_entity(
            RelateEntityArgs {
                source: probe.id.clone(),
                expected_hash: Some(probe.content_hash.clone()),
                rel_type: "INFORMED_BY".to_string(),
                target: foo.id.clone(),
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
        count_references(&engine),
        baseline_refs + 2,
        "set-membership aliasing — adding INFORMED_BY does not \
             absorb the REFERENCES relation"
    );

    // Step 3: drop the [[bar]] body link. The alias-synthesis pass
    // GCs the synthesised REFERENCES → bar atomically with the
    // body update — no second `memstead_relate --remove` needed.
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "See [[foo]] inline.".to_string());
    let updated = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: probe.id.clone(),
                expected_hash: Some(relate1.content_hash.clone()),
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
        .unwrap();
    assert_eq!(
        count_references(&engine),
        baseline_refs + 1,
        "REFERENCES → bar must be auto-GC'd when its body link drops"
    );

    // Step 4: rename the entity. Edges follow via remove + push.
    let renamed = engine
        .rename_entity(
            crate::engine::RenameEntityArgs {
                id: probe.id.clone(),
                expected_hash: Some(updated.content_hash.clone()),
                new_title: "Probe Renamed".to_string(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert_eq!(count_references(&engine), baseline_refs + 1);

    // Step 5: delete the renamed entity. The INFORMED_BY → foo
    // edge cascades; REFERENCES count unchanged.
    engine
        .delete_entity(
            crate::engine::DeleteEntityArgs {
                id: renamed.new_id.clone(),
                expected_hash: Some(renamed.content_hash.clone()),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Final assertion: every counter back to baseline.
    assert_eq!(
        engine.store().edge_count(),
        baseline_edges,
        "total edges must round-trip to baseline"
    );
    assert_eq!(
        count_references(&engine),
        baseline_refs,
        "REFERENCES counter must round-trip to baseline"
    );

    // Cross-check: a full reload of the mem produces the same
    // post-cycle counts. If the in-memory store and the on-disk
    // bytes drift, reload uncovers it.
    engine.reload_one_mem("specs").unwrap();
    assert_eq!(
        engine.store().edge_count(),
        baseline_edges,
        "total edges must match disk after reload"
    );
    assert_eq!(
        count_references(&engine),
        baseline_refs,
        "REFERENCES must match disk after reload"
    );
    // Sanity: foo + bar still in the store (they were not deleted).
    assert!(engine.store().contains(&foo.id));
    assert!(engine.store().contains(&bar.id));
}

#[test]
fn update_entity_returns_write_id_title_modified_date_warnings_shape() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Subject");
    let (actor, client) = cli_actor();

    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "edited body".to_string());

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
            None,
        )
        .unwrap();

    // Folder backend produces a synthetic CommitId.
    assert!(
        !outcome.write_id.is_empty(),
        "write_id must be populated on a real update"
    );
    // Title echoed from the parsed entity post-write.
    assert_eq!(outcome.title, "Subject");
    // The default `spec` schema declares `modified_date` with
    // `auto_timestamp: true`; the unified update path
    // auto-stamps it. Asserting non-empty pins the
    // wire-shape parity with full's UpdateResult.modified_date.
    assert!(
        !outcome.modified_date.is_empty(),
        "modified_date must be auto-stamped on update for the default spec schema",
    );
    // V1: warnings always empty (typed warning surfaces are
    // separate session work). The vec is present on the outcome
    // so the wire shape parity with full's UpdateResult holds.
    assert!(outcome.warnings.is_empty());
    // Section was modified (existing behaviour, sanity check).
    assert_eq!(
        outcome.modified_sections.replaced,
        vec!["identity".to_string()]
    );
}

// ---- Engine::update_entity no-op detection ---------------------

/// Re-setting a
/// section to its current on-disk value short-circuits to
/// `UPDATE_NOOP` and preserves `last_modified` at its pre-call
/// value. Pre-fix the auto-timestamp stamped `last_modified` to
/// `today_iso()` before the bytes-compare ran; the stamp
/// synthesised a delta and the no-op never matched.
#[test]
fn update_entity_noop_resetting_section_to_current_value_preserves_last_modified() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Section Resetter");
    let (actor, client) = cli_actor();

    // Read the pre-update `last_modified` so we can assert it
    // survives the no-op.
    let pre_last_modified = engine
        .get_entity(&seeded.id)
        .and_then(|e| e.metadata.get("last_modified"))
        .map(|v| v.to_frontmatter_string())
        .expect("seeded entity has last_modified");

    // Re-set `identity` to its current on-disk body. The seed
    // helper writes "fixture identity body" — passing the same
    // string back must be a no-op.
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "fixture identity body".to_string());
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
            None,
        )
        .unwrap();

    assert_eq!(outcome.write_id, "", "no-op must not commit");
    assert_eq!(
        outcome.content_hash, seeded.content_hash,
        "no-op must not advance content_hash",
    );
    assert!(
        outcome.warnings.iter().any(|w| w.code() == "UPDATE_NOOP"),
        "UPDATE_NOOP must fire on bytes-identical re-set",
    );
    assert_eq!(
        outcome.modified_date, pre_last_modified,
        "no-op must preserve last_modified at the pre-call value",
    );
    // The applied delta is empty on a
    // no-op — `modified_sections` must not claim `identity` was
    // replaced when nothing landed (matching the empty write_id
    // and unchanged hash above).
    assert!(
        outcome.modified_sections.replaced.is_empty()
            && outcome.modified_sections.appended.is_empty()
            && outcome.modified_sections.patched.is_empty(),
        "no-op must report an empty section delta, got {:?}",
        outcome.modified_sections,
    );

    // The on-disk entity also still carries the pre-update
    // last_modified — the no-op didn't bump it through some
    // other path.
    let post_last_modified = engine
        .get_entity(&seeded.id)
        .and_then(|e| e.metadata.get("last_modified"))
        .map(|v| v.to_frontmatter_string())
        .expect("entity still in store");
    assert_eq!(post_last_modified, pre_last_modified);
}

/// A payload with no
/// recognised mutation content refuses with `EMPTY_UPDATE` BEFORE
/// any engine work runs. Previously this same input short-
/// circuited as a success-with-`UPDATE_NOOP`-warning, which
/// hid a boundary-discipline failure mode (a
/// misspelled mutation key deserialises to empty defaults and
/// looks like a no-op success on the wire). The new refusal
/// makes "no mutation content provided" structurally distinct
/// from "mutation content provided but matched current state"
/// (which still surfaces `UPDATE_NOOP` — see
/// `update_entity_noop_same_content_surfaces_warning` below).
#[test]
fn update_entity_empty_payload_refuses_with_typed_code() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Empty Payload");
    let (actor, client) = cli_actor();

    let err = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
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
        EngineError::EmptyUpdate { id } => {
            assert_eq!(id, seeded.id.to_string());
        }
        other => panic!("expected EMPTY_UPDATE, got {other:?}"),
    }
    // No provenance row landed — the refusal preempts any write.
    let log_path = tmp.path().join(".memstead/changes.jsonl");
    if let Ok(log) = std::fs::read_to_string(&log_path) {
        let updates = log.matches("\"kind\":\"update\"").count();
        assert_eq!(updates, 0, "EMPTY_UPDATE refusal must not log an update");
    }
}

/// Complement: a
/// payload with mutation content that matches the current entity
/// state continues to land as success-with-`UPDATE_NOOP`-warning.
/// The new `EMPTY_UPDATE` refusal applies only when no mutation
/// content was provided; this path is structurally distinct.
#[test]
fn update_entity_noop_same_content_surfaces_warning() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Same Content Noop");
    let (actor, client) = cli_actor();

    // `empty_create_args` seeds `identity` with this exact body.
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "fixture identity body".to_string());

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
            None,
        )
        .unwrap();

    assert_eq!(outcome.write_id, "");
    assert_eq!(outcome.content_hash, seeded.content_hash);
    let codes: Vec<&str> = outcome.warnings.iter().map(|w| w.code()).collect();
    assert!(
        codes.contains(&"UPDATE_NOOP"),
        "same-content update must surface UPDATE_NOOP; got {codes:?}",
    );
}

#[test]
fn update_entity_noop_metadata_unset_on_absent_key() {
    // `metadata_unset=["never-set-key"]`
    // where the key was never set is a no-op — no field actually
    // changed, no commit advances, follow-up calls can chain
    // `expected_hash` without `HASH_MISMATCH`.
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Absent Key Noop");
    let (actor, client) = cli_actor();

    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                // `tags` is declared on the `spec` schema but
                // unset on the seeded entity. Unsetting it should
                // be a no-op rather than producing a fresh commit.
                metadata_unset: vec!["tags".to_string()],
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

    assert_eq!(outcome.write_id, "");
    assert_eq!(outcome.content_hash, seeded.content_hash);
    assert!(
        outcome.warnings.iter().any(|w| w.code() == "UPDATE_NOOP"),
        "absent-key metadata_unset must surface UPDATE_NOOP",
    );
    // Empty applied delta on the no-op —
    // `unset` must not claim `tags` was removed when nothing landed.
    assert!(
        outcome.modified_metadata.set.is_empty() && outcome.modified_metadata.unset.is_empty(),
        "no-op must report an empty metadata delta, got {:?}",
        outcome.modified_metadata,
    );

    // Follow-up real change against the unchanged hash succeeds —
    // no HASH_MISMATCH cascade from a phantom advance.
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "real change".to_string());
    let real = engine
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
            None,
        )
        .unwrap();
    assert!(!real.write_id.is_empty());
    assert_ne!(real.content_hash, seeded.content_hash);
}

/// The exact MCP repro — re-setting a
/// metadata key to its current value no-ops, and the response's
/// `modified_metadata` reports the applied delta (empty), not the
/// requested key. Pre-fix the no-op short-circuit echoed
/// `set: ["level"]` while `write_id` was empty and the hash
/// unchanged — a self-contradictory response.
#[test]
fn update_entity_noop_setting_metadata_to_current_value_reports_empty_delta() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Stability Resetter");
    let (actor, client) = cli_actor();

    // `level` defaults to "M0" on the spec schema, so the seed
    // carries it. Re-setting it to "M0" changes nothing.
    let mut metadata = IndexMap::new();
    metadata.insert("level".to_string(), "M0".to_string());
    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: seeded.id.clone(),
                expected_hash: Some(seeded.content_hash.clone()),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata,
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

    assert_eq!(outcome.write_id, "", "no-op must not commit");
    assert_eq!(
        outcome.content_hash, seeded.content_hash,
        "no-op must not advance hash"
    );
    assert!(
        outcome.warnings.iter().any(|w| w.code() == "UPDATE_NOOP"),
        "re-set to current value must surface UPDATE_NOOP",
    );
    assert!(
        outcome.modified_metadata.set.is_empty() && outcome.modified_metadata.unset.is_empty(),
        "no-op must not claim `level` was set — applied delta is empty, got {:?}",
        outcome.modified_metadata,
    );
}

#[test]
fn update_entity_noop_declare_already_related_edge() {
    // Re-declare an already-related edge
    // via `declare_relations` — no field, section, metadata or
    // relations list actually changes, so the bytes are identical
    // and the call no-ops.
    use crate::ops::RelateArg;
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target Already Related"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let source = engine
        .create_entity(
            empty_create_args("specs", "Source Already Related"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let after_relate = engine
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
    // Now re-declare the same edge via update.declare_relations.
    let outcome = engine
        .update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
                id: source.id.clone(),
                expected_hash: Some(after_relate.content_hash.clone()),
                sections: IndexMap::new(),
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                declare_relations: vec![RelateArg {
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    description: None,
                }],
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    assert_eq!(outcome.write_id, "");
    assert_eq!(outcome.content_hash, after_relate.content_hash);
    assert!(
        outcome.warnings.iter().any(|w| w.code() == "UPDATE_NOOP"),
        "duplicate declare must surface UPDATE_NOOP",
    );
    // `relations_declared` still records the entry — the per-
    // relation outcome is part of the surface, even for no-ops.
    assert_eq!(outcome.relations_declared.len(), 1);
    assert_eq!(outcome.relations_declared[0].rel_type, "USES");
    assert_eq!(outcome.relations_declared[0].target, target.id);
    assert!(!outcome.relations_declared[0].target_was_stubbed);
}

#[test]
fn update_entity_real_change_still_commits_and_advances_hash() {
    // Regression: the no-op short-circuit must not short-circuit
    // real changes. A section replacement still produces a
    // non-empty `write_id`, advances `content_hash`, and does
    // NOT surface UPDATE_NOOP.
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Real Change Subject");
    let (actor, client) = cli_actor();

    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "definitely new body".to_string());

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
            None,
        )
        .unwrap();

    assert!(!outcome.write_id.is_empty(), "real change must commit");
    assert_ne!(
        outcome.content_hash, seeded.content_hash,
        "real change must advance content_hash",
    );
    assert!(
        !outcome.warnings.iter().any(|w| w.code() == "UPDATE_NOOP"),
        "real change must not surface UPDATE_NOOP",
    );
}

#[test]
fn update_entity_noop_preserves_expected_hash_across_chain() {
    // A follow-up update with the original
    // hash after one or more no-ops succeeds because the hash
    // never advanced. Demonstrates the `expected_hash`-caching
    // posture the agent surface relies on.
    let tmp = TempDir::new().unwrap();
    let (mut engine, seeded) = engine_with_seed(&tmp, "Chained Noops Subject");
    let (actor, client) = cli_actor();

    // Two no-op calls in a row — both must return the same hash.
    // Pass same-content mutation
    // so UPDATE_NOOP fires (rather than EMPTY_UPDATE) and the
    // hash-chain invariant is exercised on the warning path.
    let mut noop_sections = IndexMap::new();
    noop_sections.insert("identity".to_string(), "fixture identity body".to_string());
    for _ in 0..2 {
        let outcome = engine
            .update_entity(
                UpdateEntityArgs {
                    anchors: Vec::new(),
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                    sections: noop_sections.clone(),
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
        assert_eq!(outcome.write_id, "");
        assert_eq!(outcome.content_hash, seeded.content_hash);
    }

    // Real follow-up with the original hash still works — no
    // HASH_MISMATCH cascade because the hash never advanced.
    let mut sections = IndexMap::new();
    sections.insert(
        "identity".to_string(),
        "third call: real change".to_string(),
    );
    let real = engine
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
            None,
        )
        .unwrap();
    assert!(!real.write_id.is_empty());
    assert_ne!(real.content_hash, seeded.content_hash);
}
