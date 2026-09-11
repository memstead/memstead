//! The cursor at the crate boundary: over a real git work tree the
//! source cursor reseeds without a baseline, slices exactly the files
//! that moved past the recorded baseline, and reports quiescence once
//! the baseline is current; a facet without a scope reports the
//! no-signal reason instead of an empty slice.

mod common;

use memstead_base::Engine;
use memstead_projection::slice::NoSignalReason;
use memstead_projection::{compute_source_cursor, source_moved};

use common::{FACET, MEM, commit_all, workspace, write_source};

#[test]
fn the_git_cursor_reseeds_then_slices_the_moved_files_then_rests() {
    let fixture = workspace();
    let root = fixture.root();
    write_source(root, "src/a.rs", "one\n");
    write_source(root, "src/b.rs", "one\n");
    write_source(root, "notes.md", "outside the scope\n");
    let first = commit_all(root, "base");
    let (_, resolved) = common::write_graph_binding(&fixture, &["src/**/*.rs"]);
    let mut engine = Engine::from_workspace_root(root).unwrap();

    // No baseline: nothing to diff against, so the cursor asks the
    // caller to seed the baseline at HEAD and presents no slice.
    let cursor = compute_source_cursor(&engine, &resolved, root);
    assert_eq!(cursor.binding_id, common::BINDING);
    assert_eq!(cursor.dest_mem, MEM);
    assert!(cursor.union.added.is_empty() && cursor.union.modified.is_empty());
    assert_eq!(cursor.reseed.len(), 1);
    assert_eq!(cursor.reseed[0].key, common::synced_key());
    assert_eq!(cursor.reseed[0].token, first);
    assert!(cursor.no_signal.is_empty());
    assert!(
        !source_moved(&engine, &resolved, root),
        "a first sync never counts as movement"
    );

    // Baseline at the first commit; the tree moves past it.
    engine
        .set_mem_sync_state(MEM, &common::synced_key(), &first, None)
        .unwrap();
    write_source(root, "src/b.rs", "two\n");
    write_source(root, "src/c.rs", "new\n");
    write_source(root, "notes.md", "still outside\n");
    std::fs::remove_file(root.join("src/a.rs")).unwrap();
    let second = commit_all(root, "move");

    assert!(source_moved(&engine, &resolved, root));
    let cursor = compute_source_cursor(&engine, &resolved, root);
    assert_eq!(cursor.union.added, vec!["src/c.rs".to_string()]);
    assert_eq!(cursor.union.modified, vec!["src/b.rs".to_string()]);
    assert_eq!(cursor.union.deleted, vec!["src/a.rs".to_string()]);
    assert!(cursor.any_changes);
    assert!(!cursor.degraded);
    assert!(cursor.reseed.is_empty());
    assert_eq!(cursor.write_commands.len(), 1);
    assert_eq!(cursor.write_commands[0].key, common::synced_key());
    assert_eq!(cursor.write_commands[0].token, second);

    // Baseline caught up: quiescent.
    engine
        .set_mem_sync_state(MEM, &common::synced_key(), &second, None)
        .unwrap();
    assert!(!source_moved(&engine, &resolved, root));
    let cursor = compute_source_cursor(&engine, &resolved, root);
    assert!(!cursor.any_changes);
    assert!(cursor.union.added.is_empty());
    assert!(cursor.union.modified.is_empty());
    assert!(cursor.union.deleted.is_empty());
    assert!(cursor.write_commands.is_empty());
}

#[test]
fn a_facet_without_a_scope_reports_the_reason_instead_of_an_empty_slice() {
    let fixture = workspace();
    let root = fixture.root();
    write_source(root, "src/a.rs", "one\n");
    let first = commit_all(root, "base");
    let (_, resolved) = common::write_graph_binding(&fixture, &[]);
    let mut engine = Engine::from_workspace_root(root).unwrap();
    engine
        .set_mem_sync_state(MEM, &common::synced_key(), &first, None)
        .unwrap();
    write_source(root, "src/b.rs", "two\n");
    commit_all(root, "move");

    let cursor = compute_source_cursor(&engine, &resolved, root);
    assert!(cursor.union.added.is_empty());
    assert!(!cursor.any_changes);
    assert_eq!(cursor.no_signal.len(), 1);
    assert_eq!(cursor.no_signal[0].source, FACET);
    assert_eq!(cursor.no_signal[0].reason, NoSignalReason::Unscoped);
}
