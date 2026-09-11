#![cfg(test)]

use super::*;
use crate::vcs::{Actor, ClientId, CommitContext};
use std::path::Path;
use tempfile::TempDir;

/// Build a fresh bare repo at `<tmp>/mem-repo.git` and return the
/// canonical gitdir path. Tests open the repo per-call via
/// `gix::open` so the writer's `gitdir + ref_name` shape is what
/// gets exercised.
fn fresh_repo_dir(tmp: &Path) -> PathBuf {
    let git_dir = tmp.join("mem-repo.git");
    gix::init_bare(&git_dir).unwrap();
    std::fs::canonicalize(&git_dir).unwrap()
}

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

/// `storage_present` answers for the ref, not the entity count: a
/// branch that was never created is absent (and `list_entities`
/// still says empty, which is the ambiguity boot's `MOUNT_UNBACKED`
/// probe exists to resolve); after the first commit it is present.
#[test]
fn storage_present_tracks_the_branch_ref() {
    use memstead_base::backend::MemBackend;
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir, "refs/heads/probe".to_string());
    assert!(!MemBackend::storage_present(&writer).unwrap(), "no ref yet");
    assert!(
        MemBackend::list_entities(&writer).unwrap().is_empty(),
        "and it lists as empty"
    );
    MemBackend::write_entity(&writer, Path::new("one.md"), b"# one\n").unwrap();
    MemBackend::commit(&writer, "seed", &ctx_for_test()).unwrap();
    assert!(
        MemBackend::storage_present(&writer).unwrap(),
        "the first commit creates the ref"
    );
    assert_eq!(MemBackend::list_entities(&writer).unwrap().len(), 1);
}

fn read_blob(gitdir: &Path, ref_name: &str, path: &str) -> Option<Vec<u8>> {
    let repo = gix::open(gitdir).unwrap();
    let mut reference = repo.try_find_reference(ref_name).unwrap()?;
    let id = reference.peel_to_id().unwrap();
    let commit = repo.find_object(id).unwrap().into_commit();
    let tree = commit.tree().unwrap();
    let entry = tree.lookup_entry_by_path(path).unwrap()?;
    let object = repo.find_object(entry.id()).unwrap();
    Some(object.data.clone())
}

fn tree_path_exists(gitdir: &Path, ref_name: &str, path: &str) -> bool {
    read_blob(gitdir, ref_name, path).is_some()
}

/// `entity_exists` mirrors `read_entity`'s source selection —
/// pending upsert true / pending delete false before commit, live
/// tip between transactions, unborn ref false — while stopping at
/// the tree entry (flywheel W7/02 primitive).
#[test]
fn entity_exists_tree_probe_and_pending_precedence() {
    use memstead_base::backend::MemBackend;
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    assert!(
        !MemBackend::entity_exists(&writer, Path::new("notes/a.md")).unwrap(),
        "unborn ref answers false"
    );

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
    assert!(!MemBackend::entity_exists(&writer, Path::new("notes/b.md")).unwrap());

    MemBackend::delete_entity(&writer, Path::new("notes/a.md")).unwrap();
    assert!(
        !MemBackend::entity_exists(&writer, Path::new("notes/a.md")).unwrap(),
        "staged delete answers false while the branch tip still holds the blob"
    );
}

/// `read_all_entities` is the one-walk form of list-then-read: the
/// same rows between transactions (the sidecar under `.memstead/`
/// excluded, an unborn ref empty), and the same pending precedence
/// while writes are staged — a staged upsert is served, a staged
/// delete hides the committed blob.
#[test]
fn read_all_entities_matches_per_path_reads_and_pending_precedence() {
    use memstead_base::backend::{MemBackend, read_entities_one_by_one};
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    assert!(
        MemBackend::read_all_entities(&writer).unwrap().is_empty(),
        "unborn ref reads as no entities"
    );

    MemBackend::write_entity(&writer, Path::new("notes/a.md"), b"# a\n").unwrap();
    MemBackend::write_entity(&writer, Path::new("notes/b.md"), b"# b\n").unwrap();
    writer
        .write_anchors_sidecar(b"{\"version\":1,\"entities\":{}}")
        .unwrap();
    MemBackend::commit(&writer, "land a and b", &ctx_for_test()).unwrap();

    let rows = |reads: Vec<memstead_base::backend::EntityRead>| {
        let mut rows: Vec<(String, Vec<u8>)> = reads
            .into_iter()
            .map(|(p, r)| (p.to_string_lossy().into_owned(), r.unwrap()))
            .collect();
        rows.sort();
        rows
    };
    let one_walk = rows(MemBackend::read_all_entities(&writer).unwrap());
    let per_path = rows(read_entities_one_by_one(&writer).unwrap());
    assert_eq!(one_walk, per_path);
    assert_eq!(
        one_walk,
        vec![
            ("notes/a.md".to_string(), b"# a\n".to_vec()),
            ("notes/b.md".to_string(), b"# b\n".to_vec()),
        ],
        "entities only: the anchors sidecar under .memstead/ is not an entity"
    );

    MemBackend::write_entity(&writer, Path::new("notes/c.md"), b"# c\n").unwrap();
    MemBackend::delete_entity(&writer, Path::new("notes/a.md")).unwrap();
    let staged = rows(MemBackend::read_all_entities(&writer).unwrap());
    assert_eq!(
        staged,
        vec![
            ("notes/b.md".to_string(), b"# b\n".to_vec()),
            ("notes/c.md".to_string(), b"# c\n".to_vec()),
        ],
        "with writes staged, a staged delete hides a.md and a staged upsert serves c.md"
    );
}

#[test]
fn git_tree_writer_round_trip() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    writer
        .write_entity(Path::new("notes/hello.md"), b"# hi\n")
        .unwrap();
    let sha = writer.commit("first commit", &ctx_for_test()).unwrap();
    assert_eq!(sha.len(), 40);

    let bytes = read_blob(&gitdir, "refs/heads/test", "notes/hello.md").unwrap();
    assert_eq!(bytes, b"# hi\n");
}

#[test]
fn git_tree_writer_anchors_sidecar_rides_commit_and_survives_reload() {
    use memstead_base::backend::MemBackend;
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    // Stage an entity write and the anchors sidecar, then commit
    // once — both land in the same commit.
    MemBackend::write_entity(&writer, Path::new("hello.md"), b"# hi\n").unwrap();
    writer
        .write_anchors_sidecar(b"{\"version\":1,\"entities\":{}}")
        .unwrap();
    MemBackend::commit(&writer, "entity+anchors", &ctx_for_test()).unwrap();

    // Sidecar blob is present in the branch tree at the reserved path.
    let sidecar = read_blob(&gitdir, "refs/heads/test", ".memstead/anchors.json").unwrap();
    assert_eq!(sidecar, b"{\"version\":1,\"entities\":{}}");

    // A fresh writer (engine reload) reads it back.
    let reloaded = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());
    assert_eq!(
        reloaded.read_anchors_sidecar().unwrap(),
        Some(b"{\"version\":1,\"entities\":{}}".to_vec())
    );
    // And it never surfaces as an entity.
    assert_eq!(
        MemBackend::list_entities(&reloaded).unwrap(),
        vec![PathBuf::from("hello.md")]
    );
}

#[test]
fn git_tree_writer_delete_removes_path() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    writer.write_entity(Path::new("a.md"), b"a").unwrap();
    writer.write_entity(Path::new("b.md"), b"b").unwrap();
    writer.commit("seed", &ctx_for_test()).unwrap();

    writer.delete_entity(Path::new("a.md")).unwrap();
    writer.commit("drop a", &ctx_for_test()).unwrap();

    assert!(!tree_path_exists(&gitdir, "refs/heads/test", "a.md"));
    assert!(tree_path_exists(&gitdir, "refs/heads/test", "b.md"));
}

#[test]
fn git_tree_writer_move_renames_path() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    writer
        .write_entity(Path::new("from.md"), b"payload")
        .unwrap();
    writer.commit("seed", &ctx_for_test()).unwrap();

    writer
        .move_entity(Path::new("from.md"), Path::new("nested/to.md"))
        .unwrap();
    writer.commit("rename", &ctx_for_test()).unwrap();

    assert!(!tree_path_exists(&gitdir, "refs/heads/test", "from.md"));
    let moved = read_blob(&gitdir, "refs/heads/test", "nested/to.md").unwrap();
    assert_eq!(moved, b"payload");
}

#[test]
fn git_tree_writer_multi_op_commit() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    // Seed an entry that will be deleted in the same multi-op
    // commit as two new writes.
    writer.write_entity(Path::new("doomed.md"), b"x").unwrap();
    writer.commit("seed", &ctx_for_test()).unwrap();

    writer.write_entity(Path::new("a.md"), b"alpha").unwrap();
    writer
        .write_entity(Path::new("nested/b.md"), b"beta")
        .unwrap();
    writer.delete_entity(Path::new("doomed.md")).unwrap();
    writer.commit("multi-op", &ctx_for_test()).unwrap();

    assert!(!tree_path_exists(&gitdir, "refs/heads/test", "doomed.md"));
    assert_eq!(
        read_blob(&gitdir, "refs/heads/test", "a.md").unwrap(),
        b"alpha"
    );
    assert_eq!(
        read_blob(&gitdir, "refs/heads/test", "nested/b.md").unwrap(),
        b"beta"
    );
}

#[test]
fn git_tree_writer_cas_conflict_surfaces_hash_mismatch() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());

    // Seed so both writers snapshot the same parent SHA.
    let seeder = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());
    seeder.write_entity(Path::new("seed.md"), b"x").unwrap();
    let seed_sha = seeder.commit("seed", &ctx_for_test()).unwrap();

    let a = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());
    let b = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    // Both writers take their snapshot at the same parent.
    a.write_entity(Path::new("a.md"), b"a").unwrap();
    b.write_entity(Path::new("b.md"), b"b").unwrap();
    assert_eq!(
        a.pending
            .lock()
            .unwrap()
            .parent
            .unwrap()
            .to_hex()
            .to_string(),
        seed_sha
    );
    assert_eq!(
        b.pending
            .lock()
            .unwrap()
            .parent
            .unwrap()
            .to_hex()
            .to_string(),
        seed_sha
    );

    // A commits, advancing the ref. B then tries to commit and
    // gets the typed CAS conflict.
    let new_tip = a.commit("a wins", &ctx_for_test()).unwrap();
    let err = b
        .commit("b loses", &ctx_for_test())
        .expect_err("B's commit must fail with HashMismatch");
    match err {
        BackendError::HashMismatch { current } => {
            assert_eq!(current, new_tip);
        }
        other => panic!("expected HashMismatch, got {other:?}"),
    }
    // The engine maps the backend's commit-tip conflict onto its
    // entity-level envelope, so the wire carries one HASH_MISMATCH
    // code whichever level detected the conflict.
    let engine_err = memstead_base::EngineError::from(BackendError::HashMismatch {
        current: new_tip.clone(),
    });
    assert_eq!(engine_err.code(), "HASH_MISMATCH");
    assert!(
        matches!(engine_err, memstead_base::EngineError::HashMismatch { ref current, .. } if *current == new_tip)
    );
}

#[test]
fn cas_conflict_clears_pending_so_reads_fall_back_to_committed_truth() {
    // Regression: a commit that loses the CAS race must ABORT its
    // staged ops. Before the fix, `pending` was left populated on a
    // CAS conflict, and because `read_entity` prefers pending over the
    // committed tip, the loser's never-committed write was served as
    // phantom truth (and a later `reload_one_mem` pulled it into the
    // in-memory store) until the process restarted.
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());

    // Seed a shared entity both writers will target, so they snapshot
    // the same parent SHA.
    let seeder = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());
    seeder.write_entity(Path::new("shared.md"), b"v1").unwrap();
    seeder.commit("seed", &ctx_for_test()).unwrap();

    let a = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());
    let b = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    // Both snapshot the same parent, then stage conflicting updates to
    // the SAME entity.
    a.write_entity(Path::new("shared.md"), b"A-committed")
        .unwrap();
    b.write_entity(Path::new("shared.md"), b"B-phantom")
        .unwrap();

    // A wins the race; B's commit hits the typed CAS conflict.
    a.commit("a wins", &ctx_for_test()).unwrap();
    let err = b
        .commit("b loses", &ctx_for_test())
        .expect_err("B must lose the CAS race");
    assert!(
        matches!(err, BackendError::HashMismatch { .. }),
        "expected HashMismatch, got {err:?}"
    );

    // The failed transaction must be aborted: B's pending buffer empty…
    assert!(
        b.pending.lock().unwrap().ops.is_empty(),
        "pending must be cleared after a failed commit"
    );
    // …so a read falls through to the committed tip and returns A's
    // value, NOT B's orphaned "B-phantom" staged write.
    let read = <GitTreeBackend as memstead_base::backend::MemBackend>::read_entity(
        &b,
        Path::new("shared.md"),
    )
    .unwrap();
    assert_eq!(
        read.as_deref(),
        Some(&b"A-committed"[..]),
        "read must serve committed truth, not the phantom staged write"
    );
}

#[test]
fn commit_with_expected_parent_succeeds_when_ref_matches_pin() {
    use memstead_base::backend::MemBackend;

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());

    let seeder = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());
    <GitTreeBackend as MemBackend>::write_entity(&seeder, Path::new("seed.md"), b"x").unwrap();
    let seed_sha =
        <GitTreeBackend as MemBackend>::commit(&seeder, "seed", &ctx_for_test()).unwrap();

    // Engine-style flow: snapshot head, mutate, then commit pinned.
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());
    let expected = <GitTreeBackend as MemBackend>::current_head(&writer)
        .unwrap()
        .expect("seeded ref has a head");
    assert_eq!(expected, seed_sha);

    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("after.md"), b"after").unwrap();

    let new_tip = <GitTreeBackend as MemBackend>::commit_with_expected_parent(
        &writer,
        "pinned commit",
        &ctx_for_test(),
        Some(&expected),
    )
    .expect("parent matches pin → commit must succeed");
    assert_ne!(new_tip, seed_sha);
    assert_eq!(
        read_blob(&gitdir, "refs/heads/test", "after.md").unwrap(),
        b"after"
    );
}

#[test]
fn commit_with_expected_parent_surfaces_parent_mismatch_when_sibling_advances_ref() {
    use memstead_base::backend::{BackendError, MemBackend};

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());

    // Seed so both writers start from the same commit.
    let seeder = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());
    <GitTreeBackend as MemBackend>::write_entity(&seeder, Path::new("seed.md"), b"x").unwrap();
    let seed_sha =
        <GitTreeBackend as MemBackend>::commit(&seeder, "seed", &ctx_for_test()).unwrap();

    // Engine A snapshots head — this is the pin it will retain
    // through any number of intermediate writes.
    let a = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());
    let pin = <GitTreeBackend as MemBackend>::current_head(&a)
        .unwrap()
        .expect("seeded ref has a head");
    assert_eq!(pin, seed_sha);

    // A sibling writer (another engine instance, manual git op,
    // out-of-band CLI invocation, …) advances the ref between A's
    // snapshot and A's commit attempt.
    let sibling = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());
    <GitTreeBackend as MemBackend>::write_entity(&sibling, Path::new("drift.md"), b"drift")
        .unwrap();
    let new_tip =
        <GitTreeBackend as MemBackend>::commit(&sibling, "sibling advance", &ctx_for_test())
            .unwrap();
    assert_ne!(new_tip, seed_sha);

    // A now tries to land a pinned commit. The pin no longer
    // matches the live tip → typed `ParentMismatch`.
    <GitTreeBackend as MemBackend>::write_entity(&a, Path::new("a.md"), b"a").unwrap();
    let err = <GitTreeBackend as MemBackend>::commit_with_expected_parent(
        &a,
        "pinned commit",
        &ctx_for_test(),
        Some(&pin),
    )
    .expect_err("pin no longer matches live tip → commit must refuse");
    match err {
        BackendError::ParentMismatch { expected, actual } => {
            assert_eq!(expected, pin);
            assert_eq!(actual, new_tip);
        }
        other => panic!("expected ParentMismatch, got {other:?}"),
    }
}

#[test]
fn commit_with_expected_parent_none_pin_is_equivalent_to_commit() {
    use memstead_base::backend::MemBackend;

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("hello.md"), b"hi").unwrap();
    let sha = <GitTreeBackend as MemBackend>::commit_with_expected_parent(
        &writer,
        "unpinned",
        &ctx_for_test(),
        None,
    )
    .expect("None pin → plain commit semantics, must succeed against empty ref");
    assert_eq!(sha.len(), 40);
    assert_eq!(
        read_blob(&gitdir, "refs/heads/test", "hello.md").unwrap(),
        b"hi"
    );
}

#[test]
fn git_tree_writer_blob_oid_is_content_addressed() {
    // The git-tree writer is content-addressed: writing the same
    // bytes through two independent writers must yield the same
    // blob OID, since the OID is a hash of the content.
    let tmp_a = TempDir::new().unwrap();
    let tmp_b = TempDir::new().unwrap();
    let payload = b"shared content\n";

    let gitdir_a = fresh_repo_dir(tmp_a.path());
    let writer_a = GitTreeBackend::new(gitdir_a.clone(), "refs/heads/a".to_string());
    writer_a
        .write_entity(Path::new("file.md"), payload)
        .unwrap();
    let sha_a = writer_a.commit("a", &ctx_for_test()).unwrap();
    let repo_a = gix::open(&gitdir_a).unwrap();
    let commit_a = repo_a
        .find_object(gix::ObjectId::from_hex(sha_a.as_bytes()).unwrap())
        .unwrap()
        .into_commit();
    let blob_id_a = commit_a
        .tree()
        .unwrap()
        .lookup_entry_by_path("file.md")
        .unwrap()
        .unwrap()
        .id()
        .detach();

    let gitdir_b = fresh_repo_dir(tmp_b.path());
    let writer_b = GitTreeBackend::new(gitdir_b.clone(), "refs/heads/b".to_string());
    writer_b
        .write_entity(Path::new("file.md"), payload)
        .unwrap();
    let sha_b = writer_b.commit("b", &ctx_for_test()).unwrap();
    let repo_b = gix::open(&gitdir_b).unwrap();
    let commit_b = repo_b
        .find_object(gix::ObjectId::from_hex(sha_b.as_bytes()).unwrap())
        .unwrap()
        .into_commit();
    let blob_id_b = commit_b
        .tree()
        .unwrap()
        .lookup_entry_by_path("file.md")
        .unwrap()
        .unwrap()
        .id()
        .detach();

    assert_eq!(
        blob_id_a, blob_id_b,
        "same content must produce byte-identical blob OIDs"
    );
}

/// Initialise a non-bare repo at `<workdir>` with `refs/heads/main`
/// as the symbolic HEAD. Returns `(workdir, gitdir)` — the workdir
/// is what GitHub Desktop would open; the gitdir is what
/// `GitTreeBackend::new` consumes.
fn fresh_non_bare_repo(tmp: &Path) -> (PathBuf, PathBuf) {
    let workdir = tmp.join("mem-repo-workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&workdir)
        .args(["init", "--initial-branch=main", "--quiet"])
        .status()
        .expect("git init must succeed");
    assert!(status.success(), "git init failed");
    let workdir = std::fs::canonicalize(&workdir).unwrap();
    let gitdir = workdir.join(".git");
    (workdir, gitdir)
}

#[test]
fn sync_helper_skips_on_bare_repo() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let repo = gix::open(&gitdir).unwrap();

    // No working tree exists — the helper must short-circuit Ok(())
    // regardless of the ref name passed.
    sync_index_and_worktree(&repo, "refs/heads/main").unwrap();
}

#[test]
fn sync_helper_updates_worktree_when_ref_matches_head() {
    let tmp = TempDir::new().unwrap();
    let (workdir, gitdir) = fresh_non_bare_repo(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/main".to_string());

    writer
        .write_entity(Path::new("configs/alpha.json"), b"{\"name\":\"alpha\"}\n")
        .unwrap();
    writer.commit("seed alpha", &ctx_for_test()).unwrap();

    // The writer's commit() invokes sync_index_and_worktree via the
    // post-commit hook — the file must now exist on disk.
    let on_disk = workdir.join("configs/alpha.json");
    assert!(
        on_disk.exists(),
        "worktree sync must materialise the new blob at {}",
        on_disk.display()
    );
    let bytes = std::fs::read(&on_disk).unwrap();
    assert_eq!(bytes, b"{\"name\":\"alpha\"}\n");

    // git status is also clean (HEAD == index == worktree).
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&workdir)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        output.stdout.is_empty(),
        "git status --porcelain must be empty post-sync, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn sync_helper_skips_when_ref_does_not_match_head() {
    let tmp = TempDir::new().unwrap();
    let (workdir, gitdir) = fresh_non_bare_repo(tmp.path());

    // Write to refs/heads/feature; HEAD still points at
    // refs/heads/main. The worktree must NOT receive the feature
    // branch's content.
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/feature".to_string());
    writer
        .write_entity(Path::new("only-on-feature.md"), b"feature-only\n")
        .unwrap();
    writer
        .commit("first commit on feature", &ctx_for_test())
        .unwrap();

    // Object store has the blob on the feature branch...
    assert!(tree_path_exists(
        &gitdir,
        "refs/heads/feature",
        "only-on-feature.md"
    ));
    // ...but the worktree (which reflects main) does not.
    assert!(
        !workdir.join("only-on-feature.md").exists(),
        "worktree must not be polluted by writes to a non-checked-out branch"
    );
}

#[test]
fn sync_helper_preserves_untracked_files() {
    let tmp = TempDir::new().unwrap();
    let (workdir, gitdir) = fresh_non_bare_repo(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/main".to_string());

    // Drop an untracked file in the workdir before any engine
    // commit runs. `git read-tree --reset -u HEAD` only touches
    // tracked-file state; untracked content must survive.
    let untracked = workdir.join("scratch.txt");
    std::fs::write(&untracked, b"operator notes\n").unwrap();

    writer
        .write_entity(Path::new("seed.md"), b"seed\n")
        .unwrap();
    writer.commit("create seed", &ctx_for_test()).unwrap();

    assert!(
        untracked.exists(),
        "sync must leave untracked files in place"
    );
    assert_eq!(std::fs::read(&untracked).unwrap(), b"operator notes\n");
    // The tracked entity is also materialised.
    assert_eq!(std::fs::read(workdir.join("seed.md")).unwrap(), b"seed\n");
}

#[test]
fn sync_helper_updates_through_delete_and_overwrite() {
    let tmp = TempDir::new().unwrap();
    let (workdir, gitdir) = fresh_non_bare_repo(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/main".to_string());

    writer.write_entity(Path::new("a.md"), b"first\n").unwrap();
    writer.commit("create a", &ctx_for_test()).unwrap();
    assert_eq!(std::fs::read(workdir.join("a.md")).unwrap(), b"first\n");

    writer.write_entity(Path::new("a.md"), b"second\n").unwrap();
    writer.commit("overwrite a", &ctx_for_test()).unwrap();
    assert_eq!(
        std::fs::read(workdir.join("a.md")).unwrap(),
        b"second\n",
        "overwrite must propagate to the worktree"
    );

    writer.delete_entity(Path::new("a.md")).unwrap();
    writer.commit("delete a", &ctx_for_test()).unwrap();
    assert!(
        !workdir.join("a.md").exists(),
        "delete must remove the file from the worktree"
    );

    // git status remains clean across all three transitions.
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&workdir)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        output.stdout.is_empty(),
        "git status --porcelain must be empty after every commit, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

// ----- MemBackend impl -----------------------------------------

/// Build a CommitContext that produces an `memstead: <verb> <id>`
/// subject with a given verb. The agent-notes parser keys off the
/// subject's verb to recover the mutation kind.
fn commit_with_verb(writer: &GitTreeBackend, verb: &str, entity_id: &str, ctx: &CommitContext<'_>) {
    let subject = format!("memstead: {verb} {entity_id}");
    <GitTreeBackend as MemBackend>::commit(writer, &subject, ctx).unwrap();
}

fn ctx_with_note<'a>(note: &'a str) -> CommitContext<'a> {
    CommitContext {
        actor: Actor::Agent,
        client: Some(ClientId {
            name: "claude-code".to_string(),
            version: "2.1.0".to_string(),
        }),
        tool: Some("memstead_create"),
        note: Some(note.to_string()),
        role: Default::default(),
        identity: None,
        logical_operation_id: None,
        entity_ids: None,
    }
}

#[test]
fn backend_list_entities_returns_only_md_outside_memstead_namespace() {
    use memstead_base::backend::MemBackend;

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    // Seed via MemBackend (fully-qualified to avoid trait
    // ambiguity once MemBackend enters scope below).
    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("a.md"), b"# a").unwrap();
    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("nested/b.md"), b"# b")
        .unwrap();
    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("notes.json"), b"{}").unwrap();
    <GitTreeBackend as MemBackend>::write_entity(
        &writer,
        Path::new(".memstead/config.json"),
        b"{}",
    )
    .unwrap();
    <GitTreeBackend as MemBackend>::write_entity(
        &writer,
        Path::new(".memstead/notes.md"),
        b"# skip me",
    )
    .unwrap();
    <GitTreeBackend as MemBackend>::write_entity(
        &writer,
        Path::new(".other/notes.md"),
        b"# no longer special, walked like any non-meta dir",
    )
    .unwrap();
    <GitTreeBackend as MemBackend>::commit(&writer, "seed", &ctx_for_test()).unwrap();

    let backend: &dyn MemBackend = &writer;
    let mut paths: Vec<String> = backend
        .list_entities()
        .unwrap()
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    paths.sort();
    // `.memstead/` stays skipped; an ordinary dot-dir is walked.
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
fn backend_list_entities_returns_empty_for_missing_branch() {
    use memstead_base::backend::MemBackend;

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir, "refs/heads/never".to_string());
    let backend: &dyn MemBackend = &writer;
    // Branch never created → empty, no error.
    assert!(backend.list_entities().unwrap().is_empty());
}

#[test]
fn backend_read_entity_consults_pending_then_branch_tip() {
    use memstead_base::backend::MemBackend;

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    // Seed a committed entry.
    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("on_branch.md"), b"branch")
        .unwrap();
    <GitTreeBackend as MemBackend>::commit(&writer, "seed", &ctx_for_test()).unwrap();

    let backend: &dyn MemBackend = &writer;
    // Branch path → reads from the branch tip.
    assert_eq!(
        backend.read_entity(Path::new("on_branch.md")).unwrap(),
        Some(b"branch".to_vec())
    );
    // Buffered upsert wins over the branch tip.
    backend
        .write_entity(Path::new("on_branch.md"), b"buffered")
        .unwrap();
    assert_eq!(
        backend.read_entity(Path::new("on_branch.md")).unwrap(),
        Some(b"buffered".to_vec())
    );
    // Buffered delete masks the branch.
    backend.delete_entity(Path::new("on_branch.md")).unwrap();
    assert_eq!(
        backend.read_entity(Path::new("on_branch.md")).unwrap(),
        None
    );
    // Unknown path → None.
    assert_eq!(backend.read_entity(Path::new("never.md")).unwrap(), None);
}

#[test]
fn backend_read_provenance_reconstructs_from_commit_log() {
    use memstead_base::backend::MemBackend;

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    // Two commits with memstead: subjects so the verb maps back to a
    // ProvenanceKind. The first carries an agent note, the second
    // does not.
    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("a.md"), b"a").unwrap();
    commit_with_verb(&writer, "create", "v:a", &ctx_with_note("first draft"));

    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("a.md"), b"a2").unwrap();
    commit_with_verb(
        &writer,
        "update",
        "v:a",
        &CommitContext {
            actor: Actor::Cli,
            client: None,
            tool: Some("memstead_update"),
            note: None,
            role: Default::default(),
            identity: None,
            logical_operation_id: None,
            entity_ids: None,
        },
    );

    let backend: &dyn MemBackend = &writer;
    // append_provenance is the no-op contract; calling it with a
    // throw-away record must not perturb the read path.
    backend
        .append_provenance(&memstead_base::Provenance::new(
            std::time::UNIX_EPOCH,
            memstead_base::ProvenanceKind::Create,
            Some("ignored".into()),
            Actor::Unknown,
            None,
            None,
        ))
        .unwrap();

    let records = backend.read_provenance(None).unwrap();
    assert_eq!(records.len(), 2, "expected two commits, got {records:?}");
    // Oldest-first ordering (matches folder backend).
    assert_eq!(records[0].kind, memstead_base::ProvenanceKind::Create);
    assert_eq!(records[0].entity.as_deref(), Some("v:a"));
    assert_eq!(records[0].actor, Actor::Agent);
    assert_eq!(records[0].note.as_deref(), Some("first draft"));
    assert_eq!(
        records[0]
            .client
            .as_ref()
            .map(|c| (c.name.as_str(), c.version.as_str())),
        Some(("claude-code", "2.1.0"))
    );
    assert_eq!(records[1].kind, memstead_base::ProvenanceKind::Update);
    assert_eq!(records[1].actor, Actor::Cli);
    assert!(records[1].note.is_none());
    assert!(records[1].client.is_none());
}

#[test]
fn backend_read_provenance_filters_by_cursor_sha() {
    use memstead_base::backend::MemBackend;

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    // Seed three commits; the cursor will be the SHA of the first.
    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("a.md"), b"a").unwrap();
    let first_sha =
        <GitTreeBackend as MemBackend>::commit(&writer, "memstead: create v:a", &ctx_for_test())
            .unwrap();
    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("a.md"), b"a2").unwrap();
    <GitTreeBackend as MemBackend>::commit(&writer, "memstead: update v:a", &ctx_for_test())
        .unwrap();
    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("a.md"), b"a3").unwrap();
    <GitTreeBackend as MemBackend>::commit(&writer, "memstead: update v:a", &ctx_for_test())
        .unwrap();

    let backend: &dyn MemBackend = &writer;
    // Cursor at the first SHA → returns only the two newer commits.
    let after = backend.read_provenance(Some(&first_sha)).unwrap();
    assert_eq!(
        after.len(),
        2,
        "expected commits after cursor, got {after:?}"
    );
    for r in &after {
        assert_eq!(r.kind, memstead_base::ProvenanceKind::Update);
    }
}

#[test]
fn backend_read_provenance_empty_for_missing_branch() {
    use memstead_base::backend::MemBackend;

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir, "refs/heads/never".to_string());
    let backend: &dyn MemBackend = &writer;
    // No commits yet → empty record list, no error.
    assert!(backend.read_provenance(None).unwrap().is_empty());
}

#[test]
fn backend_unknown_verb_falls_back_to_update_kind() {
    use memstead_base::backend::MemBackend;

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/test".to_string());

    <GitTreeBackend as MemBackend>::write_entity(&writer, Path::new("a.md"), b"a").unwrap();
    // Verb that isn't in the ProvenanceKind enum (e.g. lifecycle
    // verbs like `mem_create`) — round-trips as Update under the
    // tolerant-reader convention shared with the folder backend.
    commit_with_verb(&writer, "mem_create", "v:a", &ctx_for_test());

    let backend: &dyn MemBackend = &writer;
    let records = backend.read_provenance(None).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, memstead_base::ProvenanceKind::Update);
}

#[test]
fn instantiate_full_backend_constructs_git_branch_writer() {
    // Smoke test: instantiate_full_backend on a GitBranch mount
    // produces a backend that can list against an empty branch
    // without erroring (proves the writer is wired with the
    // right gitdir + ref shape).
    use memstead_base::{MemBackend, Mount, MountCapability, MountLifecycle, MountStorage};

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let mount = Mount {
        mem: "engine".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: MountStorage::GitBranch {
            gitdir,
            branch: "engine".to_string(),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let backend: Box<dyn MemBackend> = crate::storage::instantiate_full_backend(&mount).unwrap();
    // Empty branch → empty list, no error.
    assert!(backend.list_entities().unwrap().is_empty());
    // Provenance log on a fresh branch → empty.
    assert!(backend.read_provenance(None).unwrap().is_empty());
}

#[test]
fn instantiate_full_backend_accepts_branch_with_or_without_refs_prefix() {
    // The full instantiator normalises a bare branch name
    // ("engine") to its fully-qualified ref ("refs/heads/engine").
    // Mounts may carry either shape; the writer must end up keyed
    // on the same per-branch mutex regardless.
    use memstead_base::{MemBackend, Mount, MountCapability, MountLifecycle, MountStorage};

    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    for branch in ["engine", "refs/heads/engine"] {
        let mount = Mount {
            mem: "engine".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::GitBranch {
                gitdir: gitdir.clone(),
                branch: branch.to_string(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let backend: Box<dyn MemBackend> =
            crate::storage::instantiate_full_backend(&mount).unwrap();
        // Both shapes resolve cleanly (no panic, no error).
        assert!(backend.list_entities().unwrap().is_empty());
    }
}

// ---- MemBackend::current_head ----------------------------------

#[test]
fn current_head_returns_none_for_empty_branch() {
    // A fresh bare repo has no commits and no branches; the
    // writer's `try_find_reference` returns Ok(None) and
    // current_head collapses to Ok(None) — drift detection on
    // an unborn mem is a clean no-op.
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir, "refs/heads/specs".to_string());
    let head =
        <GitTreeBackend as memstead_base::backend::MemBackend>::current_head(&writer).unwrap();
    assert!(head.is_none());
}

#[test]
fn current_head_returns_hex_sha_after_commit() {
    // After the first commit, current_head returns the 40-char
    // hex SHA matching what `commit` returned. The two values
    // are read through different paths (commit returns the value
    // straight from the writer; current_head re-opens the gitdir
    // and peels the ref) so equality proves end-to-end consistency.
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());

    writer.write_entity(Path::new("a.md"), b"a").unwrap();
    let sha = writer.commit("first", &ctx_for_test()).unwrap();
    assert_eq!(sha.len(), 40);

    let head = <GitTreeBackend as memstead_base::backend::MemBackend>::current_head(&writer)
        .unwrap()
        .expect("head present after commit");
    assert_eq!(head, sha);
}

#[test]
fn current_head_advances_on_subsequent_commits() {
    // Two back-to-back commits produce two distinct SHAs;
    // current_head reflects the latest after each. This is the
    // signal Engine::reload_if_stale compares against the
    // cached last_known_head to detect a sibling writer.
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());

    writer.write_entity(Path::new("a.md"), b"a").unwrap();
    let first = writer.commit("first", &ctx_for_test()).unwrap();
    let head_after_first =
        <GitTreeBackend as memstead_base::backend::MemBackend>::current_head(&writer)
            .unwrap()
            .unwrap();
    assert_eq!(head_after_first, first);

    writer.write_entity(Path::new("b.md"), b"b").unwrap();
    let second = writer.commit("second", &ctx_for_test()).unwrap();
    assert_ne!(first, second);
    let head_after_second =
        <GitTreeBackend as memstead_base::backend::MemBackend>::current_head(&writer)
            .unwrap()
            .unwrap();
    assert_eq!(head_after_second, second);
}

#[test]
fn current_head_returns_none_for_missing_gitdir() {
    // A writer pointed at a non-existent gitdir collapses to
    // Ok(None) (with a debug log) rather than surfacing the
    // open failure as an Err. Drift detection is best-effort —
    // a transient broken mount doesn't poison the read it
    // accompanies.
    let tmp = TempDir::new().unwrap();
    let writer = GitTreeBackend::new(
        tmp.path().join("does-not-exist.git"),
        "refs/heads/specs".to_string(),
    );
    let head =
        <GitTreeBackend as memstead_base::backend::MemBackend>::current_head(&writer).unwrap();
    assert!(head.is_none());
}

// ---- git-branch changes_since dispatch --------------------------
//
// Tests the `FULL_GIT_BRANCH_OPS.changes_since` dispatcher that
// full boot installs on `memstead_base::Engine`. The dispatcher wraps
// `crate::ops::changes::changes_since` and presents it through the
// `memstead_base::GitBranchChangesSinceFn` signature.

fn dispatch_changes(
    gitdir: &Path,
    branch: &str,
    mem: &str,
    since: &str,
) -> Result<memstead_base::ops::BackendChanges, memstead_base::backend::BackendError> {
    (crate::storage::FULL_GIT_BRANCH_OPS.changes_since)(
        gitdir,
        branch,
        mem,
        since,
        memstead_base::ops::RENAME_SIMILARITY_DEFAULT,
    )
}

#[test]
fn changes_since_empty_repo_with_sentinel_returns_empty_changes() {
    // Fresh bare repo: no commits, no branches. With the empty-tree
    // sentinel as `since`, the dispatcher short-circuits to "no
    // diff, head echoes sentinel".
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let result = dispatch_changes(
        &gitdir,
        "specs",
        "specs",
        memstead_base::ops::EMPTY_TREE_SHA,
    )
    .unwrap();
    assert_eq!(result.since, memstead_base::ops::EMPTY_TREE_SHA);
    assert_eq!(result.head, memstead_base::ops::EMPTY_TREE_SHA);
    assert!(result.changes.is_empty());
}

#[test]
fn changes_since_after_commit_returns_added_envelopes_id_only() {
    // Commit two new entities, poll from the empty-tree sentinel,
    // and expect both as Added envelopes. Dispatch returns id-only
    // envelopes — the engine wrapper enriches.
    use memstead_base::ops::ChangeEnvelope;
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());

    writer
        .write_entity(Path::new("alpha.md"), b"# Alpha")
        .unwrap();
    writer
        .write_entity(Path::new("beta.md"), b"# Beta")
        .unwrap();
    let head_sha = writer.commit("seed", &ctx_for_test()).unwrap();

    let result = dispatch_changes(
        &gitdir,
        "specs",
        "specs",
        memstead_base::ops::EMPTY_TREE_SHA,
    )
    .unwrap();
    assert_eq!(result.since, memstead_base::ops::EMPTY_TREE_SHA);
    assert_eq!(result.head, head_sha);
    assert_eq!(result.changes.len(), 2);
    for env in &result.changes {
        match env {
            ChangeEnvelope::Added {
                id,
                title,
                entity_type,
            } => {
                assert!(
                    id.0.starts_with("specs--"),
                    "expected mem-prefixed id, got {}",
                    id.0
                );
                assert!(title.is_none(), "dispatch must not enrich title");
                assert!(
                    entity_type.is_none(),
                    "dispatch must not enrich entity_type"
                );
            }
            other => panic!("expected Added envelope, got {other:?}"),
        }
    }
}

#[test]
fn changes_since_between_two_commits_yields_updated_envelope() {
    use memstead_base::ops::ChangeEnvelope;
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());

    writer
        .write_entity(Path::new("alpha.md"), b"# Alpha v1")
        .unwrap();
    let sha_v1 = writer.commit("v1", &ctx_for_test()).unwrap();

    writer
        .write_entity(Path::new("alpha.md"), b"# Alpha v2")
        .unwrap();
    let sha_v2 = writer.commit("v2", &ctx_for_test()).unwrap();
    assert_ne!(sha_v1, sha_v2);

    let result = dispatch_changes(&gitdir, "specs", "specs", &sha_v1).unwrap();
    assert_eq!(result.since, sha_v1);
    assert_eq!(result.head, sha_v2);
    assert_eq!(result.changes.len(), 1);
    match &result.changes[0] {
        ChangeEnvelope::Updated {
            id,
            title,
            entity_type,
        } => {
            assert!(id.0.starts_with("specs--"));
            assert!(title.is_none());
            assert!(entity_type.is_none());
        }
        other => panic!("expected Updated envelope, got {other:?}"),
    }
}

#[test]
fn anchor_only_commit_yields_zero_entity_deltas_and_valid_cursor() {
    // Seed an entity, then land an anchor-only commit (only the
    // `.memstead/anchors.json` sidecar changed). changes_since from the
    // seed head must report ZERO entity deltas — the sidecar lives under
    // `.memstead/` which the entity-delta computation filters — while
    // the anchor commit's SHA is a valid `since` cursor.
    use memstead_base::backend::MemBackend;
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());

    MemBackend::write_entity(&writer, Path::new("alpha.md"), b"# Alpha").unwrap();
    let seed_sha = MemBackend::commit(&writer, "seed", &ctx_for_test()).unwrap();

    // Anchor-only commit: no entity write, just the sidecar.
    writer
            .write_anchors_sidecar(
                br#"{"version":1,"entities":{"specs--alpha":[{"artifact":"src/lib.rs","grain":"file","class":"anchored","hash_stability":"stable","hash":"h1"}]}}"#,
            )
            .unwrap();
    let anchor_sha = MemBackend::commit(&writer, "anchors", &ctx_for_test()).unwrap();
    assert_ne!(seed_sha, anchor_sha);

    // Zero entity deltas across the anchor-only commit.
    let from_seed = dispatch_changes(&gitdir, "specs", "specs", &seed_sha).unwrap();
    assert_eq!(from_seed.head, anchor_sha);
    assert_eq!(
        from_seed.changes.len(),
        0,
        "an anchor-only commit must produce zero entity deltas, got {:?}",
        from_seed.changes
    );

    // The anchor commit's SHA is itself a valid cursor (resolves; no
    // deltas after it).
    let from_anchor = dispatch_changes(&gitdir, "specs", "specs", &anchor_sha).unwrap();
    assert_eq!(from_anchor.head, anchor_sha);
    assert_eq!(from_anchor.changes.len(), 0);
}

#[test]
fn changes_since_unknown_cursor_returns_typed_commit_not_found_marker() {
    // A `since` that
    // doesn't resolve is a recoverable caller-argument fault, not a
    // backend fault. The dispatch encodes it as the typed prefix
    // `COMMIT_NOT_FOUND:<sha>` (untruncated) that `Engine::changes_since`
    // lifts to `EngineError::InvalidChangesCursor` (code INVALID_CURSOR)
    // — distinct from the generic `git-branch changes_since: …`
    // wrapper used for real backend faults.
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());
    // Seed one commit so the gitdir is not empty.
    writer.write_entity(Path::new("a.md"), b"a").unwrap();
    writer.commit("seed", &ctx_for_test()).unwrap();

    let bad_sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    let err = dispatch_changes(&gitdir, "specs", "specs", bad_sha).unwrap_err();
    match err {
        memstead_base::backend::BackendError::Other(msg) => {
            assert_eq!(
                msg,
                format!("COMMIT_NOT_FOUND:{bad_sha}"),
                "bad-since must carry the typed marker with the untruncated sha: {msg}",
            );
        }
        other => panic!("expected BackendError::Other, got {other:?}"),
    }
}
