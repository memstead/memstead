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
//! The method list is checked against the trait's source, so a method
//! added to the trait fails `the_method_list_is_the_trait` until it
//! joins the matrix; the verb list is tied to `ProvenanceKind` by an
//! exhaustive match, so a new kind does not compile until it is placed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use memstead_base::backend::{BackendError, CompletedCommit, EntityRead, MemBackend};
use memstead_base::provenance::ProvenanceKind;
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
        .filter_map(|line| line.strip_prefix("    fn "))
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
    fired: bool,
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

    fn fired(&self) -> bool {
        self.inner.lock().unwrap().fired
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
        let ws = Self::over(tmp, kind);
        ws.seed();
        ws
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

    /// An engine loaded from storage, with no fault wrapper.
    fn fresh_engine(&self) -> Engine {
        let mut engine = Engine::from_mounts(vec![(self.mount.clone(), self.raw_backend())])
            .expect("engine boots");
        engine.set_git_branch_ops(memstead_git_branch::storage::FULL_GIT_BRANCH_OPS);
        engine
    }

    /// An engine whose backend fails on command, plus the handle that
    /// commands it and the backend instance the engine stages into.
    fn faulted_engine(&self) -> (Engine, Arc<Faults>, Arc<dyn MemBackend>) {
        let inner: Arc<dyn MemBackend> = Arc::from(self.raw_backend());
        let faults = Arc::new(Faults::default());
        let wrapper = FaultBackend {
            inner: Arc::clone(&inner),
            faults: Arc::clone(&faults),
        };
        let mut engine = Engine::from_mounts(vec![(
            self.mount.clone(),
            Box::new(wrapper) as Box<dyn MemBackend>,
        )])
        .expect("engine boots");
        engine.set_git_branch_ops(memstead_git_branch::storage::FULL_GIT_BRANCH_OPS);
        (engine, faults, inner)
    }

    fn seed(&self) {
        let mut engine = self.fresh_engine();
        for title in ["Alpha", "Beta", "Gamma"] {
            engine
                .create_entity(create_args(title, &[]), Actor::Cli, None, None)
                .unwrap_or_else(|e| panic!("seed {title}: {e:?}"));
        }
        engine
            .relate_entity(relate_args("alpha", "beta"), Actor::Cli, None, None)
            .expect("seed relation");
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
    Update,
    Delete,
    Relate,
    Retype,
    Rename,
    BatchCreate,
    BatchUpdate,
    BatchRelate,
}

/// Every kind of mutation the engine records, with the verbs that
/// produce it. Exhaustive on purpose: a kind added to the engine does
/// not compile here until its verbs are named.
fn verbs_of(kind: ProvenanceKind) -> &'static [Verb] {
    match kind {
        ProvenanceKind::Create => &[Verb::Create, Verb::BatchCreate],
        ProvenanceKind::Update => &[Verb::Update, Verb::BatchUpdate],
        ProvenanceKind::Delete => &[Verb::Delete],
        ProvenanceKind::Relate => &[Verb::Relate, Verb::BatchRelate],
        ProvenanceKind::Rename => &[Verb::Rename],
        ProvenanceKind::Retype => &[Verb::Retype],
        // The batch forms record one line per item under the item's
        // own kind; no verb records this kind by itself.
        ProvenanceKind::Batch => &[],
    }
}

fn all_verbs() -> Vec<Verb> {
    [
        ProvenanceKind::Create,
        ProvenanceKind::Update,
        ProvenanceKind::Delete,
        ProvenanceKind::Relate,
        ProvenanceKind::Rename,
        ProvenanceKind::Retype,
        ProvenanceKind::Batch,
    ]
    .into_iter()
    .flat_map(|kind| verbs_of(kind).iter().copied())
    .collect()
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
        anchors: Vec::new(),
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
fn run(verb: Verb, engine: &mut Engine) -> Result<Vec<String>, EngineError> {
    let (actor, note) = (Actor::Cli, Some("matrix"));
    let warnings = match verb {
        Verb::Create => {
            engine
                .create_entity(create_args("Delta", &["alpha"]), actor, None, note)?
                .warnings
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
            let result = engine.batch_create(creates, actor, None, false)?;
            assert!(result.applied, "batch create applies: {result:?}");
            result.warnings
        }
        Verb::BatchUpdate => {
            let updates = vec![
                (
                    update_args(engine, "alpha", "Alpha purpose, batch"),
                    Some("first".to_string()),
                ),
                (update_args(engine, "gamma", "Gamma purpose, batch"), None),
            ];
            let result = engine.batch_update(updates, actor, None, false)?;
            assert!(result.applied, "batch update applies: {result:?}");
            result.warnings
        }
        Verb::BatchRelate => {
            let relates = vec![
                (relate_args("gamma", "alpha"), Some("first".to_string())),
                (relate_args("gamma", "beta"), None),
            ];
            let result = engine.batch_relate(relates, actor, None, false)?;
            assert!(result.applied, "batch relate applies: {result:?}");
            result.warnings
        }
    };
    Ok(warnings.iter().map(|w| w.code().to_string()).collect())
}

// ---------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------

/// The engine's view of the mem: every entity with its hash and its
/// relations in both directions, plus what a search returns. `hashes`
/// off drops the content hash, for comparing two runs whose bytes
/// differ only in the timestamps the engine stamps.
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
                "stub={} type={} title={:?} hash={} out={:?} in={:?}",
                entity.stub, entity.entity_type, entity.title, hash, outgoing, incoming
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
            .map(|r| r.hits.iter().map(|h| h.id.to_string()).collect())
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
/// - `current_head`, `write_lock_path`: drift detection is best-effort
///   and never blocks the operation it accompanies (the trait says so).
/// - `read_mem_config`, `write_mem_config_cas`, `write_mem_config`: the
///   version stamp after a commit is best-effort; a failed stamp never
///   fails the mutation before it.
/// - `discard_pending`: called on exits only; on a successful call
///   nothing is staged and its failure has nothing to leave behind.
const ABSORBED: &[Method] = &[
    Method::CurrentHead,
    Method::WriteLockPath,
    Method::ReadMemConfig,
    Method::WriteMemConfigCas,
    Method::WriteMemConfig,
    Method::DiscardPending,
];

struct Control {
    /// Calls the undisturbed verb makes, split at the commit.
    log: Vec<(Method, bool)>,
    view: BTreeMap<String, String>,
}

fn control(template: &Workspace, verb: Verb) -> Control {
    let (kind, ws) = (template.kind, template.copy());
    let (mut engine, faults, _inner) = ws.faulted_engine();
    faults.reset();
    run(verb, &mut engine).unwrap_or_else(|e| panic!("{kind:?} {verb:?} undisturbed: {e:?}"));
    let log = faults.log();
    assert_eq!(
        view(&engine, true),
        view(&ws.fresh_engine(), true),
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
    let (mut engine, faults, engine_backend) = ws.faulted_engine();
    let before = ws.storage_snapshot();

    faults.arm(method, nth);
    let result = run(verb, &mut engine);
    assert!(faults.fired(), "{cell}: the fault never fired");
    let landed = faults.log().iter().any(|(_, landed)| *landed);
    let staged = faults.staged_paths();
    faults.reset();

    match (&result, landed) {
        // Before the commit landed, refused: nothing is left behind and
        // the same call sent again is a first call.
        (Err(e), false) => {
            assert!(!e.code().is_empty(), "{cell}: the error carries no code");
            let changed = changed_keys(&before, &ws.storage_snapshot());
            assert!(
                changed.is_empty(),
                "{cell}: storage changed under a failed call ({e}): {changed:?}"
            );
            assert_nothing_staged(&ws, &staged, &engine_backend, &cell);
            assert_eq!(
                view(&engine, true),
                view(&ws.fresh_engine(), true),
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
                view(&ws.fresh_engine(), true),
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
                view(&ws.fresh_engine(), true),
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
                view(&ws.fresh_engine(), true),
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
