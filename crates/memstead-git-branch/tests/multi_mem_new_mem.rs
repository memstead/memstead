//! A mem that a second process creates while a delete or a rename is
//! under way. The verb's lock set was taken before the mem existed, so
//! nothing of the verb's makes a writer into the new mem wait. What
//! does is the writer's own lock set: a write takes the lock of every
//! mem it refers to, so a link to the verb's entity waits for the
//! verb, reloads, and is written against the entity as the verb left
//! it.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use memstead_base::backend::MemBackend;
use memstead_base::vcs::Actor;
use memstead_base::{
    CreateEntityArgs, DeleteEntityArgs, Engine, EngineError, EntityId, UpdateEntityArgs,
};
use memstead_git_branch::storage::instantiate_full_backend;
use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

use common::{Tap, Tapped, lands, lock_is_free};

const LEFT: &str = "left";
const LATE: &str = "late";
const SCHEMA: &str = "default@1.0.0";

fn id(mem: &str, slug: &str) -> EntityId {
    EntityId::new(mem, slug)
}

fn create_args(mem: &str, title: &str) -> CreateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), format!("{title} identity"));
    sections.insert("purpose".to_string(), format!("{title} purpose"));
    CreateEntityArgs {
        mem: mem.to_string(),
        title: title.to_string(),
        entity_type: "spec".to_string(),
        sections,
        metadata: Default::default(),
        relations: Vec::new(),
        anchors: Vec::new(),
        dry_run: false,
    }
}

fn update_args(engine: &Engine, target: &EntityId, purpose: &str) -> UpdateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("purpose".to_string(), purpose.to_string());
    UpdateEntityArgs {
        id: target.clone(),
        expected_hash: engine.get_entity(target).map(|e| e.content_hash.clone()),
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

type Report = Result<Vec<String>, String>;

fn report(result: Result<Vec<memstead_base::ops::WarningHint>, EngineError>) -> Report {
    result
        .map(|warnings| warnings.iter().map(|w| w.code().to_string()).collect())
        .map_err(|e| e.code().to_string())
}

fn workspace(root: &Path) {
    std::fs::create_dir_all(root.join(".memstead")).unwrap();
    std::fs::write(
        root.join(".memstead/workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n\n[cross_mem_links]\nleft = \"*\"\nlate = \"*\"\n",
    )
    .unwrap();
    init_real_mem_repo(root, &[(LEFT, SCHEMA)]);
}

/// The second process: creates the mem `late`, an entity in it, and a
/// body link from it to `left--alpha`.
fn second_process(root: &Path) -> Vec<String> {
    use memstead_base::mem_management;
    let mut out = Vec::new();
    let mut second = engine_from_workspace_root(root).expect("second boots");
    let made = mem_management::create_mem(
        &mut second,
        mem_management::MemCreateParams {
            name: LATE.to_string(),
            location: PathBuf::from(LATE),
            schema_ref: SCHEMA.parse().unwrap(),
            vcs: None,
            note: None,
            operator_mode: true,
            recovery: None,
            write_guidance: Default::default(),
            storage: Some(mem_management::StorageKind::GitBranch),
            actor: Actor::Cli,
            client: None,
        },
    );
    out.push(format!("create_mem ok={}", made.is_ok()));
    if let Err(e) = &made {
        out.push(format!("create_mem error: {e}"));
    }
    let created = report(
        second
            .create_entity(create_args(LATE, "Delta"), Actor::Cli, None, None)
            .map(|o| o.warnings),
    );
    out.push(format!("create late--delta: {created:?}"));
    let args = update_args(
        &second,
        &id(LATE, "delta"),
        "Delta purpose, see [[left--alpha]]",
    );
    let linked = report(
        second
            .update_entity(args, Actor::Cli, None, None)
            .map(|o| o.warnings),
    );
    out.push(format!("link late--delta -> left--alpha: {linked:?}"));
    out
}

fn fresh_picture(root: &Path) -> (bool, Vec<String>, Vec<String>) {
    let fresh = engine_from_workspace_root(root).expect("fresh boots");
    let alpha = id(LEFT, "alpha");
    let real = fresh.get_entity(&alpha).is_some_and(|e| !e.stub);
    let from: Vec<String> = fresh
        .store()
        .incoming(&alpha)
        .iter()
        .map(|e| e.from.to_string())
        .collect();
    let ids: Vec<String> = fresh
        .store()
        .all_entities()
        .map(|e| format!("{}{}", e.id, if e.stub { "(stub)" } else { "" }))
        .collect();
    (real, from, ids)
}

/// Re-register `left` on `engine` behind a tap.
fn tap_left(engine: &mut Engine, tap: Tap) {
    let mount = engine.mount(LEFT).unwrap().clone();
    engine.unregister_writable_mem(LEFT).unwrap();
    engine
        .register_writable_mem(
            mount.clone(),
            Box::new(Tapped {
                inner: instantiate_full_backend(&mount).unwrap(),
                mem: LEFT,
                tap,
            }) as Box<dyn MemBackend>,
            memstead_base::MemOrigin::RuntimeCreated {
                at: std::time::SystemTime::now(),
                by_tool: "test",
            },
        )
        .unwrap();
}

/// Control: the second process attaches the mem and links to alpha
/// BEFORE the delete is sent. The delete must refuse.
#[test]
fn control_a_link_from_a_mem_attached_before_the_delete_refuses_it() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    workspace(&root);
    let mut engine = engine_from_workspace_root(&root).expect("engine boots");
    engine
        .create_entity(create_args(LEFT, "Alpha"), Actor::Cli, None, None)
        .expect("seed alpha");
    let second = second_process(&root);
    let args = DeleteEntityArgs {
        id: id(LEFT, "alpha"),
        expected_hash: engine
            .get_entity(&id(LEFT, "alpha"))
            .map(|e| e.content_hash.clone()),
    };
    let deleted = engine
        .delete_entity(args, Actor::Cli, None, None)
        .map(|_| ())
        .map_err(|e| e.code().to_string());
    let (real, from, ids) = fresh_picture(&root);
    eprintln!(
        "CONTROL second={second:?} delete={deleted:?} alpha_real={real} incoming={from:?} ids={ids:?}"
    );
    assert_eq!(deleted, Err("HAS_INCOMING_REFS".to_string()));
    assert!(real);
}

/// What `held_verb` saw: the verb's result, the second process's
/// lines, whether the verb held `left`'s lock at the window, whether
/// the second process finished inside the window, and the fresh
/// engine's picture (the target real, its incoming edges, every id).
type Held = (
    Result<(), String>,
    Vec<String>,
    bool,
    bool,
    bool,
    Vec<String>,
    Vec<String>,
);

fn held_verb(verb: &str) -> Held {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    workspace(&root);
    let mut engine = engine_from_workspace_root(&root).expect("engine boots");
    engine
        .create_entity(create_args(LEFT, "Alpha"), Actor::Cli, None, None)
        .expect("seed alpha");

    let lock_path = instantiate_full_backend(engine.mount(LEFT).unwrap())
        .unwrap()
        .write_lock_path()
        .unwrap();
    // At the window: whether the lock was held, and the second
    // process's lines if it finished there, or the channel they will
    // arrive on.
    type Stage = Option<(
        bool,
        Result<Vec<String>, std::sync::mpsc::Receiver<Vec<String>>>,
    )>;
    let stage: Arc<Mutex<Stage>> = Arc::new(Mutex::new(None));
    let tap: Tap = {
        let (stage, root) = (Arc::clone(&stage), root.clone());
        Arc::new(move |_mem: &str, method: &str| {
            if !lands(method) {
                return false;
            }
            let mut stage = stage.lock().unwrap();
            if stage.is_some() {
                return false;
            }
            let locked = !lock_is_free(&lock_path);
            let (done, is_done) = std::sync::mpsc::channel::<Vec<String>>();
            let root = root.clone();
            std::thread::spawn(move || {
                let _ = done.send(second_process(&root));
            });
            // A second process that does not wait is done well within
            // this time; one that waits for the verb is not done at
            // all, and its lines are collected after the verb.
            let second = match is_done.recv_timeout(std::time::Duration::from_secs(2)) {
                Ok(lines) => Ok(lines),
                Err(_) => Err(is_done),
            };
            *stage = Some((locked, second));
            false
        })
    };
    tap_left(&mut engine, tap);

    let alpha = id(LEFT, "alpha");
    let hash = engine.get_entity(&alpha).map(|e| e.content_hash.clone());
    let outcome = match verb {
        "delete" => engine
            .delete_entity(
                DeleteEntityArgs {
                    id: alpha.clone(),
                    expected_hash: hash,
                },
                Actor::Cli,
                None,
                None,
            )
            .map(|_| ())
            .map_err(|e| e.code().to_string()),
        "rename" => engine
            .rename_entity(
                memstead_base::RenameEntityArgs {
                    id: alpha.clone(),
                    new_title: "Omega".to_string(),
                    expected_hash: hash,
                },
                Actor::Cli,
                None,
                None,
            )
            .map(|_| ())
            .map_err(|e| e.code().to_string()),
        _ => unreachable!(),
    };
    let (locked, second) = stage
        .lock()
        .unwrap()
        .take()
        .expect("the verb reached a landing call");
    drop(engine);
    let (inside, second) = match second {
        Ok(lines) => (true, lines),
        Err(later) => (
            false,
            later
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("the second process finishes once the verb is done"),
        ),
    };
    let (real, from, ids) = fresh_picture(&root);
    (outcome, second, locked, inside, real, from, ids)
}

/// The link the second process wrote, judged: it waited for the verb,
/// and its writer was told that the target is missing.
fn assert_the_link_waited_and_knows(verb: &str, held: &Held) {
    let (outcome, second, locked, inside, real, from, ids) = held;
    let context = format!(
        "{verb}={outcome:?} left_locked={locked} second_inside_window={inside} second={second:?} \
         | fresh: target_real={real} incoming={from:?} ids={ids:?}"
    );
    assert!(outcome.is_ok(), "{context}");
    assert!(
        locked,
        "the {verb} held left's lock at the window: {context}"
    );
    assert!(
        !inside,
        "the link was written inside the {verb}'s window: {context}"
    );
    let link = second
        .iter()
        .find(|line| line.starts_with("link "))
        .expect("the second process reports its link");
    assert!(
        link.contains("INLINE_WIKI_LINK_AUTO_STUBBED"),
        "the link's writer was not told its target is missing: {context}"
    );
}

/// A delete is held at its first landing call, after its referrer guard
/// ran and while it holds every lock it took. A second process attaches
/// a mem to the workspace and writes a link to the entity from it.
#[test]
fn a_link_from_a_mem_attached_while_a_delete_is_held_at_its_commit_waits_for_it() {
    assert_the_link_waited_and_knows("delete", &held_verb("delete"));
}

/// The same window on a rename: the link is written after it, to an id
/// its writer knows to be gone.
#[test]
fn a_link_from_a_mem_attached_while_a_rename_is_held_at_its_commit_waits_for_it() {
    assert_the_link_waited_and_knows("rename", &held_verb("rename"));
}
