//! Two writers over the same mems: the order their locks are taken
//! in, whatever way each spells a mem's path, and a guard that must not
//! run without a mem whose deferred load failed under the lock.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use memstead_base::backend::MemBackend;
use memstead_base::vcs::Actor;
use memstead_base::{
    CreateEntityArgs, DeleteEntityArgs, Engine, EntityId, Mount, MountCapability, MountLifecycle,
    MountStorage, UpdateEntityArgs,
};
use memstead_git_branch::storage::instantiate_full_backend;
use tempfile::TempDir;

use common::{Tap, Tapped, lock_is_free};

const LEFT: &str = "left";
const RIGHT: &str = "right";
const SCHEMA: &str = "default@1.0.0";

fn folder_mount(mem: &str, dir: &Path, capability: MountCapability) -> Mount {
    Mount {
        mem: mem.to_string(),
        schema: Some(SCHEMA.parse().unwrap()),
        storage: MountStorage::Folder {
            path: dir.to_path_buf(),
        },
        capability,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    }
}

fn seed_dir(dir: &Path) {
    std::fs::create_dir_all(dir.join(".memstead")).unwrap();
    std::fs::write(
        dir.join(".memstead/config.json"),
        format!(r#"{{"schema": "{SCHEMA}"}}"#),
    )
    .unwrap();
}

fn engine_of(mounts: &[Mount], tap: Option<Tap>) -> Engine {
    let mounts = mounts
        .iter()
        .map(|mount| {
            let backend = instantiate_full_backend(mount).expect("backend");
            let backend: Box<dyn MemBackend> = match &tap {
                Some(tap) => Box::new(Tapped {
                    inner: backend,
                    mem: if mount.mem == LEFT { LEFT } else { RIGHT },
                    tap: Arc::clone(tap),
                }),
                None => backend,
            };
            (mount.clone(), backend)
        })
        .collect();
    let mut engine = Engine::from_mounts(mounts).expect("engine boots");
    engine.set_git_branch_ops(memstead_git_branch::storage::FULL_GIT_BRANCH_OPS);
    engine.set_backend_factory(instantiate_full_backend);
    let mut settings = memstead_base::WorkspaceSettings::default();
    for mem in [LEFT, RIGHT] {
        settings.cross_mem_links.insert(
            mem.to_string(),
            memstead_schema::workspace_config::CrossLinkValue::Wildcard,
        );
    }
    engine.set_settings(settings);
    engine
}

fn id(mem: &str, slug: &str) -> EntityId {
    EntityId::new(mem, slug)
}

fn create_args(mem: &str, title: &str, uses: &[EntityId]) -> CreateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), format!("{title} identity"));
    sections.insert("purpose".to_string(), format!("{title} purpose"));
    CreateEntityArgs {
        mem: mem.to_string(),
        title: title.to_string(),
        entity_type: "spec".to_string(),
        sections,
        metadata: Default::default(),
        relations: uses
            .iter()
            .map(|target| memstead_base::ops::RelateArg {
                target: target.clone(),
                rel_type: "USES".to_string(),
                description: None,
            })
            .collect(),
        anchors: Vec::new(),
        dry_run: false,
    }
}

fn update_args(engine: &Engine, target: &EntityId, purpose: &str) -> UpdateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("purpose".to_string(), purpose.to_string());
    UpdateEntityArgs {
        id: target.clone(),
        expected_hash: memstead_base::ExpectedHash::from_token(
            engine.get_entity(target).map(|e| e.content_hash.clone()),
        ),
        sections,
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

fn lock_file(dir: &Path) -> PathBuf {
    dir.join(".memstead").join("locks").join("write.lock")
}

fn hold(path: &Path) -> std::fs::File {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    f.lock().unwrap();
    f
}

/// Two folder mems, each reachable under two spellings of its path
/// (a directory symlink `a -> m`). Engine one mounts left through the
/// link and right directly; engine two the other way round. Same
/// storage, same mems.
struct Spelled {
    _tmp: TempDir,
    one: Vec<Mount>,
    two: Vec<Mount>,
    real_left: PathBuf,
    real_right: PathBuf,
}

fn spelled() -> Spelled {
    spelled_by(true)
}

/// `symlink` false: no link at all; the second spelling goes through
/// `a/../m`, the form a relative mount path joined to a workspace root
/// takes.
fn spelled_by(symlink: bool) -> Spelled {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let real = base.join("m");
    std::fs::create_dir_all(&real).unwrap();
    let link = if symlink {
        let link = base.join("a");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        link
    } else {
        std::fs::create_dir_all(base.join("a")).unwrap();
        base.join("a").join("..").join("m")
    };
    let (real_left, real_right) = (real.join(LEFT), real.join(RIGHT));
    seed_dir(&real_left);
    seed_dir(&real_right);
    let w = MountCapability::Write;
    let one = vec![
        folder_mount(LEFT, &link.join(LEFT), w),
        folder_mount(RIGHT, &real_right, w),
    ];
    let two = vec![
        folder_mount(LEFT, &real_left, w),
        folder_mount(RIGHT, &link.join(RIGHT), w),
    ];
    let mut e = engine_of(&one, None);
    for (mem, title) in [(LEFT, "Alpha"), (RIGHT, "Gamma")] {
        e.create_entity(create_args(mem, title, &[]), Actor::Cli, None, None)
            .unwrap();
    }
    Spelled {
        _tmp: tmp,
        one,
        two,
        real_left,
        real_right,
    }
}

fn batch(engine: &mut Engine, tag: &str) -> Result<bool, String> {
    let items = vec![
        (
            update_args(engine, &id(LEFT, "alpha"), &format!("alpha {tag}")),
            None,
        ),
        (
            update_args(engine, &id(RIGHT, "gamma"), &format!("gamma {tag}")),
            None,
        ),
    ];
    engine
        .batch_update(items, Actor::Cli, None, false)
        .map(|r| r.applied)
        .map_err(|e| e.code().to_string())
}

/// Which lock does a batch over both mems take first? Hold the other
/// one from outside and see which the engine holds while it waits.
#[test]
fn two_engines_take_the_same_two_locks_in_opposite_order() {
    let s = spelled();
    let (ll, rl) = (lock_file(&s.real_left), lock_file(&s.real_right));

    // Engine one, with right held outside.
    let outside = hold(&rl);
    let mounts = s.one.clone();
    let t = std::thread::spawn(move || batch(&mut engine_of(&mounts, None), "one"));
    let start = Instant::now();
    while lock_is_free(&ll) && start.elapsed() < Duration::from_secs(10) {
        std::thread::yield_now();
    }
    let one_holds_left_first = !lock_is_free(&ll) && !t.is_finished();
    outside.unlock().unwrap();
    drop(outside);
    assert_eq!(t.join().unwrap(), Ok(true));

    // Engine two, with left held outside.
    let outside = hold(&ll);
    let mounts = s.two.clone();
    let t = std::thread::spawn(move || batch(&mut engine_of(&mounts, None), "two"));
    let start = Instant::now();
    while lock_is_free(&rl) && start.elapsed() < Duration::from_secs(10) {
        std::thread::yield_now();
    }
    let two_holds_right_first = !lock_is_free(&rl) && !t.is_finished();
    outside.unlock().unwrap();
    drop(outside);
    assert_eq!(t.join().unwrap(), Ok(true));

    assert!(
        !(one_holds_left_first && two_holds_right_first),
        "engine one holds left while waiting for right, engine two holds right while \
         waiting for left: the two take the same locks in opposite order"
    );
}

fn race(one: Vec<Mount>, two: Vec<Mount>, rounds: usize) -> Result<(), String> {
    let progress = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
    let done = Arc::new([AtomicBool::new(false), AtomicBool::new(false)]);
    for (i, mounts) in [one, two].into_iter().enumerate() {
        let (progress, done) = (Arc::clone(&progress), Arc::clone(&done));
        std::thread::spawn(move || {
            let mut engine = engine_of(&mounts, None);
            for n in 0..rounds {
                let _ = batch(&mut engine, &format!("{i}-{n}"));
                progress[i].fetch_add(1, Ordering::SeqCst);
            }
            done[i].store(true, Ordering::SeqCst);
        });
    }
    let mut last = (0, 0, Instant::now());
    loop {
        if done[0].load(Ordering::SeqCst) && done[1].load(Ordering::SeqCst) {
            return Ok(());
        }
        let now = (
            progress[0].load(Ordering::SeqCst),
            progress[1].load(Ordering::SeqCst),
        );
        if (now.0, now.1) != (last.0, last.1) {
            last = (now.0, now.1, Instant::now());
        } else if last.2.elapsed() > Duration::from_secs(15) {
            return Err(format!(
                "no batch completed for 15 s; engine one stopped after {} batches, engine two \
                 after {} (of {rounds} each): both are blocked on a write lock",
                now.0, now.1
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Control: both engines spell the mounts the same way.
#[test]
fn control_same_spelling_both_complete() {
    let s = spelled();
    assert_eq!(race(s.one.clone(), s.one.clone(), 25), Ok(()));
}

/// Two engines over the same two mems, each running batches over both.
#[test]
fn two_multi_mem_writers_both_complete() {
    let s = spelled();
    assert_eq!(race(s.one.clone(), s.two.clone(), 25), Ok(()));
}

/// The same without any symlink: one spelling carries a `..`.
#[test]
fn two_multi_mem_writers_both_complete_dotdot_spelling() {
    let s = spelled_by(false);
    assert_eq!(race(s.one.clone(), s.two.clone(), 25), Ok(()));
}

/// What `deferred_load_fails` observed: the verb's result with the
/// fault, its result without, whether the fault struck, and the two
/// views it compared.
type DeferredLoadOutcome = (
    Result<(), String>,
    Result<(), String>,
    bool,
    Vec<String>,
    Vec<String>,
);

/// A delete (and a rename) in an engine that mounts `right` lazily,
/// while the one read that loads `right` under the verb's lock fails.
/// `right--delta` USES `left--alpha` in storage.
fn deferred_load_fails(verb: &str, git: bool) -> DeferredLoadOutcome {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let mounts: Vec<Mount> = if git {
        memstead_git_branch::test_support::init_real_mem_repo(
            &base,
            &[(LEFT, SCHEMA), (RIGHT, SCHEMA)],
        );
        [LEFT, RIGHT]
            .iter()
            .map(|mem| Mount {
                mem: mem.to_string(),
                schema: Some(SCHEMA.parse().unwrap()),
                storage: MountStorage::GitBranch {
                    gitdir: base.join("mem-repo").join(".git"),
                    branch: mem.to_string(),
                },
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            })
            .collect()
    } else {
        let (left, right) = (base.join(LEFT), base.join(RIGHT));
        seed_dir(&left);
        seed_dir(&right);
        vec![
            folder_mount(LEFT, &left, MountCapability::Write),
            folder_mount(RIGHT, &right, MountCapability::Write),
        ]
    };
    {
        let mut e = engine_of(&mounts, None);
        e.create_entity(create_args(LEFT, "Alpha", &[]), Actor::Cli, None, None)
            .unwrap();
        e.create_entity(
            create_args(RIGHT, "Delta", &[id(LEFT, "alpha")]),
            Actor::Cli,
            None,
            None,
        )
        .unwrap();
    }
    let mut lazy = mounts.clone();
    lazy[1].lifecycle = MountLifecycle::Lazy;

    let run = |engine: &mut Engine| -> Result<(), String> {
        let hash = engine
            .get_entity(&id(LEFT, "alpha"))
            .unwrap()
            .content_hash
            .clone();
        match verb {
            "delete" => engine
                .delete_entity(
                    DeleteEntityArgs {
                        id: id(LEFT, "alpha"),
                        expected_hash: memstead_base::ExpectedHash::Current(hash),
                    },
                    Actor::Cli,
                    None,
                    None,
                )
                .map(|_| ()),
            _ => engine
                .rename_entity(
                    memstead_base::RenameEntityArgs {
                        id: id(LEFT, "alpha"),
                        expected_hash: memstead_base::ExpectedHash::Current(hash),
                        new_title: "Alpha Renamed".to_string(),
                    },
                    Actor::Cli,
                    None,
                    None,
                )
                .map(|_| ()),
        }
        .map_err(|e| e.code().to_string())
    };

    // Control: the same lazy engine without the fault, as a rehearsal
    // of what the guard answers (delete), on a copy of nothing: the
    // delete is refused, so it changes nothing; the rename control is
    // skipped (it would rename).
    let control = if verb == "delete" {
        run(&mut engine_of(&lazy, None))
    } else {
        Ok(())
    };

    let armed = Arc::new(AtomicBool::new(false));
    let failed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let tap: Tap = {
        let (armed, failed) = (Arc::clone(&armed), Arc::clone(&failed));
        Arc::new(move |mem: &str, method: &str| {
            if mem == RIGHT
                && armed.load(Ordering::SeqCst)
                && matches!(
                    method,
                    "read_all_entities" | "list_entities" | "read_entity"
                )
                && failed.lock().unwrap().is_empty()
            {
                failed.lock().unwrap().push(method.to_string());
                return true;
            }
            false
        })
    };
    let mut engine = engine_of(&lazy, Some(tap));
    armed.store(true, Ordering::SeqCst);
    let outcome = run(&mut engine);
    armed.store(false, Ordering::SeqCst);

    let fresh = engine_of(&mounts, None);
    let alpha_real = fresh
        .get_entity(&id(LEFT, "alpha"))
        .is_some_and(|e| !e.stub);
    let delta_out: Vec<String> = fresh
        .store()
        .outgoing(&id(RIGHT, "delta"))
        .iter()
        .map(|e| format!("{}>{}", e.rel_type, e.target))
        .collect();
    let injected = failed.lock().unwrap().clone();
    (control, outcome, alpha_real, delta_out, injected)
}

#[test]
fn a_guard_runs_without_a_mem_whose_deferred_load_failed_under_the_lock() {
    let mut bad: Vec<String> = Vec::new();
    for git in [false, true] {
        for verb in ["delete", "rename"] {
            let (control, outcome, alpha_real, delta_out, injected) =
                deferred_load_fails(verb, git);
            eprintln!(
                "git={git} {verb}: control(no fault)={control:?} outcome={outcome:?} injected={injected:?} \
                 alpha real in storage={alpha_real} right--delta edges in storage={delta_out:?}"
            );
            if verb == "delete" {
                assert_eq!(control, Err("HAS_INCOMING_REFS".to_string()), "control");
            }
            if outcome.is_ok()
                && !injected.is_empty()
                && delta_out.iter().any(|e| e.ends_with("left--alpha"))
            {
                bad.push(format!("git={git} {verb}: acknowledged; alpha real={alpha_real}; right--delta edges {delta_out:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
