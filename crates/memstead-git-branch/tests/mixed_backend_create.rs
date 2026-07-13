//! Mixed-backend workspace creation: `create_mem` with an explicit
//! `storage: Some(StorageKind::Folder)` override lands a folder mem
//! inside a mem-repo (git-branch) workspace. The mount loader and
//! runtime already dispatch per-mount (`MountStorageWire` handles
//! `type: "folder"`, `instantiate_full_backend` routes folder mounts
//! through the lean backend) — this test covers the creation surface
//! that used to be heuristic-only, plus the reboot round-trip that
//! proves both backends coexist in one workspace.

use memstead_base::vcs::Actor;
use memstead_base::workspace::MountStorage;
use memstead_base::CreateEntityArgs;
use memstead_engine::mem_management::{self, StorageKind};
use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

/// `CreateEntityArgs.sections` seed satisfying the default schema's
/// required sections (`identity`, `purpose`).
fn seed_sections() -> indexmap::IndexMap<String, String> {
    let mut sections = indexmap::IndexMap::new();
    sections.insert("identity".to_string(), "seed identity".to_string());
    sections.insert("purpose".to_string(), "seed purpose".to_string());
    sections
}

fn create_entity_in(engine: &mut memstead_base::Engine, mem: &str, title: &str) {
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: mem.to_string(),
                title: title.to_string(),
                entity_type: "spec".to_string(),
                sections: seed_sections(),
                metadata: Default::default(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("create entity {title:?} in mem {mem:?}: {e:?}"));
}

/// Folder mem created via explicit override inside a mem-repo
/// workspace: registers as a folder mount, writes
/// `<location>/.memstead/config.json`, produces a synthetic (non-sha)
/// seed cursor, and survives a full reboot side by side with
/// git-branch mems — both writable.
#[test]
fn explicit_folder_create_lands_beside_git_branch_mems() {
    let tmp = TempDir::new().unwrap();
    // Fixture: mem-repo workspace with one git-branch mem ("specs").
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");

    // A heuristic create (storage: None) in this workspace still
    // yields a git-branch mem — the override must not disturb the
    // default path.
    let plans = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            name: "plans".to_string(),
            location: std::path::PathBuf::from("plans"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            vcs: None,
            note: None,
            operator_mode: true,
            recovery: None,
            write_guidance: Default::default(),
            storage: None,
        },
    )
    .expect("heuristic create yields git-branch mem");
    assert_eq!(
        plans.seed_commit_sha.len(),
        40,
        "heuristic git-branch create produces a real 40-hex sha, got {:?}",
        plans.seed_commit_sha
    );

    // The explicit-folder create.
    let response = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            name: "notes".to_string(),
            location: std::path::PathBuf::from("notes"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            vcs: None,
            note: None,
            operator_mode: true,
            recovery: None,
            write_guidance: Default::default(),
            storage: Some(StorageKind::Folder),
        },
    )
    .expect("explicit folder create succeeds in mem-repo workspace");

    // Location resolves to the workspace-relative folder path.
    let expected_location = tmp.path().canonicalize().unwrap().join("notes");
    assert_eq!(response.location, expected_location);

    // Seed cursor is a non-empty synthetic id, not a 40-hex sha.
    assert!(
        !response.seed_commit_sha.is_empty(),
        "seed cursor must be non-empty"
    );
    assert!(
        !(response.seed_commit_sha.len() == 40
            && response
                .seed_commit_sha
                .chars()
                .all(|c| c.is_ascii_hexdigit())),
        "folder seed cursor must be synthetic, got a 40-hex sha: {:?}",
        response.seed_commit_sha
    );

    // Mount registered as a folder mount.
    let mount = engine.mount("notes").expect("notes mount registered");
    assert!(
        matches!(mount.storage, MountStorage::Folder { .. }),
        "notes must register as MountStorage::Folder, got {:?}",
        mount.storage
    );

    // Per-mem config landed on disk (folder-backend identity).
    assert!(
        expected_location
            .join(".memstead")
            .join("config.json")
            .is_file(),
        "folder mem must carry .memstead/config.json on disk"
    );

    // Second boot from the same workspace root loads BOTH backends
    // side by side — and both are writable for real (entity creates
    // land, not just router flags).
    drop(engine);
    let mut rebooted = engine_from_workspace_root(tmp.path()).expect("second boot succeeds");
    let notes_mount = rebooted.mount("notes").expect("notes survives reboot");
    assert!(
        matches!(notes_mount.storage, MountStorage::Folder { .. }),
        "notes must reload as MountStorage::Folder, got {:?}",
        notes_mount.storage
    );
    let specs_mount = rebooted.mount("specs").expect("specs survives reboot");
    assert!(
        matches!(specs_mount.storage, MountStorage::GitBranch { .. }),
        "specs must reload as MountStorage::GitBranch, got {:?}",
        specs_mount.storage
    );
    create_entity_in(&mut rebooted, "notes", "Folder Note");
    create_entity_in(&mut rebooted, "specs", "Branch Spec");
}
