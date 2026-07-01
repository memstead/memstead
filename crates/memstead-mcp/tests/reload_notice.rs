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
        }
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
    a.create_entity(create_args("specs", "Entity A"), Actor::Cli, Some(&client()), None)
        .expect("A create succeeds");

    // B, still cached at the pre-A head, creates a distinct entity. The
    // reload-before-operation check must pull A's commit in first (so
    // B's graph holds E_a) and stash a `mem_changed` notice.
    b.create_entity(create_args("specs", "Entity B"), Actor::Cli, Some(&client()), None)
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
                entries.iter().any(|e| e.primary_id() == "specs--entity-a"
                    && e.action() == "added"),
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

    a.create_entity(create_args("specs", "Entity One"), Actor::Cli, Some(&client()), None)
        .expect("create one");
    assert!(
        a.take_mem_changed_notices().is_empty(),
        "first op has nothing to reload past",
    );

    // A second op by the same engine: its own prior commit advanced the
    // cached head via record_self_write, so reload-before-op sees
    // cached == live and does not reload.
    a.create_entity(create_args("specs", "Entity Two"), Actor::Cli, Some(&client()), None)
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
    a.create_entity(create_args("specs", "Shared X"), Actor::Cli, Some(&client()), None)
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
    let fresh_hash = b.get_entity(&x).expect("B still knows X").content_hash.clone();
    assert_ne!(
        fresh_hash, stale_hash,
        "B's read sees A's fresh content, not the stale boot snapshot",
    );

    let notices = b.take_mem_changed_notices();
    assert_eq!(notices.len(), 1, "the read-triggered reload stashed one notice");
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
    a.create_entity(create_args("specs", "Shared X"), Actor::Cli, Some(&client()), None)
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
    a.create_entity(create_args("specs", "Entity X"), Actor::Cli, Some(&client()), None)
        .expect("A create X");
    a.create_entity(create_args("specs", "Entity Y"), Actor::Cli, Some(&client()), None)
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
