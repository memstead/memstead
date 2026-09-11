#![cfg(test)]

use super::*;
use crate::vcs::{Actor, ClientId, CommitContext};
use tempfile::TempDir;

fn ctx_for_test<'a>() -> CommitContext<'a> {
    CommitContext {
        actor: Actor::Cli,
        client: Some(ClientId {
            name: "claude-code".to_string(),
            version: "0.1.0".to_string(),
        }),
        tool: Some("test"),
        note: None,
        role: Default::default(),
        identity: None,
        logical_operation_id: None,
        entity_ids: None,
    }
}

/// `entity_exists` observes the same pending-buffer precedence as
/// `read_entity`: staged upsert → true before any commit, staged
/// delete → false while the file still sits on disk, and the
/// committed state answers between transactions — all without
/// reading bytes (flywheel W7/02 primitive).
#[test]
fn entity_exists_metadata_probe_and_pending_precedence() {
    use crate::backend::MemBackend;
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    assert!(!MemBackend::entity_exists(&writer, Path::new("notes/a.md")).unwrap());

    MemBackend::write_entity(
        &writer,
        Path::new("notes/a.md"),
        b"# a
",
    )
    .unwrap();
    assert!(
        MemBackend::entity_exists(&writer, Path::new("notes/a.md")).unwrap(),
        "staged upsert answers true before commit"
    );

    MemBackend::commit(&writer, "land a", &ctx_for_test()).unwrap();
    assert!(MemBackend::entity_exists(&writer, Path::new("notes/a.md")).unwrap());

    MemBackend::delete_entity(&writer, Path::new("notes/a.md")).unwrap();
    assert!(
        !MemBackend::entity_exists(&writer, Path::new("notes/a.md")).unwrap(),
        "staged delete answers false while the file is still on disk"
    );
}

#[test]
fn write_then_commit_round_trip() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    writer
        .write_entity(Path::new("notes/hello.md"), b"# hi\n")
        .unwrap();
    let id = writer.commit("first commit", &ctx_for_test()).unwrap();
    assert!(!id.is_empty());

    let bytes = std::fs::read(tmp.path().join("notes/hello.md")).unwrap();
    assert_eq!(bytes, b"# hi\n");
}

#[test]
fn delete_removes_path() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    writer.write_entity(Path::new("a.md"), b"a").unwrap();
    writer.write_entity(Path::new("b.md"), b"b").unwrap();
    writer.commit("seed", &ctx_for_test()).unwrap();

    writer.delete_entity(Path::new("a.md")).unwrap();
    writer.commit("drop a", &ctx_for_test()).unwrap();

    assert!(!tmp.path().join("a.md").exists());
    assert!(tmp.path().join("b.md").exists());
}

#[test]
fn delete_of_missing_path_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    writer.delete_entity(Path::new("never-existed.md")).unwrap();
    writer.commit("noop delete", &ctx_for_test()).unwrap();
}

#[test]
fn move_renames_path() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    writer
        .write_entity(Path::new("from.md"), b"payload")
        .unwrap();
    writer.commit("seed", &ctx_for_test()).unwrap();

    writer
        .move_entity(Path::new("from.md"), Path::new("nested/to.md"))
        .unwrap();
    writer.commit("rename", &ctx_for_test()).unwrap();

    assert!(!tmp.path().join("from.md").exists());
    let moved = std::fs::read(tmp.path().join("nested/to.md")).unwrap();
    assert_eq!(moved, b"payload");
}

#[test]
fn move_with_pending_upsert_carries_bytes() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    writer.write_entity(Path::new("a.md"), b"alpha").unwrap();
    writer
        .move_entity(Path::new("a.md"), Path::new("b.md"))
        .unwrap();
    writer.commit("write+move", &ctx_for_test()).unwrap();

    assert!(!tmp.path().join("a.md").exists());
    assert_eq!(std::fs::read(tmp.path().join("b.md")).unwrap(), b"alpha");
}

#[test]
fn move_missing_source_errors() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    let err = writer
        .move_entity(Path::new("ghost.md"), Path::new("here.md"))
        .unwrap_err();
    assert!(matches!(err, BackendError::Path(_)));
}

#[test]
fn move_with_pending_target_upsert_errors() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    writer.write_entity(Path::new("from.md"), b"x").unwrap();
    writer.write_entity(Path::new("to.md"), b"y").unwrap();
    let err = writer
        .move_entity(Path::new("from.md"), Path::new("to.md"))
        .unwrap_err();
    assert!(matches!(err, BackendError::Path(_)));
}

#[test]
fn multi_op_commit() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    writer.write_entity(Path::new("doomed.md"), b"x").unwrap();
    writer.commit("seed", &ctx_for_test()).unwrap();

    writer.write_entity(Path::new("a.md"), b"alpha").unwrap();
    writer
        .write_entity(Path::new("nested/b.md"), b"beta")
        .unwrap();
    writer.delete_entity(Path::new("doomed.md")).unwrap();
    writer.commit("multi-op", &ctx_for_test()).unwrap();

    assert!(!tmp.path().join("doomed.md").exists());
    assert_eq!(std::fs::read(tmp.path().join("a.md")).unwrap(), b"alpha");
    assert_eq!(
        std::fs::read(tmp.path().join("nested/b.md")).unwrap(),
        b"beta"
    );
}

#[test]
fn rejects_path_traversal() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    let err = writer
        .write_entity(Path::new("../escape.md"), b"x")
        .unwrap_err();
    assert!(matches!(err, BackendError::Path(_)));
}

#[test]
fn rejects_absolute_path() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    let err = writer
        .write_entity(Path::new("/etc/passwd"), b"x")
        .unwrap_err();
    assert!(matches!(err, BackendError::Path(_)));
}

#[test]
fn rejects_empty_path() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    let err = writer.write_entity(Path::new(""), b"x").unwrap_err();
    assert!(matches!(err, BackendError::Path(_)));
}

#[test]
fn write_overwrites_existing_file() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    writer.write_entity(Path::new("a.md"), b"first").unwrap();
    writer.commit("c1", &ctx_for_test()).unwrap();
    writer.write_entity(Path::new("a.md"), b"second").unwrap();
    writer.commit("c2", &ctx_for_test()).unwrap();

    assert_eq!(std::fs::read(tmp.path().join("a.md")).unwrap(), b"second");
}

#[test]
fn no_temp_files_left_after_commit() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    writer.write_entity(Path::new("a.md"), b"a").unwrap();
    writer.write_entity(Path::new("nested/b.md"), b"b").unwrap();
    writer.commit("c", &ctx_for_test()).unwrap();

    // Walk the tree and ensure no `.tmp.` artefacts survived.
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(tmp.path(), &mut files);
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy();
        assert!(
            !name.contains(".tmp."),
            "stray temp file after commit: {}",
            f.display()
        );
    }
}

#[test]
fn commit_id_is_unique_across_calls() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    writer.write_entity(Path::new("a.md"), b"a").unwrap();
    let id1 = writer.commit("c1", &ctx_for_test()).unwrap();
    writer.write_entity(Path::new("b.md"), b"b").unwrap();
    let id2 = writer.commit("c2", &ctx_for_test()).unwrap();

    assert_ne!(id1, id2);
}

#[test]
fn pending_buffer_clears_on_commit() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    writer.write_entity(Path::new("a.md"), b"a").unwrap();
    writer.commit("c1", &ctx_for_test()).unwrap();
    // A second commit with no mutations writes nothing new but
    // still returns a fresh id.
    let id2 = writer.commit("noop", &ctx_for_test()).unwrap();
    assert!(!id2.is_empty());
    // a.md still has its original content (no zombie pending op).
    assert_eq!(std::fs::read(tmp.path().join("a.md")).unwrap(), b"a");
}

// --- MemBackend impl ----------------------------------------

/// Folder backend's `write_mem_config` writes
/// `<root>/.memstead/config.json`, creating the umbrella directory
/// if needed. The subsequent `read_mem_config` round-trips the
/// bytes.
#[test]
fn backend_write_mem_config_round_trips_via_read() {
    use crate::backend::MemBackend;

    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    let backend: &dyn MemBackend = &writer;

    // No config yet — read returns None.
    assert!(backend.read_mem_config().unwrap().is_none());

    // Write — creates `.memstead/` umbrella + the config blob.
    let bytes = br#"{"version":"0.1.0","schema":"default@1.0.0"}"#.to_vec();
    backend
        .write_mem_config(
            &bytes,
            &crate::vcs::CommitContext::new(
                Some("test"),
                crate::vcs::Actor::Cli,
                None,
                None,
                crate::vcs::Role::Unspecified,
                None,
            ),
        )
        .unwrap();

    // Read returns the same bytes.
    let read_back = backend.read_mem_config().unwrap();
    assert_eq!(read_back, Some(bytes.clone()));
    // Config file lands at `<root>/.memstead/config.json`.
    let on_disk = std::fs::read(tmp.path().join(".memstead/config.json")).unwrap();
    assert_eq!(on_disk, bytes);
}

#[test]
fn backend_list_entities_returns_only_md_outside_meta_dirs() {
    use crate::backend::MemBackend;

    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    // With both traits in scope, dot-syntax `writer.foo(...)`
    // is ambiguous — seed via fully-qualified MemBackend calls.
    <FilesystemBackend as MemBackend>::write_entity(&writer, Path::new("a.md"), b"a").unwrap();
    <FilesystemBackend as MemBackend>::write_entity(&writer, Path::new("nested/b.md"), b"b")
        .unwrap();
    <FilesystemBackend as MemBackend>::write_entity(&writer, Path::new("notes.json"), b"{}")
        .unwrap();
    <FilesystemBackend as MemBackend>::commit(&writer, "seed", &ctx_for_test()).unwrap();
    // The current `.memstead/` meta dir is skipped by the walker.
    // An ordinary dot-dir (`.other/`) is not special, so markdown
    // under it is walked like any other non-meta path.
    std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
    std::fs::write(tmp.path().join(".memstead/config.json"), b"{}").unwrap();
    std::fs::write(tmp.path().join(".memstead/notes.md"), b"#").unwrap();
    std::fs::create_dir_all(tmp.path().join(".other")).unwrap();
    std::fs::write(tmp.path().join(".other/notes.md"), b"#").unwrap();

    let backend: &dyn MemBackend = &writer;
    let mut paths: Vec<String> = backend
        .list_entities()
        .unwrap()
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            ".other/notes.md".to_string(),
            "a.md".to_string(),
            "nested/b.md".to_string(),
        ]
    );
}

#[test]
fn backend_read_entity_consults_pending_then_disk() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());

    // Seed via fully-qualified MemBackend calls — the trait exposes
    // `write_entity`; importing `MemBackend` later in the test
    // makes the dot-syntax ambiguous, so we route the seed
    // through the trait that's still implicitly in scope here.
    <FilesystemBackend as MemBackend>::write_entity(&writer, Path::new("on_disk.md"), b"disk")
        .unwrap();
    <FilesystemBackend as MemBackend>::commit(&writer, "seed", &ctx_for_test()).unwrap();

    use crate::backend::MemBackend;
    let backend: &dyn MemBackend = &writer;
    // Disk path → reads from disk.
    assert_eq!(
        backend.read_entity(Path::new("on_disk.md")).unwrap(),
        Some(b"disk".to_vec())
    );
    // Buffered upsert wins over disk.
    backend
        .write_entity(Path::new("on_disk.md"), b"buffered")
        .unwrap();
    assert_eq!(
        backend.read_entity(Path::new("on_disk.md")).unwrap(),
        Some(b"buffered".to_vec())
    );
    // Buffered delete masks disk.
    backend.delete_entity(Path::new("on_disk.md")).unwrap();
    assert_eq!(backend.read_entity(Path::new("on_disk.md")).unwrap(), None);
    // Unknown path → None (idempotent).
    assert_eq!(backend.read_entity(Path::new("never.md")).unwrap(), None);
}

#[test]
fn backend_provenance_round_trips_through_jsonl() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    use crate::backend::MemBackend;
    let backend: &dyn MemBackend = &writer;

    let client = ClientId {
        name: "claude-code".into(),
        version: "2.1.0".into(),
    };
    let earlier = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let later = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_777_077_296);

    backend
        .append_provenance(&Provenance::new(
            earlier,
            ProvenanceKind::Create,
            Some("v:e1".into()),
            Actor::Agent,
            Some(client.clone()),
            Some("first".into()),
        ))
        .unwrap();
    backend
        .append_provenance(&Provenance::new(
            later,
            ProvenanceKind::Update,
            Some("v:e1".into()),
            Actor::Cli,
            None,
            None,
        ))
        .unwrap();

    let all = backend.read_provenance(None).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].kind, ProvenanceKind::Create);
    assert_eq!(all[0].entity.as_deref(), Some("v:e1"));
    assert_eq!(all[0].actor, Actor::Agent);
    assert_eq!(all[0].note.as_deref(), Some("first"));
    assert_eq!(
        all[0]
            .client
            .as_ref()
            .map(|c| (c.name.as_str(), c.version.as_str())),
        Some(("claude-code", "2.1.0"))
    );
    assert_eq!(all[0].timestamp, earlier);
    assert_eq!(all[1].kind, ProvenanceKind::Update);
    assert_eq!(all[1].timestamp, later);
    assert!(all[1].note.is_none());
    assert!(all[1].client.is_none());
}

#[test]
fn backend_provenance_cursor_filters_by_timestamp() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    use crate::backend::MemBackend;
    let backend: &dyn MemBackend = &writer;

    for (secs, label) in [
        (1_700_000_000u64, "first"),
        (1_750_000_000, "middle"),
        (1_800_000_000, "last"),
    ] {
        backend
            .append_provenance(&Provenance::new(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
                ProvenanceKind::Create,
                Some(format!("v:{label}")),
                Actor::Cli,
                None,
                None,
            ))
            .unwrap();
    }
    // Cursor between first and middle should drop the first entry.
    let cursor = changelog::format_rfc3339_utc(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_725_000_000),
    );
    let after = backend.read_provenance(Some(&cursor)).unwrap();
    let entities: Vec<_> = after.iter().filter_map(|p| p.entity.clone()).collect();
    assert_eq!(entities, vec!["v:middle".to_string(), "v:last".to_string()]);
}

#[test]
fn backend_read_provenance_handles_missing_log() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    use crate::backend::MemBackend;
    let backend: &dyn MemBackend = &writer;
    // No mutations yet → no `.memstead/changes.jsonl` → empty result, no error.
    assert!(backend.read_provenance(None).unwrap().is_empty());
}

// ---- folder JSONL synthesis (memstead_base::ops::folder_changes_since) ----

/// Helper: append a single Provenance event with an explicit
/// timestamp, so tests get deterministic JSONL ordering.
fn append_at(
    backend: &dyn crate::backend::MemBackend,
    secs: u64,
    kind: ProvenanceKind,
    entity: &str,
) {
    backend
        .append_provenance(&Provenance::new(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
            kind,
            Some(entity.to_string()),
            Actor::Cli,
            None,
            None,
        ))
        .unwrap();
}

#[test]
fn folder_changes_since_no_log_returns_empty_at_cursor() {
    // Fresh mem, no `.memstead/changes.jsonl` → empty BackendChanges
    // with `head` echoing the cursor.
    let tmp = TempDir::new().unwrap();
    let result =
        crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA, None)
            .unwrap();
    assert_eq!(result.since, crate::ops::EMPTY_TREE_SHA);
    assert_eq!(result.head, crate::ops::EMPTY_TREE_SHA);
    assert!(result.changes.is_empty());
}

/// A rename row names only the id the entity now carries; with the
/// store's view the replay pairs it with the id that vanished. Added
/// and renamed inside one window: one `added` under the final id.
/// Rename alone in the window: one `renamed` with both ids. An
/// entity created in the window that still exists keeps its `added`.
#[test]
fn folder_changes_since_pairs_a_rename_row_with_the_vanished_id() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    append_at(
        &writer,
        1_700_000_000,
        ProvenanceKind::Create,
        "specs--alpha",
    );
    append_at(
        &writer,
        1_700_000_100,
        ProvenanceKind::Create,
        "specs--keeper",
    );
    append_at(
        &writer,
        1_700_000_200,
        ProvenanceKind::Rename,
        "specs--alpha-two",
    );
    // The store after the rename: alpha is gone, alpha-two carries
    // alpha's creation instant, keeper is untouched.
    let store = |id: &crate::EntityId| -> Option<String> {
        match id.0.as_str() {
            "specs--alpha-two" => Some("2023-11-14T22:13:20Z".to_string()),
            "specs--keeper" => Some("2023-11-14T22:15:00Z".to_string()),
            _ => None,
        }
    };

    let whole = crate::ops::folder_changes_since(
        tmp.path(),
        "specs",
        crate::ops::EMPTY_TREE_SHA,
        Some(&store),
    )
    .unwrap();
    let kinds: Vec<(&str, &str)> = whole
        .changes
        .iter()
        .map(|c| (c.action(), c.primary_id()))
        .collect();
    assert_eq!(
        kinds,
        vec![("added", "specs--alpha-two"), ("added", "specs--keeper")],
        "add-then-rename folds to one added under the final id: {:?}",
        whole.changes
    );

    // Window opens after the add and before the rename.
    let mid = crate::filesystem::changelog::format_rfc3339_utc(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_150),
    );
    let later = crate::ops::folder_changes_since(tmp.path(), "specs", &mid, Some(&store)).unwrap();
    match later.changes.as_slice() {
        [crate::ops::ChangeEnvelope::Renamed { from_id, to_id, .. }] => {
            assert_eq!(from_id.0, "specs--alpha");
            assert_eq!(to_id.0, "specs--alpha-two");
        }
        other => panic!("expected the renamed event alone, got {other:?}"),
    }

    // Without the store's view the rename row reads as updated.
    let bare = crate::ops::folder_changes_since(tmp.path(), "specs", &mid, None).unwrap();
    assert_eq!(bare.changes.len(), 1);
    assert_eq!(bare.changes[0].action(), "updated");

    // A chain: alpha-two -> alpha-three -> alpha-four. Only the
    // final name exists now. The whole window folds to one added
    // under it; a reader holding alpha-two gets one renamed from
    // alpha-two to alpha-four; a reader holding alpha-three gets
    // one renamed from alpha-three; the intermediates surface
    // nowhere.
    append_at(
        &writer,
        1_700_000_300,
        ProvenanceKind::Rename,
        "specs--alpha-three",
    );
    append_at(
        &writer,
        1_700_000_400,
        ProvenanceKind::Rename,
        "specs--alpha-four",
    );
    let store = |id: &crate::EntityId| -> Option<String> {
        match id.0.as_str() {
            "specs--alpha-four" => Some("2023-11-14T22:13:20Z".to_string()),
            "specs--keeper" => Some("2023-11-14T22:15:00Z".to_string()),
            _ => None,
        }
    };
    let at = |secs: u64| {
        crate::filesystem::changelog::format_rfc3339_utc(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
        )
    };
    let whole = crate::ops::folder_changes_since(
        tmp.path(),
        "specs",
        crate::ops::EMPTY_TREE_SHA,
        Some(&store),
    )
    .unwrap();
    let kinds: Vec<(&str, &str)> = whole
        .changes
        .iter()
        .map(|c| (c.action(), c.primary_id()))
        .collect();
    assert_eq!(
        kinds,
        vec![("added", "specs--alpha-four"), ("added", "specs--keeper")],
        "{:?}",
        whole.changes
    );
    for (cursor, from) in [
        (1_700_000_250, "specs--alpha-two"),
        (1_700_000_350, "specs--alpha-three"),
    ] {
        let part = crate::ops::folder_changes_since(tmp.path(), "specs", &at(cursor), Some(&store))
            .unwrap();
        match part.changes.as_slice() {
            [crate::ops::ChangeEnvelope::Renamed { from_id, to_id, .. }] => {
                assert_eq!(from_id.0, from);
                assert_eq!(to_id.0, "specs--alpha-four");
            }
            other => panic!("expected one renamed from {from}, got {other:?}"),
        }
    }
}

#[test]
fn folder_changes_since_create_only_yields_added_envelope() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    append_at(
        &writer,
        1_700_000_000,
        ProvenanceKind::Create,
        "specs--alpha",
    );

    let result =
        crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA, None)
            .unwrap();
    assert_eq!(result.changes.len(), 1);
    match &result.changes[0] {
        crate::ops::ChangeEnvelope::Added {
            id,
            title,
            entity_type,
        } => {
            assert_eq!(id.0, "specs--alpha");
            assert!(title.is_none(), "id-only contract");
            assert!(entity_type.is_none(), "id-only contract");
        }
        other => panic!("expected Added, got {other:?}"),
    }
    // head advances to the event's timestamp.
    assert_ne!(result.head, crate::ops::EMPTY_TREE_SHA);
}

#[test]
fn folder_changes_since_create_then_delete_cancels_to_no_envelope() {
    // Within the cursor window, an entity that was created and then
    // deleted nets out to Removed (final state wins for Delete).
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    append_at(
        &writer,
        1_700_000_000,
        ProvenanceKind::Create,
        "specs--ephemeral",
    );
    append_at(
        &writer,
        1_700_000_001,
        ProvenanceKind::Delete,
        "specs--ephemeral",
    );

    let result =
        crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA, None)
            .unwrap();
    assert_eq!(result.changes.len(), 1);
    match &result.changes[0] {
        crate::ops::ChangeEnvelope::Removed { id, .. } => {
            assert_eq!(id.0, "specs--ephemeral");
        }
        other => panic!("expected Removed (Delete wins), got {other:?}"),
    }
}

#[test]
fn folder_changes_since_update_only_yields_updated_envelope() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    append_at(
        &writer,
        1_700_000_000,
        ProvenanceKind::Update,
        "specs--alpha",
    );

    let result =
        crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA, None)
            .unwrap();
    assert_eq!(result.changes.len(), 1);
    assert!(matches!(
        result.changes[0],
        crate::ops::ChangeEnvelope::Updated { .. }
    ));
}

/// The exact confusion the guard exists for: a mutation's
/// `write_id` (fixed-width hex, sorts below every timestamp)
/// passed back as `since` must refuse, never silently replay the
/// whole history. Garbage refuses on the same rule; the empty
/// string and the empty-tree sentinel stay "from the beginning".
#[test]
fn folder_changes_since_refuses_non_timestamp_cursor() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    append_at(
        &writer,
        1_700_000_000,
        ProvenanceKind::Create,
        "specs--alpha",
    );

    let write_token = make_commit_id();
    for bad in [write_token.as_str(), "not-a-timestamp"] {
        let err = crate::ops::folder_changes_since(tmp.path(), "specs", bad, None).unwrap_err();
        match err {
            BackendError::Other(msg) => {
                assert_eq!(msg, format!("INVALID_TS_CURSOR:{bad}"));
            }
            other => panic!("expected typed marker, got {other:?}"),
        }
    }
    for from_start in ["", crate::ops::EMPTY_TREE_SHA] {
        let ok = crate::ops::folder_changes_since(tmp.path(), "specs", from_start, None).unwrap();
        assert_eq!(ok.changes.len(), 1, "sentinel '{from_start}' reads all");
    }
}

#[test]
fn folder_changes_since_cursor_filters_to_window() {
    // Three events at three timestamps; cursor between first and
    // second drops the first event from the window.
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    append_at(
        &writer,
        1_700_000_000,
        ProvenanceKind::Create,
        "specs--first",
    );
    append_at(
        &writer,
        1_750_000_000,
        ProvenanceKind::Create,
        "specs--middle",
    );
    append_at(
        &writer,
        1_800_000_000,
        ProvenanceKind::Create,
        "specs--last",
    );

    let cursor = changelog::format_rfc3339_utc(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_725_000_000),
    );
    let result = crate::ops::folder_changes_since(tmp.path(), "specs", &cursor, None).unwrap();
    assert_eq!(result.changes.len(), 2);
    let ids: Vec<_> = result
        .changes
        .iter()
        .map(|e| match e {
            crate::ops::ChangeEnvelope::Added { id, .. } => id.0.clone(),
            _ => panic!("expected Added"),
        })
        .collect();
    // BTreeMap iteration order — ids sort lexicographically.
    assert_eq!(
        ids,
        vec!["specs--last".to_string(), "specs--middle".to_string()]
    );
}

#[test]
fn folder_changes_since_skips_events_for_other_mems() {
    // Defensive: changelog drift could carry events for another
    // mem prefix; the impl filters them out so envelopes only
    // surface for the queried mem.
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    append_at(
        &writer,
        1_700_000_000,
        ProvenanceKind::Create,
        "specs--mine",
    );
    append_at(
        &writer,
        1_700_000_001,
        ProvenanceKind::Create,
        "other--theirs",
    );

    let result =
        crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA, None)
            .unwrap();
    assert_eq!(result.changes.len(), 1);
    match &result.changes[0] {
        crate::ops::ChangeEnvelope::Added { id, .. } => {
            assert_eq!(id.0, "specs--mine");
        }
        other => panic!("expected Added, got {other:?}"),
    }
}

#[test]
fn folder_changes_since_skips_batch_events_with_no_entity() {
    // Batch events have entity=null. They don't surface as
    // envelopes (no per-entity id to attach to).
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    use crate::backend::MemBackend;
    // append_at requires an entity; use append_provenance directly
    // for the batch-with-no-entity case.
    writer
        .append_provenance(&Provenance::new(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
            ProvenanceKind::Batch,
            None,
            Actor::Cli,
            None,
            None,
        ))
        .unwrap();
    append_at(
        &writer,
        1_700_000_001,
        ProvenanceKind::Create,
        "specs--real",
    );

    let result =
        crate::ops::folder_changes_since(tmp.path(), "specs", crate::ops::EMPTY_TREE_SHA, None)
            .unwrap();
    // Only the Create-Real event surfaces; the batch is dropped.
    assert_eq!(result.changes.len(), 1);
    match &result.changes[0] {
        crate::ops::ChangeEnvelope::Added { id, .. } => {
            assert_eq!(id.0, "specs--real");
        }
        other => panic!("expected Added, got {other:?}"),
    }
}

#[test]
fn folder_changes_since_head_echoes_cursor_when_no_events_in_window() {
    // Events exist but all before the cursor → head echoes cursor.
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    append_at(&writer, 1_700_000_000, ProvenanceKind::Create, "specs--old");

    let cursor = changelog::format_rfc3339_utc(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_900_000_000),
    );
    let result = crate::ops::folder_changes_since(tmp.path(), "specs", &cursor, None).unwrap();
    assert!(result.changes.is_empty());
    assert_eq!(result.head, cursor);
}

#[test]
fn backend_writes_delegate_to_memwriter() {
    let tmp = TempDir::new().unwrap();
    let writer = FilesystemBackend::new(tmp.path().to_path_buf());
    use crate::backend::MemBackend;
    let backend: &dyn MemBackend = &writer;

    backend.write_entity(Path::new("a.md"), b"alpha").unwrap();
    backend.commit("seed", &ctx_for_test()).unwrap();
    assert_eq!(std::fs::read(tmp.path().join("a.md")).unwrap(), b"alpha");

    backend.delete_entity(Path::new("a.md")).unwrap();
    backend.commit("drop", &ctx_for_test()).unwrap();
    assert!(!tmp.path().join("a.md").exists());
}

#[test]
fn parse_client_id_splits_on_last_at() {
    let c = parse_client_id("claude-code@2.1.0").unwrap();
    assert_eq!(c.name, "claude-code");
    assert_eq!(c.version, "2.1.0");
    // Edge: name with `.`
    let c = parse_client_id("foo.bar@1.0").unwrap();
    assert_eq!(c.name, "foo.bar");
    // Bare strings without `@` → None (forward-compat: tolerant
    // readers ignore rather than mis-construct).
    assert!(parse_client_id("naked").is_none());
    assert!(parse_client_id("@1.0").is_none());
    assert!(parse_client_id("name@").is_none());
}
