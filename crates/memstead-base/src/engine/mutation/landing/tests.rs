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
    DeleteEntityArgs, Engine, RelateEntityArgs, RenameEntityArgs, RetypeEntityArgs,
    UpdateEntityArgs,
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
        expected_hash: crate::ExpectedHash::from_token(hash_of(engine, slug)),
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

/// Every verb that writes to one mem.
const ALL_VERBS: &[&str] = &[
    "create",
    "update",
    "delete",
    "relate",
    "retype",
    "rename",
    "batch_create",
    "batch_update",
    "batch_relate",
];

/// Run one verb and return the warnings of its receipt.
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
        "delete" => {
            let args = DeleteEntityArgs {
                id: id("gamma"),
                expected_hash: crate::ExpectedHash::from_token(hash_of(engine, "gamma")),
            };
            engine.delete_entity(args, actor, client, None)?.warnings
        }
        "relate" => {
            engine
                .relate_entity(relate_args("gamma", "alpha"), actor, client, None)?
                .warnings
        }
        "batch_relate" => {
            let relates = vec![
                (relate_args("gamma", "alpha"), None),
                (relate_args("gamma", "beta"), None),
            ];
            applied(engine.batch_relate(relates, actor, client, false)?)?
        }
        "retype" => {
            let args = RetypeEntityArgs {
                id: id("gamma"),
                expected_hash: crate::ExpectedHash::from_token(hash_of(engine, "gamma")),
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
                expected_hash: crate::ExpectedHash::from_token(hash_of(engine, "beta")),
                new_title: "Beta Renamed".to_string(),
            };
            engine.rename_entity(args, actor, client, None)?.warnings
        }
        "batch_create" => {
            let creates = vec![
                (empty_create_args(MEM, "Delta"), None),
                (empty_create_args(MEM, "Epsilon"), None),
            ];
            applied(engine.batch_create(creates, actor, client, false)?)?
        }
        "batch_update" => {
            let updates = vec![
                (update_args(engine, "alpha", "batch purpose"), None),
                (update_args(engine, "gamma", "batch purpose"), None),
            ];
            applied(engine.batch_update(updates, actor, client, false)?)?
        }
        other => unreachable!("unknown verb {other}"),
    })
}

/// A batch reports a refusal in its receipt, not as an error: read a
/// receipt that did not apply as the refusal it is.
fn applied(
    result: crate::ops::BatchResult,
) -> Result<Vec<crate::ops::WarningHint>, crate::EngineError> {
    if result.applied {
        return Ok(result.warnings);
    }
    Err(crate::EngineError::Mem(
        result
            .results
            .iter()
            .find_map(|entry| entry.error.as_ref().map(|e| e.code.clone()))
            .unwrap_or_else(|| "BATCH_NOT_APPLIED".to_string()),
    ))
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

fn faulted_engine(dir: &Path, io: &FaultIo) -> Engine {
    Engine::from_mounts(vec![(
        folder_mount(MEM, dir.to_path_buf()),
        Box::new(FilesystemBackend::with_io(dir.to_path_buf(), io.clone())) as Box<dyn MemBackend>,
    )])
    .expect("engine boots")
}

/// A folder commit hit by an IO error after its commit point, on every
/// verb and at every step: the commit is decided, so the engine
/// completes it at once. The call reports success and names the step,
/// and the running engine shows what a freshly loaded engine shows.
#[test]
fn an_io_error_after_the_commit_point_is_a_warning_on_a_landed_write() {
    let template = seeded();
    let mut cells = 0;
    for verb in ALL_VERBS {
        // The undisturbed call: its result, and where its commit point
        // falls among its IO steps.
        let control_dir = TempDir::new().unwrap();
        copy_dir(template.path(), control_dir.path());
        let io = FaultIo::new();
        let mut control = faulted_engine(control_dir.path(), &io);
        io.reset();
        run(verb, &mut control).unwrap_or_else(|e| panic!("{verb} undisturbed: {e:?}"));
        let commit_point = io.commit_point().expect("the verb commits");
        let steps = io.op_count();
        assert!(steps > commit_point + 1, "{verb}");

        for step in commit_point + 1..steps {
            let dir = TempDir::new().unwrap();
            copy_dir(template.path(), dir.path());
            let io = FaultIo::new();
            let mut engine = faulted_engine(dir.path(), &io);
            io.reset();
            io.arm_fail_at(step);
            let warnings = run(verb, &mut engine).unwrap_or_else(|e| {
                panic!("{verb} step {step}: a decided commit was reported as failed: {e:?}")
            });
            io.reset();

            assert!(
                warnings.iter().any(|w| matches!(
                    w,
                    crate::ops::WarningHint::WriteFollowUpFailed { mem, step, .. }
                        if mem == MEM && step == "commit_apply"
                )),
                "{verb} step {step}: no warning names the failed step: {warnings:?}"
            );
            assert_eq!(
                view(&engine, true),
                view(&engine_over(dir.path()), true),
                "{verb} step {step}: the engine's view differs from a fresh load"
            );
            assert_eq!(
                view(&engine, false),
                view(&control, false),
                "{verb} step {step}: the landed write is missing from the engine's view"
            );
            assert!(
                !dir.path().join(".memstead/commit.json").exists(),
                "{verb} step {step}: the record of an applied commit remains"
            );
            assert!(
                engine.reload_if_stale(Some(MEM)).is_empty(),
                "{verb} step {step}: the engine's own landed write reads as drift"
            );
            cells += 1;
        }
    }
    assert!(cells > 0);
}

/// A folder commit cut off after its commit point by a fault that
/// persists, so the engine cannot complete it at once: the call returns
/// the typed condition, nothing of the commit is served meanwhile, and
/// once storage works again the next operation on the mem completes it
/// first and names the completed files.
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
        io.arm_fail_from(step);
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
    io.arm_fail_from(commit_point + 2);
    run("update", &mut engine).expect_err("cut off");
    drop(engine);

    let mut booted = engine_over(dir.path());
    let standing = |engine: &Engine| {
        engine
            .load_warnings()
            .iter()
            .any(|w| w.code() == "INTERRUPTED_COMMIT_COMPLETED")
    };
    assert!(standing(&booted), "{:?}", booted.load_warnings());
    // A boot has no response to carry the warning, so the first
    // operation's does. The completion was the engine's own, so that
    // operation sees no drift, reloads nothing, and the standing copy
    // among the load warnings stays.
    let first = booted.reload_if_stale(Some(MEM));
    let codes: Vec<&str> = first.iter().map(|w| w.code()).collect();
    assert_eq!(codes, ["INTERRUPTED_COMMIT_COMPLETED"], "{first:?}");
    assert!(
        standing(&booted),
        "the first operation dropped the standing warning"
    );
    assert!(booted.reload_if_stale(Some(MEM)).is_empty());
    let purpose = booted
        .get_entity(&id("alpha"))
        .and_then(|e| e.sections.get("purpose").cloned())
        .unwrap_or_default();
    assert!(purpose.contains("rewritten purpose"), "{purpose:?}");
}

/// While a cut-off commit cannot be completed, nothing of it is served:
/// not by the engine's own view, and not by a read that goes to storage
/// (the anchors of an entity are read from the sidecar file on every
/// call). The commit here writes an entity and its first anchor; the
/// fault persists from each step after the commit point in turn.
#[test]
fn a_cut_off_commit_is_not_served_in_part_by_a_read_that_goes_to_storage() {
    let template = seeded();
    let with_anchor = |engine: &mut Engine| {
        let mut args = update_args(engine, "alpha", "rewritten purpose");
        args.anchors = vec![crate::anchor::AnchorInput {
            artifact: Some("docs/alpha.md".to_string()),
            grain: Some("file".to_string()),
            class: Some("informed-by".to_string()),
            ..Default::default()
        }];
        let (actor, client) = cli_actor();
        engine.update_entity(args, actor, Some(&client), None)
    };

    let probe_dir = TempDir::new().unwrap();
    copy_dir(template.path(), probe_dir.path());
    let io = FaultIo::new();
    let mut probe = faulted_engine(probe_dir.path(), &io);
    io.reset();
    with_anchor(&mut probe).unwrap();
    let commit_point = io.commit_point().unwrap();
    let steps = io.op_count();
    assert_eq!(probe.entity_anchors(&id("alpha")).len(), 1);

    for step in commit_point + 1..steps {
        let dir = TempDir::new().unwrap();
        copy_dir(template.path(), dir.path());
        let io = FaultIo::new();
        let mut engine = faulted_engine(dir.path(), &io);
        let body_before = engine
            .get_entity(&id("alpha"))
            .unwrap()
            .content_hash
            .clone();
        assert!(engine.entity_anchors(&id("alpha")).is_empty());

        io.reset();
        io.arm_fail_from(step);
        let err = with_anchor(&mut engine).expect_err("the commit is cut off");
        assert_eq!(err.code(), "COMMIT_NOT_APPLIED", "step {step}: {err}");

        // The reads an entity lookup makes, with the fault still there.
        engine.reload_if_stale(Some(MEM));
        assert_eq!(
            engine.get_entity(&id("alpha")).unwrap().content_hash,
            body_before,
            "step {step}: the new body is served from a commit that is not applied"
        );
        assert!(
            engine.entity_anchors(&id("alpha")).is_empty(),
            "step {step}: the new anchor is served from a commit that is not applied"
        );
        // A walk of the mem's directory that bypasses the backend (a
        // publish reads this way) refuses as well, as long as the
        // record is on disk. At the last step it is gone and the files
        // are whole; only the sync of its removal is owed.
        if dir.path().join(".memstead/commit.json").exists() {
            assert!(
                crate::entity::source::EntitySource::Directory {
                    root: dir.path().to_path_buf(),
                }
                .read_all()
                .is_err(),
                "step {step}: the directory reader served a mem with a cut-off commit"
            );
        }

        // Storage works again: the next operation completes the commit
        // and both halves of it are there.
        io.reset();
        engine.reload_if_stale(Some(MEM));
        assert_ne!(
            engine.get_entity(&id("alpha")).unwrap().content_hash,
            body_before,
            "step {step}"
        );
        assert_eq!(engine.entity_anchors(&id("alpha")).len(), 1, "step {step}");
    }
}

/// A write never lands on a view that is known to be behind storage.
/// Here a sibling engine has written to the mem and this engine's
/// reload before its own write fails once: the write, whose hash still
/// matches the stale view, is refused with nothing written, and the
/// sibling's write survives. The next call reloads and gets the honest
/// hash mismatch.
#[test]
fn a_write_does_not_land_on_a_mem_whose_reload_failed() {
    let template = seeded();
    let dir = TempDir::new().unwrap();
    copy_dir(template.path(), dir.path());
    let io = FaultIo::new();
    let mut engine = faulted_engine(dir.path(), &io);
    let stale = update_args(&engine, "alpha", "this engine's rewrite");

    // The sibling's write.
    let mut sibling = engine_over(dir.path());
    run("update", &mut sibling).unwrap();
    let after_sibling = view(&engine_over(dir.path()), true);

    // The reload this engine runs before its write fails once.
    io.reset();
    io.arm_fail_at(0);
    let (actor, client) = cli_actor();
    let err = engine
        .update_entity(stale.clone(), actor, Some(&client), None)
        .expect_err("a write from a stale view must not land");
    assert_eq!(err.code(), "MEM_VIEW_STALE", "{err}");
    io.reset();
    assert_eq!(
        view(&engine_over(dir.path()), true),
        after_sibling,
        "the refused write changed storage"
    );

    // The next call reloads; the stale hash is then what it is.
    let err = engine
        .update_entity(stale, actor, Some(&client), None)
        .expect_err("the hash is stale");
    assert_eq!(err.code(), "HASH_MISMATCH", "{err}");
    assert_eq!(view(&engine, true), after_sibling);
}

/// A commit that is applied except for the sync of its record's removal
/// is not finished, and the next operation finishes it first, also when
/// the reload that would do so fails once in between: the write of that
/// operation is refused, and the operation after it reloads, reports
/// the completed commit and lands.
#[test]
fn an_unfinished_commit_is_finished_before_the_next_write_lands() {
    let template = seeded();

    let probe_dir = TempDir::new().unwrap();
    copy_dir(template.path(), probe_dir.path());
    let io = FaultIo::new();
    let mut probe = faulted_engine(probe_dir.path(), &io);
    io.reset();
    run("update", &mut probe).unwrap();
    let last_step = io.op_count() - 1;

    for next in ["create", "relate", "update", "retype", "delete"] {
        let dir = TempDir::new().unwrap();
        copy_dir(template.path(), dir.path());
        let io = FaultIo::new();
        let mut engine = faulted_engine(dir.path(), &io);

        // The update is applied; the sync of its record's removal
        // fails, and keeps failing while the engine tries at once.
        io.reset();
        io.arm_fail_from(last_step);
        let err = run("update", &mut engine).expect_err("the removal sync is owed");
        assert_eq!(err.code(), "COMMIT_NOT_APPLIED", "{next}: {err}");

        // One more failing step, then storage works. The operation that
        // meets the failure writes nothing.
        io.reset();
        io.arm_fail_at(0);
        let before = view(&engine_over(dir.path()), true);
        let refused = run(next, &mut engine);
        assert!(
            refused.is_err(),
            "{next}: a write landed although the commit before it was not finished"
        );
        io.reset();
        assert_eq!(
            view(&engine_over(dir.path()), true),
            before,
            "{next}: the refused write changed storage"
        );

        // The operation after it finishes the commit first and says so.
        let warnings = engine.reload_if_stale(Some(MEM));
        assert!(
            warnings
                .iter()
                .any(|w| w.code() == "INTERRUPTED_COMMIT_COMPLETED"),
            "{next}: the finished commit is not reported: {warnings:?}"
        );
        assert_eq!(
            view(&engine, true),
            view(&engine_over(dir.path()), true),
            "{next}: the engine's view differs from a fresh load"
        );
        if next != "update" {
            run(next, &mut engine).unwrap_or_else(|e| panic!("{next}: {e:?}"));
            assert_eq!(
                view(&engine, true),
                view(&engine_over(dir.path()), true),
                "{next}: after the write the engine's view differs from a fresh load"
            );
        }
    }
}

/// One failing storage step in the reload before the next operation,
/// at every step in turn, after a sibling's write and after a commit of
/// this engine that is applied but not finished. Whatever the step, the
/// next operation either lands on a view that is level with storage or
/// is refused with storage untouched; no entity goes missing from the
/// view, nothing on disk is overwritten, and a commit the reload
/// finished is reported by the time the view is level.
#[test]
fn a_fault_in_the_next_operations_reload_loses_neither_a_write_nor_a_report() {
    let template = seeded();

    // How many steps the unfinished-commit scenario's update takes.
    let probe_dir = TempDir::new().unwrap();
    copy_dir(template.path(), probe_dir.path());
    let io = FaultIo::new();
    let mut probe = faulted_engine(probe_dir.path(), &io);
    io.reset();
    run("update", &mut probe).unwrap();
    let last_step = io.op_count() - 1;

    for unfinished_commit in [false, true] {
        for next in ALL_VERBS {
            for failing_step in 0..10 {
                let label = format!(
                    "unfinished commit: {unfinished_commit}, next: {next}, step {failing_step}"
                );
                let dir = TempDir::new().unwrap();
                copy_dir(template.path(), dir.path());
                let io = FaultIo::new();
                let mut engine = faulted_engine(dir.path(), &io);
                if unfinished_commit {
                    io.reset();
                    io.arm_fail_from(last_step);
                    let err = run("update", &mut engine).expect_err("the removal sync is owed");
                    assert_eq!(err.code(), "COMMIT_NOT_APPLIED", "{label}");
                } else {
                    run("update", &mut engine_over(dir.path())).unwrap();
                }

                io.reset();
                io.arm_fail_at(failing_step);
                let before = view(&engine_over(dir.path()), true);
                let outcome = run(next, &mut engine);
                io.reset();
                let mut reported: Vec<String> = Vec::new();
                match &outcome {
                    Ok(warnings) => {
                        reported.extend(warnings.iter().map(|w| w.code().to_string()));
                        assert_eq!(
                            view(&engine, true),
                            view(&engine_over(dir.path()), true),
                            "{label}: the write landed and the engine's view differs from a \
                             fresh load"
                        );
                    }
                    Err(_) => assert_eq!(
                        view(&engine_over(dir.path()), true),
                        before,
                        "{label}: a refused call changed storage"
                    ),
                }
                // The first update is still on disk, whatever came next.
                let on_disk = std::fs::read_to_string(dir.path().join("alpha.md")).unwrap();
                if !(outcome.is_ok() && *next == "batch_update") {
                    assert!(
                        on_disk.contains("rewritten purpose"),
                        "{label}: the first write was overwritten"
                    );
                }

                reported.extend(
                    engine
                        .reload_if_stale(Some(MEM))
                        .iter()
                        .map(|w| w.code().to_string()),
                );
                assert_eq!(
                    view(&engine, true),
                    view(&engine_over(dir.path()), true),
                    "{label}: after the next operation the engine's view differs from a \
                     fresh load"
                );
                // The report rides the response of the operation whose
                // reload finished the commit. When that operation then
                // fails for a reason of its own, its response carries
                // the error; the report stands among the load warnings.
                if unfinished_commit {
                    let standing = engine
                        .load_warnings()
                        .iter()
                        .any(|w| w.code() == "INTERRUPTED_COMMIT_COMPLETED");
                    assert!(
                        standing,
                        "{label}: the finished commit is not among the load warnings"
                    );
                    assert!(
                        outcome.is_err()
                            || reported.iter().any(|c| c == "INTERRUPTED_COMMIT_COMPLETED"),
                        "{label}: the finished commit was never reported: {reported:?}"
                    );
                }
            }
        }
    }
}

/// A mem that boots with a file it could not read serves what it read
/// and takes no write: the unread entity is missing from the view with
/// nothing to show for it. The reload before the next write either
/// reads the whole mem, and the write lands on a view level with
/// storage, or fails again, and the write is refused with nothing
/// written.
#[test]
fn a_mem_that_booted_without_a_file_takes_no_write_until_it_is_whole() {
    let template = seeded();
    let whole = view(&engine_over(template.path()), true);
    let (actor, client) = cli_actor();

    let (mut partial_boots, mut refused_stale) = (0, 0);
    for boot_step in 0..12 {
        for reload_step in 0..12 {
            let dir = TempDir::new().unwrap();
            copy_dir(template.path(), dir.path());
            let io = FaultIo::fail_at(boot_step);
            let mut engine = faulted_engine(dir.path(), &io);
            // A step that fails the listing quarantines the mem at boot
            // (it serves nothing and takes no write); a step past the
            // last read changes nothing. The steps between cost the
            // view one entity.
            let booted = view(&engine, true);
            if booted == whole || !booted.keys().any(|k| k.starts_with(MEM)) {
                break;
            }
            if reload_step == 0 {
                partial_boots += 1;
            }
            let label = format!("boot step {boot_step}, reload step {reload_step}");

            io.reset();
            io.arm_fail_at(reload_step);
            let outcome =
                engine.create_entity(empty_create_args(MEM, "Delta"), actor, Some(&client), None);
            io.reset();
            match outcome {
                Ok(_) => assert_eq!(
                    view(&engine, true),
                    view(&engine_over(dir.path()), true),
                    "{label}: the write landed and the engine's view differs from a fresh load"
                ),
                Err(e) => {
                    if e.code() == "MEM_VIEW_STALE" {
                        refused_stale += 1;
                    }
                    assert_eq!(
                        view(&engine_over(dir.path()), true),
                        whole,
                        "{label}: a refused write changed storage ({e})"
                    );
                }
            }

            // Storage delivers: the next operation sees the whole mem.
            engine.reload_if_stale(Some(MEM));
            assert_eq!(
                view(&engine, true),
                view(&engine_over(dir.path()), true),
                "{label}"
            );
        }
    }
    assert!(partial_boots > 0, "no boot step cost the view an entity");
    assert!(
        refused_stale > 0,
        "no reload step left the view incomplete for the write"
    );
}

/// The operation whose load finishes an unfinished commit reports it,
/// whichever verb it is: the batch forms carry what their reloads
/// report like the single verbs do.
#[test]
fn every_verb_reports_the_commit_its_load_finished() {
    let template = seeded();
    let (actor, client) = cli_actor();
    let first = |engine: &mut Engine| {
        engine.create_entity(empty_create_args(MEM, "Zeta"), actor, Some(&client), None)
    };

    let probe_dir = TempDir::new().unwrap();
    copy_dir(template.path(), probe_dir.path());
    let io = FaultIo::new();
    let mut probe = faulted_engine(probe_dir.path(), &io);
    io.reset();
    first(&mut probe).unwrap();
    let commit_point = io.commit_point().unwrap();

    for next in ALL_VERBS {
        let dir = TempDir::new().unwrap();
        copy_dir(template.path(), dir.path());
        let io = FaultIo::new();
        let mut engine = faulted_engine(dir.path(), &io);
        io.reset();
        io.arm_fail_from(commit_point + 2);
        let err = first(&mut engine).expect_err("the commit is cut off");
        assert_eq!(err.code(), "COMMIT_NOT_APPLIED", "{next}: {err}");

        io.reset();
        let warnings = run(next, &mut engine).unwrap_or_else(|e| panic!("{next}: {e:?}"));
        assert!(
            warnings.iter().any(|w| matches!(
                w,
                crate::ops::WarningHint::InterruptedCommitCompleted { mem, files }
                    if mem == MEM && files.iter().any(|f| f == "zeta.md")
            )),
            "{next}: the commit its load finished is not on its response: {warnings:?}"
        );
        assert_eq!(
            view(&engine, true),
            view(&engine_over(dir.path()), true),
            "{next}: the engine's view differs from a fresh load"
        );
    }
}

/// The store update after a landed commit fails, and so does the reload
/// that should bring the view level. The write is the engine's own by
/// then, so no drift will ever reload the mem: it is marked instead,
/// takes no write, and the next operation reloads it. A write that
/// carries the hash from before the landed write gets the honest
/// mismatch and does not replace it.
#[test]
fn a_failed_levelling_reload_leaves_no_view_a_write_can_land_on() {
    let template = seeded();
    let (actor, client) = cli_actor();

    let probe_dir = TempDir::new().unwrap();
    copy_dir(template.path(), probe_dir.path());
    let io = FaultIo::new();
    let mut probe = faulted_engine(probe_dir.path(), &io);
    io.reset();
    run("update", &mut probe).unwrap();
    let commit_steps = io.op_count();

    for failing_step in 0..10 {
        let dir = TempDir::new().unwrap();
        copy_dir(template.path(), dir.path());
        let io = FaultIo::new();
        let mut engine = faulted_engine(dir.path(), &io);
        let stale = update_args(&engine, "alpha", "a rewrite from the view before");

        io.reset();
        io.arm_fail_at(commit_steps + failing_step);
        test_fault::arm();
        let warnings = run("update", &mut engine).unwrap_or_else(|e| {
            panic!("step {failing_step}: a landed write reported as failed: {e:?}")
        });
        io.reset();
        assert!(
            warnings
                .iter()
                .any(|w| w.code() == "WRITE_FOLLOW_UP_FAILED"),
            "step {failing_step}: {warnings:?}"
        );
        let landed = view(&engine_over(dir.path()), true);

        let err = engine
            .update_entity(stale, actor, Some(&client), None)
            .expect_err("a write with the hash from before the landed write must not land");
        assert_eq!(err.code(), "HASH_MISMATCH", "step {failing_step}: {err}");
        assert_eq!(
            view(&engine_over(dir.path()), true),
            landed,
            "step {failing_step}: the landed write was replaced"
        );
        assert_eq!(view(&engine, true), landed, "step {failing_step}");
    }
}
