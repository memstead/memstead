#![cfg(feature = "mem-repo")]
//! Engine-level reload-before-operation test on a real git-branch
//! mem. Two `Engine` instances share one mem-repo gitdir (the
//! coherence plan's framing scenario: two sessions on one mem). A
//! sibling commit must be reloaded by the second engine *before* its
//! own write, and the reload must surface a structured `mem_changed`
//! notice describing what moved.
//!
//! This is the engine substrate the MCP `mem_changed` response field
//! rides on; the MCP wire harness drives a single process, so the
//! two-instance scenario is exercised here at the engine boundary.

use indexmap::IndexMap;
use memstead_base::ingest::Slice;
use memstead_base::ingest::advance::{AdvanceState, read_advance_store, write_advance_store};
use memstead_base::ops::NoticeChanges;
use memstead_base::vcs::{Actor, ClientId};
use memstead_base::{CreateEntityArgs, EngineError, EntityId, UpdateEntityArgs};
use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

fn create_args(mem: &str, title: &str) -> CreateEntityArgs {
    // The builtin `default` schema's `spec` type requires the
    // `identity` + `purpose` sections — seed both so the create is a
    // valid request.
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), "identity body".to_string());
    sections.insert("purpose".to_string(), "purpose body".to_string());
    CreateEntityArgs {
        anchors: Vec::new(),
        mem: mem.to_string(),
        title: title.to_string(),
        entity_type: "spec".to_string(),
        sections,
        metadata: IndexMap::new(),
        relations: Vec::new(),
        dry_run: false,
    }
}

fn client() -> ClientId {
    ClientId {
        name: "test".to_string(),
        version: "0".to_string(),
    }
}

/// Wholesale-replace the `purpose` section, gated on `expected_hash`.
fn update_purpose_args(id: EntityId, expected_hash: String, body: &str) -> UpdateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("purpose".to_string(), body.to_string());
    UpdateEntityArgs {
        anchors: Vec::new(),
        id,
        expected_hash: Some(expected_hash),
        sections,
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        dry_run: false,
        declare_relations: Vec::new(),
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    }
}

/// An anchor-only update: no section/metadata/relationship change, only
/// an `anchors[]` payload. Content stays byte-identical to on-disk, so
/// the sole delta is the anchors sidecar.
fn anchor_only_args(id: EntityId, expected_hash: String, anchor_json: &str) -> UpdateEntityArgs {
    let anchor: memstead_base::anchor::AnchorInput =
        serde_json::from_str(anchor_json).expect("valid anchor json");
    UpdateEntityArgs {
        anchors: vec![anchor],
        id,
        expected_hash: Some(expected_hash),
        sections: IndexMap::new(),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        dry_run: false,
        declare_relations: Vec::new(),
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    }
}

#[test]
fn anchor_only_update_commits_with_distinct_anchor_verb() {
    // Third clause of projection-pipeline/04 criterion 4: an anchor-only
    // commit produces zero entity deltas, its SHA is a valid `since`
    // cursor, AND — with notes surfaced (`include_notes`) — it appears as
    // a note entry carrying a DISTINCT `tool_verb` ("anchor"). The verb
    // fires only for anchor-only commits: a content update keeps "update"
    // and a create keeps "create" (the refusal complement).
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut e = engine_from_workspace_root(tmp.path()).expect("engine boots");

    // create → verb "create".
    let created = e
        .create_entity(
            create_args("specs", "Alpha"),
            Actor::Cli,
            Some(&client()),
            None,
        )
        .expect("create succeeds");
    let c1 = created.write_id.clone();

    // Anchor-only update. Anchors are excluded from `_hash`, so the
    // content hash is unchanged even though a real commit lands. The
    // anchored artifact must resolve — the write gate refuses dead refs.
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/lib.rs"), "// lib").unwrap();
    let anchor_json = r#"{"artifact":"src/lib.rs","grain":"file","class":"anchored","hash_stability":"stable","hash":"h1"}"#;
    let a = e
        .update_entity(
            anchor_only_args(
                created.id.clone(),
                created.content_hash.clone(),
                anchor_json,
            ),
            Actor::Cli,
            Some(&client()),
            None,
        )
        .expect("anchor-only update succeeds");
    let c2 = a.write_id.clone();
    assert!(!c2.is_empty(), "anchor-only update lands a real commit");
    assert_ne!(c1, c2, "anchor-only update produced a distinct commit");
    assert_eq!(
        a.content_hash, created.content_hash,
        "anchors are excluded from `_hash` — the content hash is unchanged",
    );

    // Across just the anchor commit (since = c1, exclusive): ZERO entity
    // deltas, exactly one note, verb "anchor".
    let across_anchor = e
        .changes_since("specs", &c1, None)
        .expect("changes_since c1");
    assert_eq!(across_anchor.head, c2);
    assert!(
        across_anchor.changes.is_empty(),
        "an anchor-only commit yields zero entity deltas, got {:?}",
        across_anchor.changes,
    );
    let notes = across_anchor.notes.expect("git-branch mem surfaces notes");
    assert_eq!(notes.len(), 1, "exactly the anchor commit is in range");
    assert_eq!(
        notes[0].tool_verb.as_deref(),
        Some("anchor"),
        "the anchor-only commit's note carries the distinct `anchor` verb",
    );

    // c2 is itself a valid `since` cursor: it resolves and reports no
    // deltas after it.
    let from_anchor = e
        .changes_since("specs", &c2, None)
        .expect("c2 is a valid cursor");
    assert_eq!(from_anchor.head, c2);
    assert!(from_anchor.changes.is_empty());

    // Refusal complement 1: an ordinary content update keeps verb
    // "update" and surfaces a real entity delta.
    let u = e
        .update_entity(
            update_purpose_args(
                created.id.clone(),
                created.content_hash.clone(),
                "revised purpose",
            ),
            Actor::Cli,
            Some(&client()),
            None,
        )
        .expect("content update succeeds");
    let c3 = u.write_id.clone();
    assert_ne!(c3, c2);
    let across_update = e
        .changes_since("specs", &c2, None)
        .expect("changes_since c2");
    assert_eq!(
        across_update.changes.len(),
        1,
        "a content update surfaces exactly one entity delta",
    );
    let unotes = across_update.notes.expect("notes");
    assert_eq!(unotes.len(), 1);
    assert_eq!(
        unotes[0].tool_verb.as_deref(),
        Some("update"),
        "a content-changing update keeps the plain `update` verb",
    );

    // Refusal complement 2 + fires-only-once: walk the whole branch.
    // create → "create", anchor-only → "anchor", content → "update", and
    // "anchor" appears exactly once (only the anchor-only commit earns it).
    let all = e
        .changes_since("specs", memstead_base::ops::changes::EMPTY_TREE_SHA, None)
        .expect("full walk");
    let all_notes = all.notes.expect("notes");
    let verbs: Vec<&str> = all_notes
        .iter()
        .filter_map(|n| n.tool_verb.as_deref())
        .collect();
    assert!(
        verbs.contains(&"create"),
        "create commit keeps `create`: {verbs:?}"
    );
    assert!(
        verbs.contains(&"anchor"),
        "anchor commit shows `anchor`: {verbs:?}"
    );
    assert!(
        verbs.contains(&"update"),
        "content update shows `update`: {verbs:?}"
    );
    assert_eq!(
        verbs.iter().filter(|v| **v == "anchor").count(),
        1,
        "the `anchor` verb fires only for the anchor-only commit: {verbs:?}",
    );
}

#[test]
fn second_engine_reloads_and_surfaces_mem_changed_on_create() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);

    // Both engines boot from the same workspace, cached at the same
    // (empty-tree) head before any write.
    let mut a = engine_from_workspace_root(tmp.path()).expect("engine A boots");
    let mut b = engine_from_workspace_root(tmp.path()).expect("engine B boots");

    // A creates E_a, advancing the shared mem ref.
    a.create_entity(
        create_args("specs", "Entity A"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("A create succeeds");

    // B, still cached at the pre-A head, creates a distinct entity. The
    // reload-before-operation check must pull A's commit in first (so
    // B's graph holds E_a) and stash a `mem_changed` notice.
    b.create_entity(
        create_args("specs", "Entity B"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("B create succeeds (distinct id, no collision)");

    assert!(
        b.get_entity(&EntityId::new("specs", "entity-a")).is_some(),
        "B reloaded to A's head before its write — E_a is present in B's graph",
    );

    let notices = b.take_mem_changed_notices();
    assert_eq!(
        notices.len(),
        1,
        "B's create reloaded exactly once and stashed one notice",
    );
    let n = &notices[0];
    assert_eq!(n.mem, "specs");
    match &n.changes {
        NoticeChanges::Detailed { entries } => {
            assert!(
                entries
                    .iter()
                    .any(|e| e.primary_id() == "specs--entity-a" && e.action() == "added"),
                "notice lists E_a as added: {entries:?}",
            );
            // The notice describes only the sibling's change — never
            // B's own follow-on write.
            assert!(
                !entries.iter().any(|e| e.primary_id() == "specs--entity-b"),
                "notice must not include B's own write: {entries:?}",
            );
        }
        other => panic!("expected detailed notice, got {other:?}"),
    }

    // No-silent-advance complement: B's head is now current, so a
    // follow-up quiescent reload attaches no notice.
    b.reload_if_stale(Some("specs"));
    assert!(
        b.take_mem_changed_notices().is_empty(),
        "quiescent op after the reload attaches no notice",
    );
}

#[test]
fn single_engine_no_sibling_attaches_no_notice() {
    // "Complement (single-engine unchanged)": with no sibling writer
    // the ref only moves by the engine's own commits, so no operation
    // reloads and no notice is ever stashed.
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut a = engine_from_workspace_root(tmp.path()).expect("engine boots");

    a.create_entity(
        create_args("specs", "Entity One"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("create one");
    assert!(
        a.take_mem_changed_notices().is_empty(),
        "first op has nothing to reload past",
    );

    // A second op by the same engine: its own prior commit advanced the
    // cached head via record_self_write, so reload-before-op sees
    // cached == live and does not reload.
    a.create_entity(
        create_args("specs", "Entity Two"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("create two");
    assert!(
        a.take_mem_changed_notices().is_empty(),
        "no sibling moved the ref — no notice on the engine's own follow-on write",
    );
}

#[test]
fn read_after_sibling_modify_returns_fresh_content_with_mem_changed() {
    // "Positive (read drift)": an engine cached at H0 issues a read
    // after a sibling modified X. The read path's reload refreshes X to
    // the sibling's content (not stale) and stashes the notice the MCP
    // read handler attaches to its response.
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);

    let mut a = engine_from_workspace_root(tmp.path()).expect("engine A boots");
    a.create_entity(
        create_args("specs", "Shared X"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("A create X");
    let mut b = engine_from_workspace_root(tmp.path()).expect("engine B boots");

    let x = EntityId::new("specs", "shared-x");
    let stale_hash = b.get_entity(&x).expect("B knows X").content_hash.clone();

    let a_hash = a.get_entity(&x).expect("A knows X").content_hash.clone();
    a.update_entity(
        update_purpose_args(x.clone(), a_hash, "purpose rewritten by A"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("A update X");

    // B's read path: reload-before-op, then read X.
    b.reload_if_stale(Some("specs"));
    let fresh_hash = b
        .get_entity(&x)
        .expect("B still knows X")
        .content_hash
        .clone();
    assert_ne!(
        fresh_hash, stale_hash,
        "B's read sees A's fresh content, not the stale boot snapshot",
    );

    let notices = b.take_mem_changed_notices();
    assert_eq!(
        notices.len(),
        1,
        "the read-triggered reload stashed one notice"
    );
    match &notices[0].changes {
        NoticeChanges::Detailed { entries } => assert!(
            entries
                .iter()
                .any(|e| e.primary_id() == "specs--shared-x" && e.action() == "updated"),
            "notice lists X as modified: {entries:?}",
        ),
        other => panic!("expected detailed notice, got {other:?}"),
    }
}

#[test]
fn write_collision_surfaces_hash_mismatch_with_mem_changed() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);

    // A creates the shared entity X first; B boots afterwards so B's
    // graph already holds X (cached at X's create head).
    let mut a = engine_from_workspace_root(tmp.path()).expect("engine A boots");
    a.create_entity(
        create_args("specs", "Shared X"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("A create X");
    let mut b = engine_from_workspace_root(tmp.path()).expect("engine B boots");

    let x = EntityId::new("specs", "shared-x");
    // The hash B holds for X — about to go stale.
    let b_stale_hash = b.get_entity(&x).expect("B knows X").content_hash.clone();

    // A modifies X, advancing X's hash.
    let a_hash = a.get_entity(&x).expect("A knows X").content_hash.clone();
    a.update_entity(
        update_purpose_args(x.clone(), a_hash, "purpose rewritten by A"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("A update X");

    // B updates X with its now-stale hash. Reload-before-op refreshes X
    // to A's version; the per-entity lock then sees the mismatch.
    let err = b
        .update_entity(
            update_purpose_args(x.clone(), b_stale_hash, "purpose by B"),
            Actor::Cli,
            Some(&client()),
            None,
        )
        .expect_err("stale hash refuses after the reload");
    assert!(
        matches!(err, EngineError::HashMismatch { .. }),
        "expected HASH_MISMATCH, got {err:?}",
    );

    // The notice still rides the (refused) operation.
    let notices = b.take_mem_changed_notices();
    assert_eq!(notices.len(), 1, "the reload stashed one notice");
    match &notices[0].changes {
        NoticeChanges::Detailed { entries } => assert!(
            entries
                .iter()
                .any(|e| e.primary_id() == "specs--shared-x" && e.action() == "updated"),
            "notice lists X as modified: {entries:?}",
        ),
        other => panic!("expected detailed notice, got {other:?}"),
    }
}

#[test]
fn unrelated_concurrent_write_proceeds_with_mem_changed() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);

    let mut a = engine_from_workspace_root(tmp.path()).expect("engine A boots");
    a.create_entity(
        create_args("specs", "Entity X"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("A create X");
    a.create_entity(
        create_args("specs", "Entity Y"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("A create Y");
    let mut b = engine_from_workspace_root(tmp.path()).expect("engine B boots");

    let x = EntityId::new("specs", "entity-x");
    let y = EntityId::new("specs", "entity-y");
    let y_hash = b.get_entity(&y).expect("B knows Y").content_hash.clone();

    // A modifies X.
    let x_hash = a.get_entity(&x).expect("A knows X").content_hash.clone();
    a.update_entity(
        update_purpose_args(x.clone(), x_hash, "X rewritten by A"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("A update X");

    // B updates the disjoint entity Y with a correct hash. Reload pulls
    // A's X change in, but Y is untouched, so no HASH_MISMATCH — the
    // update commits and the notice lists only X.
    b.update_entity(
        update_purpose_args(y.clone(), y_hash, "Y rewritten by B"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("disjoint update commits");

    let notices = b.take_mem_changed_notices();
    assert_eq!(notices.len(), 1);
    match &notices[0].changes {
        NoticeChanges::Detailed { entries } => {
            assert!(
                entries
                    .iter()
                    .any(|e| e.primary_id() == "specs--entity-x" && e.action() == "updated"),
                "notice lists the sibling's X change: {entries:?}",
            );
            assert!(
                !entries.iter().any(|e| e.primary_id() == "specs--entity-y"),
                "notice excludes B's own Y write: {entries:?}",
            );
        }
        other => panic!("expected detailed notice, got {other:?}"),
    }
}

/// D13 / AC11 — `sync_state` is **mem-scoped** state: an out-of-band
/// `sync_state` write by a sibling engine (here A's `set_mem_sync_state`) is
/// picked up by the second engine's per-mem reload, and the reload surfaces the
/// `mem_changed` drift notice like any other mem-branch change.
#[test]
fn sync_state_write_surfaces_via_per_mem_reload() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);

    let mut a = engine_from_workspace_root(tmp.path()).expect("engine A boots");
    // Seed one entity so both engines cache a common non-empty head.
    a.create_entity(
        create_args("specs", "Entity One"),
        Actor::Cli,
        Some(&client()),
        None,
    )
    .expect("A create");
    let mut b = engine_from_workspace_root(tmp.path()).expect("engine B boots");

    // A writes a projection baseline into the mem's `sync_state`, out of band
    // from B (advancing the shared mem ref with a config-only commit).
    a.set_mem_sync_state("specs", "engine/graph/source-tree#synced", "deadbeef", None)
        .expect("A writes sync_state");

    // Before the reload B still holds its boot snapshot — no baseline.
    assert!(
        b.mem_config_for("specs")
            .map(|c| c.sync_state.is_empty())
            .unwrap_or(true),
        "B has not yet observed A's out-of-band sync_state write",
    );

    // A per-mem reload of the destination mem picks up the new `sync_state`
    // value: it is mem-scoped state that rides the destination mem's config.
    b.reload_one_mem("specs").expect("B reloads specs");
    let synced = b
        .mem_config_for("specs")
        .and_then(|c| c.sync_state.get("engine/graph/source-tree#synced").cloned());
    assert_eq!(
        synced.as_deref(),
        Some("deadbeef"),
        "per-mem reload picks up the sibling's out-of-band sync_state write",
    );
}

/// D13 / AC11 — the advance/disposition store is **workspace-store** state read
/// fresh from disk per call, so it is reload-independent: an out-of-band write
/// to `.memstead/state/advance/` is visible via `read_advance_store` with no
/// engine reload, and a re-write is picked up on the next read (per-call fresh),
/// while the engine's reload machinery neither refreshes nor invalidates it.
#[test]
fn advance_store_is_reload_independent() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");

    // Absent → None, no reload involved.
    assert!(
        read_advance_store(tmp.path(), "specs", "graph")
            .unwrap()
            .is_none(),
    );

    // Out-of-band write (as a sibling `projection advance` would land it).
    let state = AdvanceState {
        binding: "specs/graph".to_string(),
        frozen_slice: Slice {
            added: vec!["a.rs".to_string()],
            modified: vec![],
            deleted: vec![],
        },
        dispositions: Default::default(),
        exclusions: Default::default(),
    };
    write_advance_store(tmp.path(), "specs", "graph", &state).unwrap();

    // Read fresh per call — visible immediately, with NO engine reload.
    let read1 = read_advance_store(tmp.path(), "specs", "graph")
        .unwrap()
        .expect("store present without any reload");
    assert_eq!(read1, state);

    // A reload does not refresh/invalidate the workspace-store advance state.
    engine.reload_if_stale(Some("specs"));
    let read2 = read_advance_store(tmp.path(), "specs", "graph")
        .unwrap()
        .expect("store still present after a reload");
    assert_eq!(
        read2, state,
        "advance store is independent of engine reload"
    );

    // A subsequent out-of-band rewrite is seen on the next per-call read —
    // proving the store is read fresh from disk, never cached across reload.
    let mut state2 = state.clone();
    state2
        .dispositions
        .insert("a.rs".to_string(), "worked".to_string());
    write_advance_store(tmp.path(), "specs", "graph", &state2).unwrap();
    let read3 = read_advance_store(tmp.path(), "specs", "graph")
        .unwrap()
        .expect("rewritten store present");
    assert_eq!(read3, state2, "per-call fresh read reflects the rewrite");
}

/// Batch per-entry notes survive on the git-branch backend
/// (backlog-sweep plan 05, decision 3): each batch family's ONE commit
/// carries the notes as `<id>: <note>` lines in its note record,
/// retrievable via `changes_since(...).notes` — where they previously
/// survived nowhere (the per-entry `append_provenance` route is a
/// documented no-op on git-branch). Complement: a batch with no notes
/// produces no note artifact at all.
#[test]
fn batch_per_entry_notes_survive_on_git_branch() {
    use indexmap::IndexMap;
    use memstead_base::RelateEntityArgs;
    use memstead_base::UpdateEntityArgs;

    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut e = engine_from_workspace_root(tmp.path()).expect("engine boots");
    let baseline = e
        .create_entity(
            create_args("specs", "Anchor Point"),
            Actor::Cli,
            Some(&client()),
            None,
        )
        .expect("baseline create")
        .write_id;

    // --- batch_create: one noted entry, one un-noted.
    let r = e
        .batch_create(
            vec![
                (
                    create_args("specs", "Alpha Noted"),
                    Some("why alpha".to_string()),
                ),
                (create_args("specs", "Beta Silent"), None),
            ],
            Actor::Cli,
            Some(&client()),
            false,
        )
        .expect("batch create succeeds");
    assert_eq!(r.succeeded, 2);
    let report = e.changes_since("specs", &baseline, None).expect("changes");
    let notes = report.notes.expect("git-branch mem surfaces notes");
    let create_note = notes
        .iter()
        .find(|n| {
            n.tool_verb.as_deref() == Some("batch_create") || n.subject.contains("batch-create")
        })
        .expect("the batch-create commit appears in the note stream");
    let text = create_note
        .note
        .as_deref()
        .expect("noted batch carries a note record");
    assert!(
        text.contains("specs--alpha-noted: why alpha"),
        "per-entry note attributed to its entry: {text:?}"
    );
    assert!(
        !text.contains("beta-silent"),
        "un-noted entry contributes no line: {text:?}"
    );

    // --- batch_update with a note; batch_relate with a note.
    let alpha_hash = e
        .get_entity(&EntityId("specs--alpha-noted".into()))
        .unwrap()
        .content_hash
        .clone();
    let upd = UpdateEntityArgs {
        anchors: Vec::new(),
        id: EntityId("specs--alpha-noted".into()),
        expected_hash: Some(alpha_hash),
        sections: IndexMap::from_iter([("identity".to_string(), "revised".to_string())]),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
        anchors_unset: Vec::new(),
    };
    let before_update = e.changes_since("specs", &baseline, None).unwrap().head;
    e.batch_update(
        vec![(upd, Some("revision rationale".to_string()))],
        Actor::Cli,
        Some(&client()),
        false,
    )
    .expect("batch update succeeds");
    let unotes = e
        .changes_since("specs", &before_update, None)
        .unwrap()
        .notes
        .expect("notes");
    let utext = unotes
        .iter()
        .filter_map(|n| n.note.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        utext.contains("specs--alpha-noted: revision rationale"),
        "batch_update note survives: {utext:?}"
    );

    let before_relate = e.changes_since("specs", &baseline, None).unwrap().head;
    e.batch_relate(
        vec![(
            RelateEntityArgs {
                source: EntityId("specs--alpha-noted".into()),
                expected_hash: None,
                rel_type: "USES".to_string(),
                target: EntityId("specs--beta-silent".into()),
                remove: false,
                description: None,
                dry_run: false,
            },
            Some("edge rationale".to_string()),
        )],
        Actor::Cli,
        Some(&client()),
        false,
    )
    .expect("batch relate succeeds");
    let rnotes = e
        .changes_since("specs", &before_relate, None)
        .unwrap()
        .notes
        .expect("notes");
    let rtext = rnotes
        .iter()
        .filter_map(|n| n.note.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rtext.contains("specs--alpha-noted: edge rationale"),
        "batch_relate note survives, keyed by the edge's source: {rtext:?}"
    );

    // --- Complement: a batch with NO notes produces no note record.
    let before_silent = e.changes_since("specs", &baseline, None).unwrap().head;
    e.batch_create(
        vec![(create_args("specs", "Gamma Quiet"), None)],
        Actor::Cli,
        Some(&client()),
        false,
    )
    .expect("silent batch succeeds");
    let snotes = e
        .changes_since("specs", &before_silent, None)
        .unwrap()
        .notes
        .expect("notes stream exists");
    let silent_commit = snotes
        .iter()
        .find(|n| n.subject.contains("batch-create"))
        .expect("the silent batch commit is in range");
    assert!(
        silent_commit.note.is_none(),
        "a batch with no notes produces no note artifact: {:?}",
        silent_commit.note
    );
}

/// Rot axis for UNSTAMPED seals (backlog-sweep plan 06, decision 19):
/// a schema sealed under an older engine — retired
/// `propagating_relationships` key, no install-provenance stamp —
/// keeps its mem running on the tolerant seal, and health surfaces a
/// low-tier `SCHEMA_UNSTAMPED_SOURCE_ROT` hint naming the condition
/// and the re-install remedy. Complement: an unstamped seal that still
/// passes current-language authoring validation produces no hint, and
/// the stamped-divergence axis stays silent throughout (its
/// no-false-positive contract is untouched — these pins carry no
/// stamp).
#[test]
fn unstamped_sealed_schema_rot_surfaces_as_low_tier_hint() {
    const MANIFEST: &str = r#"name: rotted
version: 0.1.0
description: sealed under an older engine
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
    const DOC_TYPE_CLEAN: &str = r#"name: doc
description: t
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

    let seal = |root: &std::path::Path, doc_yaml: &str| {
        init_real_mem_repo(root, &[("hold", "rotted@0.1.0")]);
        let gitdir = root.join("mem-repo").join(".git");
        // Simulate an old-engine seal: write the package onto the
        // `__MEMSTEAD:schemas/` ref as-given (no format marker, no
        // install-provenance stamp) — the state a pre-stamping install
        // left behind. The current install path refuses this content,
        // which is exactly why only health can surface it.
        memstead_git_branch::storage_memstead::write_schema_to_memstead_ref(
            &gitdir,
            "rotted",
            "0.1.0",
            &[
                ("schema.yaml".to_string(), MANIFEST.as_bytes().to_vec()),
                ("types/doc.yaml".to_string(), doc_yaml.as_bytes().to_vec()),
            ],
        )
        .expect("seal writes");
        engine_from_workspace_root(root).expect("tolerant boot keeps the mem running")
    };

    // Rotted content: the retired key refuses under the authoring tier.
    let rotted_doc = format!("{DOC_TYPE_CLEAN}propagating_relationships: []\n");
    let tmp = TempDir::new().unwrap();
    let engine = seal(tmp.path(), &rotted_doc);
    assert_eq!(engine.mem_names(), vec!["hold"], "the holding runs fine");
    let warnings = engine.health().warnings;
    let rot = warnings
        .iter()
        .find(|w| w.code() == "SCHEMA_UNSTAMPED_SOURCE_ROT")
        .expect("rotted unstamped seal must surface the low-tier hint");
    let payload = serde_json::to_value(rot).unwrap();
    assert_eq!(payload["details"]["schema_ref"], "rotted@0.1.0");
    assert_eq!(payload["details"]["mems"], serde_json::json!(["hold"]));
    assert!(
        rot.message().contains("schema install"),
        "the hint names the stamping remedy: {}",
        rot.message()
    );
    assert!(
        !warnings
            .iter()
            .any(|w| w.code().starts_with("SCHEMA_AUTHORING_SOURCE_")),
        "the stamped-divergence axis must stay silent for unstamped pins"
    );

    // Complement: a clean unstamped seal parses under the authoring
    // tier — no hint of any kind.
    let tmp2 = TempDir::new().unwrap();
    let engine2 = seal(tmp2.path(), DOC_TYPE_CLEAN);
    let warnings2 = engine2.health().warnings;
    assert!(
        !warnings2
            .iter()
            .any(|w| w.code() == "SCHEMA_UNSTAMPED_SOURCE_ROT"
                || w.code().starts_with("SCHEMA_AUTHORING_SOURCE_")),
        "a parsing unstamped seal produces no rot or drift finding: {:?}",
        warnings2.iter().map(|w| w.code()).collect::<Vec<_>>()
    );
}

/// Plan 07 complement: the git-branch backend's mem-repo is
/// engine-managed and cannot acquire merge conflicts through supported
/// use — `conflicts` operations refuse typed there instead of
/// pretending applicability.
#[test]
fn conflict_operations_refuse_on_git_branch_backend() {
    use memstead_base::engine::conflicts::ConflictSide;

    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");

    let err = engine.list_merge_conflicts(Some("specs")).unwrap_err();
    assert_eq!(err.code(), "CONFLICT_RESOLVE_UNSUPPORTED_BACKEND");

    let err = engine
        .resolve_merge_conflict(
            &EntityId("specs--anything".into()),
            ConflictSide::Ours,
            Actor::Cli,
            Some(&client()),
            None,
        )
        .unwrap_err();
    assert_eq!(err.code(), "CONFLICT_RESOLVE_UNSUPPORTED_BACKEND");

    // The unscoped sweep simply reports nothing conflicted — the
    // git-branch mount is not applicable, not an error.
    assert!(engine.list_merge_conflicts(None).unwrap().is_empty());
}
