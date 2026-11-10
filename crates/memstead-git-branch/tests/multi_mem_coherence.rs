//! Operations that read or write more than one mem, run against a
//! second writer.
//!
//! Two mems, `left` and `right`, on every pairing of the folder and the
//! git-branch backend. The operation under test runs in one engine; the
//! second writer is another engine over the same storage on another
//! thread, which is what a second process is to the write lock (the
//! lock is an OS file lock per open file, and reentrant per thread
//! only).
//!
//! The interleaving is fixed by a hook, never by timing. The operation's
//! backends are wrapped, and the wrapper stops the operation at its
//! first commit call, into whichever mem: every read the operation
//! plans from is behind it, no write of it has landed. At that point
//! the hook asks one question with a lock probe of its own: does the
//! operation hold both mems' write locks? If it does not, the second
//! writer runs to its end right there, inside the window. If it does,
//! the second writer is started and can only finish once the operation
//! released the locks; the hook lets the operation go on when the
//! second writer has loaded its engine and is about to send its write,
//! so that write is prepared against the state before the operation.
//! Either way the order of the two writes is decided before the
//! operation goes on.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use indexmap::IndexMap;
use memstead_base::backend::MemBackend;
use memstead_base::engine::conflicts::ConflictSide;
use memstead_base::vcs::Actor;
use memstead_base::{
    CreateEntityArgs, DeleteEntityArgs, Engine, EngineError, EntityId, Mount, MountCapability,
    MountLifecycle, MountStorage, RelateEntityArgs, RenameEntityArgs, RetypeEntityArgs,
    UpdateEntityArgs,
};
use memstead_git_branch::storage::instantiate_full_backend;
use memstead_git_branch::test_support::init_real_mem_repo;
use tempfile::TempDir;

use common::{Tap, Tapped, lands, lock_is_free};

const LEFT: &str = "left";
const RIGHT: &str = "right";
const SCHEMA: &str = "default@1.0.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Folder,
    GitBranch,
}

/// Every pairing of the two backends: both of a kind, and both mixes.
const PAIRS: [(Kind, Kind); 4] = [
    (Kind::Folder, Kind::Folder),
    (Kind::GitBranch, Kind::GitBranch),
    (Kind::Folder, Kind::GitBranch),
    (Kind::GitBranch, Kind::Folder),
];

// ---------------------------------------------------------------------
// The workspace
// ---------------------------------------------------------------------

/// Two seeded mems. Cheap to clone, so a second writer's thread takes
/// its own handle on the same storage.
#[derive(Clone)]
struct Workspace {
    _tmp: Arc<TempDir>,
    mounts: [Mount; 2],
}

impl Workspace {
    fn new((left, right): (Kind, Kind)) -> Self {
        let tmp = TempDir::new().unwrap();
        let on_branches: Vec<(&str, &str)> = [(LEFT, left), (RIGHT, right)]
            .into_iter()
            .filter(|(_, kind)| *kind == Kind::GitBranch)
            .map(|(mem, _)| (mem, SCHEMA))
            .collect();
        if !on_branches.is_empty() {
            init_real_mem_repo(tmp.path(), &on_branches);
        }
        let mount = |mem: &str, kind: Kind| {
            let storage = match kind {
                Kind::Folder => {
                    let dir = tmp.path().join(mem);
                    std::fs::create_dir_all(dir.join(".memstead")).unwrap();
                    std::fs::write(
                        dir.join(".memstead/config.json"),
                        format!(r#"{{"schema": "{SCHEMA}"}}"#),
                    )
                    .unwrap();
                    MountStorage::Folder { path: dir }
                }
                Kind::GitBranch => MountStorage::GitBranch {
                    gitdir: tmp.path().join("mem-repo").join(".git"),
                    branch: mem.to_string(),
                },
            };
            Mount {
                mem: mem.to_string(),
                schema: Some(SCHEMA.parse().unwrap()),
                storage,
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            }
        };
        let ws = Self {
            mounts: [mount(LEFT, left), mount(RIGHT, right)],
            _tmp: Arc::new(tmp),
        };
        let mut engine = ws.engine();
        for (mem, title) in [(LEFT, "Alpha"), (LEFT, "Beta"), (RIGHT, "Gamma")] {
            engine
                .create_entity(create_args(mem, title, &[]), Actor::Cli, None, None)
                .unwrap_or_else(|e| panic!("seed {mem}--{title}: {e:?}"));
        }
        ws
    }

    /// An engine of its own over the storage, loaded from it now.
    fn engine(&self) -> Engine {
        self.engine_with(None)
    }

    /// The same, with every storage call reported to `tap` first.
    fn tapped_engine(&self, tap: Tap) -> Engine {
        self.engine_with(Some(tap))
    }

    /// The same as [`Self::engine`], with `right` mounted lazily: its
    /// loading is deferred until an operation needs the mem.
    fn engine_with_right_deferred(&self) -> Engine {
        self.build_engine(None, true)
    }

    fn engine_with(&self, tap: Option<Tap>) -> Engine {
        self.build_engine(tap, false)
    }

    fn build_engine(&self, tap: Option<Tap>, right_deferred: bool) -> Engine {
        let mounts = self
            .mounts
            .iter()
            .map(|mount| {
                let mut mount = mount.clone();
                if right_deferred && mount.mem == RIGHT {
                    mount.lifecycle = MountLifecycle::Lazy;
                }
                let mount = &mount;
                let backend = instantiate_full_backend(mount).expect("backend instantiates");
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

    /// The write lock's file of `mem`.
    fn lock_path(&self, mem: &str) -> PathBuf {
        let mount = self.mounts.iter().find(|m| m.mem == mem).unwrap();
        instantiate_full_backend(mount)
            .expect("backend instantiates")
            .write_lock_path()
            .expect("an on-disk mem has a write lock")
    }

    /// Run `write` in a second engine on another thread: the second
    /// process. The engine is loaded on that thread, when it starts.
    fn second_writer<T: Send + 'static>(
        &self,
        write: impl FnOnce(&mut Engine) -> T + Send + 'static,
    ) -> JoinHandle<T> {
        let ws = self.clone();
        std::thread::spawn(move || write(&mut ws.engine()))
    }
}

// ---------------------------------------------------------------------
// Arguments and views
// ---------------------------------------------------------------------

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

fn relate_args(from: &EntityId, to: &EntityId) -> RelateEntityArgs {
    RelateEntityArgs {
        source: from.clone(),
        expected_hash: None,
        rel_type: "USES".to_string(),
        target: to.clone(),
        remove: false,
        description: None,
        dry_run: false,
    }
}

fn delete_args(engine: &Engine, target: &EntityId) -> DeleteEntityArgs {
    DeleteEntityArgs {
        id: target.clone(),
        expected_hash: engine.get_entity(target).map(|e| e.content_hash.clone()),
    }
}

fn retype_args(engine: &Engine, target: &EntityId) -> RetypeEntityArgs {
    RetypeEntityArgs {
        id: target.clone(),
        expected_hash: engine.get_entity(target).map(|e| e.content_hash.clone()),
        target_type: "concept".to_string(),
        section_map: IndexMap::from_iter([
            ("identity".to_string(), "definition".to_string()),
            ("purpose".to_string(), "explanation".to_string()),
        ]),
        // `level` is a spec field the concept type does not carry.
        drop_metadata: vec!["level".to_string()],
        dry_run: false,
    }
}

/// What a call reported: its warnings' codes, or the code it refused
/// with.
type Report = Result<Vec<String>, String>;

fn report(result: Result<Vec<memstead_base::ops::WarningHint>, EngineError>) -> Report {
    result
        .map(|warnings| warnings.iter().map(|w| w.code().to_string()).collect())
        .map_err(|e| e.code().to_string())
}

fn batch_report(result: Result<memstead_base::ops::BatchResult, EngineError>) -> Report {
    let result = result.map_err(|e| e.code().to_string())?;
    if result.applied {
        Ok(result
            .warnings
            .iter()
            .map(|w| w.code().to_string())
            .collect())
    } else {
        Err(result
            .results
            .iter()
            .find_map(|entry| entry.error.as_ref().map(|e| e.code.clone()))
            .unwrap_or_else(|| "BATCH_NOT_APPLIED".to_string()))
    }
}

/// The engine's view of both mems: every entity with its hash and its
/// edges in both directions.
fn view(engine: &Engine) -> BTreeMap<String, String> {
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
        out.insert(
            entity.id.to_string(),
            format!(
                "stub={} type={} hash={} out={:?} in={:?}",
                entity.stub, entity.entity_type, entity.content_hash, outgoing, incoming
            ),
        );
    }
    out
}

/// After its next operation's reload, the engine serves what an engine
/// loaded from storage now serves.
fn assert_level_with_storage(ws: &Workspace, engine: &mut Engine, context: &str) {
    engine.reload_if_stale(None);
    assert_eq!(
        view(engine),
        view(&ws.engine()),
        "{context}: the running engine's view equals a freshly loaded one's"
    );
}

// ---------------------------------------------------------------------
// The hook
// ---------------------------------------------------------------------

/// What the hook found and did at the operation's first commit call.
struct Window<T> {
    /// The operation held both mems' write locks when it reached its
    /// first commit.
    both_locked: bool,
    /// The second writer's report: taken inside the window when a
    /// lock was free, after the operation otherwise.
    second: T,
}

/// Run `operation` in an engine over `ws` and stop it at its first
/// commit call; decide there, by a probe of both write locks, whether
/// the second writer runs inside the window or behind the operation
/// (see the module comment). Returns the operation's result with what
/// the hook found, and the operation's engine.
///
/// `second` gets its engine and a `ready` call, which it makes when its
/// arguments are built and its write is the next thing it does.
fn hold_at_commit<R, T: Send + 'static>(
    ws: &Workspace,
    second: impl FnOnce(&mut Engine, &dyn Fn()) -> T + Send + 'static,
    operation: impl FnOnce(&mut Engine) -> R,
) -> (R, Window<T>, Engine) {
    hold_at_commit_probing(ws, &[LEFT, RIGHT], second, operation)
}

/// [`hold_at_commit`] for an operation that owes the lock of `must_hold`
/// only (it writes one mem and reads no other): the probe asks for
/// those locks, and `Window::both_locked` reports them.
fn hold_at_commit_probing<R, T: Send + 'static>(
    ws: &Workspace,
    must_hold: &[&str],
    second: impl FnOnce(&mut Engine, &dyn Fn()) -> T + Send + 'static,
    operation: impl FnOnce(&mut Engine) -> R,
) -> (R, Window<T>, Engine) {
    enum Stage<T> {
        Armed(Box<dyn FnOnce() -> JoinHandle<T> + Send>),
        Finished { both_locked: bool, second: T },
        Running(JoinHandle<T>),
        Taken,
    }
    let locks: Vec<PathBuf> = must_hold.iter().map(|mem| ws.lock_path(mem)).collect();
    let (ready, is_ready) = std::sync::mpsc::channel::<()>();
    let is_ready = Mutex::new(is_ready);
    let spawn = {
        let ws = ws.clone();
        Box::new(move || {
            ws.second_writer(move |engine| {
                second(engine, &|| {
                    let _ = ready.send(());
                })
            })
        }) as Box<dyn FnOnce() -> JoinHandle<T> + Send>
    };
    let stage = Arc::new(Mutex::new(Stage::Armed(spawn)));
    let tap: Tap = {
        let stage = Arc::clone(&stage);
        Arc::new(move |_mem: &str, method: &str| {
            if !lands(method) {
                return false;
            }
            let mut stage = stage.lock().unwrap();
            let Stage::Armed(_) = &*stage else {
                return false;
            };
            let Stage::Armed(spawn) = std::mem::replace(&mut *stage, Stage::Taken) else {
                unreachable!()
            };
            // Asked before the second writer exists, so a held lock is
            // the operation's own.
            let both_locked = locks.iter().all(|lock| !lock_is_free(lock));
            let handle = spawn();
            *stage = if both_locked {
                is_ready
                    .lock()
                    .unwrap()
                    .recv()
                    .expect("the second writer reaches its write");
                Stage::Running(handle)
            } else {
                Stage::Finished {
                    both_locked,
                    second: handle.join().expect("the second writer does not panic"),
                }
            };
            false
        })
    };
    let mut engine = ws.tapped_engine(tap);
    let result = operation(&mut engine);
    let window = match std::mem::replace(&mut *stage.lock().unwrap(), Stage::Taken) {
        Stage::Finished {
            both_locked,
            second,
        } => Window {
            both_locked,
            second,
        },
        Stage::Running(handle) => Window {
            both_locked: true,
            second: handle.join().expect("the second writer does not panic"),
        },
        Stage::Armed(_) => panic!("the operation never reached a commit"),
        Stage::Taken => unreachable!(),
    };
    (result, window, engine)
}

// ---------------------------------------------------------------------
// Delete: the referrer guard reads every mem
// ---------------------------------------------------------------------

/// How the second writer refers to `left--alpha` from `right`.
#[derive(Debug, Clone, Copy)]
enum Link {
    /// A relation declared on a new entity.
    Relation,
    /// A wiki link in the body of an existing entity.
    BodyLink,
}

fn add_link(engine: &mut Engine, link: Link, ready: &dyn Fn()) -> Report {
    match link {
        Link::Relation => {
            ready();
            report(
                engine
                    .create_entity(
                        create_args(RIGHT, "Delta", &[id(LEFT, "alpha")]),
                        Actor::Cli,
                        None,
                        None,
                    )
                    .map(|o| o.warnings),
            )
        }
        Link::BodyLink => {
            let args = update_args(
                engine,
                &id(RIGHT, "gamma"),
                "Gamma purpose, see [[left--alpha]]",
            );
            ready();
            report(
                engine
                    .update_entity(args, Actor::Cli, None, None)
                    .map(|o| o.warnings),
            )
        }
    }
}

/// The edges from `right` that point at `left--alpha`, as storage has
/// them now.
fn links_to_alpha(ws: &Workspace) -> (bool, Vec<String>) {
    let fresh = ws.engine();
    let alpha = id(LEFT, "alpha");
    let real = fresh.get_entity(&alpha).is_some_and(|e| !e.stub);
    let from: Vec<String> = fresh
        .store()
        .incoming(&alpha)
        .iter()
        .map(|e| e.from.to_string())
        .collect();
    (real, from)
}

/// A second process adds a reference to an entity from another mem,
/// and only then the first engine, which loaded that mem before, is
/// asked to delete the entity. The delete's guard has to see the
/// reference: it reloads every mem under its lock.
#[test]
fn a_delete_sees_a_reference_a_second_process_added_from_another_mem() {
    for pair in PAIRS {
        for link in [Link::Relation, Link::BodyLink] {
            let context = format!("{pair:?} {link:?}");
            let ws = Workspace::new(pair);
            let mut engine = ws.engine();

            let added = ws
                .second_writer(move |second| add_link(second, link, &|| {}))
                .join()
                .unwrap();
            assert!(added.is_ok(), "{context}: {added:?}");

            let args = delete_args(&engine, &id(LEFT, "alpha"));
            let deleted = report(
                engine
                    .delete_entity(args, Actor::Cli, None, None)
                    .map(|o| o.warnings),
            );
            assert_eq!(
                deleted,
                Err("HAS_INCOMING_REFS".to_string()),
                "{context}: the guard sees the reference and refuses"
            );
            let (real, from) = links_to_alpha(&ws);
            assert!(real, "{context}: the entity is still there");
            assert_eq!(from.len(), 1, "{context}: and so is the reference");
            assert_level_with_storage(&ws, &mut engine, &context);
        }
    }
}

/// The delete is held at its commit, after its guard found no
/// reference, and a second process adds one from the other mem. The
/// reference must not end up pointing at an entity its writer took for
/// existing: the delete holds the other mem's lock, so the second write
/// waits, reloads and is computed against a target that is gone (a
/// forward reference to a stub, the documented path of an add).
#[test]
fn a_reference_added_while_a_delete_is_held_at_its_commit_is_not_left_dangling() {
    for pair in PAIRS {
        for link in [Link::Relation, Link::BodyLink] {
            let context = format!("{pair:?} {link:?}");
            let ws = Workspace::new(pair);
            let (deleted, window, mut engine) = hold_at_commit(
                &ws,
                move |second, ready| {
                    let added = add_link(second, link, ready);
                    let target_real = second
                        .get_entity(&id(LEFT, "alpha"))
                        .is_some_and(|e| !e.stub);
                    (added, target_real)
                },
                |engine| {
                    let args = delete_args(engine, &id(LEFT, "alpha"));
                    report(
                        engine
                            .delete_entity(args, Actor::Cli, None, None)
                            .map(|o| o.warnings),
                    )
                },
            );
            assert!(deleted.is_ok(), "{context}: {deleted:?}");
            assert!(
                window.both_locked,
                "{context}: the delete holds the write lock of every writable mem at its commit"
            );
            let (real, from) = links_to_alpha(&ws);
            assert!(!real, "{context}: the delete landed");
            // The second write came after the delete and was computed
            // against it: its engine knew the target as missing when it
            // acknowledged the reference, and a body link says so in
            // its warning. A refused write left nothing.
            let (added, target_real) = &window.second;
            match added {
                Ok(warnings) => {
                    assert!(
                        !target_real,
                        "{context}: the second writer acknowledged a reference to an entity it \
                         took for existing, and the entity is deleted"
                    );
                    if matches!(link, Link::BodyLink) {
                        assert!(
                            warnings
                                .iter()
                                .any(|w| w == "INLINE_WIKI_LINK_AUTO_STUBBED"),
                            "{context}: {warnings:?}"
                        );
                    }
                }
                Err(_) => assert!(from.is_empty(), "{context}: a refused write left {from:?}"),
            }
            assert_level_with_storage(&ws, &mut engine, &context);
        }
    }
}

// ---------------------------------------------------------------------
// Waiting on a lock another process holds
// ---------------------------------------------------------------------

/// The verbs that wait on the write lock of a mem they do not write, or
/// of every mem their items name.
#[derive(Debug, Clone, Copy)]
enum Waiter {
    /// Deletes `left--beta`; waits on `right`'s lock too.
    Delete,
    /// Retypes `left--beta`; waits on `right`'s lock too.
    Retype,
    /// Renames `left--beta`; waits on `right`'s lock too.
    Rename,
    /// Renames the mem `left`; waits on `right`'s lock too.
    MemRename,
    /// Resolves the conflicted `left--alpha`; waits on `left`'s lock.
    ResolveConflict,
    /// One item per mem.
    BatchCreate,
    BatchUpdate,
    BatchRelate,
}

fn run_waiter(waiter: Waiter, engine: &mut Engine) -> Report {
    let actor = Actor::Cli;
    match waiter {
        Waiter::Delete => {
            let args = delete_args(engine, &id(LEFT, "beta"));
            report(
                engine
                    .delete_entity(args, actor, None, None)
                    .map(|o| o.warnings),
            )
        }
        Waiter::Retype => {
            let args = retype_args(engine, &id(LEFT, "beta"));
            report(
                engine
                    .retype_entity(args, actor, None, None)
                    .map(|o| o.warnings),
            )
        }
        Waiter::Rename => {
            let args = rename_args(engine, &id(LEFT, "beta"), "Beta Renamed");
            report(
                engine
                    .rename_entity(args, actor, None, None)
                    .map(|o| o.warnings),
            )
        }
        Waiter::MemRename => memstead_base::mem_management::rename_mem(
            engine,
            memstead_base::mem_management::MemRenameParams {
                old: LEFT.to_string(),
                new: "lefty".to_string(),
                operator_mode: true,
                note: None,
            },
        )
        .map(|response| {
            response
                .warnings
                .iter()
                .map(|w| w.code().to_string())
                .collect()
        })
        .map_err(|e| format!("{e:?}")),
        Waiter::ResolveConflict => report(
            engine
                .resolve_merge_conflict(&id(LEFT, "alpha"), ConflictSide::Ours, actor, None, None)
                .map(|o| o.warnings),
        ),
        Waiter::BatchCreate => batch_report(engine.batch_create(
            vec![
                (create_args(LEFT, "Delta", &[]), None),
                (create_args(RIGHT, "Epsilon", &[]), None),
            ],
            actor,
            None,
            false,
        )),
        Waiter::BatchUpdate => {
            let updates = vec![
                (
                    update_args(engine, &id(LEFT, "alpha"), "Alpha purpose, batch"),
                    None,
                ),
                (
                    update_args(engine, &id(RIGHT, "gamma"), "Gamma purpose, batch"),
                    None,
                ),
            ];
            batch_report(engine.batch_update(updates, actor, None, false))
        }
        Waiter::BatchRelate => batch_report(engine.batch_relate(
            vec![
                (relate_args(&id(LEFT, "alpha"), &id(LEFT, "beta")), None),
                (relate_args(&id(RIGHT, "gamma"), &id(LEFT, "beta")), None),
            ],
            actor,
            None,
            false,
        )),
    }
}

/// While another process holds the write lock of `right`, the verb
/// lands nothing in either mem; it lands once the lock is released.
///
/// The other process is this test's own open file on the lock. The
/// verb runs on a second thread with its storage calls tapped: a call
/// that lands a write while the lock is still held fails the test, on
/// whichever mem it lands. The lock is released when the verb has
/// asked for `right`'s lock, or has ended without asking (which is
/// then a verb that did not wait).
fn assert_waits_on_the_peers_lock(waiters: &[Waiter]) {
    assert_waits_on_the_lock_of(RIGHT, &PAIRS, waiters);
}

/// The same for the lock of `held_mem`, on the given pairings.
fn assert_waits_on_the_lock_of(held_mem: &'static str, pairs: &[(Kind, Kind)], waiters: &[Waiter]) {
    for &pair in pairs {
        for &waiter in waiters {
            let context = format!("{pair:?} {waiter:?}");
            let ws = Workspace::new(pair);
            if matches!(waiter, Waiter::ResolveConflict) {
                conflict_alpha(&ws);
            }
            let lock_path = ws.lock_path(held_mem);
            std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
                .unwrap();
            lock.lock().unwrap();

            let held = Arc::new(AtomicBool::new(true));
            let asked = Arc::new(AtomicBool::new(false));
            let landed_early = Arc::new(Mutex::new(Vec::<String>::new()));
            let tap: Tap = {
                let (held, asked, landed_early) = (
                    Arc::clone(&held),
                    Arc::clone(&asked),
                    Arc::clone(&landed_early),
                );
                Arc::new(move |mem: &str, method: &str| {
                    if mem == held_mem && method == "write_lock_path" {
                        asked.store(true, Ordering::SeqCst);
                    }
                    if lands(method) && held.load(Ordering::SeqCst) {
                        landed_early
                            .lock()
                            .unwrap()
                            .push(format!("{mem}: {method}"));
                    }
                    false
                })
            };
            let verb = {
                let ws = ws.clone();
                std::thread::spawn(move || {
                    // Loaded before the tap counts: the boot's own
                    // calls are no part of the verb.
                    let mut engine = ws.tapped_engine(tap);
                    run_waiter(waiter, &mut engine)
                })
            };
            while !asked.load(Ordering::SeqCst) && !verb.is_finished() {
                std::thread::yield_now();
            }
            // The verb asked for the lock and is about to block on it,
            // or is blocked already. A moment more costs nothing and
            // lets a verb that asks without waiting show itself.
            std::thread::sleep(std::time::Duration::from_millis(30));
            let finished_while_held = verb.is_finished();
            held.store(false, Ordering::SeqCst);
            lock.unlock().unwrap();
            let outcome = verb.join().unwrap();

            assert!(
                landed_early.lock().unwrap().is_empty(),
                "{context}: landed while another process held the lock: {:?}",
                landed_early.lock().unwrap()
            );
            assert!(
                !finished_while_held,
                "{context}: the verb ended while another process held the lock"
            );
            assert!(outcome.is_ok(), "{context}: {outcome:?}");
            // After a mem rename the workspace handle's mounts name a
            // mem that is gone; the check is for the other verbs.
            if !matches!(waiter, Waiter::MemRename) {
                assert_level_with_storage(&ws, &mut ws.engine(), &context);
            }
        }
    }
}

/// A delete and a retype read every mem for their referrer guard, and
/// a rename reads every mem for its referrers, so they hold every
/// writable mem's lock: the lock of a mem they do not write makes them
/// wait.
#[test]
fn a_delete_a_retype_and_a_rename_wait_on_the_lock_of_a_mem_they_only_read() {
    assert_waits_on_the_peers_lock(&[Waiter::Delete, Waiter::Retype, Waiter::Rename]);
}

/// A mem rename writes whichever mem holds a reference to the renamed
/// one, so it holds every writable mem's lock from its first write to
/// its last.
#[test]
fn a_mem_rename_waits_on_the_lock_of_every_writable_mem() {
    assert_waits_on_the_peers_lock(&[Waiter::MemRename]);
}

/// A batch holds the lock of every mem its items name before it lands
/// anything in any of them.
#[test]
fn a_batch_waits_on_the_lock_of_every_mem_its_items_name() {
    assert_waits_on_the_peers_lock(&[
        Waiter::BatchCreate,
        Waiter::BatchUpdate,
        Waiter::BatchRelate,
    ]);
}

/// The batch forms under the hook: held at the first commit, with a
/// second process writing to what the batch's item in the other mem
/// writes. The batch holds that mem's lock, so the second write waits,
/// reloads, and is refused on what the batch changed; nothing either
/// caller was told landed is missing.
#[test]
fn a_write_into_a_batchs_other_mem_while_the_batch_is_held_waits_and_is_refused_honestly() {
    for pair in PAIRS {
        for waiter in [
            Waiter::BatchCreate,
            Waiter::BatchUpdate,
            Waiter::BatchRelate,
        ] {
            let context = format!("{pair:?} {waiter:?}");
            let ws = Workspace::new(pair);
            let (batch, window, mut engine) = hold_at_commit(
                &ws,
                move |second, ready| match waiter {
                    // The entity the batch creates in `right`.
                    Waiter::BatchCreate => {
                        ready();
                        report(
                            second
                                .create_entity(
                                    create_args(RIGHT, "Epsilon", &[]),
                                    Actor::Cli,
                                    None,
                                    None,
                                )
                                .map(|o| o.warnings),
                        )
                    }
                    // The entity the batch rewrites in `right`, with
                    // the hash from before the batch.
                    _ => {
                        let args =
                            update_args(second, &id(RIGHT, "gamma"), "Gamma purpose, second");
                        ready();
                        report(
                            second
                                .update_entity(args, Actor::Cli, None, None)
                                .map(|o| o.warnings),
                        )
                    }
                },
                |engine| run_waiter(waiter, engine),
            );
            assert!(batch.is_ok(), "{context}: {batch:?}");
            assert!(
                window.both_locked,
                "{context}: the batch holds the lock of every mem its items name at its first \
                 commit"
            );
            let expected = match waiter {
                Waiter::BatchCreate => "ENTITY_ALREADY_EXISTS",
                _ => "HASH_MISMATCH",
            };
            assert_eq!(
                window.second,
                Err(expected.to_string()),
                "{context}: the second write is judged against what the batch landed"
            );
            let fresh = ws.engine();
            let gamma = fresh.get_entity(&id(RIGHT, "gamma")).unwrap();
            assert!(
                !gamma
                    .sections
                    .get("purpose")
                    .is_some_and(|p| p.contains("second")),
                "{context}: the refused write left nothing"
            );
            assert_level_with_storage(&ws, &mut engine, &context);
        }
    }
}

/// Two processes run a batch over the same two mems, named in opposite
/// order, at once. Both complete: the locks are taken in one order
/// whatever order the items name.
#[test]
fn two_batches_over_the_same_mems_in_opposite_order_both_complete() {
    for pair in PAIRS {
        let ws = Workspace::new(pair);
        let start = Arc::new(std::sync::Barrier::new(2));
        let writers: Vec<JoinHandle<Report>> = [(LEFT, RIGHT, "One"), (RIGHT, LEFT, "Two")]
            .into_iter()
            .map(|(first, second, tag)| {
                let (ws, start) = (ws.clone(), Arc::clone(&start));
                std::thread::spawn(move || {
                    let mut engine = ws.engine();
                    start.wait();
                    let mut last = Ok(Vec::new());
                    for round in 0..5 {
                        last = batch_report(engine.batch_create(
                            vec![
                                (create_args(first, &format!("{tag} A{round}"), &[]), None),
                                (create_args(second, &format!("{tag} B{round}"), &[]), None),
                            ],
                            Actor::Cli,
                            None,
                            false,
                        ));
                        if last.is_err() {
                            break;
                        }
                    }
                    last
                })
            })
            .collect();
        for writer in writers {
            let outcome = writer.join().unwrap();
            assert!(outcome.is_ok(), "{pair:?}: {outcome:?}");
        }
        let fresh = ws.engine();
        let created = fresh
            .store()
            .all_entities()
            .filter(|e| e.title.starts_with("One ") || e.title.starts_with("Two "))
            .count();
        assert_eq!(
            created, 20,
            "{pair:?}: every acknowledged create is in storage"
        );
    }
}

// ---------------------------------------------------------------------
// Entity rename: referrers in other mems
// ---------------------------------------------------------------------

/// Give `right--gamma` a body link to `left--alpha`, so a rename of
/// alpha has a referrer to rewrite in the other mem.
fn link_gamma_to_alpha(ws: &Workspace) {
    let mut engine = ws.engine();
    let linked = add_link(&mut engine, Link::BodyLink, &|| {});
    assert!(linked.is_ok(), "{linked:?}");
}

fn rename_args(engine: &Engine, target: &EntityId, new_title: &str) -> RenameEntityArgs {
    RenameEntityArgs {
        id: target.clone(),
        expected_hash: engine.get_entity(target).map(|e| e.content_hash.clone()),
        new_title: new_title.to_string(),
    }
}

/// The view without content hashes: two runs of one operation stamp
/// different times into the bytes and agree on everything else.
fn shape(engine: &Engine) -> BTreeMap<String, String> {
    view(engine)
        .into_iter()
        .map(|(id, line)| {
            let hash_at = line.find(" hash=").unwrap();
            let out_at = line.find(" out=").unwrap();
            (id, format!("{}{}", &line[..hash_at], &line[out_at..]))
        })
        .collect()
}

/// What an undisturbed rename of `left--alpha` ends in, on a workspace
/// of its own.
fn undisturbed_rename(pair: (Kind, Kind)) -> BTreeMap<String, String> {
    let ws = Workspace::new(pair);
    link_gamma_to_alpha(&ws);
    let mut engine = ws.engine();
    let args = rename_args(&engine, &id(LEFT, "alpha"), "Alpha Renamed");
    engine
        .rename_entity(args, Actor::Cli, None, None)
        .expect("an undisturbed rename lands");
    shape(&ws.engine())
}

/// A rename is held at its first commit and a second process rewrites
/// the referrer the rename is about to rewrite in the other mem. The
/// second write is never lost: the rename holds that mem's lock, so the
/// write waits and is then judged against the referrer as the rename
/// left it.
#[test]
fn an_update_to_a_referrer_while_a_rename_is_held_at_its_commit_is_not_lost() {
    for pair in PAIRS {
        let context = format!("{pair:?}");
        let ws = Workspace::new(pair);
        link_gamma_to_alpha(&ws);
        let (renamed, window, mut engine) = hold_at_commit(
            &ws,
            |second, ready| {
                let args = update_args(
                    second,
                    &id(RIGHT, "gamma"),
                    "Gamma purpose, second, see [[left--alpha]]",
                );
                ready();
                report(
                    second
                        .update_entity(args, Actor::Cli, None, None)
                        .map(|o| o.warnings),
                )
            },
            |engine| {
                let args = rename_args(engine, &id(LEFT, "alpha"), "Alpha Renamed");
                report(
                    engine
                        .rename_entity(args, Actor::Cli, None, None)
                        .map(|o| o.warnings),
                )
            },
        );
        assert!(renamed.is_ok(), "{context}: {renamed:?}");
        assert!(
            window.both_locked,
            "{context}: the rename holds the write lock of every writable mem at its first commit"
        );
        let fresh = ws.engine();
        let gamma = fresh.get_entity(&id(RIGHT, "gamma")).unwrap();
        let purpose = gamma.sections.get("purpose").cloned().unwrap_or_default();
        assert!(
            purpose.contains("[[left--alpha-renamed]]"),
            "{context}: the referrer points at the new id: {purpose}"
        );
        // An acknowledged second write is in storage; a refused one is
        // refused on the hash the rename changed.
        match &window.second {
            Ok(_) => assert!(
                purpose.contains("second"),
                "{context}: the second process's update was acknowledged and is gone: {purpose}"
            ),
            Err(code) => assert_eq!(code, "HASH_MISMATCH", "{context}"),
        }
        assert!(
            fresh.get_entity(&id(LEFT, "alpha")).is_none(),
            "{context}: nothing is left under the old id"
        );
        assert_level_with_storage(&ws, &mut engine, &context);
    }
}

/// A rename rewrites the referrers in a mem whose loading was deferred
/// and that no operation has loaded yet.
#[test]
fn a_rename_rewrites_the_referrers_in_a_mem_whose_loading_was_deferred() {
    for pair in PAIRS {
        let context = format!("{pair:?}");
        let ws = Workspace::new(pair);
        link_gamma_to_alpha(&ws);
        let mut engine = ws.engine_with_right_deferred();
        assert!(engine.mem_is_deferred(RIGHT), "{context}");
        let args = rename_args(&engine, &id(LEFT, "alpha"), "Alpha Renamed");
        engine
            .rename_entity(args, Actor::Cli, None, None)
            .unwrap_or_else(|e| panic!("{context}: {e:?}"));
        assert_eq!(
            shape(&ws.engine()),
            undisturbed_rename(pair),
            "{context}: the deferred peer's referrer follows the rename"
        );
    }
}

/// A storage failure at the last commit of a rename, after the peer
/// mem's commit landed. The call returns the partial-failure envelope;
/// no mem holds a staged write; the engine's view of every mem is what
/// a freshly loaded engine sees; the entity is still under the id and
/// the hash the caller sent; and the identical call completes the
/// rename into the state an undisturbed rename produces.
#[test]
fn a_rename_that_fails_after_a_peer_landed_reports_what_landed_and_the_same_call_completes_it() {
    for pair in PAIRS {
        let context = format!("{pair:?}");
        let ws = Workspace::new(pair);
        link_gamma_to_alpha(&ws);

        // Fail the first commit into `left`: the renamed entity's own
        // mem, which commits after the peer.
        let failed = Arc::new(AtomicBool::new(false));
        let order = Arc::new(Mutex::new(Vec::<String>::new()));
        let tap: Tap = {
            let (failed, order) = (Arc::clone(&failed), Arc::clone(&order));
            Arc::new(move |mem: &str, method: &str| {
                if !matches!(method, "commit" | "commit_with_expected_parent") {
                    return false;
                }
                order.lock().unwrap().push(mem.to_string());
                mem == LEFT && !failed.swap(true, Ordering::SeqCst)
            })
        };
        let mut engine = ws.tapped_engine(tap);
        let alpha = id(LEFT, "alpha");
        let args = rename_args(&engine, &alpha, "Alpha Renamed");
        let sent_hash = args.expected_hash.clone();
        let again = RenameEntityArgs {
            id: args.id.clone(),
            expected_hash: args.expected_hash.clone(),
            new_title: args.new_title.clone(),
        };

        let err = engine
            .rename_entity(args, Actor::Cli, None, None)
            .expect_err("the injected failure fails the call");
        assert_eq!(
            *order.lock().unwrap(),
            vec![RIGHT.to_string(), LEFT.to_string()],
            "{context}: the peer commits first, the renamed entity's own mem last"
        );
        let EngineError::RenamePartialFailure {
            committed_mems,
            failed_mem,
            failure_cause,
            mems,
        } = &err
        else {
            panic!("{context}: expected the partial-failure envelope, got {err:?}");
        };
        assert_eq!(committed_mems, &vec![RIGHT.to_string()], "{context}");
        assert_eq!(failed_mem, LEFT, "{context}");
        assert_eq!(failure_cause, "storage", "{context}");

        // The envelope against storage, mem by mem and item by item.
        let fresh = ws.engine();
        let gamma = id(RIGHT, "gamma");
        let purpose_of = |engine: &Engine| {
            engine
                .get_entity(&gamma)
                .and_then(|e| e.sections.get("purpose").cloned())
                .unwrap_or_default()
        };
        assert!(
            purpose_of(&fresh).contains("[[left--alpha-renamed]]"),
            "{context}: the peer's commit landed"
        );
        assert!(
            fresh.get_entity(&alpha).is_some_and(|e| !e.stub),
            "{context}: the entity is still under the id the caller used"
        );
        assert_eq!(
            fresh.get_entity(&alpha).map(|e| e.content_hash.clone()),
            sent_hash,
            "{context}: with the hash the caller sent"
        );
        for reported in mems {
            let landed_in_storage = match reported.mem.as_str() {
                RIGHT => true,
                LEFT => false,
                other => panic!("{context}: the envelope names a mem nobody wrote: {other}"),
            };
            assert_eq!(
                reported.landed, landed_in_storage,
                "{context}: {reported:?}"
            );
            assert_eq!(
                reported.error.is_some(),
                reported.mem == LEFT,
                "{context}: {reported:?}"
            );
            for item in &reported.items {
                assert_eq!(item.landed, landed_in_storage, "{context}: {item:?}");
                let (mem, slug) = item.id.split_once("--").unwrap();
                let now = id(mem, slug);
                assert_eq!(
                    item.current_hash,
                    fresh.get_entity(&now).map(|e| e.content_hash.clone()),
                    "{context}: {item:?}"
                );
            }
        }
        let named: Vec<(&str, Vec<&str>)> = mems
            .iter()
            .map(|m| {
                (
                    m.mem.as_str(),
                    m.items.iter().map(|i| i.id.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            named,
            vec![(RIGHT, vec!["right--gamma"]), (LEFT, vec!["left--alpha"])],
            "{context}: every mem and every entity the rename writes is named"
        );

        // The running engine's view, as it is, equals a fresh load's:
        // nothing staged is served, nothing landed is missing.
        assert_eq!(view(&engine), view(&fresh), "{context}");

        // The identical call, with the identical hash, completes it.
        let completed = engine
            .rename_entity(again, Actor::Cli, None, None)
            .unwrap_or_else(|e| panic!("{context}: the same call sent again: {e:?}"));
        assert_eq!(completed.new_id, id(LEFT, "alpha-renamed"), "{context}");
        assert_eq!(
            shape(&ws.engine()),
            undisturbed_rename(pair),
            "{context}: the completed rename equals an undisturbed one"
        );
        assert_level_with_storage(&ws, &mut engine, &context);
    }
}

// ---------------------------------------------------------------------
// Mem rename: the sweep over the peers
// ---------------------------------------------------------------------

/// References to `left` in both mems: `right--gamma` links to
/// `left--alpha`, and `left--beta` links to it by its full id.
fn link_both_mems_to_alpha(ws: &Workspace) {
    link_gamma_to_alpha(ws);
    let mut engine = ws.engine();
    let args = update_args(
        &engine,
        &id(LEFT, "beta"),
        "Beta purpose, see [[left--alpha]]",
    );
    engine
        .update_entity(args, Actor::Cli, None, None)
        .expect("the self-reference lands");
}

fn purpose(engine: &Engine, target: &EntityId) -> String {
    engine
        .get_entity(target)
        .and_then(|e| e.sections.get("purpose").cloned())
        .unwrap_or_default()
}

/// The sweep of a mem rename is held at its first commit and a second
/// process rewrites the peer's referrer the sweep is about to rewrite.
/// The sweep holds that mem's lock, so the second write waits and is
/// judged against the referrer as the sweep left it; it is never
/// written over.
#[test]
fn an_update_to_a_referrer_while_a_mem_renames_sweep_is_held_at_its_commit_is_not_lost() {
    for pair in PAIRS {
        let context = format!("{pair:?}");
        let ws = Workspace::new(pair);
        link_both_mems_to_alpha(&ws);
        let (swept, window, mut engine) = hold_at_commit(
            &ws,
            |second, ready| {
                let args = update_args(
                    second,
                    &id(RIGHT, "gamma"),
                    "Gamma purpose, second, see [[left--alpha]]",
                );
                ready();
                report(
                    second
                        .update_entity(args, Actor::Cli, None, None)
                        .map(|o| o.warnings),
                )
            },
            |engine| {
                engine
                    .rewrite_mem_references(LEFT, "lefty", None)
                    .map(|outcome| outcome.rewritten_mems)
                    .map_err(|e| e.code().to_string())
            },
        );
        assert_eq!(
            swept,
            Ok(vec![RIGHT.to_string(), LEFT.to_string()]),
            "{context}: the peer is swept first, the renamed mem last"
        );
        assert!(
            window.both_locked,
            "{context}: the sweep holds the write lock of every writable mem at its first commit"
        );
        let fresh = ws.engine();
        let gamma = purpose(&fresh, &id(RIGHT, "gamma"));
        assert!(
            gamma.contains("[[lefty--alpha]]"),
            "{context}: the referrer points at the new mem name: {gamma}"
        );
        match &window.second {
            Ok(_) => assert!(
                gamma.contains("second"),
                "{context}: the second process's update was acknowledged and is gone: {gamma}"
            ),
            Err(code) => assert_eq!(code, "HASH_MISMATCH", "{context}"),
        }
        assert_level_with_storage(&ws, &mut engine, &context);
    }
}

/// A storage failure at the sweep's last commit, into the renamed mem
/// itself, after the peer's commit landed. The sweep returns the
/// partial-failure envelope; the engine serves what storage holds; and
/// the same sweep sent again finds the peer done and completes into
/// the state an undisturbed sweep produces.
#[test]
fn a_mem_renames_sweep_that_fails_after_a_peer_landed_reports_what_landed_and_completes_when_resent()
 {
    for pair in PAIRS {
        let context = format!("{pair:?}");
        let undisturbed = {
            let ws = Workspace::new(pair);
            link_both_mems_to_alpha(&ws);
            ws.engine()
                .rewrite_mem_references(LEFT, "lefty", None)
                .expect("an undisturbed sweep lands");
            shape(&ws.engine())
        };

        let ws = Workspace::new(pair);
        link_both_mems_to_alpha(&ws);
        let failed = Arc::new(AtomicBool::new(false));
        let tap: Tap = {
            let failed = Arc::clone(&failed);
            Arc::new(move |mem: &str, method: &str| {
                matches!(method, "commit" | "commit_with_expected_parent")
                    && mem == LEFT
                    && !failed.swap(true, Ordering::SeqCst)
            })
        };
        let mut engine = ws.tapped_engine(tap);
        let beta_before = engine
            .get_entity(&id(LEFT, "beta"))
            .map(|e| e.content_hash.clone());

        let err = engine
            .rewrite_mem_references(LEFT, "lefty", None)
            .expect_err("the injected failure fails the sweep");
        let EngineError::RenamePartialFailure {
            committed_mems,
            failed_mem,
            failure_cause,
            mems,
        } = &err
        else {
            panic!("{context}: expected the partial-failure envelope, got {err:?}");
        };
        assert_eq!(committed_mems, &vec![RIGHT.to_string()], "{context}");
        assert_eq!(failed_mem, LEFT, "{context}");
        assert_eq!(failure_cause, "storage", "{context}");

        // The envelope against storage.
        let fresh = ws.engine();
        assert!(
            purpose(&fresh, &id(RIGHT, "gamma")).contains("[[lefty--alpha]]"),
            "{context}: the peer's commit landed"
        );
        assert!(
            purpose(&fresh, &id(LEFT, "beta")).contains("[[left--alpha]]"),
            "{context}: the renamed mem's own part did not land"
        );
        assert_eq!(
            fresh
                .get_entity(&id(LEFT, "beta"))
                .map(|e| e.content_hash.clone()),
            beta_before,
            "{context}"
        );
        let reported: Vec<String> = mems
            .iter()
            .map(|m| {
                let items: Vec<String> = m
                    .items
                    .iter()
                    .map(|i| format!("{} landed={}", i.id, i.landed))
                    .collect();
                format!(
                    "{} landed={} error={} [{}]",
                    m.mem,
                    m.landed,
                    m.error.is_some(),
                    items.join(", ")
                )
            })
            .collect();
        assert_eq!(
            reported,
            vec![
                "right landed=true error=false [right--gamma landed=true]",
                "left landed=false error=true [left--beta landed=false]",
            ],
            "{context}: every mem and every entity the sweep writes is named"
        );
        for item in mems.iter().flat_map(|m| &m.items) {
            let (mem, slug) = item.id.split_once("--").unwrap();
            assert_eq!(
                item.current_hash,
                fresh
                    .get_entity(&id(mem, slug))
                    .map(|e| e.content_hash.clone()),
                "{context}: {item:?}"
            );
        }
        // The running engine's view, as it is, equals a fresh load's.
        assert_eq!(view(&engine), view(&fresh), "{context}");

        // The same sweep again: the peer is done, the renamed mem lands.
        let again = engine
            .rewrite_mem_references(LEFT, "lefty", None)
            .unwrap_or_else(|e| panic!("{context}: the same sweep sent again: {e:?}"));
        assert_eq!(again.rewritten_mems, vec![LEFT.to_string()], "{context}");
        assert_eq!(shape(&ws.engine()), undisturbed, "{context}");
        assert_eq!(view(&engine), view(&ws.engine()), "{context}");
    }
}

// ---------------------------------------------------------------------
// Conflict resolution: one folder mem
// ---------------------------------------------------------------------

/// The pairings whose `left` is a folder mem: a merge conflict in an
/// entity file is a folder mem's state.
const LEFT_IS_A_FOLDER: [(Kind, Kind); 2] = [
    (Kind::Folder, Kind::Folder),
    (Kind::Folder, Kind::GitBranch),
];

/// Leave `left--alpha` as a git merge left it: both sides in the file,
/// between conflict markers. Written as a file, as the merge would.
fn conflict_alpha(ws: &Workspace) {
    let MountStorage::Folder { path } = &ws.mounts[0].storage else {
        panic!("a conflicted entity file needs `left` on the folder backend");
    };
    std::fs::write(
        path.join("alpha.md"),
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n\n\
         # Alpha\n\n## Identity\n\n<<<<<<< HEAD\nours identity\n=======\ntheirs identity\n\
         >>>>>>> theirs\n\n## Purpose\n\nAlpha purpose\n",
    )
    .unwrap();
}

/// A conflict resolution commits to its mem under that mem's lock.
#[test]
fn a_conflict_resolution_waits_on_the_lock_of_the_mem_it_commits_to() {
    assert_waits_on_the_lock_of(LEFT, &LEFT_IS_A_FOLDER, &[Waiter::ResolveConflict]);
}

/// A resolution is held at its commit and a second process resolves
/// the same entity to the other side. The second resolution is never
/// written over: the first holds the mem's lock, so the second waits
/// and then finds the entity resolved.
#[test]
fn a_second_resolution_while_a_conflict_resolution_is_held_at_its_commit_is_not_written_over() {
    for pair in LEFT_IS_A_FOLDER {
        let context = format!("{pair:?}");
        let ws = Workspace::new(pair);
        conflict_alpha(&ws);
        let alpha = id(LEFT, "alpha");
        let (first, window, mut engine) = hold_at_commit_probing(
            &ws,
            &[LEFT],
            {
                let alpha = alpha.clone();
                move |second, ready| {
                    ready();
                    report(
                        second
                            .resolve_merge_conflict(
                                &alpha,
                                ConflictSide::Theirs,
                                Actor::Cli,
                                None,
                                None,
                            )
                            .map(|o| o.warnings),
                    )
                }
            },
            |engine| {
                report(
                    engine
                        .resolve_merge_conflict(&alpha, ConflictSide::Ours, Actor::Cli, None, None)
                        .map(|o| o.warnings),
                )
            },
        );
        assert!(first.is_ok(), "{context}: {first:?}");
        assert!(
            window.both_locked,
            "{context}: the resolution holds its mem's write lock at its commit"
        );
        let fresh = ws.engine();
        let identity = fresh
            .get_entity(&alpha)
            .and_then(|e| e.sections.get("identity").cloned())
            .unwrap_or_default();
        // An acknowledged second resolution is what storage holds; a
        // refused one found the entity already resolved.
        match &window.second {
            Ok(_) => assert!(
                identity.contains("theirs"),
                "{context}: the second resolution was acknowledged and is gone: {identity}"
            ),
            Err(code) => {
                assert_eq!(code, "NOT_CONFLICTED", "{context}");
                assert!(identity.contains("ours"), "{context}: {identity}");
            }
        }
        assert_level_with_storage(&ws, &mut engine, &context);
    }
}

// ---------------------------------------------------------------------
// Guards that read more than the mems a call names: the cycle guard,
// a peer whose reload failed, a bare slug
// ---------------------------------------------------------------------

fn depends_update(engine: &Engine, from: &EntityId, to: &EntityId) -> UpdateEntityArgs {
    let mut args = update_args(engine, from, "");
    args.sections.clear();
    args.declare_relations = vec![memstead_base::ops::RelateArg {
        target: to.clone(),
        rel_type: "DEPENDS_ON".to_string(),
        description: None,
    }];
    args
}

fn depends_edge(engine: &Engine, from: &EntityId, to: &EntityId) -> bool {
    engine
        .store()
        .outgoing(from)
        .iter()
        .any(|e| e.rel_type == "DEPENDS_ON" && &e.target == to)
}

fn stored_relationships(ws: &Workspace, mem: &str, file: &str) -> String {
    let mount = ws.mounts.iter().find(|m| m.mem == mem).unwrap();
    let bytes = instantiate_full_backend(mount)
        .unwrap()
        .read_entity(Path::new(file))
        .unwrap()
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    text.split("## Relationships")
        .nth(1)
        .unwrap_or("(none)")
        .trim()
        .to_string()
}

#[derive(Debug, Clone, Copy)]
enum First {
    /// Batch update of left--alpha declaring DEPENDS_ON onto right--gamma.
    BatchUpdate,
    /// Batch create of left--delta with DEPENDS_ON onto right--gamma.
    BatchCreate,
}

fn run_first(first: First, engine: &mut Engine) -> Report {
    match first {
        First::BatchUpdate => {
            let args = depends_update(engine, &id(LEFT, "alpha"), &id(RIGHT, "gamma"));
            batch_report(engine.batch_update(vec![(args, None)], Actor::Cli, None, false))
        }
        First::BatchCreate => {
            let mut args = create_args(LEFT, "Delta", &[id(RIGHT, "gamma")]);
            args.relations[0].rel_type = "DEPENDS_ON".to_string();
            batch_report(engine.batch_create(vec![(args, None)], Actor::Cli, None, false))
        }
    }
}

fn first_source(first: First) -> (EntityId, &'static str) {
    match first {
        First::BatchUpdate => (id(LEFT, "alpha"), "alpha.md"),
        First::BatchCreate => (id(LEFT, "delta"), "delta.md"),
    }
}

/// The second process: a batch update of right--gamma declaring
/// DEPENDS_ON back onto the first one's source.
fn run_second(first: First, engine: &mut Engine, ready: &dyn Fn()) -> Report {
    let args = depends_update(engine, &id(RIGHT, "gamma"), &first_source(first).0);
    ready();
    batch_report(engine.batch_update(vec![(args, None)], Actor::Cli, None, false))
}

/// DEPENDS_ON is acyclic. Two single-item batches, each in its own mem,
/// each declaring a DEPENDS_ON onto the other's entity. Run one after
/// the other, the second is refused as a cycle. With the first held at
/// its commit and the second run by a second process, both must not be
/// acknowledged, and an acknowledged edge must be served by a fresh
/// engine.
#[test]
fn two_writes_in_two_mems_cannot_close_a_cycle_between_them() {
    let gamma = id(RIGHT, "gamma");
    let mut failures = Vec::new();
    for first in [First::BatchUpdate, First::BatchCreate] {
        for pair in PAIRS {
            let context = format!("{first:?} {pair:?}");
            let (source, file) = first_source(first);
            // Control: one after the other.
            {
                let ws = Workspace::new(pair);
                let one = run_first(first, &mut ws.engine());
                assert!(one.is_ok(), "{context}: {one:?}");
                let two = run_second(first, &mut ws.engine(), &|| {});
                eprintln!("{context}: sequential control: first={one:?} second={two:?}");
                assert_eq!(two, Err("RELATIONSHIP_CYCLE".to_string()), "{context}");
            }
            // Interleaved.
            let ws = Workspace::new(pair);
            let (one, window, engine) = hold_at_commit(
                &ws,
                move |second, ready| run_second(first, second, ready),
                |engine| run_first(first, engine),
            );
            drop(engine);
            let fresh = ws.engine();
            let (out, back) = (
                depends_edge(&fresh, &source, &gamma),
                depends_edge(&fresh, &gamma, &source),
            );
            let line = format!(
                "{context}: interleaved: first={one:?} second={:?} both_locked={} | fresh engine serves {source}>gamma={out} gamma>{source}={back} | storage left/{file}: {:?} right/gamma.md: {:?}",
                window.second,
                window.both_locked,
                stored_relationships(&ws, LEFT, file),
                stored_relationships(&ws, RIGHT, "gamma.md"),
            );
            eprintln!("{line}");
            let both_acknowledged = one.is_ok() && window.second.is_ok();
            if both_acknowledged || (one.is_ok() && !out) || (window.second.is_ok() && !back) {
                failures.push(line);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A second process adds a reference from `right`; then this engine's
/// reload of `right`, under the lock, meets one storage read failure.
/// The verb must not be acknowledged on a guard (or a referrer list)
/// computed from the view of `right` it took before the lock.
#[test]
fn a_failed_reload_of_the_other_mem_refuses_a_guard_that_would_run_on_the_old_view() {
    let mut failures = Vec::new();
    for verb in ["delete", "rename"] {
        for pair in PAIRS {
            let context = format!("{verb} {pair:?}");
            let ws = Workspace::new(pair);
            let armed = Arc::new(AtomicBool::new(false));
            let fired = Arc::new(Mutex::new(Vec::<String>::new()));
            let tap: Tap = {
                let (armed, fired) = (Arc::clone(&armed), Arc::clone(&fired));
                Arc::new(move |mem: &str, method: &str| {
                    if mem == RIGHT
                        && matches!(
                            method,
                            "read_all_entities" | "list_entities" | "read_entity"
                        )
                        && armed.swap(false, Ordering::SeqCst)
                    {
                        fired.lock().unwrap().push(method.to_string());
                        return true;
                    }
                    false
                })
            };
            let mut engine = ws.tapped_engine(tap);
            let alpha = id(LEFT, "alpha");
            let hash = engine.get_entity(&alpha).map(|e| e.content_hash.clone());
            let added = ws
                .second_writer(move |second| add_link(second, Link::BodyLink, &|| {}))
                .join()
                .unwrap();
            assert!(added.is_ok(), "{context}: {added:?}");

            armed.store(true, Ordering::SeqCst);
            let outcome = if verb == "delete" {
                report(
                    engine
                        .delete_entity(
                            DeleteEntityArgs {
                                id: alpha.clone(),
                                expected_hash: hash,
                            },
                            Actor::Cli,
                            None,
                            None,
                        )
                        .map(|o| o.warnings),
                )
            } else {
                report(
                    engine
                        .rename_entity(
                            RenameEntityArgs {
                                id: alpha.clone(),
                                expected_hash: hash,
                                new_title: "Alpha Renamed".to_string(),
                            },
                            Actor::Cli,
                            None,
                            None,
                        )
                        .map(|o| o.warnings),
                )
            };
            armed.store(false, Ordering::SeqCst);
            let fresh = ws.engine();
            let alpha_real = fresh.get_entity(&alpha).is_some_and(|e| !e.stub);
            let gamma_purpose = fresh
                .get_entity(&id(RIGHT, "gamma"))
                .and_then(|e| e.sections.get("purpose").cloned())
                .unwrap_or_default();
            let line = format!(
                "{context}: fault fired on {:?}; verb returned {outcome:?}; storage: left--alpha real={alpha_real}, right--gamma purpose={gamma_purpose:?}",
                fired.lock().unwrap()
            );
            eprintln!("{line}");
            let dangling = !alpha_real && gamma_purpose.contains("[[left--alpha]]");
            if fired.lock().unwrap().is_empty() || (outcome.is_ok() && dangling) {
                failures.push(line);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A bare slug is resolved across every mem. A second process creates
/// a second carrier of the slug in the other mem; a freshly loaded
/// engine then refuses the bare slug as ambiguous. The engine that
/// loaded before must not pick its target from that older view.
#[test]
fn a_short_id_is_resolved_under_the_lock_not_from_the_view_before_it() {
    let mut failures = Vec::new();
    for verb in ["delete", "rename", "retype"] {
        for pair in PAIRS {
            let context = format!("{verb} {pair:?}");
            let ws = Workspace::new(pair);
            let mut engine = ws.engine();
            let created = ws
                .second_writer(|second| {
                    report(
                        second
                            .create_entity(create_args(RIGHT, "Alpha", &[]), Actor::Cli, None, None)
                            .map(|o| o.warnings),
                    )
                })
                .join()
                .unwrap();
            assert!(created.is_ok(), "{context}: {created:?}");
            let bare = EntityId("alpha".to_string());
            let run = |engine: &mut Engine| match verb {
                "delete" => report(
                    engine
                        .delete_entity(
                            DeleteEntityArgs {
                                id: bare.clone(),
                                expected_hash: None,
                            },
                            Actor::Cli,
                            None,
                            None,
                        )
                        .map(|o| o.warnings),
                ),
                "rename" => report(
                    engine
                        .rename_entity(
                            RenameEntityArgs {
                                id: bare.clone(),
                                expected_hash: None,
                                new_title: "Alpha Renamed".to_string(),
                            },
                            Actor::Cli,
                            None,
                            None,
                        )
                        .map(|o| o.warnings),
                ),
                _ => {
                    let mut args = retype_args(engine, &id(LEFT, "alpha"));
                    args.id = bare.clone();
                    args.expected_hash = None;
                    report(
                        engine
                            .retype_entity(args, Actor::Cli, None, None)
                            .map(|o| o.warnings),
                    )
                }
            };
            let control = run(&mut ws.engine());
            let outcome = run(&mut engine);
            let line = format!(
                "{context}: a freshly loaded engine answers {control:?}; the engine loaded before answers {outcome:?}"
            );
            eprintln!("{line}");
            if control.is_err() && outcome.is_ok() {
                failures.push(line);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// ---------------------------------------------------------------------
// A batch that fails after one mem committed
// ---------------------------------------------------------------------

/// The item of each batch form that goes to `right`, on its own: what
/// a caller sends after the envelope named it as not landed.
fn resend_the_right_item(waiter: Waiter, engine: &mut Engine) -> Report {
    let actor = Actor::Cli;
    match waiter {
        Waiter::BatchCreate => batch_report(engine.batch_create(
            vec![(create_args(RIGHT, "Epsilon", &[]), None)],
            actor,
            None,
            false,
        )),
        Waiter::BatchUpdate => {
            let updates = vec![(
                update_args(engine, &id(RIGHT, "gamma"), "Gamma purpose, batch"),
                None,
            )];
            batch_report(engine.batch_update(updates, actor, None, false))
        }
        Waiter::BatchRelate => batch_report(engine.batch_relate(
            vec![(relate_args(&id(RIGHT, "gamma"), &id(LEFT, "beta")), None)],
            actor,
            None,
            false,
        )),
        other => panic!("{other:?} is no batch form"),
    }
}

/// A storage failure at a batch's second commit, after its first mem
/// committed. The batch returns the partial-failure envelope, which
/// names every mem and every item with whether it landed and its hash
/// now; no mem holds a staged write; the engine serves what storage
/// holds, the first mem's commit included; and sending the item the
/// envelope names as not landed completes the batch into the state an
/// undisturbed batch produces.
#[test]
fn a_batch_that_fails_after_one_mem_committed_reports_what_landed_and_the_rest_completes_it() {
    for pair in PAIRS {
        for waiter in [
            Waiter::BatchCreate,
            Waiter::BatchUpdate,
            Waiter::BatchRelate,
        ] {
            let context = format!("{pair:?} {waiter:?}");
            let undisturbed = {
                let ws = Workspace::new(pair);
                let done = run_waiter(waiter, &mut ws.engine());
                assert!(done.is_ok(), "{context}: {done:?}");
                shape(&ws.engine())
            };

            let ws = Workspace::new(pair);
            let failed = Arc::new(AtomicBool::new(false));
            let tap: Tap = {
                let failed = Arc::clone(&failed);
                Arc::new(move |mem: &str, method: &str| {
                    matches!(method, "commit" | "commit_with_expected_parent")
                        && mem == RIGHT
                        && !failed.swap(true, Ordering::SeqCst)
                })
            };
            let mut engine = ws.tapped_engine(tap);
            let actor = Actor::Cli;
            let err = match waiter {
                Waiter::BatchCreate => engine
                    .batch_create(
                        vec![
                            (create_args(LEFT, "Delta", &[]), None),
                            (create_args(RIGHT, "Epsilon", &[]), None),
                        ],
                        actor,
                        None,
                        false,
                    )
                    .err(),
                Waiter::BatchUpdate => {
                    let updates = vec![
                        (
                            update_args(&engine, &id(LEFT, "alpha"), "Alpha purpose, batch"),
                            None,
                        ),
                        (
                            update_args(&engine, &id(RIGHT, "gamma"), "Gamma purpose, batch"),
                            None,
                        ),
                    ];
                    engine.batch_update(updates, actor, None, false).err()
                }
                _ => engine
                    .batch_relate(
                        vec![
                            (relate_args(&id(LEFT, "alpha"), &id(LEFT, "beta")), None),
                            (relate_args(&id(RIGHT, "gamma"), &id(LEFT, "beta")), None),
                        ],
                        actor,
                        None,
                        false,
                    )
                    .err(),
            };
            let Some(EngineError::RenamePartialFailure {
                committed_mems,
                failed_mem,
                failure_cause,
                mems,
            }) = &err
            else {
                panic!("{context}: expected the partial-failure envelope, got {err:?}");
            };
            assert_eq!(committed_mems, &vec![LEFT.to_string()], "{context}");
            assert_eq!(failed_mem, RIGHT, "{context}");
            assert_eq!(failure_cause, "storage", "{context}");

            // The envelope against storage, mem by mem and item by item.
            let fresh = ws.engine();
            let (left_item, right_item) = match waiter {
                Waiter::BatchCreate => ("left--delta", "right--epsilon"),
                _ => ("left--alpha", "right--gamma"),
            };
            let reported: Vec<String> = mems
                .iter()
                .map(|m| {
                    let items: Vec<String> = m
                        .items
                        .iter()
                        .map(|i| format!("{} landed={}", i.id, i.landed))
                        .collect();
                    format!(
                        "{} landed={} error={} [{}]",
                        m.mem,
                        m.landed,
                        m.error.is_some(),
                        items.join(", ")
                    )
                })
                .collect();
            assert_eq!(
                reported,
                vec![
                    format!("left landed=true error=false [{left_item} landed=true]"),
                    format!("right landed=false error=true [{right_item} landed=false]"),
                ],
                "{context}: every mem and every item of the batch is named"
            );
            for item in mems.iter().flat_map(|m| &m.items) {
                let (mem, slug) = item.id.split_once("--").unwrap();
                assert_eq!(
                    item.current_hash,
                    fresh
                        .get_entity(&id(mem, slug))
                        .filter(|e| !e.stub)
                        .map(|e| e.content_hash.clone()),
                    "{context}: {item:?}"
                );
            }
            // The first mem's item is in storage, the second mem's is
            // not, as the envelope says.
            let in_storage = |mem: &str, slug: &str| match waiter {
                Waiter::BatchCreate => fresh.get_entity(&id(mem, slug)).is_some(),
                Waiter::BatchUpdate => purpose(&fresh, &id(mem, slug)).contains("batch"),
                _ => !fresh.store().outgoing(&id(mem, slug)).is_empty(),
            };
            let (left_slug, right_slug) = (
                left_item.split_once("--").unwrap().1,
                right_item.split_once("--").unwrap().1,
            );
            assert!(
                in_storage(LEFT, left_slug),
                "{context}: the first mem's commit landed"
            );
            assert!(
                !in_storage(RIGHT, right_slug),
                "{context}: the second mem's did not"
            );
            // The running engine serves what storage holds, as it is.
            assert_eq!(view(&engine), view(&fresh), "{context}");

            // The item named as not landed, sent on its own.
            let rest = resend_the_right_item(waiter, &mut engine);
            assert!(rest.is_ok(), "{context}: {rest:?}");
            assert_eq!(shape(&ws.engine()), undisturbed, "{context}");
            assert_eq!(view(&engine), view(&ws.engine()), "{context}");
        }
    }
}

// ---------------------------------------------------------------------
// A mem attached to the workspace after the engine started
// ---------------------------------------------------------------------

/// An engine boots from a workspace; a second process then creates a
/// mem in it. The first engine's first write into that mem is made
/// under the mem's write lock: the roster is reconciled before the lock
/// set is computed, so the mem is mounted, and locked, before the write
/// is planned.
///
/// Another process holds the new mem's lock, as an open file held by a
/// second thread. That thread releases it when the write has returned
/// (which is then a write that did not wait) or after half a second,
/// and notes the mem's head at that moment.
#[test]
fn the_first_write_into_a_mem_attached_after_the_engine_started_waits_on_its_lock() {
    use memstead_base::mem_management;
    use memstead_git_branch::workspace_store::engine_from_workspace_root;

    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", SCHEMA)]);
    let mut first = engine_from_workspace_root(tmp.path()).expect("the first engine boots");

    // The second process: it creates the mem and is gone.
    {
        let mut second = engine_from_workspace_root(tmp.path()).expect("the second engine boots");
        mem_management::create_mem(
            &mut second,
            mem_management::MemCreateParams {
                name: "late".to_string(),
                location: PathBuf::from("late"),
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
        )
        .expect("the mem is created");
    }
    assert!(
        first.mount("late").is_none(),
        "the first engine has not seen the new mem yet"
    );

    let late = engine_from_workspace_root(tmp.path())
        .expect("a fresh engine boots")
        .mount("late")
        .expect("a fresh engine mounts the new mem")
        .clone();
    let head = move || {
        instantiate_full_backend(&late)
            .unwrap()
            .current_head()
            .unwrap()
    };
    let lock_path = {
        let fresh = engine_from_workspace_root(tmp.path()).unwrap();
        instantiate_full_backend(fresh.mount("late").unwrap())
            .unwrap()
            .write_lock_path()
            .expect("a git-branch mem has a write lock")
    };
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    lock.lock().unwrap();
    let head_before = head();

    let returned = Arc::new(AtomicBool::new(false));
    let holder = {
        let returned = Arc::clone(&returned);
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while !returned.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                std::thread::yield_now();
            }
            let returned_while_held = returned.load(Ordering::SeqCst);
            let head_while_held = head();
            lock.unlock().unwrap();
            (returned_while_held, head_while_held, head)
        })
    };

    let created = first.create_entity(create_args("late", "Delta", &[]), Actor::Cli, None, None);
    returned.store(true, Ordering::SeqCst);
    let (returned_while_held, head_while_held, head) = holder.join().unwrap();

    assert!(
        !returned_while_held,
        "the write returned while another process held the new mem's lock: {created:?}"
    );
    assert_eq!(
        head_while_held, head_before,
        "the write landed while another process held the new mem's lock"
    );
    created.expect("the write lands once the lock is released");
    assert_ne!(head(), head_before);
}

// ---------------------------------------------------------------------
// More interleavings: unconditional second writes, a full mem rename,
// retype, a deferred peer, single verbs closing a cycle
// ---------------------------------------------------------------------

// =====================================================================
// own refutation attempts
// =====================================================================

fn engine_after_rename(ws: &Workspace, new: &str) -> Engine {
    let mounts = ws
        .mounts
        .iter()
        .map(|m| {
            let mut m = m.clone();
            if m.mem == LEFT {
                m.mem = new.to_string();
                if let MountStorage::GitBranch { branch, .. } = &mut m.storage {
                    *branch = new.to_string();
                }
            }
            let backend = instantiate_full_backend(&m).expect("backend instantiates");
            (m, backend)
        })
        .collect();
    let mut engine = Engine::from_mounts(mounts).expect("engine boots");
    engine.set_git_branch_ops(memstead_git_branch::storage::FULL_GIT_BRANCH_OPS);
    engine.set_backend_factory(instantiate_full_backend);
    let mut settings = memstead_base::WorkspaceSettings::default();
    for mem in [new, RIGHT] {
        settings.cross_mem_links.insert(
            mem.to_string(),
            memstead_schema::workspace_config::CrossLinkValue::Wildcard,
        );
    }
    engine.set_settings(settings);
    engine
}

fn identity_update(engine: &Engine, target: &EntityId, text: &str) -> UpdateEntityArgs {
    let mut args = update_args(engine, target, "");
    args.sections.clear();
    args.sections
        .insert("identity".to_string(), text.to_string());
    args.expected_hash = None;
    args
}

fn section(engine: &Engine, target: &EntityId, key: &str) -> String {
    engine
        .get_entity(target)
        .and_then(|e| e.sections.get(key).cloned())
        .unwrap_or_default()
}

/// (1) literal reading: an unconditional update sent while the rename /
/// the sweep is held is present in the final state, next to the
/// operation's rewrite.
#[test]
fn unconditional_update_behind_rename_and_sweep_is_present() {
    let mut failures = Vec::new();
    for pair in PAIRS {
        for op in ["rename", "sweep"] {
            for target in [(RIGHT, "gamma"), (LEFT, "beta")] {
                let context = format!("{pair:?} {op} {target:?}");
                let ws = Workspace::new(pair);
                link_both_mems_to_alpha(&ws);
                let tid = id(target.0, target.1);
                let (done, window, mut engine) = hold_at_commit(
                    &ws,
                    {
                        let tid = tid.clone();
                        move |second, ready| {
                            let args = identity_update(second, &tid, "identity, second");
                            ready();
                            report(
                                second
                                    .update_entity(args, Actor::Cli, None, None)
                                    .map(|o| o.warnings),
                            )
                        }
                    },
                    |engine| {
                        if op == "rename" {
                            let args = rename_args(engine, &id(LEFT, "alpha"), "Alpha Renamed");
                            report(
                                engine
                                    .rename_entity(args, Actor::Cli, None, None)
                                    .map(|o| o.warnings),
                            )
                        } else {
                            engine
                                .rewrite_mem_references(LEFT, "lefty", None)
                                .map(|o| o.rewritten_mems)
                                .map_err(|e| e.code().to_string())
                        }
                    },
                );
                let fresh = ws.engine();
                let identity = section(&fresh, &tid, "identity");
                let purpose = section(&fresh, &tid, "purpose");
                let want = if op == "rename" {
                    "[[left--alpha-renamed]]"
                } else {
                    "[[lefty--alpha]]"
                };
                engine.reload_if_stale(None);
                let level = view(&engine) == view(&fresh);
                let line = format!(
                    "{context}: op={done:?} second={:?} both_locked={} identity={identity:?} purpose={purpose:?} level={level}",
                    window.second, window.both_locked
                );
                eprintln!("{line}");
                if done.is_err()
                    || !window.both_locked
                    || window.second.is_err()
                    || !identity.contains("second")
                    || !purpose.contains(want)
                    || !level
                {
                    failures.push(line);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A full mem rename held at its first commit; the second process
/// writes into the mem being renamed. An acknowledged write must be in
/// the renamed mem.
#[test]
fn write_into_the_renamed_mem_during_a_mem_rename() {
    let mut failures = Vec::new();
    for pair in PAIRS {
        for what in ["update", "create", "update_right"] {
            let context = format!("{pair:?} {what}");
            let ws = Workspace::new(pair);
            link_both_mems_to_alpha(&ws);
            let (renamed, window, engine) = hold_at_commit(
                &ws,
                move |second, ready| match what {
                    "update" => {
                        let args = identity_update(second, &id(LEFT, "beta"), "identity, second");
                        ready();
                        report(
                            second
                                .update_entity(args, Actor::Cli, None, None)
                                .map(|o| o.warnings),
                        )
                    }
                    "create" => {
                        ready();
                        report(
                            second
                                .create_entity(
                                    create_args(LEFT, "Zeta", &[]),
                                    Actor::Cli,
                                    None,
                                    None,
                                )
                                .map(|o| o.warnings),
                        )
                    }
                    _ => {
                        let args = identity_update(second, &id(RIGHT, "gamma"), "identity, second");
                        ready();
                        report(
                            second
                                .update_entity(args, Actor::Cli, None, None)
                                .map(|o| o.warnings),
                        )
                    }
                },
                |engine| run_waiter(Waiter::MemRename, engine),
            );
            drop(engine);
            if renamed.is_err() {
                failures.push(format!("{context}: rename failed {renamed:?}"));
                continue;
            }
            let fresh = engine_after_rename(&ws, "lefty");
            let present = match what {
                "update" => section(&fresh, &id("lefty", "beta"), "identity").contains("second"),
                "create" => fresh
                    .get_entity(&id("lefty", "zeta"))
                    .is_some_and(|e| !e.stub),
                _ => section(&fresh, &id(RIGHT, "gamma"), "identity").contains("second"),
            };
            let ids: Vec<String> = fresh
                .store()
                .all_entities()
                .map(|e| format!("{}{}", e.id, if e.stub { "(stub)" } else { "" }))
                .collect();
            let line = format!(
                "{context}: rename={renamed:?} second={:?} both_locked={} present_in_renamed={present} fresh ids={ids:?}",
                window.second, window.both_locked
            );
            eprintln!("{line}");
            if !window.both_locked || (window.second.is_ok() && !present) {
                failures.push(line);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// (2) retype: the guard counts the edge a second process added from
/// the other mem before the retype ran in an engine loaded earlier.
#[test]
fn a_retype_rechecks_a_reference_a_second_process_added() {
    let mut failures = Vec::new();
    for pair in PAIRS {
        for link in [Link::Relation, Link::BodyLink] {
            for deferred in [false, true] {
                let context = format!("{pair:?} {link:?} deferred={deferred}");
                let ws = Workspace::new(pair);
                let mut engine = if deferred {
                    ws.engine_with_right_deferred()
                } else {
                    ws.engine()
                };
                let added = ws
                    .second_writer(move |second| add_link(second, link, &|| {}))
                    .join()
                    .unwrap();
                assert!(added.is_ok(), "{context}: {added:?}");
                let args = retype_args(&engine, &id(LEFT, "alpha"));
                let out = engine.retype_entity(args, Actor::Cli, None, None);
                let line = format!(
                    "{context}: retype={:?}",
                    out.as_ref()
                        .map(|o| o.edges_rechecked)
                        .map_err(|e| e.code().to_string())
                );
                eprintln!("{line}");
                match out {
                    Ok(o) if o.edges_rechecked == 1 => {}
                    _ => failures.push(line),
                }
                let (real, from) = links_to_alpha(&ws);
                if !real || from.len() != 1 {
                    failures.push(format!("{context}: real={real} from={from:?}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// (2) retype under the hook.
#[test]
fn a_reference_added_while_a_retype_is_held_at_its_commit() {
    let mut failures = Vec::new();
    for pair in PAIRS {
        for link in [Link::Relation, Link::BodyLink] {
            let context = format!("{pair:?} {link:?}");
            let ws = Workspace::new(pair);
            let (retyped, window, mut engine) = hold_at_commit(
                &ws,
                move |second, ready| add_link(second, link, ready),
                |engine| {
                    let args = retype_args(engine, &id(LEFT, "alpha"));
                    engine
                        .retype_entity(args, Actor::Cli, None, None)
                        .map(|o| o.edges_rechecked)
                        .map_err(|e| e.code().to_string())
                },
            );
            let fresh = ws.engine();
            let alpha = fresh.get_entity(&id(LEFT, "alpha")).cloned();
            let (real, from) = links_to_alpha(&ws);
            engine.reload_if_stale(None);
            let level = view(&engine) == view(&fresh);
            let line = format!(
                "{context}: retype={retyped:?} second={:?} both_locked={} type={:?} real={real} from={from:?} level={level}",
                window.second,
                window.both_locked,
                alpha.map(|a| a.entity_type)
            );
            eprintln!("{line}");
            if retyped.is_err()
                || !window.both_locked
                || !real
                || (window.second.is_ok() && from.len() != 1)
                || !level
            {
                failures.push(line);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// (2) delete: a referrer in a peer whose loading was deferred, added
/// by a second process after this engine booted.
#[test]
fn a_delete_sees_a_reference_in_a_deferred_peer() {
    let mut failures = Vec::new();
    for pair in PAIRS {
        for link in [Link::Relation, Link::BodyLink] {
            let context = format!("{pair:?} {link:?}");
            let ws = Workspace::new(pair);
            let mut engine = ws.engine_with_right_deferred();
            assert!(engine.mem_is_deferred(RIGHT));
            let added = ws
                .second_writer(move |second| add_link(second, link, &|| {}))
                .join()
                .unwrap();
            assert!(added.is_ok(), "{context}: {added:?}");
            let args = delete_args(&engine, &id(LEFT, "alpha"));
            let deleted = report(
                engine
                    .delete_entity(args, Actor::Cli, None, None)
                    .map(|o| o.warnings),
            );
            let (real, from) = links_to_alpha(&ws);
            let line = format!("{context}: delete={deleted:?} real={real} from={from:?}");
            eprintln!("{line}");
            if deleted != Err("HAS_INCOMING_REFS".to_string()) || !real || from.len() != 1 {
                failures.push(line);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The cycle guard with the single-item verbs on both sides.
#[test]
fn single_verbs_cannot_close_a_cycle_between_two_mems() {
    let gamma = id(RIGHT, "gamma");
    let mut failures = Vec::new();
    for first in ["update", "relate", "create"] {
        for second_kind in ["update", "relate", "create"] {
            for pair in PAIRS {
                let context = format!("{first}/{second_kind} {pair:?}");
                let ws = Workspace::new(pair);
                let source = if first == "create" {
                    id(LEFT, "delta")
                } else {
                    id(LEFT, "alpha")
                };
                // the second's source in `right`
                let back_from = if second_kind == "create" {
                    id(RIGHT, "epsilon")
                } else {
                    gamma.clone()
                };
                // first: source DEPENDS_ON back_from; second: back_from DEPENDS_ON source.
                // For a created back_from the first one's target does not exist yet,
                // so the first targets gamma and the created epsilon is put between:
                // epsilon DEPENDS_ON source, and gamma DEPENDS_ON epsilon is seeded? keep simple:
                if second_kind == "create" && first == "create" {
                    continue;
                }
                let first_target = gamma.clone();
                if second_kind == "create" {
                    // seed gamma DEPENDS_ON epsilon is impossible before epsilon exists;
                    // instead the second creates epsilon with DEPENDS_ON source and
                    // the first makes source DEPENDS_ON epsilon (a forward reference).
                }
                let first_target = if second_kind == "create" {
                    back_from.clone()
                } else {
                    first_target
                };
                let (one, window, engine) = hold_at_commit(
                    &ws,
                    {
                        let (source, back_from) = (source.clone(), back_from.clone());
                        move |second, ready| match second_kind {
                            "update" => {
                                let args = depends_update(second, &back_from, &source);
                                ready();
                                report(
                                    second
                                        .update_entity(args, Actor::Cli, None, None)
                                        .map(|o| o.warnings),
                                )
                            }
                            "relate" => {
                                let mut args = relate_args(&back_from, &source);
                                args.rel_type = "DEPENDS_ON".to_string();
                                ready();
                                report(
                                    second
                                        .relate_entity(args, Actor::Cli, None, None)
                                        .map(|o| o.warnings),
                                )
                            }
                            _ => {
                                let mut args =
                                    create_args(RIGHT, "Epsilon", std::slice::from_ref(&source));
                                args.relations[0].rel_type = "DEPENDS_ON".to_string();
                                ready();
                                report(
                                    second
                                        .create_entity(args, Actor::Cli, None, None)
                                        .map(|o| o.warnings),
                                )
                            }
                        }
                    },
                    |engine| match first {
                        "update" => {
                            let args = depends_update(engine, &source, &first_target);
                            report(
                                engine
                                    .update_entity(args, Actor::Cli, None, None)
                                    .map(|o| o.warnings),
                            )
                        }
                        "relate" => {
                            let mut args = relate_args(&source, &first_target);
                            args.rel_type = "DEPENDS_ON".to_string();
                            report(
                                engine
                                    .relate_entity(args, Actor::Cli, None, None)
                                    .map(|o| o.warnings),
                            )
                        }
                        _ => {
                            let mut args =
                                create_args(LEFT, "Delta", std::slice::from_ref(&first_target));
                            args.relations[0].rel_type = "DEPENDS_ON".to_string();
                            report(
                                engine
                                    .create_entity(args, Actor::Cli, None, None)
                                    .map(|o| o.warnings),
                            )
                        }
                    },
                );
                drop(engine);
                let fresh = ws.engine();
                let (out, back) = (
                    depends_edge(&fresh, &source, &first_target),
                    depends_edge(&fresh, &back_from, &source),
                );
                let line = format!(
                    "{context}: first={one:?} second={:?} both_locked={} out={out} back={back}",
                    window.second, window.both_locked
                );
                eprintln!("{line}");
                if (out && back) || (one.is_ok() && !out) || (window.second.is_ok() && !back) {
                    failures.push(line);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A failed reload of the peer under the lock: the sweep and the retype.
#[test]
fn a_failed_reload_refuses_the_sweep_and_the_retype() {
    let mut failures = Vec::new();
    for verb in ["sweep", "retype", "mem_rename"] {
        for pair in PAIRS {
            let context = format!("{verb} {pair:?}");
            let ws = Workspace::new(pair);
            let armed = Arc::new(AtomicBool::new(false));
            let fired = Arc::new(Mutex::new(Vec::<String>::new()));
            let tap: Tap = {
                let (armed, fired) = (Arc::clone(&armed), Arc::clone(&fired));
                Arc::new(move |mem: &str, method: &str| {
                    if mem == RIGHT
                        && matches!(
                            method,
                            "read_all_entities" | "list_entities" | "read_entity"
                        )
                        && armed.swap(false, Ordering::SeqCst)
                    {
                        fired.lock().unwrap().push(method.to_string());
                        return true;
                    }
                    false
                })
            };
            let mut engine = ws.tapped_engine(tap);
            let alpha = id(LEFT, "alpha");
            let added = ws
                .second_writer(move |second| add_link(second, Link::BodyLink, &|| {}))
                .join()
                .unwrap();
            assert!(added.is_ok(), "{context}: {added:?}");
            armed.store(true, Ordering::SeqCst);
            let outcome: Result<String, String> = match verb {
                "sweep" => engine
                    .rewrite_mem_references(LEFT, "lefty", None)
                    .map(|o| format!("{:?}", o.rewritten_mems))
                    .map_err(|e| e.code().to_string()),
                "retype" => {
                    let args = retype_args(&engine, &alpha);
                    engine
                        .retype_entity(args, Actor::Cli, None, None)
                        .map(|o| format!("edges_rechecked={}", o.edges_rechecked))
                        .map_err(|e| e.code().to_string())
                }
                _ => run_waiter(Waiter::MemRename, &mut engine).map(|w| format!("{w:?}")),
            };
            armed.store(false, Ordering::SeqCst);
            let gamma_raw = stored_purpose(&ws);
            let line = format!(
                "{context}: fault fired on {:?}; verb returned {outcome:?}; right/gamma.md has old link={} new link={}",
                fired.lock().unwrap(),
                gamma_raw.contains("[[left--alpha]]"),
                gamma_raw.contains("[[lefty--alpha]]"),
            );
            eprintln!("{line}");
            let bad = match verb {
                "retype" => outcome.as_ref().is_ok_and(|o| o != "edges_rechecked=1"),
                _ => outcome.is_ok() && gamma_raw.contains("[[left--alpha]]"),
            };
            if fired.lock().unwrap().is_empty() || bad {
                failures.push(line);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

fn stored_purpose(ws: &Workspace) -> String {
    let mount = ws.mounts.iter().find(|m| m.mem == RIGHT).unwrap();
    let bytes = instantiate_full_backend(mount)
        .unwrap()
        .read_entity(Path::new("gamma.md"))
        .unwrap()
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).to_string()
}

/// The same with engines booted from a workspace root, so the roster
/// reconcile is live in both processes.
#[test]
fn workspace_rooted_mem_rename_and_a_write_into_the_renamed_mem() {
    use memstead_git_branch::workspace_store::engine_from_workspace_root;
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    for what in ["update", "create"] {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join(".memstead")).unwrap();
        std::fs::write(
            root.join(".memstead/workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n\n[cross_mem_links]\nleft = \"*\"\nright = \"*\"\n",
        )
        .unwrap();
        init_real_mem_repo(&root, &[(LEFT, SCHEMA), (RIGHT, SCHEMA)]);
        let mut engine = engine_from_workspace_root(&root).expect("engine boots");
        for (mem, title) in [(LEFT, "Alpha"), (LEFT, "Beta"), (RIGHT, "Gamma")] {
            engine
                .create_entity(create_args(mem, title, &[]), Actor::Cli, None, None)
                .unwrap_or_else(|e| panic!("seed {mem}--{title}: {e:?}"));
        }
        for (target, text) in [
            (id(RIGHT, "gamma"), "Gamma purpose, see [[left--alpha]]"),
            (id(LEFT, "beta"), "Beta purpose, see [[left--alpha]]"),
        ] {
            let args = update_args(&engine, &target, text);
            engine
                .update_entity(args, Actor::Cli, None, None)
                .unwrap_or_else(|e| panic!("link {target}: {e:?}"));
        }

        // The hook: `right` again, tapped; at its first landing call the
        // second process is started and the rename goes on when the
        // second is about to send its write.
        let lock_paths: Vec<PathBuf> = [LEFT, RIGHT]
            .iter()
            .map(|mem| {
                instantiate_full_backend(engine.mount(mem).unwrap())
                    .unwrap()
                    .write_lock_path()
                    .unwrap()
            })
            .collect();
        type Second = JoinHandle<(Report, Vec<String>)>;
        let stage: Arc<Mutex<Option<(bool, Second)>>> = Arc::new(Mutex::new(None));
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
                let both_locked = lock_paths.iter().all(|l| !lock_is_free(l));
                let (ready, is_ready) = std::sync::mpsc::channel::<()>();
                let root = root.clone();
                let handle = std::thread::spawn(move || {
                    let mut second = engine_from_workspace_root(&root).expect("second boots");
                    let out = if what == "update" {
                        let args = identity_update(&second, &id(LEFT, "beta"), "identity, second");
                        let _ = ready.send(());
                        report(
                            second
                                .update_entity(args, Actor::Cli, None, None)
                                .map(|o| o.warnings),
                        )
                    } else {
                        let _ = ready.send(());
                        report(
                            second
                                .create_entity(
                                    create_args(LEFT, "Zeta", &[]),
                                    Actor::Cli,
                                    None,
                                    None,
                                )
                                .map(|o| o.warnings),
                        )
                    };
                    let mems: Vec<String> = second.mounts().iter().map(|m| m.mem.clone()).collect();
                    (out, mems)
                });
                if both_locked {
                    is_ready.recv().expect("the second reaches its write");
                }
                *stage = Some((both_locked, handle));
                false
            })
        };
        let mount = engine.mount(RIGHT).unwrap().clone();
        engine.unregister_writable_mem(RIGHT).unwrap();
        engine
            .register_writable_mem(
                mount.clone(),
                Box::new(Tapped {
                    inner: instantiate_full_backend(&mount).unwrap(),
                    mem: RIGHT,
                    tap,
                }) as Box<dyn MemBackend>,
                memstead_base::MemOrigin::RuntimeCreated {
                    at: std::time::SystemTime::now(),
                    by_tool: "test",
                },
            )
            .unwrap();

        let renamed = run_waiter(Waiter::MemRename, &mut engine);
        let (both_locked, handle) = stage.lock().unwrap().take().expect("the rename committed");
        let (second, second_mems) = handle.join().unwrap();
        drop(engine);

        let fresh = engine_from_workspace_root(&root).expect("fresh boots");
        let mems: Vec<String> = fresh.mounts().iter().map(|m| m.mem.clone()).collect();
        let ids: Vec<String> = fresh
            .store()
            .all_entities()
            .map(|e| format!("{}{}", e.id, if e.stub { "(stub)" } else { "" }))
            .collect();
        let present = if what == "update" {
            fresh
                .store()
                .all_entities()
                .any(|e| e.sections.values().any(|s| s.contains("identity, second")))
        } else {
            fresh
                .store()
                .all_entities()
                .any(|e| e.title == "Zeta" && !e.stub)
        };
        let line = format!(
            "workspace {what}: rename={renamed:?} second={second:?} (second's mems after: {second_mems:?}) both_locked={both_locked} | fresh engine mems={mems:?} ids={ids:?} second's write served={present}"
        );
        eprintln!("{line}");
        if second.is_ok() && !present {
            failures.push(line.clone());
        }
        lines.push(line);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Unconditional writes behind a batch, and an unconditional update of
/// the renamed entity itself behind a rename.
#[test]
fn unconditional_writes_behind_batches_and_the_renamed_entity() {
    let mut failures = Vec::new();
    for pair in PAIRS {
        for waiter in [Waiter::BatchUpdate, Waiter::BatchRelate, Waiter::Rename] {
            let context = format!("{pair:?} {waiter:?}");
            let ws = Workspace::new(pair);
            link_gamma_to_alpha(&ws);
            // Rename here renames left--beta; the second updates beta itself.
            let tid = if matches!(waiter, Waiter::Rename) {
                id(LEFT, "beta")
            } else {
                id(RIGHT, "gamma")
            };
            let (done, window, mut engine) = hold_at_commit(
                &ws,
                {
                    let tid = tid.clone();
                    move |second, ready| {
                        let args = identity_update(second, &tid, "identity, second");
                        ready();
                        report(
                            second
                                .update_entity(args, Actor::Cli, None, None)
                                .map(|o| o.warnings),
                        )
                    }
                },
                |engine| run_waiter(waiter, engine),
            );
            let fresh = ws.engine();
            let served = fresh
                .store()
                .all_entities()
                .filter(|e| e.sections.values().any(|s| s.contains("identity, second")))
                .map(|e| e.id.to_string())
                .collect::<Vec<_>>();
            let ids: Vec<String> = fresh
                .store()
                .all_entities()
                .map(|e| format!("{}{}", e.id, if e.stub { "(stub)" } else { "" }))
                .collect();
            engine.reload_if_stale(None);
            let level = view(&engine) == view(&fresh);
            let line = format!(
                "{context}: op={done:?} second={:?} both_locked={} served_in={served:?} ids={ids:?} level={level}",
                window.second, window.both_locked
            );
            eprintln!("{line}");
            if done.is_err()
                || !window.both_locked
                || (window.second.is_ok() && served.is_empty())
                || !level
            {
                failures.push(line);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
