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

// ---------------------------------------------------------------------------
// The mirror follows the mem through its lifecycle: a rename moves the
// binding records and the per-binding state with the mem, on disk and on
// the schema-and-config ref, with every mem-prefixed id rewritten; a
// destructive delete removes both; a create seeds the mirror from the
// records already on disk for that name. Until 2026-09-11 a rename left
// the old mirror rows under the old leaf and the advance state under the
// old name, and a delete left the records and their mirror behind, a row
// pointing at a mem that no longer existed.
// ---------------------------------------------------------------------------

/// The blob at `path` in `__MEMSTEAD`'s tip tree, if present.
fn memstead_tree_blob(workspace_root: &std::path::Path, path: &str) -> Option<Vec<u8>> {
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
    let entry = tree.lookup_entry_by_path(path).ok().flatten()?;
    Some(entry.object().expect("blob").data.clone())
}

fn seed_state_files(root: &std::path::Path, mem: &str) {
    let findings = root.join(".memstead/state/findings").join(mem);
    std::fs::create_dir_all(&findings).unwrap();
    std::fs::write(
        findings.join("graph.json"),
        format!(
            r#"{{"binding":"{mem}/graph","batches":[{{"key":{{"binding_hash":"h"}},"findings":[{{"key":{{"entity":"{mem}--one","artifact":"{mem}/graph/codebase#src/a.rs"}},"facet":"identity","target":{{"kind":"anchor","entity":"{mem}--one","artifact":"{mem}/graph/codebase#src/a.rs"}},"class":"drifted","detail":"{mem}/graph moved","created_at":"2026-09-11T00:00:00Z"}}]}}]}}"#
        ),
    )
    .unwrap();
    let advance = root.join(".memstead/state/advance").join(mem);
    std::fs::create_dir_all(&advance).unwrap();
    std::fs::write(
        advance.join("graph.json"),
        format!(
            r#"{{"binding":"{mem}/graph","frozen_slice":{{"added":["{mem}/graph/codebase#src/a.rs"],"modified":[],"deleted":[]}},"dispositions":{{"{mem}/graph/codebase#src/a.rs":"worked"}}}}"#
        ),
    )
    .unwrap();
}

#[test]
fn mem_rename_moves_the_mirror_and_the_state_and_rewrites_every_mem_id() {
    use memstead_base::mem_management::{MemRenameParams, rename_mem};
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    engine
        .add_projection_json("specs", "graph", BINDING_PATCH, Some("bound"))
        .expect("add lands");
    seed_state_files(tmp.path(), "specs");

    rename_mem(
        &mut engine,
        MemRenameParams {
            old: "specs".to_string(),
            new: "plans".to_string(),
            operator_mode: true,
            note: Some("renamed for the pin".to_string()),
        },
    )
    .expect("rename lands");

    // The mirror moved with the mem and its record names the new mem.
    assert!(
        !memstead_tree_has(tmp.path(), "pipeline/projections/specs/graph.json"),
        "old mirror row gone"
    );
    let mirrored = memstead_tree_blob(tmp.path(), "pipeline/projections/plans/graph.json")
        .expect("mirror row under the new leaf");
    let mirrored: serde_json::Value = serde_json::from_slice(&mirrored).unwrap();
    assert_eq!(
        mirrored["destination_mem"], "plans",
        "mirror record rewritten: {mirrored}"
    );
    // The mirror equals the disk record.
    let on_disk = std::fs::read(tmp.path().join(".memstead/projections/plans/graph.json")).unwrap();
    let on_disk: serde_json::Value = serde_json::from_slice(&on_disk).unwrap();
    assert_eq!(mirrored, on_disk, "mirror and disk agree after the rename");

    // Every per-mem state directory moved, and every mem-prefixed id inside
    // reads the new name.
    for dir in ["projections", "state/findings", "state/advance"] {
        assert!(
            !tmp.path()
                .join(".memstead")
                .join(dir)
                .join("specs")
                .exists(),
            "{dir}/specs gone"
        );
        let file = tmp
            .path()
            .join(".memstead")
            .join(dir)
            .join("plans/graph.json");
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|_| panic!("{dir}/plans/graph.json present"));
        assert!(
            !text.contains("specs/") && !text.contains("specs--") && !text.contains("\"specs\""),
            "{dir}: no id names the old mem: {text}"
        );
        if dir != "projections" {
            assert!(
                text.contains("plans/graph"),
                "{dir}: ids name the new mem: {text}"
            );
        }
    }
    let advance: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(tmp.path().join(".memstead/state/advance/plans/graph.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(advance["binding"], "plans/graph");
    assert_eq!(
        advance["dispositions"]["plans/graph/codebase#src/a.rs"],
        "worked"
    );
    let findings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(tmp.path().join(".memstead/state/findings/plans/graph.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        findings["batches"][0]["findings"][0]["target"]["entity"],
        "plans--one"
    );
}

#[test]
fn mem_delete_removes_the_records_their_state_and_the_mirror() {
    use memstead_base::mem_management::{self, MemDeleteParams};
    use memstead_base::vcs::Actor;
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    engine
        .add_projection_json("specs", "graph", BINDING_PATCH, Some("bound"))
        .expect("add lands");
    seed_state_files(tmp.path(), "specs");

    mem_management::delete_mem(
        &mut engine,
        MemDeleteParams {
            name: "specs".to_string(),
            delete_files: true,
            note: Some("retired".to_string()),
            actor: Actor::Cli,
            client: None,
            operator_mode: true,
            detach_incoming: false,
        },
    )
    .expect("delete lands");

    assert!(
        !memstead_tree_has(tmp.path(), "pipeline/projections/specs/graph.json"),
        "mirror row pruned with the mem"
    );
    assert!(
        !memstead_tree_has(tmp.path(), "mems/specs/config.json"),
        "config blob pruned"
    );
    for dir in ["projections", "state/findings", "state/advance"] {
        assert!(
            !tmp.path()
                .join(".memstead")
                .join(dir)
                .join("specs")
                .exists(),
            "{dir}/specs removed with the mem"
        );
    }
}

#[test]
fn mem_unregister_keeps_the_records_and_the_mirror() {
    use memstead_base::mem_management::{self, MemDeleteParams};
    use memstead_base::vcs::Actor;
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    engine
        .add_projection_json("specs", "graph", BINDING_PATCH, Some("bound"))
        .expect("add lands");
    seed_state_files(tmp.path(), "specs");

    mem_management::delete_mem(
        &mut engine,
        MemDeleteParams {
            name: "specs".to_string(),
            delete_files: false,
            note: Some("unregistered".to_string()),
            actor: Actor::Cli,
            client: None,
            operator_mode: true,
            detach_incoming: false,
        },
    )
    .expect("unregister lands");

    assert!(
        memstead_tree_has(tmp.path(), "pipeline/projections/specs/graph.json"),
        "an unregister keeps the mirror, as it keeps the branch"
    );
    for dir in ["projections", "state/findings", "state/advance"] {
        assert!(
            tmp.path()
                .join(".memstead")
                .join(dir)
                .join("specs/graph.json")
                .is_file(),
            "{dir}/specs kept by an unregister"
        );
    }
}

#[test]
fn mem_create_seeds_the_mirror_from_the_records_on_disk() {
    use memstead_base::mem_management::{self, MemCreateParams, StorageKind};
    use memstead_base::vcs::Actor;
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");

    // A binding declared before its destination exists (legal: the
    // destination may arrive later) sits on disk with no mirror row.
    let later_patch = BINDING_PATCH.replace("\"specs\"", "\"later\"");
    memstead_base::pipeline_edit::add_binding_json(tmp.path(), "later", "graph", &later_patch)
        .expect("binding on disk");
    assert!(!memstead_tree_has(
        tmp.path(),
        "pipeline/projections/later/graph.json"
    ));

    mem_management::create_mem(
        &mut engine,
        MemCreateParams {
            name: "later".to_string(),
            location: std::path::PathBuf::from("later"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            vcs: None,
            note: Some("created after its binding".to_string()),
            operator_mode: true,
            recovery: None,
            write_guidance: Default::default(),
            storage: Some(StorageKind::GitBranch),
            actor: Actor::Cli,
            client: None,
        },
    )
    .expect("create lands");

    let mirrored = memstead_tree_blob(tmp.path(), "pipeline/projections/later/graph.json")
        .expect("the create seeds the mirror from the disk record");
    let on_disk = std::fs::read(tmp.path().join(".memstead/projections/later/graph.json")).unwrap();
    assert_eq!(mirrored, on_disk, "mirror equals the disk record");
}
