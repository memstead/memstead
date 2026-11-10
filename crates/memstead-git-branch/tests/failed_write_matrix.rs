//! A failed write leaves nothing behind, on either writable backend.
//!
//! The matrix is generated, not listed: every mutation verb, times
//! every method of the storage trait, times every call of that method
//! the verb makes, on the folder backend and on the git-branch backend.
//! A wrapping backend fails exactly one call per run.
//!
//! - A failure **before** the commit landed must return a typed error,
//!   leave storage byte for byte as it was, leave nothing staged, leave
//!   the running engine's view equal to a freshly loaded engine's, and
//!   let the same call succeed when sent again with the result an
//!   undisturbed call produces.
//! - A failure **after** the commit landed must not be reported as a
//!   failed write: the call succeeds, the write is in storage, and the
//!   running engine's view equals a freshly loaded engine's.
//!
//! Both lists are checked against the engine's source. A method added
//! to the storage trait fails `the_method_list_is_the_trait` until it
//! joins the matrix and is classified as reading or writing, and a
//! function of the engine that calls a writing method fails
//! `the_verb_list_is_the_engines_writers` until it is part of a matrix
//! verb or is named with the reason it is not.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

const MEM: &str = "notes";
/// A second mem beside the one under test, for the two verbs that need
/// one (see [`Peer`]).
const PEER: &str = "peer";

/// How a verb wants the second mem mounted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Peer {
    /// Not at all: the verb is a mutation of one mem, and a second mem
    /// would bring in what an operation does to mems it does not write.
    Absent,
    /// Lazily and never loaded: a relation into it is verified against
    /// its storage.
    Lazy,
    /// Loaded and read-only: its entity links to a target that does not
    /// exist in the mem under test, which keeps a stub alive there that
    /// only a read-only mem refers to (the one stub a delete accepts).
    ReadOnly,
}

// ---------------------------------------------------------------------
// The storage trait's methods
// ---------------------------------------------------------------------

macro_rules! methods {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        enum Method { $($variant),+ }
        impl Method {
            const ALL: &'static [Method] = &[$(Method::$variant),+];
            fn name(self) -> &'static str {
                match self { $(Method::$variant => $name),+ }
            }
        }
    };
}

methods! {
    ListEntities => "list_entities",
    ReadEntity => "read_entity",
    EntityExists => "entity_exists",
    ReadAllEntities => "read_all_entities",
    StoragePresent => "storage_present",
    WriteEntity => "write_entity",
    DeleteEntity => "delete_entity",
    MoveEntity => "move_entity",
    DiscardPending => "discard_pending",
    Commit => "commit",
    CommitWithExpectedParent => "commit_with_expected_parent",
    StageProvenance => "stage_provenance",
    AppendProvenance => "append_provenance",
    CompleteInterruptedCommit => "complete_interrupted_commit",
    ReadProvenance => "read_provenance",
    CurrentHead => "current_head",
    WriteLockPath => "write_lock_path",
    ReadMemConfig => "read_mem_config",
    ReadArchiveProvenance => "read_archive_provenance",
    ReadArchiveChecks => "read_archive_checks",
    ReadProposalRecord => "read_proposal_record",
    WriteMemConfig => "write_mem_config",
    WriteMemConfigCas => "write_mem_config_cas",
    RecordPipelineEdit => "record_pipeline_edit",
    ReadAnchorsSidecar => "read_anchors_sidecar",
    WriteAnchorsSidecar => "write_anchors_sidecar",
    DeleteArtifacts => "delete_artifacts",
}

/// The matrix's method list is the trait's method list. Read from the
/// trait's source, because a trait method with a default body would
/// otherwise join the trait without joining the wrapper below, and its
/// calls would silently bypass the fault.
#[test]
fn the_method_list_is_the_trait() {
    let source = include_str!("../../memstead-base/src/backend.rs");
    let start = source
        .find("pub trait MemBackend")
        .expect("the trait is declared in backend.rs");
    let body = &source[start..];
    let end = body.find("\n}\n").expect("the trait block closes");
    let in_trait: BTreeSet<&str> = body[..end]
        .lines()
        // A method is declared at the trait's own indentation, with
        // or without `unsafe`, `async` or `const` in front.
        .filter_map(|line| line.strip_prefix("    "))
        .filter_map(|mut rest| {
            while let Some(after) = ["async ", "const ", "unsafe "]
                .iter()
                .find_map(|q| rest.strip_prefix(q))
            {
                rest = after;
            }
            rest.strip_prefix("fn ")
        })
        .filter_map(|rest| rest.split(['(', '<']).next())
        .collect();
    let in_matrix: BTreeSet<&str> = Method::ALL.iter().map(|m| m.name()).collect();
    assert_eq!(
        in_trait, in_matrix,
        "the storage trait and the fault matrix name different methods"
    );
}

// ---------------------------------------------------------------------
// The wrapping backend
// ---------------------------------------------------------------------

#[derive(Default)]
struct Faults {
    inner: Mutex<FaultState>,
}

#[derive(Default)]
struct FaultState {
    /// Every call since the last reset, with whether a commit had
    /// already landed when it was made.
    log: Vec<(Method, bool)>,
    /// Fail the n-th call (1-based) of this method since the reset.
    armed: Option<(Method, usize)>,
    /// Fail every call of this method as well, until the next reset.
    always: Option<Method>,
    fired: bool,
    /// The commit had already landed when the fault fired.
    fired_after_landing: bool,
    landed: bool,
    /// Paths a write was staged for since the reset.
    staged_paths: BTreeSet<PathBuf>,
}

impl Faults {
    fn reset(&self) {
        *self.inner.lock().unwrap() = FaultState::default();
    }

    fn arm(&self, method: Method, nth: usize) {
        self.reset();
        self.inner.lock().unwrap().armed = Some((method, nth));
    }

    /// On top of an armed fault: every call of `method` fails too.
    fn always_failing(&self, method: Method) {
        self.inner.lock().unwrap().always = Some(method);
    }

    fn fired(&self) -> bool {
        self.inner.lock().unwrap().fired
    }

    fn fired_after_landing(&self) -> bool {
        self.inner.lock().unwrap().fired_after_landing
    }

    fn log(&self) -> Vec<(Method, bool)> {
        self.inner.lock().unwrap().log.clone()
    }

    fn staged_paths(&self) -> BTreeSet<PathBuf> {
        self.inner.lock().unwrap().staged_paths.clone()
    }

    /// Record the call; `true` when this is the call to fail.
    fn enter(&self, method: Method) -> bool {
        let mut state = self.inner.lock().unwrap();
        let landed = state.landed;
        state.log.push((method, landed));
        let nth = state.log.iter().filter(|(m, _)| *m == method).count();
        if state.armed == Some((method, nth)) && !state.fired {
            state.fired = true;
            state.fired_after_landing = landed;
            return true;
        }
        if state.always == Some(method) {
            return true;
        }
        false
    }

    fn staged(&self, path: &Path) {
        self.inner
            .lock()
            .unwrap()
            .staged_paths
            .insert(path.to_path_buf());
    }

    fn landed(&self) {
        self.inner.lock().unwrap().landed = true;
    }
}

fn injected(method: Method) -> BackendError {
    BackendError::Io(std::io::Error::other(format!(
        "injected failure in {}",
        method.name()
    )))
}

struct FaultBackend {
    inner: Arc<dyn MemBackend>,
    faults: Arc<Faults>,
}

impl FaultBackend {
    fn gate(&self, method: Method) -> Result<(), BackendError> {
        if self.faults.enter(method) {
            Err(injected(method))
        } else {
            Ok(())
        }
    }

    fn note_commit<T>(&self, result: &Result<T, BackendError>) {
        if matches!(result, Ok(_) | Err(BackendError::FollowUpFailed { .. })) {
            self.faults.landed();
        }
    }
}

impl MemBackend for FaultBackend {
    fn list_entities(&self) -> Result<Vec<PathBuf>, BackendError> {
        self.gate(Method::ListEntities)?;
        self.inner.list_entities()
    }
    fn read_entity(&self, rel_path: &Path) -> Result<Option<Vec<u8>>, BackendError> {
        self.gate(Method::ReadEntity)?;
        self.inner.read_entity(rel_path)
    }
    fn entity_exists(&self, rel_path: &Path) -> Result<bool, BackendError> {
        self.gate(Method::EntityExists)?;
        self.inner.entity_exists(rel_path)
    }
    fn read_all_entities(&self) -> Result<Vec<EntityRead>, BackendError> {
        self.gate(Method::ReadAllEntities)?;
        self.inner.read_all_entities()
    }
    fn storage_present(&self) -> Result<bool, BackendError> {
        self.gate(Method::StoragePresent)?;
        self.inner.storage_present()
    }
    fn write_entity(&self, rel_path: &Path, content: &[u8]) -> Result<(), BackendError> {
        self.gate(Method::WriteEntity)?;
        self.faults.staged(rel_path);
        self.inner.write_entity(rel_path, content)
    }
    fn delete_entity(&self, rel_path: &Path) -> Result<(), BackendError> {
        self.gate(Method::DeleteEntity)?;
        self.faults.staged(rel_path);
        self.inner.delete_entity(rel_path)
    }
    fn move_entity(&self, from: &Path, to: &Path) -> Result<(), BackendError> {
        self.gate(Method::MoveEntity)?;
        self.faults.staged(from);
        self.faults.staged(to);
        self.inner.move_entity(from, to)
    }
    fn discard_pending(&self) -> Result<(), BackendError> {
        self.gate(Method::DiscardPending)?;
        self.inner.discard_pending()
    }
    fn commit(&self, message: &str, ctx: &CommitContext<'_>) -> Result<CommitId, BackendError> {
        self.gate(Method::Commit)?;
        let result = self.inner.commit(message, ctx);
        self.note_commit(&result);
        result
    }
    fn commit_with_expected_parent(
        &self,
        message: &str,
        ctx: &CommitContext<'_>,
        expected_parent: Option<&str>,
    ) -> Result<CommitId, BackendError> {
        self.gate(Method::CommitWithExpectedParent)?;
        let result = self
            .inner
            .commit_with_expected_parent(message, ctx, expected_parent);
        self.note_commit(&result);
        result
    }
    fn stage_provenance(&self, record: &Provenance) -> Result<(), BackendError> {
        self.gate(Method::StageProvenance)?;
        self.inner.stage_provenance(record)
    }
    fn append_provenance(&self, record: &Provenance) -> Result<(), BackendError> {
        self.gate(Method::AppendProvenance)?;
        self.inner.append_provenance(record)
    }
    fn complete_interrupted_commit(&self) -> Result<Option<CompletedCommit>, BackendError> {
        self.gate(Method::CompleteInterruptedCommit)?;
        self.inner.complete_interrupted_commit()
    }
    fn read_provenance(&self, cursor: Option<&str>) -> Result<Vec<Provenance>, BackendError> {
        self.gate(Method::ReadProvenance)?;
        self.inner.read_provenance(cursor)
    }
    fn current_head(&self) -> Result<Option<String>, BackendError> {
        self.gate(Method::CurrentHead)?;
        self.inner.current_head()
    }
    fn write_lock_path(&self) -> Option<PathBuf> {
        // Infallible by signature: recorded, never failed.
        self.faults.enter(Method::WriteLockPath);
        self.inner.write_lock_path()
    }
    fn read_mem_config(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.gate(Method::ReadMemConfig)?;
        self.inner.read_mem_config()
    }
    fn read_archive_provenance(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.gate(Method::ReadArchiveProvenance)?;
        self.inner.read_archive_provenance()
    }
    fn read_archive_checks(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.gate(Method::ReadArchiveChecks)?;
        self.inner.read_archive_checks()
    }
    fn read_proposal_record(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.gate(Method::ReadProposalRecord)?;
        self.inner.read_proposal_record()
    }
    fn write_mem_config(&self, bytes: &[u8], ctx: &CommitContext<'_>) -> Result<(), BackendError> {
        self.gate(Method::WriteMemConfig)?;
        self.inner.write_mem_config(bytes, ctx)
    }
    fn write_mem_config_cas(
        &self,
        expected: Option<&[u8]>,
        bytes: &[u8],
        ctx: &CommitContext<'_>,
    ) -> Result<bool, BackendError> {
        self.gate(Method::WriteMemConfigCas)?;
        self.inner.write_mem_config_cas(expected, bytes, ctx)
    }
    fn record_pipeline_edit(
        &self,
        kind: &str,
        edits: &[(String, Option<Vec<u8>>)],
        ctx: &CommitContext<'_>,
        verb: &str,
    ) -> Result<(), BackendError> {
        self.gate(Method::RecordPipelineEdit)?;
        self.inner.record_pipeline_edit(kind, edits, ctx, verb)
    }
    fn read_anchors_sidecar(&self) -> Result<Option<Vec<u8>>, BackendError> {
        self.gate(Method::ReadAnchorsSidecar)?;
        self.inner.read_anchors_sidecar()
    }
    fn write_anchors_sidecar(&self, bytes: &[u8]) -> Result<(), BackendError> {
        self.gate(Method::WriteAnchorsSidecar)?;
        self.faults
            .staged(Path::new(memstead_base::anchor::ANCHOR_SIDECAR_PATH));
        self.inner.write_anchors_sidecar(bytes)
    }
    fn delete_artifacts(&self, ctx: &CommitContext<'_>) -> Result<(), BackendError> {
        self.gate(Method::DeleteArtifacts)?;
        self.inner.delete_artifacts(ctx)
    }
}

/// Every listed method is gated by the wrapper: with the fault armed on
/// its first call, the call returns the injected error and never
/// reaches the wrapped backend. The dispatch below is exhaustive over
/// the method list, so a method that joins the list without a gate in
/// the wrapper fails here.
#[test]
fn every_listed_method_is_gated_by_the_wrapper() {
    let faults = Arc::new(Faults::default());
    let backend = FaultBackend {
        inner: Arc::new(memstead_base::storage::InMemoryBackend::new()),
        faults: Arc::clone(&faults),
    };
    let ctx = CommitContext::internal();
    let path = Path::new("a.md");
    let record = Provenance::new(
        std::time::SystemTime::now(),
        memstead_base::provenance::ProvenanceKind::Create,
        None,
        Actor::Cli,
        None,
        None,
    );
    for &method in Method::ALL {
        faults.arm(method, 1);
        let failed = match method {
            Method::ListEntities => backend.list_entities().is_err(),
            Method::ReadEntity => backend.read_entity(path).is_err(),
            Method::EntityExists => backend.entity_exists(path).is_err(),
            Method::ReadAllEntities => backend.read_all_entities().is_err(),
            Method::StoragePresent => backend.storage_present().is_err(),
            Method::WriteEntity => backend.write_entity(path, b"x").is_err(),
            Method::DeleteEntity => backend.delete_entity(path).is_err(),
            Method::MoveEntity => backend.move_entity(path, Path::new("b.md")).is_err(),
            Method::DiscardPending => backend.discard_pending().is_err(),
            Method::Commit => backend.commit("m", &ctx).is_err(),
            Method::CommitWithExpectedParent => backend
                .commit_with_expected_parent("m", &ctx, None)
                .is_err(),
            Method::StageProvenance => backend.stage_provenance(&record).is_err(),
            Method::AppendProvenance => backend.append_provenance(&record).is_err(),
            Method::CompleteInterruptedCommit => backend.complete_interrupted_commit().is_err(),
            Method::ReadProvenance => backend.read_provenance(None).is_err(),
            Method::CurrentHead => backend.current_head().is_err(),
            // Infallible by signature: recorded, never failed.
            Method::WriteLockPath => {
                backend.write_lock_path();
                faults.fired()
            }
            Method::ReadMemConfig => backend.read_mem_config().is_err(),
            Method::ReadArchiveProvenance => backend.read_archive_provenance().is_err(),
            Method::ReadArchiveChecks => backend.read_archive_checks().is_err(),
            Method::ReadProposalRecord => backend.read_proposal_record().is_err(),
            Method::WriteMemConfig => backend.write_mem_config(b"{}", &ctx).is_err(),
            Method::WriteMemConfigCas => backend.write_mem_config_cas(None, b"{}", &ctx).is_err(),
            Method::RecordPipelineEdit => backend
                .record_pipeline_edit("mediums", &[], &ctx, "set")
                .is_err(),
            Method::ReadAnchorsSidecar => backend.read_anchors_sidecar().is_err(),
            Method::WriteAnchorsSidecar => backend.write_anchors_sidecar(b"{}").is_err(),
            Method::DeleteArtifacts => backend.delete_artifacts(&ctx).is_err(),
        };
        assert!(failed, "{} is not gated by the wrapper", method.name());
        assert!(faults.fired(), "{}: the fault never fired", method.name());
    }
}

// ---------------------------------------------------------------------
// The two backends
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Folder,
    GitBranch,
}

struct Workspace {
    _tmp: TempDir,
    kind: Kind,
    mount: Mount,
    folder: Option<PathBuf>,
}

impl Workspace {
    /// A seeded workspace, built once per matrix and copied per cell:
    /// seeding through the engine costs several synced commits, a
    /// directory copy costs none.
    fn template(kind: Kind) -> Self {
        let tmp = TempDir::new().unwrap();
        match kind {
            Kind::Folder => {
                let dir = tmp.path().join(MEM);
                std::fs::create_dir_all(dir.join(".memstead")).unwrap();
                std::fs::write(
                    dir.join(".memstead/config.json"),
                    r#"{"schema": "default@1.0.0"}"#,
                )
                .unwrap();
            }
            Kind::GitBranch => {
                init_real_mem_repo(tmp.path(), &[(MEM, "default@1.0.0")]);
            }
        }
        let peer = tmp.path().join(PEER);
        std::fs::create_dir_all(peer.join(".memstead")).unwrap();
        std::fs::write(
            peer.join(".memstead/config.json"),
            r#"{"schema": "default@1.0.0"}"#,
        )
        .unwrap();
        let ws = Self::over(tmp, kind);
        ws.seed();
        ws
    }

    fn peer_mount(&self, peer: Peer) -> Mount {
        let (capability, lifecycle) = match peer {
            Peer::Lazy => (MountCapability::Write, MountLifecycle::Lazy),
            Peer::ReadOnly => (MountCapability::ReadOnly, MountLifecycle::Eager),
            Peer::Absent => (MountCapability::Write, MountLifecycle::Eager),
        };
        Mount {
            mem: PEER.to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::Folder {
                path: self._tmp.path().join(PEER),
            },
            capability,
            lifecycle,
            cross_linkable: true,
            migration_target: None,
        }
    }

    fn settle(mut engine: Engine) -> Engine {
        engine.set_git_branch_ops(memstead_git_branch::storage::FULL_GIT_BRANCH_OPS);
        // As an engine over a mem-repo workspace has it: the factory
        // that can build a backend for either kind of mount.
        engine.set_backend_factory(instantiate_full_backend);
        let mut settings = memstead_base::WorkspaceSettings::default();
        settings.cross_mem_links.insert(
            PEER.to_string(),
            memstead_schema::workspace_config::CrossLinkValue::Wildcard,
        );
        settings.cross_mem_links.insert(
            MEM.to_string(),
            memstead_schema::workspace_config::CrossLinkValue::Wildcard,
        );
        engine.set_settings(settings);
        engine
    }

    /// A private copy of this workspace's storage.
    fn copy(&self) -> Self {
        let tmp = TempDir::new().unwrap();
        copy_dir(self._tmp.path(), tmp.path());
        Self::over(tmp, self.kind)
    }

    fn over(tmp: TempDir, kind: Kind) -> Self {
        let (storage, folder) = match kind {
            Kind::Folder => {
                let dir = tmp.path().join(MEM);
                (MountStorage::Folder { path: dir.clone() }, Some(dir))
            }
            Kind::GitBranch => (
                MountStorage::GitBranch {
                    gitdir: tmp.path().join("mem-repo").join(".git"),
                    branch: MEM.to_string(),
                },
                None,
            ),
        };
        let mount = Mount {
            mem: MEM.to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage,
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        Self {
            _tmp: tmp,
            kind,
            mount,
            folder,
        }
    }

    /// A backend instance of its own over the same storage: it shares
    /// no staged set with any engine.
    fn raw_backend(&self) -> Box<dyn MemBackend> {
        instantiate_full_backend(&self.mount).expect("backend instantiates")
    }

    /// An engine loaded from storage, with no fault wrapper. `peer`
    /// says whether and how the second mem is mounted beside it.
    fn fresh_engine(&self, peer: Peer) -> Engine {
        let mut mounts = vec![(self.mount.clone(), self.raw_backend())];
        if peer != Peer::Absent {
            let mount = self.peer_mount(peer);
            let backend = instantiate_full_backend(&mount).expect("peer backend");
            mounts.push((mount, backend));
        }
        Self::settle(Engine::from_mounts(mounts).expect("engine boots"))
    }

    /// An engine whose backends fail on command (the mem's and, with
    /// `peer`, the lazy peer's share one fault handle), plus that
    /// handle and the backend instance the engine stages the mem's
    /// writes into.
    fn faulted_engine(&self, peer: Peer) -> (Engine, Arc<Faults>, Arc<dyn MemBackend>) {
        let inner: Arc<dyn MemBackend> = Arc::from(self.raw_backend());
        let faults = Arc::new(Faults::default());
        let wrapper = FaultBackend {
            inner: Arc::clone(&inner),
            faults: Arc::clone(&faults),
        };
        let mut mounts = vec![(self.mount.clone(), Box::new(wrapper) as Box<dyn MemBackend>)];
        if peer != Peer::Absent {
            let mount = self.peer_mount(peer);
            let wrapper = FaultBackend {
                inner: Arc::from(instantiate_full_backend(&mount).expect("peer backend")),
                faults: Arc::clone(&faults),
            };
            mounts.push((mount, Box::new(wrapper) as Box<dyn MemBackend>));
        }
        let engine = Self::settle(Engine::from_mounts(mounts).expect("engine boots"));
        (engine, faults, inner)
    }

    fn seed(&self) {
        let mut engine = self.fresh_engine(Peer::Absent);
        for title in ["Alpha", "Beta", "Gamma"] {
            engine
                .create_entity(create_args(title, &[]), Actor::Cli, None, None)
                .unwrap_or_else(|e| panic!("seed {title}: {e:?}"));
        }
        engine
            .relate_entity(relate_args("alpha", "beta"), Actor::Cli, None, None)
            .expect("seed relation");

        // The peer's one entity, written as a file: it links to
        // `notes--ghost`, which does not exist, and no engine verb
        // authors a link to a missing target in another mem.
        std::fs::write(
            self._tmp.path().join(PEER).join("target.md"),
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n\n\
             # Target\n\n## Identity\nTarget identity\n\n## Purpose\nTarget purpose\n\n\
             ## Relationships\n- **USES**: [[notes--ghost]]\n",
        )
        .unwrap();
    }

    /// Storage, byte for byte: the folder's files (ledger included,
    /// lock files aside), or the branch's commit, which names its tree.
    fn storage_snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        match (&self.folder, self.kind) {
            (Some(dir), _) => snapshot_dir(dir, dir, &mut out),
            (None, _) => {
                let head = self
                    .raw_backend()
                    .current_head()
                    .unwrap()
                    .unwrap_or_default();
                out.insert("refs/heads".to_string(), head.into_bytes());
            }
        }
        out
    }
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

/// The entries two snapshots disagree on, by name (the bytes are not
/// worth printing).
fn changed_keys(a: &BTreeMap<String, Vec<u8>>, b: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    keys.into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .cloned()
        .collect()
}

fn snapshot_dir(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .to_string();
        if rel == ".memstead/locks" {
            continue;
        }
        if path.is_dir() {
            out.insert(format!("{rel}/"), Vec::new());
            snapshot_dir(root, &path, out);
        } else {
            out.insert(rel, std::fs::read(&path).unwrap());
        }
    }
}

// ---------------------------------------------------------------------
// The verbs
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Create,
    /// A create whose relation points into the lazy peer: the engine
    /// reads the target from the peer's storage before the commit, so
    /// this verb has a storage call on a second backend to fail.
    CreateIntoDeferredMem,
    Update,
    Delete,
    /// The delete of a stub: no file and so no commit, the provenance
    /// record appended on its own is the whole write.
    DeleteStub,
    Relate,
    Retype,
    Rename,
    BatchCreate,
    BatchUpdate,
    BatchRelate,
}

impl Verb {
    /// Whether and how the verb wants the second mem mounted.
    fn peer(self) -> Peer {
        match self {
            Verb::CreateIntoDeferredMem => Peer::Lazy,
            Verb::DeleteStub => Peer::ReadOnly,
            _ => Peer::Absent,
        }
    }
}

impl Method {
    /// Does the method change what the backend holds or has staged?
    /// Exhaustive, so a method added to the trait is classified before
    /// this file compiles; the writers' check below scans for calls of
    /// every method that answers yes.
    fn mutates(self) -> bool {
        match self {
            Method::WriteEntity
            | Method::DeleteEntity
            | Method::MoveEntity
            | Method::DiscardPending
            | Method::Commit
            | Method::CommitWithExpectedParent
            | Method::StageProvenance
            | Method::AppendProvenance
            | Method::WriteMemConfig
            | Method::WriteMemConfigCas
            | Method::RecordPipelineEdit
            | Method::WriteAnchorsSidecar
            | Method::DeleteArtifacts => true,
            // Completing a cut-off commit writes, and belongs to every
            // load; a load is not a writer of its own.
            Method::CompleteInterruptedCommit
            | Method::ListEntities
            | Method::ReadEntity
            | Method::EntityExists
            | Method::ReadAllEntities
            | Method::StoragePresent
            | Method::ReadProvenance
            | Method::CurrentHead
            | Method::WriteLockPath
            | Method::ReadMemConfig
            | Method::ReadArchiveProvenance
            | Method::ReadArchiveChecks
            | Method::ReadProposalRecord
            | Method::ReadAnchorsSidecar => false,
        }
    }
}

/// The git operations the engine reaches through its hook table (they
/// live in this crate, behind function pointers), each with whether it
/// writes to a mem-repo. Checked against the table's fields below, so a
/// hook added to the table is classified before the writers' check
/// passes.
const HOOKS: &[(&str, bool)] = &[
    ("changes_since", false),
    ("diff", false),
    ("branch_reset", true),
    ("fetch", false),
    ("pull", true),
    ("push", false),
    ("ls_remote", false),
    ("resolve_ref", false),
    ("is_ancestor", false),
    ("is_empty_root", false),
    ("merge_base", false),
    ("update_ref", true),
    ("remote_add", false),
    ("read_tree", false),
    ("export", false),
    ("export_to_bytes", false),
    ("prune_residue", true),
    ("prune_config_blob", true),
    ("branch_namespace_conflicts", false),
    ("create_branch_at", true),
    ("read_config_at_ref", false),
    ("rename_mem_storage", true),
    ("write_schema", true),
    ("read_schema_file", false),
    ("read_ref_schemas", false),
    ("collect_ref_schema_source", false),
];

/// The engine's own write helpers, which a writer may call instead of
/// a storage method: the write lock and the one commit call.
const WRITE_HELPERS: &[&str] = &[
    "lock_mems_for_write",
    "lock_mems_for_write_if",
    "lock_for_write",
    "lock_workspace_for_write_if",
    "commit_staged",
    "commit_staged_pinned",
    "append_record",
];

/// Every function of the engine that writes to a mem (it calls a
/// mutating storage method or one of the write helpers) and belongs to
/// a mutation verb, with the matrix verbs that drive it. A function is
/// named by its file and its name, so a second function of the same
/// name elsewhere is a writer of its own.
const VERBS: &[(&str, &[Verb])] = &[
    (
        "engine/mutation/create/mod.rs::create_entity",
        &[Verb::Create, Verb::CreateIntoDeferredMem],
    ),
    (
        "engine/mutation/create/commit.rs::commit_prepared_create",
        &[Verb::Create, Verb::CreateIntoDeferredMem],
    ),
    (
        "engine/mutation/create/stage.rs::stage_prepared_create",
        &[Verb::Create, Verb::CreateIntoDeferredMem, Verb::BatchCreate],
    ),
    (
        "engine/mutation/create/batch.rs::batch_create",
        &[Verb::BatchCreate],
    ),
    (
        "engine/mutation/update/mod.rs::update_entity",
        &[Verb::Update],
    ),
    (
        "engine/mutation/update/commit.rs::commit_prepared_update",
        &[Verb::Update],
    ),
    (
        "engine/mutation/update/stage.rs::stage_prepared_update",
        &[Verb::Update, Verb::BatchUpdate],
    ),
    (
        "engine/mutation/update/batch.rs::batch_update",
        &[Verb::BatchUpdate],
    ),
    (
        "engine/mutation/mod.rs::stage_anchors_sidecar",
        &[
            Verb::Create,
            Verb::BatchCreate,
            Verb::Update,
            Verb::BatchUpdate,
        ],
    ),
    (
        "engine/mutation/mod.rs::stage_derivation_sidecar",
        &[Verb::Create, Verb::Update, Verb::Relate],
    ),
    (
        "engine/mutation/delete.rs::delete_entity",
        &[Verb::Delete, Verb::DeleteStub],
    ),
    (
        "engine/mutation/delete.rs::delete_entity_with_ctx",
        &[Verb::Delete, Verb::DeleteStub],
    ),
    (
        "engine/mutation/mod.rs::stage_anchors_removal",
        &[Verb::Delete],
    ),
    ("engine/mutation/relate.rs::relate_entity", &[Verb::Relate]),
    (
        "engine/mutation/relate.rs::stage_prepared_relate",
        &[Verb::Relate, Verb::BatchRelate],
    ),
    (
        "engine/mutation/relate.rs::batch_relate",
        &[Verb::BatchRelate],
    ),
    ("engine/mutation/retype.rs::retype_entity", &[Verb::Retype]),
    (
        "engine/mutation/mod.rs::stage_proposal_marks_retype",
        &[Verb::Retype],
    ),
    (
        "engine/mutation/rename/mod.rs::rename_entity",
        &[Verb::Rename],
    ),
    (
        "engine/mutation/rename/stage.rs::stage_rename_source",
        &[Verb::Rename],
    ),
    // The matrix's rename has no referrer in another mem; a rename
    // that writes a peer mem is an operation over several mems.
    (
        "engine/mutation/rename/stage.rs::stage_rename_peer",
        &[Verb::Rename],
    ),
    (
        "engine/mutation/rename/commit.rs::commit_prepared_rename",
        &[Verb::Rename],
    ),
    (
        "engine/mutation/mod.rs::stage_anchors_rename",
        &[Verb::Rename],
    ),
    (
        "engine/mutation/mod.rs::stage_proposal_marks_rename",
        &[Verb::Rename],
    ),
    (
        "engine/mutation/mod.rs::stage_proposal_marks_rehash",
        &[Verb::Rename],
    ),
    (
        "engine/mutation/update/batch.rs::discard_all_pending",
        &[Verb::BatchCreate, Verb::BatchUpdate, Verb::BatchRelate],
    ),
    // The write lock and the one commit call, shared by every verb.
    ("engine/write_lock.rs::lock_mems_for_write_if", EVERY_VERB),
    // A verb that names its mems locks them after the roster is
    // reconciled; a verb whose guard reads the workspace locks every
    // writable mount.
    (
        "engine/write_lock.rs::lock_for_write",
        &[
            Verb::Create,
            Verb::CreateIntoDeferredMem,
            Verb::BatchCreate,
            Verb::Update,
            Verb::BatchUpdate,
            Verb::Relate,
            Verb::BatchRelate,
        ],
    ),
    (
        "engine/write_lock.rs::lock_workspace_for_write_if",
        &[Verb::Delete, Verb::DeleteStub, Verb::Retype, Verb::Rename],
    ),
    ("engine/write_lock.rs::drop", EVERY_VERB),
    ("engine/write_lock.rs::discard_strays", EVERY_VERB),
    ("engine/mutation/landing.rs::commit_staged", EVERY_VERB),
    (
        "engine/mutation/landing.rs::commit_staged_pinned",
        EVERY_VERB,
    ),
    ("engine/mutation/landing.rs::settle", EVERY_VERB),
    (
        "engine/mutation/landing.rs::append_record",
        &[Verb::DeleteStub],
    ),
];

/// Every verb of the matrix, for the functions all of them share.
const EVERY_VERB: &[Verb] = &[
    Verb::Create,
    Verb::CreateIntoDeferredMem,
    Verb::BatchCreate,
    Verb::Update,
    Verb::BatchUpdate,
    Verb::Delete,
    Verb::DeleteStub,
    Verb::Relate,
    Verb::BatchRelate,
    Verb::Retype,
    Verb::Rename,
];

/// Functions that write to a mem and are not a mutation verb on one
/// mem's entities, each with the reason it stays out of the matrix.
const NOT_VERBS: &[(&str, &str)] = &[
    (
        "engine/lifecycle/mem_settings.rs::set_mem_version",
        "writes the mem's config, which no entity commit carries",
    ),
    (
        "engine/lifecycle/mem_settings.rs::set_mem_description",
        "the mem's config",
    ),
    (
        "engine/lifecycle/mem_settings.rs::set_mem_title",
        "the mem's config",
    ),
    (
        "engine/lifecycle/mem_settings.rs::set_mem_subject",
        "the mem's config",
    ),
    (
        "engine/lifecycle/mem_settings.rs::set_mem_internal",
        "the mem's config",
    ),
    (
        "engine/lifecycle/mem_settings.rs::set_mem_process_mem",
        "the mem's config",
    ),
    (
        "engine/lifecycle/mem_settings.rs::set_mem_sync_state",
        "the mem's config",
    ),
    (
        "engine/lifecycle/schema_pin.rs::write_mem_config_merged",
        "the mem's config",
    ),
    (
        "backend.rs::commit_with_expected_parent",
        "a default method of the storage trait, a backend delegating to itself",
    ),
    (
        "backend.rs::write_mem_config_cas",
        "a default method of the storage trait, a backend delegating to itself",
    ),
    (
        "engine/lifecycle/schema_pin.rs::bump_backend_schema_pin",
        "the mem's config: the schema pin",
    ),
    (
        "engine/review.rs::set_review_mark",
        "a review mark is a provenance line of its own, with no entity write",
    ),
    (
        "engine/mutation/mod.rs::record_anchor_observed_hashes",
        "the verify pass's observation record on the anchors sidecar, not an entity verb",
    ),
    (
        "engine/mutation/mod.rs::record_anchor_observations",
        "the verify pass's observation record on the anchors sidecar, not an entity verb",
    ),
    (
        "engine/lifecycle/export.rs::record_pipeline_edit_provenance",
        "the provenance mirror of a pipeline-config edit, not an entity write",
    ),
    (
        "engine/mutation/relate.rs::refresh_derivation_baseline_on_noop",
        "a relate that finds its edge present and refreshes a derivation baseline: it needs a \
         schema that declares a derivation relation, which no built-in schema does; it commits \
         through the verbs' one commit call",
    ),
    (
        "engine/mutation/mod.rs::stage_anchors_replace",
        "the merge of a proposal replaces an entity's anchors; no verb does",
    ),
    (
        "engine/checks_transport.rs::publish_checks",
        "writes check records to the check-ledger ref, not to a mem's entities",
    ),
    (
        "engine/conflicts.rs::resolve_merge_conflict",
        "the resolution of a pull's conflict, an operation over a merge",
    ),
    (
        "engine/mutation/mem_sweep.rs::rewrite_mem_references",
        "the sweep after a mem is renamed, an operation over several mems",
    ),
    (
        "engine/mutation/mem_sweep.rs::land_mem_sweep",
        "one mem's commit of that sweep",
    ),
    (
        "engine/mutation/proposal_merge.rs::proposal_merge",
        "the merge of a proposal, an operation over several commits",
    ),
    (
        "mem_management/fork.rs::retarget_fork_tree",
        "the fork of a mem",
    ),
    ("mem_management/fork.rs::fork_mem", "the fork of a mem"),
    (
        "mem_management/fork.rs::roll_back",
        "the rollback of a fork that failed",
    ),
    (
        "engine/drift.rs::pull",
        "transport: brings a remote's commits into a mem's branch",
    ),
    (
        "engine/drift.rs::fast_forward_ref",
        "transport: moves a branch to a fetched commit",
    ),
    (
        "engine/drift.rs::branch_reset",
        "resets a mem's branch to an earlier commit",
    ),
    (
        "engine/lifecycle/export.rs::export_markdown",
        "writes every entity of a folder mem back to its files from the store, a whole-mem \
         export; it writes the files itself and not through the backend",
    ),
    (
        "engine/lifecycle/schema.rs::install_schema",
        "writes a schema, not a mem's entities",
    ),
    (
        "engine/lifecycle/schema.rs::write_to_local_schema_source",
        "writes a schema, not a mem's entities",
    ),
    (
        "mem_management/lifecycle.rs::prune_orphan_config",
        "removes the config of a mem that is gone",
    ),
    (
        "mem_management/lifecycle.rs::create_mem",
        "the creation of a mem",
    ),
    (
        "mem_management/lifecycle.rs::delete_mem",
        "the deletion of a mem",
    ),
    (
        "mem_management/lifecycle.rs::rename_mem",
        "the renaming of a mem",
    ),
];

fn all_verbs() -> Vec<Verb> {
    let mut verbs: Vec<Verb> = Vec::new();
    for verb in VERBS.iter().flat_map(|(_, verbs)| verbs.iter().copied()) {
        if !verbs.contains(&verb) {
            verbs.push(verb);
        }
    }
    verbs
}

/// The name a line declares, if it declares a function.
fn declared_fn(line: &str) -> Option<String> {
    let mut rest = line.trim_start();
    if let Some(after) = rest.strip_prefix("pub") {
        rest = after.trim_start();
        if rest.starts_with('(') {
            rest = rest[rest.find(')')? + 1..].trim_start();
        }
    }
    while let Some(after) = ["async ", "const ", "unsafe "]
        .iter()
        .find_map(|q| rest.strip_prefix(q))
    {
        rest = after.trim_start();
    }
    let name: String = rest
        .strip_prefix("fn ")?
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Does the line call a mutating storage method or a write helper?
fn writes(line: &str) -> bool {
    // A method is called on a value (`backend.commit(…)`) or by
    // path (`MemBackend::commit(backend, …)`).
    let method_call = Method::ALL.iter().filter(|m| m.mutates()).any(|m| {
        [".", "::"].iter().any(|prefix| {
            match line.find(&format!("{prefix}{}(", m.name())) {
                // The search index's argument-less `commit()` is its
                // own and no backend's.
                Some(at) if *m == Method::Commit => !line[at..].starts_with(".commit()"),
                Some(_) => true,
                None => false,
            }
        })
    });
    method_call
        || WRITE_HELPERS
            .iter()
            .any(|helper| line.contains(&format!("{helper}(")))
        // A hook is a function pointer in a field: `(ops.pull)(…)`.
        || HOOKS
            .iter()
            .any(|(hook, writes)| *writes && line.contains(&format!(".{hook})(")))
}

/// The line without its string and character literals, so a brace
/// inside one is not taken for a block's.
fn without_literals(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                while let Some(d) = chars.next() {
                    match d {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            // A character literal: `'x'` or `'\x'`. A lifetime has
            // no closing quote two or three characters on.
            '\'' => {
                let rest: String = chars.clone().take(3).collect();
                let len = if rest.starts_with('\\') { 3 } else { 2 };
                if rest.chars().nth(len - 1) == Some('\'') {
                    for _ in 0..len {
                        chars.next();
                    }
                } else {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// The writers declared in one source file: every function, outside
/// test modules and test functions, with a line that writes.
fn writers_in(file: &str, text: &str, found: &mut BTreeSet<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut current: Option<String> = None;
    let mut next_is_a_test = false;
    // Depth inside an inline test module, whose lines are
    // skipped; production code can follow such a module.
    let mut test_module_depth: Option<i32> = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(depth) = test_module_depth.as_mut() {
            if !trimmed.starts_with("//") {
                let code = without_literals(line);
                *depth += code.matches('{').count() as i32;
                *depth -= code.matches('}').count() as i32;
            }
            if *depth <= 0 {
                test_module_depth = None;
                current = None;
            }
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        if trimmed == "#[cfg(test)]"
            && lines.get(i + 1).is_some_and(|next| {
                let next = next.trim();
                next.contains("mod ") && next.ends_with('{')
            })
        {
            test_module_depth = Some(0);
            continue;
        }
        if trimmed == "#[test]" {
            next_is_a_test = true;
            continue;
        }
        if let Some(name) = declared_fn(line) {
            // A test calls verbs; it is not a writer of the
            // engine.
            current = (!next_is_a_test).then(|| format!("{file}::{name}"));
            next_is_a_test = false;
            // A function written on one line has its body here.
            let body = line
                .split_once(&format!("fn {name}"))
                .map_or("", |(_, b)| b);
            if writes(body)
                && let Some(name) = &current
            {
                found.insert(name.clone());
            }
            continue;
        }
        if writes(line)
            && let Some(name) = &current
        {
            found.insert(name.clone());
        }
    }
}

fn scan(root: &Path, dir: &Path, skip: &[std::path::PathBuf], found: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if skip.contains(&path) {
            continue;
        }
        if path.is_dir() {
            // A `tests` directory holds test modules, which call
            // verbs and are no writers of the engine.
            if name != "tests" {
                scan(root, &path, skip, found);
            }
            continue;
        }
        if !name.ends_with(".rs") || name.ends_with("tests.rs") {
            continue;
        }
        let file = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(&path).unwrap();
        writers_in(&file, &text, found);
    }
}

/// The scan that finds the engine's writers, held against the ways a
/// writer can be written. Each probe is a source file as a writer added
/// later might look; the scan must name the writer in each, and must
/// name nothing in the files that hold none.
#[test]
fn the_writers_scan_sees_every_way_a_writer_is_written() {
    let found_in = |file: &str, text: &str| {
        let mut found = BTreeSet::new();
        writers_in(file, text, &mut found);
        found.into_iter().collect::<Vec<_>>()
    };
    let writer = |file: &str, name: &str| vec![format!("{file}::{name}")];

    let probes: &[(&str, &str, &str)] = &[
        (
            "a storage method called on a value",
            "pub fn w(b: &dyn MemBackend) {\n    b.write_entity(p, x);\n}\n",
            "w",
        ),
        (
            "a storage method called by path",
            "fn w(b: &dyn MemBackend) {\n    let _ = MemBackend::commit(b, m, c);\n}\n",
            "w",
        ),
        (
            "a qualified path call",
            "fn w(b: &B) {\n    <B as MemBackend>::delete_entity(b, p);\n}\n",
            "w",
        ),
        (
            "a writing hook of the git table",
            "fn w(&self) {\n    (ops.update_ref)(dir, r, sha);\n}\n",
            "w",
        ),
        (
            "an engine write helper",
            "fn w(&mut self) {\n    let _g = self.lock_mems_for_write(&[m])?;\n}\n",
            "w",
        ),
        (
            "an async function",
            "pub(crate) async fn w(b: &B) {\n    b.append_provenance(r);\n}\n",
            "w",
        ),
        (
            "a const unsafe function with a scoped visibility",
            "pub(in crate::engine) const unsafe fn w(b: &B) {\n    b.discard_pending();\n}\n",
            "w",
        ),
        (
            "a function written on one line",
            "pub fn w(b: &dyn MemBackend) { let _ = b.discard_pending(); }\n",
            "w",
        ),
        (
            "a writer after an inline test module",
            "fn reads() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        b.commit(&m, &c);\n    }\n}\n\npub fn w(b: &B) {\n    b.write_anchors_sidecar(x);\n}\n",
            "w",
        ),
        (
            "a writer after a test module with braces in its literals",
            "#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        assert!(\"a{b\".contains('{'));\n        let s = \"}}}\";\n    }\n}\n\npub fn w(b: &B) {\n    b.move_entity(a, c);\n}\n",
            "w",
        ),
        (
            "a writer after a test function",
            "#[test]\nfn t() {\n    b.commit(&m, &c);\n}\n\nfn w(b: &B) {\n    b.stage_provenance(r);\n}\n",
            "w",
        ),
        (
            "a method of an impl, a drop among them",
            "impl Drop for G {\n    fn drop(&mut self) {\n        let _ = self.0.discard_pending();\n    }\n}\n",
            "drop",
        ),
    ];
    for (what, text, name) in probes {
        assert_eq!(
            found_in("engine/probe.rs", text),
            writer("engine/probe.rs", name),
            "{what}"
        );
    }

    // The same name in another file is another writer.
    assert_ne!(
        found_in("engine/a.rs", probes[11].1),
        found_in("engine/b.rs", probes[11].1)
    );

    let quiet: &[(&str, &str)] = &[
        (
            "a function that reads",
            "fn r(b: &B) {\n    b.read_entity(p);\n    b.current_head();\n}\n",
        ),
        (
            "the search index's own commit",
            "fn r(idx: &mut I) {\n    idx.commit();\n}\n",
        ),
        (
            "a write named in a comment",
            "fn r() {\n    // b.write_entity(p, x) is what a verb does\n}\n",
        ),
        (
            "a test function",
            "#[test]\nfn t() {\n    b.write_entity(p, x);\n}\n",
        ),
        (
            "an inline test module",
            "#[cfg(test)]\nmod tests {\n    fn helper(b: &B) {\n        b.commit(&m, &c);\n    }\n}\n",
        ),
    ];
    for (what, text) in quiet {
        assert_eq!(
            found_in("engine/probe.rs", text),
            Vec::<String>::new(),
            "{what}"
        );
    }
}

/// The matrix's verb list is the engine's list of writers. Read from
/// the engine's source: every function that calls a mutating method of
/// the storage trait, a writing git hook, or one of the engine's write
/// helpers is either part of a matrix verb or named above with the
/// reason it is not, so a write path added later fails here until it
/// is placed.
#[test]
fn the_verb_list_is_the_engines_writers() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../memstead-base/src");

    // The hook list is the hook table's field list.
    let table = std::fs::read_to_string(source.join("engine/mod.rs")).unwrap();
    let start = table
        .find("pub struct GitBranchOps {")
        .expect("the hook table is declared in engine/mod.rs");
    let end = start + table[start..].find("\n}\n").expect("the table closes");
    let in_table: BTreeSet<String> = table[start..end]
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub "))
        .filter_map(|field| field.split(':').next())
        .filter(|name| !name.contains(' '))
        .map(str::to_string)
        .collect();
    let classified: BTreeSet<String> = HOOKS.iter().map(|(name, _)| name.to_string()).collect();
    assert_eq!(
        in_table, classified,
        "the git hook table and the fault matrix's hook list differ"
    );

    // The backends are what is written to, and the search index commits
    // to itself: these two directories, and nothing else of the crate,
    // are left out.
    let skip = [source.join("storage"), source.join("search_index")];
    let mut in_engine = BTreeSet::new();
    scan(&source, &source, &skip, &mut in_engine);
    let placed: BTreeSet<String> = VERBS
        .iter()
        .map(|(name, _)| name.to_string())
        .chain(NOT_VERBS.iter().map(|(name, _)| name.to_string()))
        .collect();
    assert_eq!(
        in_engine, placed,
        "the engine's writers and the fault matrix's verb list differ"
    );
}

fn id(slug: &str) -> EntityId {
    EntityId::new(MEM, slug)
}

fn create_args(title: &str, uses: &[&str]) -> CreateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("identity".to_string(), format!("{title} identity"));
    sections.insert("purpose".to_string(), format!("{title} purpose"));
    CreateEntityArgs {
        mem: MEM.to_string(),
        title: title.to_string(),
        entity_type: "spec".to_string(),
        sections,
        metadata: Default::default(),
        relations: uses
            .iter()
            .map(|slug| memstead_base::ops::RelateArg {
                target: id(slug),
                rel_type: "USES".to_string(),
                description: None,
            })
            .collect(),
        anchors: vec![memstead_base::anchor::AnchorInput {
            artifact: Some(format!("docs/{}.md", title.to_lowercase())),
            grain: Some("file".to_string()),
            class: Some("informed-by".to_string()),
            ..Default::default()
        }],
        dry_run: false,
    }
}

fn update_args(engine: &Engine, slug: &str, text: &str) -> UpdateEntityArgs {
    let mut sections = IndexMap::new();
    sections.insert("purpose".to_string(), text.to_string());
    UpdateEntityArgs {
        id: id(slug),
        expected_hash: engine.get_entity(&id(slug)).map(|e| e.content_hash.clone()),
        sections,
        append_sections: Default::default(),
        patch_sections: Default::default(),
        sections_unset: Vec::new(),
        metadata: Default::default(),
        metadata_unset: Vec::new(),
        dry_run: false,
        declare_relations: Vec::new(),
        // An anchor, so an update is a commit of two files like a
        // create and the sidecar's calls are cells of its own.
        anchors: vec![memstead_base::anchor::AnchorInput {
            artifact: Some(format!("docs/{slug}-update.md")),
            grain: Some("file".to_string()),
            class: Some("informed-by".to_string()),
            ..Default::default()
        }],
        anchors_unset: Vec::new(),
        relations_unset: Vec::new(),
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

fn hash_of(engine: &Engine, slug: &str) -> Option<String> {
    engine.get_entity(&id(slug)).map(|e| e.content_hash.clone())
}

/// Run one verb. The arguments are rebuilt from the engine's current
/// view on every call, so "the same call sent again" is literal.
fn run(verb: Verb, engine: &mut Engine) -> Result<Vec<String>, String> {
    run_verb(verb, engine).map_err(|e| e.code().to_string())?
}

/// A batch that refused: the code of the item that refused it. A batch
/// reports a refusal in its receipt (`applied: false`), not as an error.
fn batch_outcome(result: memstead_base::ops::BatchResult) -> Result<Vec<String>, String> {
    if result.applied {
        return Ok(result
            .warnings
            .iter()
            .map(|w| w.code().to_string())
            .collect());
    }
    Err(result
        .results
        .iter()
        .find_map(|entry| entry.error.as_ref().map(|e| e.code.clone()))
        .unwrap_or_else(|| "BATCH_NOT_APPLIED".to_string()))
}

fn run_verb(verb: Verb, engine: &mut Engine) -> Result<Result<Vec<String>, String>, EngineError> {
    let (actor, note) = (Actor::Cli, Some("matrix"));
    let warnings = match verb {
        Verb::Create => {
            engine
                .create_entity(create_args("Delta", &["alpha"]), actor, None, note)?
                .warnings
        }
        Verb::CreateIntoDeferredMem => {
            let mut args = create_args("Delta", &[]);
            args.relations.push(memstead_base::ops::RelateArg {
                target: EntityId::new(PEER, "target"),
                rel_type: "USES".to_string(),
                description: None,
            });
            engine.create_entity(args, actor, None, note)?.warnings
        }
        Verb::Update => {
            let args = update_args(engine, "alpha", "Alpha purpose, rewritten");
            engine.update_entity(args, actor, None, note)?.warnings
        }
        Verb::Delete => {
            let args = DeleteEntityArgs {
                id: id("gamma"),
                expected_hash: hash_of(engine, "gamma"),
            };
            engine.delete_entity(args, actor, None, note)?.warnings
        }
        Verb::DeleteStub => {
            assert!(
                engine.get_entity(&id("ghost")).is_some_and(|e| e.stub),
                "the fixture keeps a stub that only the read-only peer refers to"
            );
            let args = DeleteEntityArgs {
                id: id("ghost"),
                expected_hash: Some(String::new()),
            };
            engine.delete_entity(args, actor, None, note)?.warnings
        }
        Verb::Relate => {
            engine
                .relate_entity(relate_args("gamma", "alpha"), actor, None, note)?
                .warnings
        }
        Verb::Retype => {
            let args = RetypeEntityArgs {
                id: id("gamma"),
                expected_hash: hash_of(engine, "gamma"),
                target_type: "concept".to_string(),
                section_map: IndexMap::from_iter([
                    ("identity".to_string(), "definition".to_string()),
                    ("purpose".to_string(), "explanation".to_string()),
                ]),
                // `level` is a spec field the concept type does not carry.
                drop_metadata: vec!["level".to_string()],
                dry_run: false,
            };
            engine.retype_entity(args, actor, None, note)?.warnings
        }
        Verb::Rename => {
            let args = RenameEntityArgs {
                id: id("beta"),
                expected_hash: hash_of(engine, "beta"),
                new_title: "Beta Renamed".to_string(),
            };
            engine.rename_entity(args, actor, None, note)?.warnings
        }
        Verb::BatchCreate => {
            let creates = vec![
                (create_args("Delta", &["alpha"]), Some("first".to_string())),
                (create_args("Epsilon", &[]), None),
            ];
            return Ok(batch_outcome(
                engine.batch_create(creates, actor, None, false)?,
            ));
        }
        Verb::BatchUpdate => {
            let updates = vec![
                (
                    update_args(engine, "alpha", "Alpha purpose, batch"),
                    Some("first".to_string()),
                ),
                (update_args(engine, "gamma", "Gamma purpose, batch"), None),
            ];
            return Ok(batch_outcome(
                engine.batch_update(updates, actor, None, false)?,
            ));
        }
        Verb::BatchRelate => {
            let relates = vec![
                (relate_args("gamma", "alpha"), Some("first".to_string())),
                (relate_args("gamma", "beta"), None),
            ];
            return Ok(batch_outcome(
                engine.batch_relate(relates, actor, None, false)?,
            ));
        }
    };
    Ok(Ok(warnings.iter().map(|w| w.code().to_string()).collect()))
}

// ---------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------

/// The engine's view of the mem: every entity with its hash and its
/// relations in both directions, plus what a search returns. `hashes`
/// off drops the content hash, for comparing two runs whose bytes
/// differ only in the timestamps the engine stamps. The lazy peer's
/// own entities are left out: whether an operation happened to load
/// that mem (a delete's referrer guard does) is not the mem's state.
fn view(engine: &Engine, hashes: bool) -> BTreeMap<String, String> {
    let store = engine.store();
    let mut out = BTreeMap::new();
    for entity in store.all_entities().filter(|e| e.id.mem() == MEM) {
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
        // The anchors are read from the sidecar in storage on every
        // call: a view of their own, compared like the store's.
        let mut anchors: Vec<String> = engine
            .entity_anchors(&entity.id)
            .iter()
            .map(|a| a.artifact.clone())
            .collect();
        anchors.sort();
        out.insert(
            entity.id.to_string(),
            format!(
                "stub={} type={} title={:?} hash={} out={:?} in={:?} anchors={:?}",
                entity.stub, entity.entity_type, entity.title, hash, outgoing, incoming, anchors
            ),
        );
    }
    for term in ["purpose", "rewritten", "batch", "delta", "renamed", "gamma"] {
        let scope = memstead_base::ops::SearchScope {
            query: Some(memstead_base::ops::Query {
                any: vec![term.to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut hits: Vec<String> = engine
            .search(&scope)
            .map(|r| {
                r.hits
                    .iter()
                    .filter(|h| h.id.mem() == MEM)
                    .map(|h| h.id.to_string())
                    .collect()
            })
            .unwrap_or_else(|e| vec![format!("search failed: {e}")]);
        hits.sort();
        out.insert(format!("~search:{term}"), format!("{hits:?}"));
    }
    out
}

// ---------------------------------------------------------------------
// The matrix
// ---------------------------------------------------------------------

/// Methods whose failure the engine absorbs by design, so the call
/// still succeeds with the undisturbed result. Each has its reason:
///
/// - `current_head`: before the commit, a failed drift probe makes the
///   engine reload the mem instead of trusting the head, and the call
///   goes on from the reloaded view; after the commit it only decides
///   which cursor is recorded for the engine's own write.
/// - `write_lock_path`: infallible by signature.
/// - `discard_pending`: called on exits only; on a successful call
///   nothing is staged and its failure has nothing to leave behind.
const ABSORBED: &[Method] = &[
    Method::CurrentHead,
    Method::WriteLockPath,
    Method::DiscardPending,
];

struct Control {
    /// Calls the undisturbed verb makes, split at the commit.
    log: Vec<(Method, bool)>,
    view: BTreeMap<String, String>,
}

fn control(template: &Workspace, verb: Verb) -> Control {
    let (kind, ws) = (template.kind, template.copy());
    let peer = verb.peer();
    let (mut engine, faults, _inner) = ws.faulted_engine(peer);
    faults.reset();
    run(verb, &mut engine).unwrap_or_else(|e| panic!("{kind:?} {verb:?} undisturbed: {e:?}"));
    let log = faults.log();
    assert_eq!(
        view(&engine, true),
        view(&ws.fresh_engine(peer), true),
        "{kind:?} {verb:?}: the undisturbed run's view differs from a fresh load"
    );
    Control {
        log,
        view: view(&engine, false),
    }
}

/// No path a write was staged for reads differently through the
/// engine's backend than through a backend of its own on the same
/// storage: nothing staged is left to be served.
fn assert_nothing_staged(
    ws: &Workspace,
    staged: &BTreeSet<PathBuf>,
    engine_backend: &Arc<dyn MemBackend>,
    cell: &str,
) {
    let fresh = ws.raw_backend();
    for path in staged {
        assert_eq!(
            engine_backend.read_entity(path).unwrap(),
            fresh.read_entity(path).unwrap(),
            "{cell}: {} is still staged after the failed call",
            path.display()
        );
    }
}

fn run_cell(template: &Workspace, verb: Verb, method: Method, nth: usize, control: &Control) {
    let kind = template.kind;
    let cell = format!("{kind:?} {verb:?} {}#{nth}", method.name());
    let ws = template.copy();
    let peer = verb.peer();
    let (mut engine, faults, engine_backend) = ws.faulted_engine(peer);
    let before = ws.storage_snapshot();

    faults.arm(method, nth);
    let result = run(verb, &mut engine);
    assert!(faults.fired(), "{cell}: the fault never fired");
    let landed = faults.fired_after_landing();
    let staged = faults.staged_paths();
    faults.reset();

    match (&result, landed) {
        // Before the commit landed, refused: nothing is left behind and
        // the same call sent again is a first call.
        (Err(e), false) => {
            assert!(!e.is_empty(), "{cell}: the error carries no code");
            let changed = changed_keys(&before, &ws.storage_snapshot());
            assert!(
                changed.is_empty(),
                "{cell}: storage changed under a failed call ({e}): {changed:?}"
            );
            assert_nothing_staged(&ws, &staged, &engine_backend, &cell);
            assert_eq!(
                view(&engine, true),
                view(&ws.fresh_engine(peer), true),
                "{cell}: after the failed call the engine's view differs from a fresh load"
            );
            run(verb, &mut engine).unwrap_or_else(|e| panic!("{cell}: the retry failed: {e:?}"));
            assert_eq!(
                view(&engine, false),
                control.view,
                "{cell}: the retry's result differs from an undisturbed call"
            );
            assert_eq!(
                view(&engine, true),
                view(&ws.fresh_engine(peer), true),
                "{cell}: after the retry the engine's view differs from a fresh load"
            );
        }
        // Absorbed by design: the call succeeds with the undisturbed
        // result, before or after the commit.
        (Ok(_), _) if ABSORBED.contains(&method) => {
            assert_eq!(
                view(&engine, false),
                control.view,
                "{cell}: an absorbed failure changed the result"
            );
            assert_eq!(
                view(&engine, true),
                view(&ws.fresh_engine(peer), true),
                "{cell}: after an absorbed failure the engine's view differs from a fresh load"
            );
        }
        // After the commit landed: the write is in storage, so the
        // call reports success, names the step, and the views agree.
        (Ok(codes), true) => {
            assert!(
                codes.iter().any(|c| c == "WRITE_FOLLOW_UP_FAILED"),
                "{cell}: a step after the commit failed and no warning names it: {codes:?}"
            );
            assert_eq!(
                view(&engine, false),
                control.view,
                "{cell}: the landed write is missing from the engine's view"
            );
            assert_eq!(
                view(&engine, true),
                view(&ws.fresh_engine(peer), true),
                "{cell}: after a failed follow-up the engine's view differs from a fresh load"
            );
        }
        (Err(e), true) => panic!("{cell}: a write that is in storage was reported as failed: {e}"),
        (Ok(_), false) => panic!("{cell}: a call whose storage call failed reported success"),
    }
}

fn run_matrix(kind: Kind) {
    let template = Workspace::template(kind);
    let mut cells = 0usize;
    for verb in all_verbs() {
        let control = control(&template, verb);
        if std::env::var_os("MATRIX_TRACE").is_some() {
            let calls: Vec<String> = control
                .log
                .iter()
                .map(|(m, landed)| format!("{}{}", if *landed { "+" } else { "" }, m.name()))
                .collect();
            eprintln!("{kind:?} {verb:?}: {}", calls.join(" "));
        }
        for &method in Method::ALL {
            // Infallible by signature: there is no failure to inject.
            if method == Method::WriteLockPath {
                continue;
            }
            let calls = control.log.iter().filter(|(m, _)| *m == method).count();
            for nth in 1..=calls {
                run_cell(&template, verb, method, nth, &control);
                cells += 1;
            }
        }
    }
    assert!(cells > 0, "{kind:?}: the matrix generated no cell");
    eprintln!("{kind:?}: {cells} cells");
}

#[test]
fn every_failed_storage_call_leaves_nothing_behind_on_the_folder_backend() {
    run_matrix(Kind::Folder);
}

#[test]
fn every_failed_storage_call_leaves_nothing_behind_on_the_git_branch_backend() {
    run_matrix(Kind::GitBranch);
}

/// A failed commit must not switch drift detection off: the engine
/// records a write as its own only once it is committed, so a sibling's
/// commit after the failure is still seen and reloaded.
#[test]
fn a_failed_commit_is_not_recorded_as_the_engines_own_write() {
    for kind in [Kind::Folder, Kind::GitBranch] {
        let ws = Workspace::template(kind);
        let (mut engine, faults, _backend) = ws.faulted_engine(Peer::Absent);
        faults.arm(Method::CommitWithExpectedParent, 1);
        run(Verb::Update, &mut engine).expect_err("the commit fails");
        faults.reset();

        // A sibling process writes to the same mem.
        run(Verb::Relate, &mut ws.fresh_engine(Peer::Absent)).expect("the sibling's write lands");

        let warnings = engine.reload_if_stale(Some(MEM));
        assert!(
            warnings.iter().any(|w| w.code() == "MEM_RELOADED"),
            "{kind:?}: the sibling's commit went unseen: {warnings:?}"
        );
        assert_eq!(
            view(&engine, true),
            view(&ws.fresh_engine(Peer::Absent), true),
            "{kind:?}: the engine's view differs from a fresh load"
        );
    }
}

/// Two faults in one call: a storage call of the commit path fails, and
/// the discard that should clear what was staged fails as well, once or
/// for good. The staged writes outlive the call, and must not outlive
/// the start of the next operation: no read serves them, the next
/// commit does not land them, and the engine's view stays level with
/// storage. A backend whose discard keeps failing is replaced by a
/// fresh one over the same storage, which has nothing staged.
#[test]
fn a_failed_discard_leaves_no_staged_write_for_the_next_operation() {
    for kind in [Kind::Folder, Kind::GitBranch] {
        let template = Workspace::template(kind);
        let mut cells = 0;
        for verb in all_verbs() {
            let control = control(&template, verb);
            let staging_calls: Vec<(Method, usize)> = [
                Method::WriteEntity,
                Method::DeleteEntity,
                Method::MoveEntity,
                Method::WriteAnchorsSidecar,
                Method::ReadAnchorsSidecar,
                Method::StageProvenance,
                Method::CommitWithExpectedParent,
            ]
            .into_iter()
            .flat_map(|method| {
                let calls = control.log.iter().filter(|(m, _)| *m == method).count();
                (1..=calls).map(move |nth| (method, nth))
            })
            .collect();
            for (method, nth) in staging_calls {
                for discard_fails_for_good in [false, true] {
                    let cell = format!(
                        "{kind:?} {verb:?} {}#{nth} and the discard (for good: \
                         {discard_fails_for_good})",
                        method.name()
                    );
                    let ws = template.copy();
                    let peer = verb.peer();
                    let (mut engine, faults, engine_backend) = ws.faulted_engine(peer);
                    let before = ws.storage_snapshot();

                    faults.arm(method, nth);
                    faults.always_failing(Method::DiscardPending);
                    let result = run(verb, &mut engine);
                    let staged = faults.staged_paths();
                    assert!(result.is_err(), "{cell}: the call succeeded");
                    assert!(
                        changed_keys(&before, &ws.storage_snapshot()).is_empty(),
                        "{cell}: storage changed under a failed call"
                    );

                    // The next operation starts: nothing staged is served.
                    if !discard_fails_for_good {
                        faults.reset();
                    }
                    engine.reload_if_stale(Some(MEM));
                    if !discard_fails_for_good {
                        // The backend the engine staged into is clean
                        // again. (When the discard never succeeds the
                        // engine has left that backend behind.)
                        assert_nothing_staged(&ws, &staged, &engine_backend, &cell);
                    }
                    assert_eq!(
                        view(&engine, true),
                        view(&ws.fresh_engine(peer), true),
                        "{cell}: the engine's view differs from a fresh load"
                    );
                    faults.reset();

                    // And the same call, sent again, is a first call: what
                    // it commits is what an undisturbed call commits.
                    run(verb, &mut engine)
                        .unwrap_or_else(|e| panic!("{cell}: the retry failed: {e}"));
                    assert_eq!(
                        view(&engine, false),
                        control.view,
                        "{cell}: the retry's result differs from an undisturbed call"
                    );
                    assert_eq!(
                        view(&engine, true),
                        view(&ws.fresh_engine(peer), true),
                        "{cell}: after the retry the engine's view differs from a fresh load"
                    );
                    cells += 1;
                }
            }
        }
        assert!(cells > 0);
    }
}

/// A drift probe that fails is not "no drift". A sibling has written to
/// the mem, this engine's probe before its own write fails, and the
/// write carries the hash of the view from before the sibling's write:
/// it must get the honest hash mismatch, not land over the sibling's
/// write.
#[test]
fn a_failed_drift_probe_does_not_let_a_write_land_on_a_stale_view() {
    for kind in [Kind::Folder, Kind::GitBranch] {
        let ws = Workspace::template(kind);
        let (mut engine, faults, _backend) = ws.faulted_engine(Peer::Absent);
        let stale = update_args(&engine, "alpha", "this engine's rewrite");

        run(Verb::Update, &mut ws.fresh_engine(Peer::Absent)).expect("the sibling's write lands");
        let after_sibling = view(&ws.fresh_engine(Peer::Absent), true);

        faults.arm(Method::CurrentHead, 1);
        let err = engine
            .update_entity(stale, Actor::Cli, None, None)
            .expect_err("a write from the view before the sibling's write must not land");
        assert!(faults.fired(), "{kind:?}: the probe was not faulted");
        assert_eq!(err.code(), "HASH_MISMATCH", "{kind:?}: {err}");
        assert_eq!(
            view(&ws.fresh_engine(Peer::Absent), true),
            after_sibling,
            "{kind:?}: the refused write changed storage"
        );
        assert_eq!(
            view(&engine, true),
            after_sibling,
            "{kind:?}: the engine's view is not level with storage"
        );
    }
}

/// The refresh of a checked-out worktree runs after the branch has
/// moved. When it fails, the verb reports the write as done, names the
/// step, and the engine's view is level with storage.
#[cfg(unix)]
#[test]
fn a_failed_worktree_refresh_is_a_warning_on_a_landed_write() {
    use std::os::unix::fs::PermissionsExt;
    let set_mode = |path: &Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap()
    };

    let ws = Workspace::template(Kind::GitBranch);
    let repo = ws._tmp.path().join("mem-repo");
    // Give the repository a worktree with the mem's branch checked out.
    for args in [
        vec!["config", "core.bare", "false"],
        vec!["symbolic-ref", "HEAD", "refs/heads/notes"],
    ] {
        let status = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(repo.join(".git"))
            .args(&args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
    let mut engine = ws.fresh_engine(Peer::Absent);

    // A worktree that takes no file: the commit lands, its checkout
    // cannot.
    set_mode(&repo, 0o555);
    let probe = repo.join("permission-probe");
    if std::fs::write(&probe, b"").is_ok() {
        let _ = std::fs::remove_file(&probe);
        set_mode(&repo, 0o755);
        eprintln!("skipped: this user writes through read-only directories");
        return;
    }
    let result = run(Verb::Update, &mut engine);
    set_mode(&repo, 0o755);

    let codes = result.expect("a write that landed is not reported as failed");
    assert!(
        codes.iter().any(|c| c == "WRITE_FOLLOW_UP_FAILED"),
        "no warning names the failed refresh: {codes:?}"
    );
    assert_eq!(
        view(&engine, true),
        view(&ws.fresh_engine(Peer::Absent), true),
        "the engine's view differs from a fresh load"
    );
    assert!(
        engine.reload_if_stale(Some(MEM)).is_empty(),
        "the engine's own landed write reads as drift"
    );
}
