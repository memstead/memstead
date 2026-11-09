//! What a mutation reports once its commit is past the commit point:
//! a failed follow-up step is a warning on a successful call with the
//! engine's view level with storage, and a folder commit that is
//! decided and not applied is completed by the next operation.

use std::collections::BTreeMap;
use std::path::Path;

use indexmap::IndexMap;
use tempfile::TempDir;

use super::test_fault;
use crate::backend::MemBackend;
use crate::engine::test_helpers::{cli_actor, empty_create_args, folder_mount};
use crate::engine::{
    Engine, RelateEntityArgs, RenameEntityArgs, RetypeEntityArgs, UpdateEntityArgs,
};
use crate::entity::EntityId;
use crate::storage::FilesystemBackend;
use crate::storage::filesystem::fault_io::FaultIo;

const MEM: &str = "specs";

fn id(slug: &str) -> EntityId {
    EntityId::new(MEM, slug)
}

fn engine_over(dir: &Path) -> Engine {
    Engine::from_mounts(vec![(
        folder_mount(MEM, dir.to_path_buf()),
        Box::new(FilesystemBackend::new(dir.to_path_buf())) as Box<dyn MemBackend>,
    )])
    .expect("engine boots")
}

/// A mem with three entities and one relation.
fn seeded() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let mut engine = engine_over(tmp.path());
    let (actor, client) = cli_actor();
    for title in ["Alpha", "Beta", "Gamma"] {
        engine
            .create_entity(empty_create_args(MEM, title), actor, Some(&client), None)
            .unwrap();
    }
    engine
        .relate_entity(relate_args("alpha", "beta"), actor, Some(&client), None)
        .unwrap();
    tmp
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

fn relate_args(from: &str, to: &str) -> RelateEntityArgs {
    RelateEntityArgs {
        source: id(from),
        expected_hash: None,
        rel_type: "USES".to_string(),
        target: id(to),
        remove: false,
        description: None,
        dry_run: false,
    }
}

fn update_args(engine: &Engine, slug: &str, text: &str) -> UpdateEntityArgs {
    UpdateEntityArgs {
        id: id(slug),
        expected_hash: hash_of(engine, slug),
        sections: IndexMap::from_iter([("purpose".to_string(), text.to_string())]),
        append_sections: Default::default(),
        patch_sections: Default::default(),
        sections_unset: Vec::new(),
        metadata: Default::default(),
        metadata_unset: Vec::new(),
        dry_run: false,
        declare_relations: Vec::new(),
        anchors: Vec::new(),
        anchors_unset: Vec::new(),
        relations_unset: Vec::new(),
    }
}

fn hash_of(engine: &Engine, slug: &str) -> Option<String> {
    engine.get_entity(&id(slug)).map(|e| e.content_hash.clone())
}

/// The verbs whose tail updates the store after the commit. Delete and
/// batch relate are not here: their store work is done before or
/// without a fallible step, so they have no follow-up to fail.
const VERBS: &[&str] = &[
    "create",
    "update",
    "relate",
    "retype",
    "rename",
    "batch_create",
    "batch_update",
];

/// Run one verb and return the warning codes of its receipt, with the
/// `details` of every follow-up warning.
fn run(
    verb: &str,
    engine: &mut Engine,
) -> Result<Vec<crate::ops::WarningHint>, crate::EngineError> {
    let (actor, client) = cli_actor();
    let client = Some(&client);
    Ok(match verb {
        "create" => {
            engine
                .create_entity(empty_create_args(MEM, "Delta"), actor, client, None)?
                .warnings
        }
        "update" => {
            let args = update_args(engine, "alpha", "rewritten purpose");
            engine.update_entity(args, actor, client, None)?.warnings
        }
        "relate" => {
            engine
                .relate_entity(relate_args("gamma", "alpha"), actor, client, None)?
                .warnings
        }
        "retype" => {
            let args = RetypeEntityArgs {
                id: id("gamma"),
                expected_hash: hash_of(engine, "gamma"),
                target_type: "concept".to_string(),
                section_map: IndexMap::from_iter([
                    ("identity".to_string(), "definition".to_string()),
                    ("purpose".to_string(), "explanation".to_string()),
                ]),
                drop_metadata: vec!["level".to_string()],
                dry_run: false,
            };
            engine.retype_entity(args, actor, client, None)?.warnings
        }
        "rename" => {
            let args = RenameEntityArgs {
                id: id("beta"),
                expected_hash: hash_of(engine, "beta"),
                new_title: "Beta Renamed".to_string(),
            };
            engine.rename_entity(args, actor, client, None)?.warnings
        }
        "batch_create" => {
            let creates = vec![
                (empty_create_args(MEM, "Delta"), None),
                (empty_create_args(MEM, "Epsilon"), None),
            ];
            engine.batch_create(creates, actor, client, false)?.warnings
        }
        "batch_update" => {
            let updates = vec![
                (update_args(engine, "alpha", "batch purpose"), None),
                (update_args(engine, "gamma", "batch purpose"), None),
            ];
            engine.batch_update(updates, actor, client, false)?.warnings
        }
        other => unreachable!("unknown verb {other}"),
    })
}

/// The engine's view of the mem: every entity with its hash and its
/// relations in both directions, plus search hits. `hashes` off drops
/// the content hash, for comparing runs that differ in timestamps.
fn view(engine: &Engine, hashes: bool) -> BTreeMap<String, String> {
    let store = engine.store();
    let mut out = BTreeMap::new();
    for entity in store.all_entities() {
        let mut outgoing: Vec<String> = store
            .outgoing(&entity.id)
            .iter()
            .map(|e| format!("{}>{}", e.rel_type, e.target))
            .collect();
        outgoing.sort();
        let mut incoming: Vec<String> = store
            .incoming(&entity.id)
            .iter()
            .map(|e| format!("{}<{}", e.rel_type, e.from))
            .collect();
        incoming.sort();
        let hash = if hashes {
            entity.content_hash.as_str()
        } else {
            ""
        };
        out.insert(
            entity.id.to_string(),
            format!(
                "stub={} type={} title={:?} hash={hash} out={outgoing:?} in={incoming:?}",
                entity.stub, entity.entity_type, entity.title
            ),
        );
    }
    for term in ["purpose", "rewritten", "batch", "delta", "renamed"] {
        let scope = crate::ops::SearchScope {
            query: Some(crate::ops::Query {
                any: vec![term.to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut hits: Vec<String> = engine
            .search(&scope)
            .map(|r| r.hits.iter().map(|h| h.id.to_string()).collect())
            .unwrap_or_else(|e| vec![format!("search failed: {e}")]);
        hits.sort();
        out.insert(format!("~search:{term}"), format!("{hits:?}"));
    }
    out
}

/// A store update that fails after the commit landed: the call reports
/// success with a warning that names the step, and the running engine
/// shows what a freshly loaded engine shows, the write included.
#[test]
fn a_failed_store_update_is_a_warning_on_a_landed_write() {
    let template = seeded();
    for verb in VERBS {
        // What an undisturbed call leaves.
        let control_dir = TempDir::new().unwrap();
        copy_dir(template.path(), control_dir.path());
        let mut control = engine_over(control_dir.path());
        run(verb, &mut control).unwrap_or_else(|e| panic!("{verb} undisturbed: {e:?}"));

        let dir = TempDir::new().unwrap();
        copy_dir(template.path(), dir.path());
        let mut engine = engine_over(dir.path());
        test_fault::arm();
        let warnings =
            run(verb, &mut engine).unwrap_or_else(|e| panic!("{verb}: reported as failed: {e:?}"));

        let follow_up: Vec<&crate::ops::WarningHint> = warnings
            .iter()
            .filter(|w| w.code() == "WRITE_FOLLOW_UP_FAILED")
            .collect();
        assert_eq!(follow_up.len(), 1, "{verb}: {warnings:?}");
        match follow_up[0] {
            crate::ops::WarningHint::WriteFollowUpFailed { mem, step, .. } => {
                assert_eq!(
                    (mem.as_str(), step.as_str()),
                    (MEM, "store_update"),
                    "{verb}"
                );
            }
            other => unreachable!("{other:?}"),
        }
        assert_eq!(
            view(&engine, true),
            view(&engine_over(dir.path()), true),
            "{verb}: the engine's view differs from a fresh load"
        );
        assert_eq!(
            view(&engine, false),
            view(&control, false),
            "{verb}: the landed write is missing from the engine's view"
        );
        // The write is the engine's own: the next operation sees no
        // drift to reload for.
        assert!(
            engine.reload_if_stale(Some(MEM)).is_empty(),
            "{verb}: the engine's own landed write reads as drift"
        );
    }
}

/// A folder commit cut off by an IO error after its commit point: the
/// call returns the typed condition, nothing of the commit is served
/// meanwhile, and the next operation on the mem completes it first and
/// names the completed files.
#[test]
fn a_folder_commit_cut_off_after_its_commit_point_is_completed_by_the_next_operation() {
    let template = seeded();

    // Where the commit point of this write falls, and how many steps
    // follow it.
    let probe_dir = TempDir::new().unwrap();
    copy_dir(template.path(), probe_dir.path());
    let io = FaultIo::new();
    let mut probe = Engine::from_mounts(vec![(
        folder_mount(MEM, probe_dir.path().to_path_buf()),
        Box::new(FilesystemBackend::with_io(
            probe_dir.path().to_path_buf(),
            io.clone(),
        )) as Box<dyn MemBackend>,
    )])
    .unwrap();
    io.reset();
    run("rename", &mut probe).unwrap();
    let commit_point = io.commit_point().expect("the rename commits");
    let steps = io.op_count();
    let control = view(&probe, false);
    assert!(steps > commit_point + 1);

    for step in commit_point + 1..steps {
        let dir = TempDir::new().unwrap();
        copy_dir(template.path(), dir.path());
        let io = FaultIo::new();
        let mut engine = Engine::from_mounts(vec![(
            folder_mount(MEM, dir.path().to_path_buf()),
            Box::new(FilesystemBackend::with_io(
                dir.path().to_path_buf(),
                io.clone(),
            )) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let before = view(&engine, true);

        io.reset();
        io.arm_fail_at(step);
        let err = run("rename", &mut engine).expect_err("the cut-off commit is not a success");
        assert_eq!(err.code(), "COMMIT_NOT_APPLIED", "step {step}: {err}");
        io.reset();
        // Nothing of the commit is served before it is completed.
        assert_eq!(view(&engine, true), before, "step {step}");

        // The next operation on the mem completes the commit first.
        let warnings = engine.reload_if_stale(Some(MEM));
        let completed: Vec<&crate::ops::WarningHint> = warnings
            .iter()
            .filter(|w| w.code() == "INTERRUPTED_COMMIT_COMPLETED")
            .collect();
        assert_eq!(completed.len(), 1, "step {step}: {warnings:?}");
        match completed[0] {
            crate::ops::WarningHint::InterruptedCommitCompleted { mem, files } => {
                assert_eq!(mem, MEM);
                assert!(
                    files.iter().any(|f| f == "beta-renamed.md")
                        && files.iter().any(|f| f == "beta.md"),
                    "step {step}: {files:?}"
                );
            }
            other => unreachable!("{other:?}"),
        }
        assert_eq!(view(&engine, false), control, "step {step}");
        assert_eq!(
            view(&engine, true),
            view(&engine_over(dir.path()), true),
            "step {step}: the engine's view differs from a fresh load"
        );
        assert!(
            !dir.path().join(".memstead/commit.json").exists(),
            "step {step}: the record of an applied commit remains"
        );
        // Completed once: the following operation reloads nothing.
        assert!(engine.reload_if_stale(Some(MEM)).is_empty(), "step {step}");
    }
}

/// A fresh engine over a mem whose last commit was cut off completes it
/// at boot and says so among its load warnings.
#[test]
fn a_boot_completes_a_cut_off_folder_commit_and_reports_it() {
    let template = seeded();
    let dir = TempDir::new().unwrap();
    copy_dir(template.path(), dir.path());
    let io = FaultIo::new();
    let mut engine = Engine::from_mounts(vec![(
        folder_mount(MEM, dir.path().to_path_buf()),
        Box::new(FilesystemBackend::with_io(
            dir.path().to_path_buf(),
            io.clone(),
        )) as Box<dyn MemBackend>,
    )])
    .unwrap();
    io.reset();
    run("update", &mut engine).unwrap();
    let commit_point = io.commit_point().unwrap();

    let dir = TempDir::new().unwrap();
    copy_dir(template.path(), dir.path());
    let io = FaultIo::new();
    let mut engine = Engine::from_mounts(vec![(
        folder_mount(MEM, dir.path().to_path_buf()),
        Box::new(FilesystemBackend::with_io(
            dir.path().to_path_buf(),
            io.clone(),
        )) as Box<dyn MemBackend>,
    )])
    .unwrap();
    io.reset();
    io.arm_fail_at(commit_point + 2);
    run("update", &mut engine).expect_err("cut off");
    drop(engine);

    let booted = engine_over(dir.path());
    assert!(
        booted
            .load_warnings()
            .iter()
            .any(|w| w.code() == "INTERRUPTED_COMMIT_COMPLETED"),
        "{:?}",
        booted.load_warnings()
    );
    let purpose = booted
        .get_entity(&id("alpha"))
        .and_then(|e| e.sections.get("purpose").cloned())
        .unwrap_or_default();
    assert!(purpose.contains("rewritten purpose"), "{purpose:?}");
}
