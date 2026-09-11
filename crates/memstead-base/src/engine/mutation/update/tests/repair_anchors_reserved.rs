//! The repair-power gate on `relations_unset`, anchors on update
//! (merge, unset, re-pin, hash stability), reserved-key unsets, and
//! the cycle refusals on `declare_relations`.

use super::*;

// ---- relations_unset repair-power gating -------------------------

/// Markdown for a deliberately non-conformant `spec`: carries an
/// undeclared metadata field (`zzz_bogus_field`) plus one USES
/// relation. Written straight to disk before engine construction —
/// the write path refuses non-conformant entities, so out-of-band
/// state is the only way to seed one (which is exactly the
/// repair-power scenario: drift entered outside the engine).
const DRIFTED_MD: &str = "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nzzz_bogus_field: x\n---\n# Drifted\n\n## Identity\n\nNon-conformant fixture.\n\n## Purpose\n\nRepair-gate tests.\n\n## Relationships\n\n- **USES**: [[anchor]]\n";

fn repair_engine() -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    std::fs::write(
            mem_dir.join("anchor.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\n---\n# Anchor\n\n## Identity\n\nTarget.\n\n## Purpose\n\nRelation target.\n",
        )
        .unwrap();
    std::fs::write(mem_dir.join("drifted.md"), DRIFTED_MD).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    (tmp, engine)
}

fn repair_args(id: EntityId, hash: Option<String>) -> UpdateEntityArgs {
    UpdateEntityArgs {
        anchors: Vec::new(),
        id,
        expected_hash: hash,
        sections: IndexMap::new(),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: vec![crate::ops::RelationUnsetArg {
            rel_type: "USES".to_string(),
            target: EntityId::new("specs", "anchor"),
        }],
        anchors_unset: Vec::new(),
    }
}

/// A conformant entity refuses repair-shaped input with
/// `REPAIR_NOT_NEEDED` and is not modified — even though it has a
/// relation that `relations_unset` names. The recovery text points
/// at the focused detach path.
#[test]
fn relations_unset_on_conformant_entity_refuses_repair_not_needed() {
    let (_tmp, mut engine) = repair_engine();
    // `anchor` is conformant. Give it a relation first via the
    // ordinary relate path so there is something to (not) remove.
    let anchor = EntityId::new("specs", "anchor");
    let drifted = EntityId::new("specs", "drifted");
    engine
        .relate_entity(
            RelateEntityArgs {
                source: anchor.clone(),
                expected_hash: None,
                rel_type: "USES".to_string(),
                target: drifted.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .expect("relate on conformant entity works");
    let mut args = repair_args(anchor.clone(), None);
    args.relations_unset[0].target = drifted.clone();
    let err = engine
        .update_entity(args, Actor::Cli, None, None)
        .unwrap_err();
    match err {
        EngineError::RepairNotNeeded { id, recovery } => {
            assert_eq!(id, anchor.to_string());
            assert!(
                recovery.contains("memstead_relate"),
                "recovery must point at the focused tool; got {recovery}"
            );
        }
        other => panic!("expected RepairNotNeeded, got {other:?}"),
    }
    // Entity unmodified — the relation is still there.
    let entity = engine.store().get(&anchor).unwrap();
    assert!(
        entity.relationships.iter().any(|r| r.target == drifted),
        "gate must not modify the entity"
    );
}

/// A non-conformant entity accepts `relations_unset`: the named
/// relation is removed atomically within the same update that also
/// repairs the conformance break (`metadata_unset` on the
/// undeclared field). The post-write entity is integral.
#[test]
fn relations_unset_repairs_non_conformant_entity_atomically() {
    let (_tmp, mut engine) = repair_engine();
    let drifted = EntityId::new("specs", "drifted");
    // Pre-state really is non-conformant.
    let pre = engine.conformance_findings("specs", None).unwrap();
    assert!(
        pre.iter().any(|f| f.id == drifted.to_string()),
        "fixture must lint non-conformant; got {pre:?}"
    );
    let mut args = repair_args(drifted.clone(), None);
    args.metadata_unset = vec!["zzz_bogus_field".to_string()];
    engine
        .update_entity(args, Actor::Cli, None, None)
        .expect("repair update lands");
    let entity = engine.store().get(&drifted).unwrap();
    assert!(
        entity.relationships.is_empty(),
        "relation must be removed; got {:?}",
        entity.relationships
    );
    assert!(
        !entity.metadata.contains_key("zzz_bogus_field"),
        "conformance break must be repaired in the same update"
    );
    let post = engine.conformance_findings("specs", None).unwrap();
    assert!(
        post.iter().all(|f| f.id != drifted.to_string()),
        "post-repair entity must be conformant; got {post:?}"
    );
}

/// Repair widens accepted inputs, never admissible outputs: a
/// repair write whose post-state would violate the schema refuses
/// with the relevant write-time code and nothing lands.
#[test]
fn relations_unset_post_state_must_still_validate() {
    let (_tmp, mut engine) = repair_engine();
    let drifted = EntityId::new("specs", "drifted");
    let mut args = repair_args(drifted.clone(), None);
    // Post-state violation: an unknown section alongside the
    // repair input.
    args.sections = IndexMap::from_iter([("nonexistent_section".to_string(), "x".to_string())]);
    let err = engine
        .update_entity(args, Actor::Cli, None, None)
        .unwrap_err();
    assert_eq!(
        err.code(),
        "UNKNOWN_SECTION",
        "strict-write post-condition must hold during repair; got {err:?}"
    );
    // Nothing landed: the relation survives.
    let entity = engine.store().get(&drifted).unwrap();
    assert!(
        !entity.relationships.is_empty(),
        "refused repair must not partially apply"
    );
}

/// Absent `(rel_type, target)` pairs are silent no-ops — symmetric
/// with `metadata_unset` — so a repair retry is idempotent.
#[test]
fn relations_unset_absent_pair_is_silent_noop() {
    let (_tmp, mut engine) = repair_engine();
    let drifted = EntityId::new("specs", "drifted");
    let mut args = repair_args(drifted.clone(), None);
    args.relations_unset[0].rel_type = "NEVER_DECLARED".to_string();
    // Also repair the field so the post-state is integral.
    args.metadata_unset = vec!["zzz_bogus_field".to_string()];
    engine
        .update_entity(args, Actor::Cli, None, None)
        .expect("absent pair no-ops, update lands");
    let entity = engine.store().get(&drifted).unwrap();
    assert_eq!(
        entity.relationships.len(),
        1,
        "the USES relation must survive an unmatched unset"
    );
}

// ---- anchors merge / unset -------------------------------------------

fn anchor_input(artifact: &str, hash: &str) -> crate::anchor::AnchorInput {
    crate::anchor::AnchorInput {
        artifact: Some(artifact.to_string()),
        grain: Some("file".to_string()),
        class: Some("anchored".to_string()),
        hash: Some(hash.to_string()),
        hash_stability: Some("stable".to_string()),
        ..Default::default()
    }
}

fn anchor_unset(artifact: &str) -> crate::anchor::AnchorUnsetInput {
    crate::anchor::AnchorUnsetInput {
        artifact: Some(artifact.to_string()),
        grain: None,
        class: None,
    }
}

/// Bare update-args shell for anchor tests — no content mutation.
fn anchor_args(id: EntityId, hash: Option<String>) -> UpdateEntityArgs {
    UpdateEntityArgs {
        anchors: Vec::new(),
        anchors_unset: Vec::new(),
        id,
        expected_hash: hash,
        sections: IndexMap::new(),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
    }
}

/// Engine over a folder mount, seeded with one entity carrying two
/// anchors (a.rs, b.rs). Returns the engine, tempdir, id, and hash.
fn anchored_engine() -> (Engine, TempDir, EntityId, String) {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let mut args = empty_create_args("specs", "Anchored");
    args.anchors = vec![anchor_input("a.rs", "h-a"), anchor_input("b.rs", "h-b")];
    let created = engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();
    let id = EntityId::new("specs", "anchored");
    assert_eq!(engine.entity_anchors(&id).len(), 2);
    (engine, tmp, id, created.content_hash)
}

/// Merge acceptance: one new anchor on N existing yields N+1 with the
/// others byte-identical; a same-`(artifact, grain, class)` write
/// replaces exactly that one; the motivating regression is dead —
/// "anchor batch A, later anchor batch B" leaves A ∪ B queryable.
#[test]
fn update_anchors_merge_appends_and_replaces_by_triple() {
    let (mut engine, _tmp, id, hash) = anchored_engine();
    let (actor, client) = cli_actor();

    // Batch B: one new artifact. A ∪ B must survive.
    let mut args = anchor_args(id.clone(), Some(hash));
    args.anchors = vec![anchor_input("c.rs", "h-c")];
    let out = engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap();
    let anchors = engine.entity_anchors(&id);
    assert_eq!(anchors.len(), 3, "N existing + 1 new = N+1");
    assert_eq!(anchors[0].artifact, "a.rs");
    assert_eq!(anchors[0].hash.as_deref(), Some("h-a"));
    assert_eq!(anchors[1].artifact, "b.rs");
    assert_eq!(anchors[2].artifact, "c.rs");
    assert!(!engine.anchors_referencing_artifact("a.rs").is_empty());
    assert!(!engine.anchors_referencing_artifact("c.rs").is_empty());

    // Same-triple write replaces exactly that one, in place.
    let mut args = anchor_args(id.clone(), Some(out.content_hash));
    args.anchors = vec![anchor_input("a.rs", "h-a2")];
    engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap();
    let anchors = engine.entity_anchors(&id);
    assert_eq!(anchors.len(), 3);
    assert_eq!(anchors[0].artifact, "a.rs");
    assert_eq!(anchors[0].hash.as_deref(), Some("h-a2"));
    assert_eq!(anchors[1].hash.as_deref(), Some("h-b"), "b untouched");
    assert_eq!(anchors[2].hash.as_deref(), Some("h-c"), "c untouched");
}

/// Re-sending the full current set is a no-op on the stored sidecar
/// bytes, and an update with an empty/absent `anchors` list leaves the
/// stored set untouched.
#[test]
fn update_anchors_full_resend_and_absent_are_noops_on_stored_set() {
    let (mut engine, tmp, id, hash) = anchored_engine();
    let (actor, client) = cli_actor();
    let sidecar_path = tmp.path().join(crate::anchor::ANCHOR_SIDECAR_PATH);
    let before = std::fs::read(&sidecar_path).unwrap();

    // Full re-send of the current set.
    let mut args = anchor_args(id.clone(), Some(hash));
    args.anchors = vec![anchor_input("a.rs", "h-a"), anchor_input("b.rs", "h-b")];
    let out = engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap();
    assert_eq!(
        std::fs::read(&sidecar_path).unwrap(),
        before,
        "full re-send keeps the stored bytes"
    );

    // Absent anchors list + a real content change: set untouched.
    let mut args = anchor_args(id.clone(), Some(out.content_hash));
    args.sections
        .insert("identity".to_string(), "changed body".to_string());
    engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap();
    assert_eq!(
        std::fs::read(&sidecar_path).unwrap(),
        before,
        "an anchorless update never touches the stored set"
    );
}

/// Unset acceptance: bare-artifact unset removes all of that
/// artifact's anchors and nothing else; a grain-narrowed unset removes
/// only the match; a nonexistent target succeeds and changes nothing;
/// unset + merge in one call apply unset-first.
#[test]
fn update_anchors_unset_bare_narrowed_idempotent_and_unset_first() {
    let (mut engine, _tmp, id, hash) = anchored_engine();
    let (actor, client) = cli_actor();

    // Add a span-grain anchor on a.rs so a.rs carries two grains.
    let mut span = anchor_input("a.rs", "h-span");
    span.grain = Some("span".to_string());
    let mut args = anchor_args(id.clone(), Some(hash));
    args.anchors = vec![span];
    let out = engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap();
    assert_eq!(engine.entity_anchors(&id).len(), 3);

    // Narrowed unset: only the span-grain anchor goes.
    let mut narrowed = anchor_unset("a.rs");
    narrowed.grain = Some("span".to_string());
    let mut args = anchor_args(id.clone(), Some(out.content_hash));
    args.anchors_unset = vec![narrowed];
    let out = engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap();
    let anchors = engine.entity_anchors(&id);
    assert_eq!(anchors.len(), 2);
    assert!(
        anchors
            .iter()
            .all(|a| a.grain == crate::anchor::AnchorGrain::File)
    );

    // Nonexistent target: succeeds, changes nothing.
    let mut args = anchor_args(id.clone(), Some(out.content_hash.clone()));
    args.anchors_unset = vec![anchor_unset("never-there.rs")];
    engine
        .update_entity(args, actor, Some(&client), None)
        .expect("unset of a nonexistent target is a no-op, not an error");
    assert_eq!(engine.entity_anchors(&id).len(), 2);

    // Unset + merge in one call: bare unset of a.rs plus a fresh a.rs
    // anchor — unset applies first, so the fresh anchor lands.
    let mut args = anchor_args(id.clone(), Some(out.content_hash));
    args.anchors_unset = vec![anchor_unset("a.rs")];
    args.anchors = vec![anchor_input("a.rs", "h-a-fresh")];
    engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap();
    let anchors = engine.entity_anchors(&id);
    assert_eq!(anchors.len(), 2);
    assert_eq!(anchors[0].artifact, "b.rs", "b.rs untouched throughout");
    assert_eq!(anchors[1].hash.as_deref(), Some("h-a-fresh"));
}

/// Criterion 9 (consistency-sweep 03/03): one payload naming the same
/// `(artifact, grain, class)` triple twice is refused. That triple is the
/// sidecar's merge identity, so the repeats used to collapse to the last
/// occurrence and the caller was never told an anchor it sent had gone.
/// Criterion 10's complement rides along: the same artifact at two grains
/// is two rows and still writes.
#[test]
fn a_payload_naming_one_triple_twice_is_refused() {
    let (mut engine, _tmp, id, hash) = anchored_engine();
    let (actor, client) = cli_actor();

    let mut args = anchor_args(id.clone(), Some(hash.clone()));
    args.anchors = vec![
        anchor_input("a.rs", "h-first"),
        anchor_input("a.rs", "h-second"),
    ];
    let err = engine
        .update_entity(args, actor, Some(&client), None)
        .expect_err("the repeated triple must refuse");
    assert_eq!(err.code(), crate::anchor::INVALID_ANCHOR_CODE);
    assert!(
        format!("{err}").contains("more than once"),
        "the refusal names the collapse: {err}"
    );

    // Refused before any state change: the stored rows are untouched.
    assert_eq!(engine.entity_anchors(&id).len(), 2);

    // The same artifact at a different grain is a different row, and
    // still writes in one payload.
    let mut span = anchor_input("a.rs", "h-span");
    span.grain = Some("span".to_string());
    let mut args = anchor_args(id.clone(), Some(hash));
    args.anchors = vec![anchor_input("a.rs", "h-file"), span];
    engine
        .update_entity(args, actor, Some(&client), None)
        .expect("two grains on one artifact are two rows");
    assert_eq!(engine.entity_anchors(&id).len(), 3);
}

/// Criterion 5 through the real write surface: a later call carrying the
/// same triple with no hash keeps the stored baseline, rather than
/// dropping it for the next verify to silently re-establish.
#[test]
fn a_re_pin_without_a_hash_keeps_the_stored_baseline() {
    let (mut engine, _tmp, id, hash) = anchored_engine();
    let (actor, client) = cli_actor();

    let mut hashless = anchor_input("a.rs", "");
    hashless.hash = None;
    let mut args = anchor_args(id.clone(), Some(hash));
    args.anchors = vec![hashless];
    engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap();

    let kept = engine
        .entity_anchors(&id)
        .into_iter()
        .find(|a| a.artifact == "a.rs")
        .expect("the row is still there");
    assert_eq!(
        kept.hash.as_deref(),
        Some("h-a"),
        "the baseline the re-pin did not mention survives it"
    );
}

/// An anchor-write update never moves `_hash`, and an unset-only
/// update is a real commit (not a no-op) that leaves the entity bytes
/// untouched.
#[test]
fn update_anchor_only_and_unset_only_commit_without_hash_movement() {
    let (mut engine, _tmp, id, hash) = anchored_engine();
    let (actor, client) = cli_actor();

    let mut args = anchor_args(id.clone(), Some(hash.clone()));
    args.anchors_unset = vec![anchor_unset("b.rs")];
    let out = engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap();
    assert!(
        !out.write_id.is_empty(),
        "unset-only update commits the sidecar"
    );
    assert_eq!(out.content_hash, hash, "anchors never move `_hash`");
    assert_eq!(engine.entity_anchors(&id).len(), 1);

    // Refusal complement: with no anchors, no unsets, and no content,
    // the empty-update guard still fires.
    let err = engine
        .update_entity(
            anchor_args(id.clone(), Some(hash)),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::EmptyUpdate { .. }));
}

/// The same contract across a wall-clock second boundary: an
/// anchor-only update landing a full second after the create must
/// not auto-stamp `last_modified` (which would change the entity
/// bytes and move `_hash`). Pre-fix, the anchor-only leg fell past
/// the no-op guard into the unconditional auto-stamp and this
/// failed whenever create and update straddled a second — the
/// pinned clock makes that straddle deterministic.
#[test]
fn anchor_only_update_across_second_boundary_never_moves_hash() {
    let (mut engine, _tmp, id, hash) = anchored_engine();
    let (actor, client) = cli_actor();

    let t0 = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_778_243_696);
    engine.set_mutation_clock(std::sync::Arc::new(move || t0));
    // Re-create baseline under the pinned clock so the stamped
    // `last_modified` is exactly t0's second.
    let mut args = anchor_args(id.clone(), Some(hash));
    args.metadata = [("level".to_string(), "M1".to_string())]
        .into_iter()
        .collect();
    let restamped = engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap();

    // One second later: anchor-only update.
    let t1 = t0 + std::time::Duration::from_secs(1);
    engine.set_mutation_clock(std::sync::Arc::new(move || t1));
    let mut args = anchor_args(id.clone(), Some(restamped.content_hash.clone()));
    args.anchors = vec![anchor_input("c.rs", "h-c")];
    let out = engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap();
    assert!(!out.write_id.is_empty(), "anchor-only update commits");
    assert_eq!(
        out.content_hash, restamped.content_hash,
        "anchors never move `_hash`, even across a second boundary"
    );
    // The preserved `last_modified` is observable on the entity too.
    let entity = engine.store().get(&id).unwrap();
    assert_eq!(
        entity
            .metadata
            .get("last_modified")
            .and_then(|v| v.as_str()),
        Some("2026-05-08T12:34:56Z"),
        "anchor-only update must not restamp last_modified"
    );
}

/// A malformed `anchors_unset[]` selector refuses the whole update
/// with the typed `INVALID_ANCHOR` envelope and nothing is written —
/// merge introduces no partial-apply.
#[test]
fn malformed_anchor_unset_refuses_and_nothing_is_written() {
    let (mut engine, tmp, id, hash) = anchored_engine();
    let (actor, client) = cli_actor();
    let sidecar_path = tmp.path().join(crate::anchor::ANCHOR_SIDECAR_PATH);
    let before = std::fs::read(&sidecar_path).unwrap();

    let mut bad = anchor_unset("a.rs");
    bad.grain = Some("paragraph".to_string()); // unknown grain
    let mut args = anchor_args(id.clone(), Some(hash));
    args.anchors_unset = vec![bad];
    // A valid incoming anchor rides the same call — it must not land.
    args.anchors = vec![anchor_input("c.rs", "h-c")];
    let err = engine
        .update_entity(args, actor, Some(&client), None)
        .unwrap_err();
    assert_eq!(err.code(), crate::anchor::INVALID_ANCHOR_CODE);
    assert_eq!(engine.entity_anchors(&id).len(), 2, "no partial apply");
    assert_eq!(std::fs::read(&sidecar_path).unwrap(), before);
}

// ---- reserved metadata keys: set refused, unset is the repair --------

/// Bare update-args shell for the reserved-key tests.
fn bare_args(id: EntityId, hash: Option<String>) -> UpdateEntityArgs {
    UpdateEntityArgs {
        anchors: Vec::new(),
        anchors_unset: Vec::new(),
        id,
        expected_hash: hash,
        sections: IndexMap::new(),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
    }
}

/// On an entity fixture carrying historically smuggled reserved
/// keys (`mem` / `id` in its frontmatter, written before the write
/// gates closed), `metadata_unset` naming them succeeds, removes
/// them from the store and the on-disk file, and the entity
/// round-trips cleanly thereafter. Refusal complement: `metadata`
/// (set) with a reserved key still refuses on update — single and
/// batch.
#[test]
fn reserved_key_unset_repairs_smuggled_entity_and_set_stays_refused() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    // Pre-gate fixture: frontmatter smuggles `mem` and `id`.
    std::fs::write(
            mem_dir.join("smuggled.md"),
            "---\ntype: spec\nmem: wrong-mem\nid: bogus-id\n---\n# Smuggled\n\n## Identity\n\nsmuggled identity.\n\n## Purpose\n\nsmuggled purpose.\n",
        )
        .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let id = EntityId::new("specs", "smuggled");
    let entity = engine.get_entity(&id).expect("fixture boots");
    assert!(
        entity.metadata.contains_key("mem") && entity.metadata.contains_key("id"),
        "fixture must carry the smuggled keys after boot"
    );
    let hash = entity.content_hash.clone();

    // Refusal complement, single: SET of a reserved key refuses.
    for reserved in ["type", "mem", "id"] {
        let mut args = bare_args(id.clone(), Some(hash.clone()));
        args.metadata
            .insert(reserved.to_string(), "resmuggled".to_string());
        let err = engine
            .update_entity(args, actor, Some(&client), None)
            .expect_err("reserved-key set must refuse on update");
        assert_eq!(err.code(), "READ_ONLY_FIELD", "key '{reserved}': {err:?}");
    }
    // Refusal complement, batch: same refusal through batch_update
    // (atomic — nothing lands).
    let mut batch_item = bare_args(id.clone(), Some(hash.clone()));
    batch_item
        .metadata
        .insert("id".to_string(), "resmuggled".to_string());
    let batch = engine
        .batch_update(vec![(batch_item, None)], actor, Some(&client), false)
        .expect("batch returns a result envelope");
    assert!(
        !batch.applied,
        "batch with a reserved-key set must not apply"
    );
    assert_eq!(batch.failed, 1);

    // The sanctioned repair: unset both smuggled keys in one call.
    let mut args = bare_args(id.clone(), Some(hash));
    args.metadata_unset = vec!["mem".to_string(), "id".to_string()];
    let out = engine
        .update_entity(args, actor, Some(&client), None)
        .expect("reserved-key unset is the sanctioned repair");
    assert!(!out.write_id.is_empty(), "repair is a real commit");
    assert_eq!(
        out.modified_metadata.unset,
        vec!["mem".to_string(), "id".to_string()]
    );

    // Invariant restored: store and disk are clean, and the entity
    // round-trips through a further ordinary update.
    let entity = engine.get_entity(&id).expect("entity survives repair");
    assert!(
        !entity.metadata.contains_key("mem") && !entity.metadata.contains_key("id"),
        "smuggled keys must be gone from the store"
    );
    let on_disk = std::fs::read_to_string(mem_dir.join("smuggled.md")).unwrap();
    assert!(
        !on_disk.contains("wrong-mem") && !on_disk.contains("bogus-id"),
        "smuggled keys must be gone from the file: {on_disk}"
    );
    let mut args = bare_args(id.clone(), Some(entity.content_hash.clone()));
    args.sections
        .insert("identity".to_string(), "repaired identity".to_string());
    engine
        .update_entity(args, actor, Some(&client), None)
        .expect("post-repair entity round-trips cleanly");
}

/// Unsetting `type` never leaves an entity typeless: the engine
/// re-seeds the authoritative discriminator, so on a healthy entity
/// the unset is a committed-nothing no-op (`UPDATE_NOOP`) and the
/// type survives on disk. (A missing `type:` would silently re-type
/// the entity to the mem's default on the next parse — the re-seed
/// forecloses that.) Unset of a nonexistent reserved key is equally
/// a no-op, not an error.
#[test]
fn reserved_type_unset_reseeds_and_is_a_noop_on_healthy_entities() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let created = engine
        .create_entity(
            empty_create_args("specs", "Healthy"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let id = EntityId::new("specs", "healthy");

    for key in ["type", "mem", "id"] {
        let mut args = bare_args(id.clone(), Some(created.content_hash.clone()));
        args.metadata_unset = vec![key.to_string()];
        let out = engine
            .update_entity(args, actor, Some(&client), None)
            .unwrap_or_else(|e| panic!("unset '{key}' on a healthy entity must no-op: {e:?}"));
        assert!(
            out.write_id.is_empty(),
            "unset '{key}' on a healthy entity is a no-op, not a commit"
        );
        assert!(
            out.warnings.iter().any(|w| w.code() == "UPDATE_NOOP"),
            "no-op must carry the UPDATE_NOOP warning for '{key}'"
        );
    }
    let entity = engine.get_entity(&id).unwrap();
    assert_eq!(entity.entity_type, "spec");
    assert_eq!(
        entity.metadata.get("type").and_then(|v| v.as_str()),
        Some("spec"),
        "the discriminator survives a type unset"
    );
}

// ---- cycle family on declare_relations -------------------------------

/// `update.declare_relations` runs the same cycle family as
/// `memstead_relate`: a cycle-closing edge on an acyclic rel-type
/// refuses `RELATIONSHIP_CYCLE` with the relate path's recovery
/// detail, a self-loop on a listed no-self-loop rel-type refuses
/// identically, and — refusal complement — a non-cycle edge on the
/// acyclic type is accepted exactly as today.
#[test]
fn declare_relations_refuses_cycle_and_self_loop_like_relate() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    // alpha PART_OF beta lands via create + relate.
    let alpha = engine
        .create_entity(
            empty_create_args("specs", "Alpha"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let beta = engine
        .create_entity(
            empty_create_args("specs", "Beta"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    engine
        .relate_entity(
            crate::engine::RelateEntityArgs {
                source: alpha.id.clone(),
                target: beta.id.clone(),
                rel_type: "PART_OF".to_string(),
                remove: false,
                expected_hash: None,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let declare = |rel_type: &str, from: &EntityId, to: &EntityId, hash: String| {
        let mut args = bare_args(from.clone(), Some(hash));
        args.declare_relations = vec![crate::ops::RelateArg {
            target: to.clone(),
            rel_type: rel_type.to_string(),
            description: None,
        }];
        args
    };

    // beta declaring PART_OF→alpha closes beta→alpha→beta.
    let err = engine
        .update_entity(
            declare("PART_OF", &beta.id, &alpha.id, beta.content_hash.clone()),
            actor,
            Some(&client),
            None,
        )
        .expect_err("cycle-closing declare_relations must refuse");
    assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");
    let details = err.details();
    assert_eq!(details["rel_type"], "PART_OF");
    assert!(details["existing_path"].is_array());
    assert!(
        engine
            .get_entity(&beta.id)
            .unwrap()
            .relationships
            .is_empty(),
        "the refused edge must not land"
    );

    // Self-loop on a listed no-self-loop rel-type (spec lists USES).
    // Alpha's hash moved with the relate above — read the live one.
    let alpha_hash = engine.get_entity(&alpha.id).unwrap().content_hash.clone();
    let err = engine
        .update_entity(
            declare("USES", &alpha.id, &alpha.id, alpha_hash),
            actor,
            Some(&client),
            None,
        )
        .expect_err("self-loop declare_relations must refuse");
    assert_eq!(err.code(), "RELATIONSHIP_CYCLE", "{err:?}");

    // Refusal complement: a non-cycle PART_OF edge is accepted.
    engine
        .update_entity(
            declare(
                "PART_OF",
                &beta.id,
                &EntityId::new("specs", "gamma"),
                beta.content_hash.clone(),
            ),
            actor,
            Some(&client),
            None,
        )
        .expect("a non-cycle PART_OF declare must land as today");
}
