//! Mixed-backend workspace creation: `create_mem` with an explicit
//! `storage: Some(StorageKind::Folder)` override lands a folder mem
//! inside a mem-repo (git-branch) workspace. The mount loader and
//! runtime already dispatch per-mount (`MountStorageWire` handles
//! `type: "folder"`, `instantiate_full_backend` routes folder mounts
//! through the lean backend) — this test covers the creation surface
//! that used to be heuristic-only, plus the reboot round-trip that
//! proves both backends coexist in one workspace.

use memstead_base::CreateEntityArgs;
use memstead_base::vcs::Actor;
use memstead_base::workspace::MountStorage;
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

/// Minimal recursive tree copy for the clone-portability assertion —
/// mirrors `cp -R` for regular files and directories (the only node
/// kinds these fixtures produce).
fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_recursive(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A folder mem whose location lies OUTSIDE the workspace root
/// (`../side/<name>`, the monorepo/submodule case): the expressed
/// relative form round-trips through `mounts.json` unchanged, the
/// mount survives reboot, and — the portability contract — the whole
/// tree copied to a different absolute prefix still resolves the
/// mount and stays writable there.
#[test]
fn out_of_root_folder_mount_round_trips_relative_and_survives_clone() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    init_real_mem_repo(&ws, &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(&ws).expect("engine boots");

    mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            name: "oor-notes".to_string(),
            location: std::path::PathBuf::from("../side/oor-notes"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            vcs: None,
            note: None,
            operator_mode: true,
            recovery: None,
            write_guidance: Default::default(),
            storage: Some(StorageKind::Folder),
        },
    )
    .expect("operator-mode out-of-root folder create succeeds");

    // The physical mem landed outside the workspace root, beside it.
    assert!(
        tmp.path()
            .join("side/oor-notes/.memstead/config.json")
            .is_file(),
        "mem config must land at <parent>/side/oor-notes/, outside the workspace root"
    );

    // `mounts.json` records the caller's expressed relative form —
    // not an absolute path — so the record survives a clone to a
    // different absolute prefix.
    let mounts_text =
        std::fs::read_to_string(ws.join(".memstead/state/mounts.json")).expect("mounts.json");
    assert!(
        mounts_text.contains("../side/oor-notes"),
        "mounts.json must carry the relative out-of-root path, got:\n{mounts_text}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&mounts_text).unwrap();
    let oor_path = parsed["mounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["mem"] == "oor-notes")
        .expect("oor-notes mount present")["storage"]["path"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        oor_path, "../side/oor-notes",
        "the serialised mount path must be exactly the expressed relative form"
    );

    create_entity_in(&mut engine, "oor-notes", "Out Of Root Note");

    // Reboot from the same root resolves the out-of-root mount.
    drop(engine);
    let mut rebooted = engine_from_workspace_root(&ws).expect("reboot succeeds");
    let mount = rebooted.mount("oor-notes").expect("mount survives reboot");
    assert!(
        matches!(mount.storage, MountStorage::Folder { .. }),
        "oor-notes must reload as MountStorage::Folder, got {:?}",
        mount.storage
    );
    create_entity_in(&mut rebooted, "oor-notes", "Post Reboot Note");
    drop(rebooted);

    // Clone-portability: copy the WHOLE tree (workspace + sibling
    // mem dir) to a different absolute prefix; the relative anchor
    // must resolve there with no path rewriting.
    let clone = TempDir::new().unwrap();
    copy_dir_recursive(tmp.path(), clone.path());
    let ws2 = clone.path().join("ws");
    let mut cloned = engine_from_workspace_root(&ws2).expect("cloned workspace boots");
    cloned
        .mount("oor-notes")
        .expect("out-of-root mount resolves at the new absolute prefix");
    create_entity_in(&mut cloned, "oor-notes", "Cloned Tree Note");
    // The write landed in the CLONE's sibling dir, not the original's.
    let cloned_md: Vec<_> = std::fs::read_dir(clone.path().join("side/oor-notes"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .collect();
    assert!(
        cloned_md
            .iter()
            .any(|e| std::fs::read_to_string(e.path()).unwrap().contains("Cloned Tree Note")),
        "the cloned workspace's write must land in the cloned side dir"
    );
    assert!(
        !std::fs::read_dir(tmp.path().join("side/oor-notes"))
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.path().extension().is_some_and(|x| x == "md")
                    && std::fs::read_to_string(e.path())
                        .unwrap()
                        .contains("Cloned Tree Note")
            }),
        "the original tree must not receive the clone's write"
    );
}

/// The `engineering@0.1.0` builtin enforces the knowledge/system-model
/// class boundary at write time: a mem pinned to it accepts `decision`
/// / `principle` / `memo` and refuses a current-state type (`spec`)
/// with `UNKNOWN_ENTITY_TYPE`.
#[test]
fn engineering_schema_refuses_current_state_types_at_write() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");

    mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            name: "knowledge".to_string(),
            location: std::path::PathBuf::from("knowledge"),
            schema_ref: "engineering@0.1.0".parse().unwrap(),
            vcs: None,
            note: None,
            operator_mode: true,
            recovery: None,
            write_guidance: Default::default(),
            storage: Some(StorageKind::Folder),
        },
    )
    .expect("mem pinned to the engineering builtin creates");

    // The knowledge types write.
    let mut sections = indexmap::IndexMap::new();
    sections.insert("decision".to_string(), "We chose the gate.".to_string());
    sections.insert("context".to_string(), "Boundary test.".to_string());
    sections.insert("consequences".to_string(), "- enforced".to_string());
    let mut metadata: indexmap::IndexMap<String, String> = Default::default();
    metadata.insert("decided_on".to_string(), "2026-07-13".to_string());
    metadata.insert("deciders".to_string(), "test".to_string());
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "knowledge".to_string(),
                title: "Gate The Boundary".to_string(),
                entity_type: "decision".to_string(),
                sections,
                metadata,
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .expect("decision entity writes into the engineering mem");

    // A current-state type refuses — the class boundary is a gate.
    let mut spec_sections = indexmap::IndexMap::new();
    spec_sections.insert("identity".to_string(), "x".to_string());
    spec_sections.insert("purpose".to_string(), "x".to_string());
    let err = engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "knowledge".to_string(),
                title: "Smuggled Spec".to_string(),
                entity_type: "spec".to_string(),
                sections: spec_sections,
                metadata: Default::default(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .expect_err("spec must refuse in an engineering-pinned mem");
    assert_eq!(
        err.code(),
        "UNKNOWN_ENTITY_TYPE",
        "the refusal must be the typed class-boundary gate, got {err:?}"
    );
}

/// The mem-replacement affordance: `delete_mem` with
/// `detach_incoming: true` succeeds despite live incoming cross-mem
/// edges (reporting the detached referrers), the referrer's file
/// keeps its relationship row, and a same-name re-creation re-adopts
/// the edge — the re-homing flow under a stable name. Without the
/// flag the same delete refuses `MEM_HAS_INCOMING_REFS`.
#[test]
fn detach_incoming_delete_supports_same_name_rehoming() {
    let tmp = TempDir::new().unwrap();
    // Static cross-link grant so the referrer may edge into the
    // target; both mems seeded at boot (static entries must name
    // existing mems).
    std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
    std::fs::write(
        tmp.path().join(".memstead/workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n\
         [persistence_adapter]\nname = \"file-two-layer\"\n\n\
         [cross_mem_links]\nreferrer = [\"target-mem\"]\n",
    )
    .unwrap();
    init_real_mem_repo(
        tmp.path(),
        &[("target-mem", "default@1.0.0"), ("referrer", "default@1.0.0")],
    );
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    create_entity_in(&mut engine, "target-mem", "Anchor Spec");
    create_entity_in(&mut engine, "referrer", "Pointing Spec");
    engine
        .relate_entity(
            memstead_base::RelateEntityArgs {
                source: memstead_base::EntityId::new("referrer", "pointing-spec"),
                expected_hash: None,
                rel_type: "DEPENDS_ON".to_string(),
                target: memstead_base::EntityId::new("target-mem", "anchor-spec"),
                remove: false,
                description: None,
            },
            Actor::Cli,
            None,
            None,
        )
        .expect("cross-mem edge lands under the grant");
    drop(engine);

    // Revoke the grant on disk (policy gate would otherwise fire
    // first and mask the incoming-refs axis), reboot, and assert the
    // edge-graph refusal without the flag.
    memstead_engine::workspace_config_edit::revoke_cross_link(
        tmp.path(),
        "referrer",
        &memstead_engine::workspace_config_edit::CrossLinkTarget::Named("target-mem".to_string()),
    )
    .expect("revoke grant");
    let mut engine = engine_from_workspace_root(tmp.path()).expect("reboot after revoke");
    let err = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "target-mem".to_string(),
            delete_files: true,
            note: None,
            operator_mode: true,
            detach_incoming: false,
        },
    )
    .expect_err("live incoming edge must refuse without detach");
    assert_eq!(err.code(), "MEM_HAS_INCOMING_REFS");

    let response = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "target-mem".to_string(),
            delete_files: true,
            note: None,
            operator_mode: true,
            detach_incoming: true,
        },
    )
    .expect("detach-incoming delete succeeds despite the live edge");
    assert_eq!(response.detached_referrers.len(), 1);
    assert_eq!(
        response.detached_referrers[0].from_id,
        "referrer--pointing-spec"
    );
    assert!(
        response.detached_referrers[0]
            .rel_types
            .contains(&"DEPENDS_ON".to_string())
    );

    // Same-name re-creation (folder backend this time — the re-homing
    // case) + same-slug entity; the referrer's file was never touched,
    // so a fresh boot re-adopts the edge.
    mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            name: "target-mem".to_string(),
            location: std::path::PathBuf::from("target-mem"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            vcs: None,
            note: None,
            operator_mode: true,
            recovery: None,
            write_guidance: Default::default(),
            storage: Some(StorageKind::Folder),
        },
    )
    .expect("same-name re-creation succeeds");
    create_entity_in(&mut engine, "target-mem", "Anchor Spec");
    drop(engine);

    let rebooted = engine_from_workspace_root(tmp.path()).expect("reboot succeeds");
    let incoming: Vec<String> = rebooted
        .store()
        .incoming(&memstead_base::EntityId::new("target-mem", "anchor-spec"))
        .iter()
        .map(|e| format!("{} {}", e.rel_type, e.from))
        .collect();
    assert!(
        incoming
            .iter()
            .any(|s| s == "DEPENDS_ON referrer--pointing-spec"),
        "the detached edge must re-adopt onto the re-homed same-name mem; got {incoming:?}"
    );
}

/// Out-of-root placement stays operator-gated: an agent-mode create
/// whose location resolves outside the workspace root refuses with
/// `MEM_PATH_NOT_ALLOWED` / `outside_workspace` even when the name
/// matches a create rule.
#[test]
fn agent_mode_out_of_root_location_refuses_outside_workspace() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(ws.join(".memstead")).unwrap();
    // Allowlist admits the name, so the refusal below is attributable
    // to the location, not to a missing rule.
    std::fs::write(
        ws.join(".memstead/workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n\
         [persistence_adapter]\nname = \"file-two-layer\"\n\n\
         [[mem_management.create]]\npattern = \"oor-*\"\nschemas = [\"default@1.0.0\"]\n",
    )
    .unwrap();
    init_real_mem_repo(&ws, &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(&ws).expect("engine boots");

    let err = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            name: "oor-escape".to_string(),
            location: std::path::PathBuf::from("../side/oor-escape"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            vcs: None,
            note: None,
            operator_mode: false,
            recovery: None,
            write_guidance: Default::default(),
            storage: Some(StorageKind::Folder),
        },
    )
    .expect_err("agent-mode out-of-root create must refuse");
    match err {
        memstead_engine::error::FullEngineError::MemPathNotAllowed { reason, .. } => {
            assert_eq!(reason, "outside_workspace");
        }
        other => panic!("expected MemPathNotAllowed/outside_workspace, got {other:?}"),
    }
    assert!(
        !tmp.path().join("side").exists(),
        "the refused create must leave no disk residue outside the root"
    );
}
