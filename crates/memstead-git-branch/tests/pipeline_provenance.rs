//! Pipeline-edit provenance: the canonical pipeline configs are plain
//! JSON files under `.memstead/` with no commit of their own, so each
//! edit mirrors its bytes under `__MEMSTEAD:pipeline/<kind>/<mem>/<name>.json`
//! in one commit whose body carries the operator's note — the commit is
//! the audit record, the disk file stays the read path. Folder-backed
//! workspaces have no commit timeline: the note is accepted and dropped,
//! matching `set_mem_version`'s posture.

use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

fn medium() -> memstead_base::Medium {
    serde_json::from_str(r#"{"name": "codebase", "type": "codebase", "pointer": "src/"}"#)
        .expect("valid medium json")
}

/// Read the `__MEMSTEAD` tip commit's full message from the mem-repo.
fn memstead_tip_message(workspace_root: &std::path::Path) -> String {
    let gitdir = workspace_root.join("mem-repo").join(".git");
    let repo = gix::open(&gitdir).expect("open mem-repo");
    let tip = repo
        .find_reference("refs/heads/__MEMSTEAD")
        .expect("__MEMSTEAD exists")
        .into_fully_peeled_id()
        .expect("peel");
    let commit = repo
        .find_object(tip.detach())
        .expect("commit")
        .into_commit();
    String::from_utf8_lossy(commit.message_raw().expect("message")).to_string()
}

/// Whether `__MEMSTEAD`'s tip tree contains `path`.
fn memstead_tree_has(workspace_root: &std::path::Path, path: &str) -> bool {
    let gitdir = workspace_root.join("mem-repo").join(".git");
    let repo = gix::open(&gitdir).expect("open mem-repo");
    let tip = repo
        .find_reference("refs/heads/__MEMSTEAD")
        .expect("__MEMSTEAD exists")
        .into_fully_peeled_id()
        .expect("peel");
    let tree = repo
        .find_object(tip.detach())
        .expect("commit")
        .into_commit()
        .tree()
        .expect("tree");
    tree.lookup_entry_by_path(path).ok().flatten().is_some()
}

#[test]
fn pipeline_edit_commits_provenance_with_note() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");

    engine
        .add_medium(
            "specs",
            "codebase",
            &medium(),
            Some("wire the source tree into specs"),
        )
        .expect("add_medium lands");

    // The disk file is the read path…
    assert!(
        tmp.path()
            .join(".memstead/mediums/specs/codebase.json")
            .exists(),
        "canonical disk config written"
    );
    // …and the __MEMSTEAD commit is the provenance record: subject names
    // the edit, the note rides the body, the mirror blob is in the tree.
    let msg = memstead_tip_message(tmp.path());
    assert!(
        msg.contains("add mediums specs/codebase"),
        "subject names the edit: {msg}"
    );
    assert!(
        msg.contains("wire the source tree into specs"),
        "note rides the commit body: {msg}"
    );
    assert!(
        memstead_tree_has(tmp.path(), "pipeline/mediums/specs/codebase.json"),
        "mirror blob committed"
    );

    // Delete removes the mirror in a fresh provenance commit.
    engine
        .delete_medium("specs", "codebase", Some("retired"))
        .expect("delete lands");
    let msg = memstead_tip_message(tmp.path());
    assert!(
        msg.contains("delete mediums specs/codebase"),
        "delete subject: {msg}"
    );
    assert!(msg.contains("retired"), "delete note: {msg}");
    assert!(
        !memstead_tree_has(tmp.path(), "pipeline/mediums/specs/codebase.json"),
        "mirror blob removed"
    );
}

#[test]
fn rename_mirrors_as_remove_plus_upsert_in_one_commit() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");

    engine
        .add_medium("specs", "old-name", &medium(), None)
        .expect("add lands");
    engine
        .rename_medium("specs", "old-name", "new-name", Some("clearer name"))
        .expect("rename lands");

    assert!(
        !memstead_tree_has(tmp.path(), "pipeline/mediums/specs/old-name.json"),
        "old mirror path removed"
    );
    assert!(
        memstead_tree_has(tmp.path(), "pipeline/mediums/specs/new-name.json"),
        "new mirror path present"
    );
    let msg = memstead_tip_message(tmp.path());
    assert!(msg.contains("rename mediums"), "rename subject: {msg}");
    assert!(msg.contains("clearer name"), "rename note: {msg}");
}

#[test]
fn note_is_accepted_without_commit_on_folder_workspaces() {
    // Folder workspace: no mem-repo, no commit timeline. The edit lands,
    // the note is accepted and dropped — same posture as
    // set_mem_version on folder backends. Never an error.
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
    std::fs::write(
        tmp.path().join(".memstead/workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(tmp.path().join(".memstead/state")).unwrap();
    std::fs::write(
        tmp.path().join(".memstead/state/mounts.json"),
        r#"{"format":"memstead-mounts-3","mounts":[{"mem":"specs","schema":"default@1.0.0","storage":{"type":"folder","path":"specs"},"capability":"write","lifecycle":"eager","cross_linkable":true}]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(tmp.path().join("specs")).unwrap();

    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    engine
        .add_medium("specs", "codebase", &medium(), Some("noted anyway"))
        .expect("folder edit lands, note accepted and dropped");
    assert!(
        tmp.path()
            .join(".memstead/mediums/specs/codebase.json")
            .exists()
    );
}
