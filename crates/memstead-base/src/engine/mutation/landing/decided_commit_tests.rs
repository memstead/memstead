//! A folder commit that is decided (its record is in storage) and
//! whose apply step fails, as the second mem of an operation over two
//! mems after the first mem's commit landed. The faults strike the
//! first apply step after the commit point and then the first step of
//! each completion the engine attempts.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use indexmap::IndexMap;
use tempfile::TempDir;

use crate::backend::MemBackend;
use crate::engine::test_helpers::{cli_actor, empty_create_args, folder_mount};
use crate::engine::{Engine, EngineError, RenameEntityArgs, UpdateEntityArgs};
use crate::entity::EntityId;
use crate::storage::FilesystemBackend;
use crate::storage::filesystem::fault_io::FaultIo;
use crate::storage::filesystem::io::{DirEntry, FolderIo};

const ONE: &str = "one";
const TWO: &str = "two";

/// Real IO, recorded by the inner `FaultIo`; the calls whose index is
/// in `fail` return an error instead of running. Exactly those.
#[derive(Clone)]
struct TwoFaults {
    inner: FaultIo,
    count: Arc<AtomicUsize>,
    fail: Arc<std::sync::Mutex<Vec<usize>>>,
    struck: Arc<AtomicUsize>,
}

impl TwoFaults {
    fn new() -> Self {
        Self {
            inner: FaultIo::new(),
            count: Arc::new(AtomicUsize::new(0)),
            fail: Arc::new(std::sync::Mutex::new(Vec::new())),
            struck: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn reset(&self) {
        self.inner.reset();
        self.count.store(0, Ordering::SeqCst);
        self.fail.lock().unwrap().clear();
        self.struck.store(0, Ordering::SeqCst);
    }
    fn arm(&self, at: &[usize]) {
        *self.fail.lock().unwrap() = at.to_vec();
    }
    fn disarm(&self) {
        self.fail.lock().unwrap().clear();
    }
    fn gate(&self) -> io::Result<()> {
        let index = self.count.fetch_add(1, Ordering::SeqCst);
        if self.fail.lock().unwrap().contains(&index) {
            self.struck.fetch_add(1, Ordering::SeqCst);
            return Err(io::Error::other(format!(
                "grader: injected error at step {index}"
            )));
        }
        Ok(())
    }
}

impl FolderIo for TwoFaults {
    fn exists(&self, path: &Path) -> io::Result<bool> {
        self.gate()?;
        self.inner.exists(path)
    }
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        self.gate()?;
        self.inner.read(path)
    }
    fn read_tail(&self, path: &Path, max: u64) -> io::Result<Option<Vec<u8>>> {
        self.gate()?;
        self.inner.read_tail(path, max)
    }
    fn list_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.gate()?;
        self.inner.list_dir(path)
    }
    fn create_dir(&self, path: &Path) -> io::Result<()> {
        self.gate()?;
        self.inner.create_dir(path)
    }
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.gate()?;
        self.inner.write(path, bytes)
    }
    fn append(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.gate()?;
        self.inner.append(path, bytes)
    }
    fn sync_file(&self, path: &Path) -> io::Result<()> {
        self.gate()?;
        self.inner.sync_file(path)
    }
    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        self.gate()?;
        self.inner.sync_dir(path)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.gate()?;
        self.inner.rename(from, to)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.gate()?;
        self.inner.remove_file(path)
    }
    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        self.gate()?;
        self.inner.remove_dir(path)
    }
}

fn id(mem: &str, slug: &str) -> EntityId {
    EntityId::new(mem, slug)
}

fn settings(engine: &mut Engine) {
    let mut settings = crate::WorkspaceSettings::default();
    for mem in [ONE, TWO] {
        settings.cross_mem_links.insert(
            mem.to_string(),
            memstead_schema::workspace_config::CrossLinkValue::Wildcard,
        );
    }
    engine.set_settings(settings);
}

fn dirs(root: &Path) -> (PathBuf, PathBuf) {
    (root.join(ONE), root.join(TWO))
}

/// A fresh engine over both mems, plain backends.
fn fresh(root: &Path) -> Engine {
    let (one, two) = dirs(root);
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount(ONE, one.clone()),
            Box::new(FilesystemBackend::new(one)) as Box<dyn MemBackend>,
        ),
        (
            folder_mount(TWO, two.clone()),
            Box::new(FilesystemBackend::new(two)) as Box<dyn MemBackend>,
        ),
    ])
    .expect("engine boots");
    settings(&mut engine);
    engine
}

/// The same, with `faulted`'s IO under `io`.
fn faulted(root: &Path, faulted_mem: &str, io: &TwoFaults) -> Engine {
    let (one, two) = dirs(root);
    let backend = |mem: &str, dir: PathBuf| -> Box<dyn MemBackend> {
        if mem == faulted_mem {
            Box::new(FilesystemBackend::with_io(dir, io.clone()))
        } else {
            Box::new(FilesystemBackend::new(dir))
        }
    };
    let mut engine = Engine::from_mounts(vec![
        (folder_mount(ONE, one.clone()), backend(ONE, one)),
        (folder_mount(TWO, two.clone()), backend(TWO, two)),
    ])
    .expect("engine boots");
    settings(&mut engine);
    engine
}

fn update_args(
    engine: &Engine,
    target: &EntityId,
    hash: Option<String>,
    text: &str,
) -> UpdateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("purpose".to_string(), text.to_string());
    UpdateEntityArgs {
        id: target.clone(),
        expected_hash: hash.or_else(|| engine.get_entity(target).map(|e| e.content_hash.clone())),
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

/// one: Alpha, Beta (Beta links [[one--alpha]]); two: Gamma (links
/// [[one--alpha]]).
fn seeded() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let (one, two) = dirs(tmp.path());
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    let mut engine = fresh(tmp.path());
    let (actor, client) = cli_actor();
    for (mem, title) in [(ONE, "Alpha"), (ONE, "Beta"), (TWO, "Gamma")] {
        engine
            .create_entity(empty_create_args(mem, title), actor, Some(&client), None)
            .unwrap();
    }
    for (mem, slug) in [(ONE, "beta"), (TWO, "gamma")] {
        let args = update_args(&engine, &id(mem, slug), None, "see [[one--alpha]]");
        engine
            .update_entity(args, actor, Some(&client), None)
            .unwrap();
    }
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
        out.insert(
            entity.id.to_string(),
            format!(
                "stub={} hash={} out={:?}",
                entity.stub, entity.content_hash, outgoing
            ),
        );
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    BatchCreate,
    BatchUpdate,
    Rename,
    Sweep,
}

/// The mem whose commit comes second (and is faulted).
fn second_mem(op: Op) -> &'static str {
    match op {
        Op::BatchCreate | Op::BatchUpdate => TWO,
        // Peers first, the renamed entity's / mem's own mem last.
        Op::Rename | Op::Sweep => ONE,
    }
}

fn run(op: Op, engine: &mut Engine) -> Result<String, EngineError> {
    let (actor, client) = cli_actor();
    match op {
        Op::BatchCreate => engine
            .batch_create(
                vec![
                    (empty_create_args(ONE, "Delta"), None),
                    (empty_create_args(TWO, "Epsilon"), None),
                ],
                actor,
                Some(&client),
                false,
            )
            .map(|r| format!("applied={}", r.applied)),
        Op::BatchUpdate => {
            let updates = vec![
                (
                    update_args(engine, &id(ONE, "alpha"), None, "batch text"),
                    None,
                ),
                (
                    update_args(
                        engine,
                        &id(TWO, "gamma"),
                        None,
                        "batch text, see [[one--alpha]]",
                    ),
                    None,
                ),
            ];
            engine
                .batch_update(updates, actor, Some(&client), false)
                .map(|r| format!("applied={}", r.applied))
        }
        Op::Rename => {
            let args = RenameEntityArgs {
                id: id(ONE, "alpha"),
                expected_hash: engine
                    .get_entity(&id(ONE, "alpha"))
                    .map(|e| e.content_hash.clone()),
                new_title: "Alpha Renamed".to_string(),
            };
            engine
                .rename_entity(args, actor, Some(&client), None)
                .map(|o| o.new_id.to_string())
        }
        Op::Sweep => engine
            .rewrite_mem_references(ONE, "uno", None)
            .map(|o| format!("{:?}", o.rewritten_mems)),
    }
}

/// Run `op` with `faults` failures in a row from the first apply step
/// after the second mem's commit point. Returns what the call came to,
/// the engine, its storage, and the ids an undisturbed run ends with.
fn scenario(op: Op, faults: usize) -> (Result<String, EngineError>, Engine, TempDir, Vec<String>) {
    let template = seeded();
    let mem2 = second_mem(op);

    // Probe: where the commit point of the second mem's commit falls.
    let probe_dir = TempDir::new().unwrap();
    copy_dir(template.path(), probe_dir.path());
    let io = TwoFaults::new();
    let mut probe = faulted(probe_dir.path(), mem2, &io);
    io.reset();
    run(op, &mut probe).expect("the undisturbed call lands");
    let cp = io.inner.commit_point().expect("the second mem commits");
    assert!(
        io.inner.op_count() > cp + 1,
        "{op:?}: steps after the commit point"
    );
    let undisturbed: Vec<String> = view(&fresh(probe_dir.path())).into_keys().collect();
    drop(probe);

    let dir = TempDir::new().unwrap();
    copy_dir(template.path(), dir.path());
    let io = TwoFaults::new();
    let mut engine = faulted(dir.path(), mem2, &io);
    io.reset();
    let at: Vec<usize> = (1..=faults).map(|n| cp + n).collect();
    io.arm(&at);
    let outcome = run(op, &mut engine);
    assert_eq!(
        io.struck.load(Ordering::SeqCst),
        faults,
        "{op:?}: every injected fault struck"
    );
    io.disarm();
    (outcome, engine, dir, undisturbed)
}

const OPS: [Op; 4] = [Op::BatchCreate, Op::BatchUpdate, Op::Rename, Op::Sweep];

/// The apply step fails and so does the completion the commit call
/// runs at once. The commit is in storage all the same, so an
/// operation over several mems tries the completion once more before
/// it stops: the operation lands whole, and the engine serves what a
/// fresh load serves.
#[test]
fn a_decided_commit_whose_first_completion_fails_still_lands_the_operation() {
    for op in OPS {
        let (outcome, engine, dir, undisturbed) = scenario(op, 2);
        outcome.unwrap_or_else(|e| panic!("{op:?}: {e:?}"));
        let fresh_view = view(&fresh(dir.path()));
        assert_eq!(view(&engine), fresh_view, "{op:?}");
        assert_eq!(
            fresh_view.into_keys().collect::<Vec<_>>(),
            undisturbed,
            "{op:?}: the operation is complete in storage"
        );
    }
}

/// The completion fails a second time as well. The operation stops and
/// reports through the partial-failure envelope, and the report does
/// not call the decided commit not landed: its mem is named as landed,
/// with the failure as its error, and the reload that builds the
/// report completes the commit, so the hashes are the entities' hashes
/// now and nothing is left to send again.
#[test]
fn a_decided_commit_that_cannot_be_completed_at_once_is_reported_as_landed() {
    for op in OPS {
        let (outcome, engine, dir, undisturbed) = scenario(op, 3);
        let err = outcome.expect_err("three faults stop the operation");
        let EngineError::RenamePartialFailure {
            committed_mems,
            failed_mem,
            mems,
            ..
        } = &err
        else {
            panic!("{op:?}: expected the partial-failure envelope, got {err:?}");
        };
        assert_eq!(failed_mem, second_mem(op), "{op:?}");
        assert_eq!(
            committed_mems.len(),
            2,
            "{op:?}: both commits are in storage"
        );
        let fresh_engine = fresh(dir.path());
        for reported in mems {
            assert!(reported.landed, "{op:?}: {reported:?}");
            assert_eq!(
                reported.error.is_some(),
                &reported.mem == failed_mem,
                "{op:?}: {reported:?}"
            );
            for item in &reported.items {
                assert!(item.landed, "{op:?}: {item:?}");
                let (mem, slug) = item.id.split_once("--").unwrap();
                let now = fresh_engine
                    .get_entity(&id(mem, slug))
                    .filter(|e| !e.stub)
                    .map(|e| e.content_hash.clone());
                assert!(
                    now.is_some(),
                    "{op:?}: {item:?} names an entity that exists"
                );
                assert_eq!(item.current_hash, now, "{op:?}: {item:?}");
            }
        }
        let fresh_view = view(&fresh_engine);
        assert_eq!(view(&engine), fresh_view, "{op:?}");
        assert_eq!(
            fresh_view.into_keys().collect::<Vec<_>>(),
            undisturbed,
            "{op:?}: the operation is complete in storage"
        );
    }
}
