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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use indexmap::IndexMap;
use memstead_base::backend::{BackendError, CompletedCommit, EntityRead, MemBackend};
use memstead_base::storage::CommitId;
use memstead_base::vcs::{Actor, CommitContext};
use memstead_base::{
    CreateEntityArgs, DeleteEntityArgs, Engine, EngineError, EntityId, Mount, MountCapability,
    MountLifecycle, MountStorage, Provenance, RelateEntityArgs, RenameEntityArgs, RetypeEntityArgs,
    UpdateEntityArgs,
};
use memstead_git_branch::storage::instantiate_full_backend;
use memstead_git_branch::test_support::init_real_mem_repo;
use tempfile::TempDir;

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
// The tap: a backend wrapper that reports every call before it runs
// ---------------------------------------------------------------------

/// Called with the mem and the storage method's name before the method
/// runs, on the thread that runs it. `true` fails the call in place of
/// running it: a storage error at that step.
type Tap = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

struct Tapped {
    inner: Box<dyn MemBackend>,
    mem: &'static str,
    tap: Tap,
}

impl Tapped {
    fn on(&self, method: &str) -> Result<(), BackendError> {
        if (self.tap)(self.mem, method) {
            Err(BackendError::Other(format!(
                "injected failure of {method} on {}",
                self.mem
            )))
        } else {
            Ok(())
        }
    }
}

/// The storage methods that land a write.
fn lands(method: &str) -> bool {
    matches!(
        method,
        "commit"
            | "commit_with_expected_parent"
            | "append_provenance"
            | "write_mem_config"
            | "write_mem_config_cas"
            | "record_pipeline_edit"
            | "delete_artifacts"
    )
}

impl MemBackend for Tapped {
    fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
        self.on("list_entities")?;
        self.inner.list_entities()
    }
    fn read_entity(&self, rel_path: &Path) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_entity")?;
        self.inner.read_entity(rel_path)
    }
    fn entity_exists(&self, rel_path: &Path) -> Result<bool, BackendError> {
        self.on("entity_exists")?;
        self.inner.entity_exists(rel_path)
    }
    fn read_all_entities(&self) -> Result<Vec<EntityRead>, BackendError> {
        self.on("read_all_entities")?;
        self.inner.read_all_entities()
    }
    fn storage_present(&self) -> Result<bool, BackendError> {
        self.on("storage_present")?;
        self.inner.storage_present()
    }
    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), BackendError> {
        self.on("write_entity")?;
        self.inner.write_entity(rel_path, content)
    }
    fn delete_entity(&self, rel_path: &Path) -> Result<(), BackendError> {
        self.on("delete_entity")?;
        self.inner.delete_entity(rel_path)
    }
    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), BackendError> {
        self.on("move_entity")?;
        self.inner.move_entity(from, to)
    }
    fn discard_pending(&self) -> Result<(), BackendError> {
        self.on("discard_pending")?;
        self.inner.discard_pending()
    }
    fn commit(&self, message: &str, ctx: &CommitContext<'_>) -> Result<CommitId, BackendError> {
        self.on("commit")?;
        self.inner.commit(message, ctx)
    }
    fn commit_with_expected_parent(
        &self,
        message: &str,
        ctx: &CommitContext<'_>,
        expected_parent: Option<&str>,
    ) -> Result<CommitId, BackendError> {
        self.on("commit_with_expected_parent")?;
        self.inner
            .commit_with_expected_parent(message, ctx, expected_parent)
    }
    fn stage_provenance(&self, record: &Provenance) -> Result<(), BackendError> {
        self.on("stage_provenance")?;
        self.inner.stage_provenance(record)
    }
    fn append_provenance(&self, record: &Provenance) -> Result<(), BackendError> {
        self.on("append_provenance")?;
        self.inner.append_provenance(record)
    }
    fn complete_interrupted_commit(&self) -> Result<Option<CompletedCommit>, BackendError> {
        self.on("complete_interrupted_commit")?;
        self.inner.complete_interrupted_commit()
    }
    fn read_provenance(&self, cursor: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
        self.on("read_provenance")?;
        self.inner.read_provenance(cursor)
    }
    fn current_head(&self) -> Result<Option<String>, BackendError> {
        self.on("current_head")?;
        self.inner.current_head()
    }
    fn write_lock_path(&self) -> Option<PathBuf> {
        let _ = self.on("write_lock_path");
        self.inner.write_lock_path()
    }
    fn read_mem_config(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_mem_config")?;
        self.inner.read_mem_config()
    }
    fn read_archive_provenance(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_archive_provenance")?;
        self.inner.read_archive_provenance()
    }
    fn read_archive_checks(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_archive_checks")?;
        self.inner.read_archive_checks()
    }
    fn read_proposal_record(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_proposal_record")?;
        self.inner.read_proposal_record()
    }
    fn write_mem_config(&self, bytes: &[u8], ctx: &CommitContext<'_>) -> Result<(), BackendError> {
        self.on("write_mem_config")?;
        self.inner.write_mem_config(bytes, ctx)
    }
    fn write_mem_config_cas(
        &self,
        expected: Option<&[u8]>,
        bytes: &[u8],
        ctx: &CommitContext<'_>,
    ) -> Result<bool, BackendError> {
        self.on("write_mem_config_cas")?;
        self.inner.write_mem_config_cas(expected, bytes, ctx)
    }
    fn record_pipeline_edit(
        &self,
        kind: &str,
        edits: &[(String, Option<Vec<u8>>)],
        ctx: &CommitContext<'_>,
        verb: &str,
    ) -> Result<(), BackendError> {
        self.on("record_pipeline_edit")?;
        self.inner.record_pipeline_edit(kind, edits, ctx, verb)
    }
    fn read_anchors_sidecar(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.on("read_anchors_sidecar")?;
        self.inner.read_anchors_sidecar()
    }
    fn write_anchors_sidecar(&self, bytes: &[u8]) -> Result<(), BackendError> {
        self.on("write_anchors_sidecar")?;
        self.inner.write_anchors_sidecar(bytes)
    }
    fn delete_artifacts(&self, ctx: &CommitContext<'_>) -> Result<(), BackendError> {
        self.on("delete_artifacts")?;
        self.inner.delete_artifacts(ctx)
    }
}

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

/// Whether nobody holds the lock at `path`, asked through an open file
/// of this probe's own: it conflicts with every holder, a holder on the
/// calling thread included.
fn lock_is_free(path: &Path) -> bool {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    else {
        // No lock file yet: nobody ever took the lock.
        return true;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            true
        }
        Err(_) => false,
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
    enum Stage<T> {
        Armed(Box<dyn FnOnce() -> JoinHandle<T> + Send>),
        Finished { both_locked: bool, second: T },
        Running(JoinHandle<T>),
        Taken,
    }
    let locks = [ws.lock_path(LEFT), ws.lock_path(RIGHT)];
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
    for pair in PAIRS {
        for &waiter in waiters {
            let context = format!("{pair:?} {waiter:?}");
            let ws = Workspace::new(pair);
            let lock_path = ws.lock_path(RIGHT);
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
                    if mem == RIGHT && method == "write_lock_path" {
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
