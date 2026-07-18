//! Pipeline-edit provenance: the canonical pipeline configs are plain
//! JSON files under `.memstead/` with no commit of their own, so each
//! edit mirrors its bytes under `__MEMSTEAD:pipeline/<kind>/<mem>/<name>.json`
//! in one commit whose body carries the operator's note — the commit is
//! the audit record, the disk file stays the read path. Folder-backed
//! workspaces have no commit timeline: the note is accepted and dropped,
//! matching `set_mem_version`'s posture.
//!
//! The edit surface is the v2 single-record binding (the standalone
//! medium/facet records — and their provenance kinds — are gone with the
//! 2026-07 consolidation): every mirror path is `pipeline/projections/…`.

use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

/// A minimal v2 binding patch: one inline codebase source, destination `specs`.
const BINDING_PATCH: &str = r#"{
    "sources": [{ "name": "codebase", "type": "codebase", "pointer": "src/",
                  "scope": [{ "path": "**/*", "mode": "allow" }] }],
    "destination_mem": "specs"
}"#;

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
        .add_projection_json(
            "specs",
            "graph",
            BINDING_PATCH,
            Some("wire the source tree into specs"),
        )
        .expect("add_projection_json lands");

    // The disk file is the read path…
    assert!(
        tmp.path()
            .join(".memstead/projections/specs/graph.json")
            .exists(),
        "canonical disk config written"
    );
    // …and the __MEMSTEAD commit is the provenance record: subject names
    // the edit, the note rides the body, the mirror blob is in the tree.
    let msg = memstead_tip_message(tmp.path());
    assert!(
        msg.contains("add projections specs/graph"),
        "subject names the edit: {msg}"
    );
    assert!(
        msg.contains("wire the source tree into specs"),
        "note rides the commit body: {msg}"
    );
    assert!(
        memstead_tree_has(tmp.path(), "pipeline/projections/specs/graph.json"),
        "mirror blob committed"
    );

    // Delete removes the mirror in a fresh provenance commit.
    engine
        .delete_projection("specs", "graph", Some("retired"))
        .expect("delete lands");
    let msg = memstead_tip_message(tmp.path());
    assert!(
        msg.contains("delete projections specs/graph"),
        "delete subject: {msg}"
    );
    assert!(msg.contains("retired"), "delete note: {msg}");
    assert!(
        !memstead_tree_has(tmp.path(), "pipeline/projections/specs/graph.json"),
        "mirror blob removed"
    );
}

#[test]
fn rename_mirrors_as_remove_plus_upsert_in_one_commit() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");

    engine
        .add_projection_json("specs", "old-name", BINDING_PATCH, None)
        .expect("add lands");
    engine
        .rename_projection("specs", "old-name", "new-name", Some("clearer name"))
        .expect("rename lands");

    assert!(
        !memstead_tree_has(tmp.path(), "pipeline/projections/specs/old-name.json"),
        "old mirror path removed"
    );
    assert!(
        memstead_tree_has(tmp.path(), "pipeline/projections/specs/new-name.json"),
        "new mirror path present"
    );
    let msg = memstead_tip_message(tmp.path());
    assert!(msg.contains("rename projections"), "rename subject: {msg}");
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
        .add_projection_json("specs", "graph", BINDING_PATCH, Some("noted anyway"))
        .expect("folder edit lands, note accepted and dropped");
    assert!(
        tmp.path()
            .join(".memstead/projections/specs/graph.json")
            .exists()
    );
}
