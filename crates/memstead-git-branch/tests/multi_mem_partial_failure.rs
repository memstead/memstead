//! A pin mismatch on a real git-branch peer after another
//! mem's commit landed, against a storage failure at the same place.
//! Three mems: `left` (the renamed entity's / mem's own), `mid` and
//! `right` (peers, committed in name order).

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use indexmap::IndexMap;
use memstead_base::backend::MemBackend;
use memstead_base::vcs::{Actor, CommitContext};
use memstead_base::{
    CreateEntityArgs, Engine, EngineError, EntityId, Mount, MountCapability, MountLifecycle,
    MountStorage, RenameEntityArgs, UpdateEntityArgs,
};
use memstead_git_branch::storage::instantiate_full_backend;
use memstead_git_branch::test_support::init_real_mem_repo;
use tempfile::TempDir;

use common::{Tap, Tapped};

const LEFT: &str = "left";
const MID: &str = "mid";
const RIGHT: &str = "right";
const MEMS: [&str; 3] = [LEFT, MID, RIGHT];
const SCHEMA: &str = "default@1.0.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Folder,
    GitBranch,
}

struct Workspace {
    _tmp: TempDir,
    mounts: Vec<Mount>,
}

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

impl Workspace {
    fn new(kinds: [Kind; 3]) -> Self {
        let tmp = TempDir::new().unwrap();
        let on_branches: Vec<(&str, &str)> = MEMS
            .iter()
            .zip(kinds)
            .filter(|(_, kind)| *kind == Kind::GitBranch)
            .map(|(mem, _)| (*mem, SCHEMA))
            .collect();
        if !on_branches.is_empty() {
            init_real_mem_repo(tmp.path(), &on_branches);
        }
        let mounts = MEMS
            .iter()
            .zip(kinds)
            .map(|(mem, kind)| {
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
            })
            .collect();
        let ws = Self { _tmp: tmp, mounts };
        let mut engine = ws.engine(None);
        for (mem, title) in [
            (LEFT, "Alpha"),
            (LEFT, "Beta"),
            (MID, "Mu"),
            (RIGHT, "Gamma"),
        ] {
            engine
                .create_entity(create_args(mem, title), Actor::Cli, None, None)
                .unwrap_or_else(|e| panic!("seed {mem}--{title}: {e:?}"));
        }
        for (mem, slug) in [(LEFT, "beta"), (MID, "mu"), (RIGHT, "gamma")] {
            let args = update_args(&engine, &id(mem, slug), "see [[left--alpha]]");
            engine
                .update_entity(args, Actor::Cli, None, None)
                .unwrap_or_else(|e| panic!("link {mem}--{slug}: {e:?}"));
        }
        ws
    }

    fn engine(&self, tap: Option<Tap>) -> Engine {
        let mounts = self
            .mounts
            .iter()
            .map(|mount| {
                let backend = instantiate_full_backend(mount).expect("backend instantiates");
                let backend: Box<dyn MemBackend> = match &tap {
                    Some(tap) => Box::new(Tapped {
                        inner: backend,
                        mem: MEMS.iter().find(|m| **m == mount.mem).unwrap(),
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
        for mem in MEMS {
            settings.cross_mem_links.insert(
                mem.to_string(),
                memstead_schema::workspace_config::CrossLinkValue::Wildcard,
            );
        }
        engine.set_settings(settings);
        engine
    }

    /// Move `mem`'s head through a real backend handle of its own: a
    /// commit of a file that is no entity, so no view changes.
    fn move_head(&self, mem: &str) {
        let mount = self.mounts.iter().find(|m| m.mem == mem).unwrap();
        let backend = instantiate_full_backend(mount).expect("backend instantiates");
        let before = backend.current_head().unwrap();
        backend
            .write_entity(Path::new("side-note.txt"), b"moved by another writer\n")
            .expect("stage the side note");
        backend
            .commit(
                "a commit that did not take the lock",
                &CommitContext::internal(),
            )
            .expect("the head moves");
        assert_ne!(before, backend.current_head().unwrap(), "the head moved");
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

fn purpose(engine: &Engine, target: &EntityId) -> String {
    engine
        .get_entity(target)
        .and_then(|e| e.sections.get("purpose").cloned())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Rename,
    Sweep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// The pinned head of `right` moved before its commit.
    Drift,
    /// The commit into `right` returns a storage error.
    Storage,
}

fn run(op: Op, engine: &mut Engine, hash: Option<String>) -> Result<String, EngineError> {
    match op {
        Op::Rename => engine
            .rename_entity(
                RenameEntityArgs {
                    id: id(LEFT, "alpha"),
                    expected_hash: memstead_base::ExpectedHash::from_token(hash),
                    new_title: "Alpha Renamed".to_string(),
                },
                Actor::Cli,
                None,
                None,
            )
            .map(|o| o.new_id.to_string()),
        Op::Sweep => engine
            .rewrite_mem_references(LEFT, "lefty", None)
            .map(|o| format!("{:?}", o.rewritten_mems)),
    }
}

/// The new link text a landed rewrite leaves in a referrer.
fn new_link(op: Op) -> &'static str {
    match op {
        Op::Rename => "[[left--alpha-renamed]]",
        Op::Sweep => "[[lefty--alpha]]",
    }
}

fn zeta(engine: &mut Engine) {
    engine
        .create_entity(create_args(RIGHT, "Zeta"), Actor::Cli, None, None)
        .expect("an unrelated write into right lands");
}

/// Envelope without hashes and error texts.
fn envelope_shape(err: &EngineError) -> Vec<String> {
    let EngineError::RenamePartialFailure { mems, .. } = err else {
        panic!("no envelope: {err:?}")
    };
    mems.iter()
        .map(|m| {
            let items: Vec<String> = m
                .items
                .iter()
                .map(|i| {
                    format!(
                        "{} landed={} hash={}",
                        i.id,
                        i.landed,
                        i.current_hash.is_some()
                    )
                })
                .collect();
            format!(
                "{} landed={} error={} [{}]",
                m.mem,
                m.landed,
                m.error.is_some(),
                items.join(", ")
            )
        })
        .collect()
}

fn scenario(kinds: [Kind; 3], op: Op, fault: Fault) -> Vec<String> {
    let context = format!("{kinds:?} {op:?} {fault:?}");
    let undisturbed = {
        let ws = Workspace::new(kinds);
        let mut engine = ws.engine(None);
        let hash = engine
            .get_entity(&id(LEFT, "alpha"))
            .map(|e| e.content_hash.clone());
        run(op, &mut engine, hash).expect("an undisturbed call lands");
        zeta(&mut engine);
        shape(&ws.engine(None))
    };

    let ws = Arc::new(Workspace::new(kinds));
    let fired = Arc::new(AtomicBool::new(false));
    let tap: Tap = {
        let (fired, ws) = (Arc::clone(&fired), Arc::clone(&ws));
        Arc::new(move |mem: &str, method: &str| {
            if !matches!(method, "commit" | "commit_with_expected_parent") || mem != RIGHT {
                return false;
            }
            if fired.swap(true, Ordering::SeqCst) {
                return false;
            }
            match fault {
                Fault::Drift => {
                    ws.move_head(RIGHT);
                    false
                }
                Fault::Storage => true,
            }
        })
    };
    let mut engine = ws.engine(Some(tap));
    let sent_hash = engine
        .get_entity(&id(LEFT, "alpha"))
        .map(|e| e.content_hash.clone());
    let err = run(op, &mut engine, sent_hash.clone()).expect_err("the fault fails the call");
    assert!(fired.load(Ordering::SeqCst), "{context}: the fault fired");
    let EngineError::RenamePartialFailure {
        committed_mems,
        failed_mem,
        failure_cause,
        mems,
    } = &err
    else {
        panic!(
            "{context}: expected the partial-failure envelope, got {} {err:?}",
            err.code()
        );
    };
    eprintln!(
        "=== {context}: committed={committed_mems:?} failed={failed_mem} cause={failure_cause}"
    );
    for m in mems {
        eprintln!(
            "    {} landed={} error={:?} items={:?}",
            m.mem, m.landed, m.error, m.items
        );
    }
    assert_eq!(committed_mems, &vec![MID.to_string()], "{context}");
    assert_eq!(failed_mem, RIGHT, "{context}");
    assert_eq!(
        failure_cause,
        match fault {
            Fault::Drift => "drift",
            Fault::Storage => "storage",
        },
        "{context}"
    );

    // Storage, read by a fresh engine, against the envelope.
    let fresh = ws.engine(None);
    assert!(
        purpose(&fresh, &id(MID, "mu")).contains(new_link(op)),
        "{context}: mid landed"
    );
    assert!(
        purpose(&fresh, &id(RIGHT, "gamma")).contains("[[left--alpha]]"),
        "{context}: right did not land"
    );
    assert!(
        purpose(&fresh, &id(LEFT, "beta")).contains("[[left--alpha]]"),
        "{context}: left did not land"
    );
    let alpha = fresh.get_entity(&id(LEFT, "alpha")).filter(|e| !e.stub);
    assert_eq!(
        alpha.map(|e| e.content_hash.clone()),
        sent_hash,
        "{context}: the entity is under the id and hash the caller sent"
    );
    let mut named = Vec::new();
    for m in mems {
        let in_storage = m.mem == MID;
        assert_eq!(m.landed, in_storage, "{context}: {m:?}");
        assert_eq!(m.error.is_some(), m.mem == RIGHT, "{context}: {m:?}");
        for item in &m.items {
            assert_eq!(item.landed, in_storage, "{context}: {item:?}");
            let (mem, slug) = item.id.split_once("--").unwrap();
            assert_eq!(
                item.current_hash,
                fresh
                    .get_entity(&id(mem, slug))
                    .filter(|e| !e.stub)
                    .map(|e| e.content_hash.clone()),
                "{context}: {item:?}"
            );
            named.push(item.id.clone());
        }
    }
    let expected_named = match op {
        Op::Rename => vec!["mid--mu", "right--gamma", "left--alpha", "left--beta"],
        Op::Sweep => vec!["mid--mu", "right--gamma", "left--beta"],
    };
    assert_eq!(
        named, expected_named,
        "{context}: every mem and item the operation writes is named"
    );

    // The running engine's view, as it is, equals the fresh one's.
    assert_eq!(
        view(&engine),
        view(&fresh),
        "{context}: running view == fresh view"
    );
    drop(fresh);

    // Nothing staged on `right`: an unrelated write into it lands
    // without the rewrite of gamma.
    zeta(&mut engine);
    let fresh = ws.engine(None);
    assert!(
        purpose(&fresh, &id(RIGHT, "gamma")).contains("[[left--alpha]]"),
        "{context}: a staged rewrite rode the next commit into right"
    );
    assert_eq!(
        view(&engine),
        view(&fresh),
        "{context}: running view == fresh view after the next write"
    );
    drop(fresh);

    // The identical call, with the identical hash.
    let again = run(op, &mut engine, sent_hash).unwrap_or_else(|e| {
        panic!(
            "{context}: the identical call is refused: {} {e:?}",
            e.code()
        )
    });
    eprintln!("    identical call: {again}");
    assert_eq!(
        shape(&ws.engine(None)),
        undisturbed,
        "{context}: completed == undisturbed"
    );
    assert_eq!(
        view(&engine),
        view(&ws.engine(None)),
        "{context}: running view == fresh view after completion"
    );
    envelope_shape(&err)
}

const KINDS: [[Kind; 3]; 3] = [
    [Kind::GitBranch, Kind::GitBranch, Kind::GitBranch],
    [Kind::Folder, Kind::Folder, Kind::GitBranch],
    [Kind::GitBranch, Kind::Folder, Kind::GitBranch],
];

#[test]
fn rename_pin_mismatch_on_a_real_git_branch_peer() {
    for kinds in KINDS {
        let drift = scenario(kinds, Op::Rename, Fault::Drift);
        let storage = scenario(kinds, Op::Rename, Fault::Storage);
        assert_eq!(
            drift, storage,
            "{kinds:?}: the two faults produce the same envelope"
        );
    }
}

#[test]
fn sweep_pin_mismatch_on_a_real_git_branch_peer() {
    for kinds in KINDS {
        let drift = scenario(kinds, Op::Sweep, Fault::Drift);
        let storage = scenario(kinds, Op::Sweep, Fault::Storage);
        assert_eq!(
            drift, storage,
            "{kinds:?}: the two faults produce the same envelope"
        );
    }
}

// ---------------------------------------------------------------------
// A failure while the failed mem holds staged content (the provenance
// record is refused after the entity writes were staged)
// ---------------------------------------------------------------------

fn staged_scenario(kinds: [Kind; 3], op: Op) {
    let context = format!("{kinds:?} {op:?} records-refused");
    let ws = Arc::new(Workspace::new(kinds));
    let fired = Arc::new(AtomicBool::new(false));
    let tap: Tap = {
        let fired = Arc::clone(&fired);
        Arc::new(move |mem: &str, method: &str| {
            method == "stage_provenance" && mem == RIGHT && !fired.swap(true, Ordering::SeqCst)
        })
    };
    let mut engine = ws.engine(Some(tap));
    let sent_hash = engine
        .get_entity(&id(LEFT, "alpha"))
        .map(|e| e.content_hash.clone());
    let err = run(op, &mut engine, sent_hash.clone()).expect_err("the fault fails the call");
    let EngineError::RenamePartialFailure {
        committed_mems,
        failed_mem,
        failure_cause,
        ..
    } = &err
    else {
        panic!(
            "{context}: expected the envelope, got {} {err:?}",
            err.code()
        );
    };
    eprintln!(
        "=== {context}: committed={committed_mems:?} failed={failed_mem} cause={failure_cause}"
    );
    let fresh = ws.engine(None);
    assert_eq!(view(&engine), view(&fresh), "{context}: running == fresh");
    drop(fresh);
    zeta(&mut engine);
    let fresh = ws.engine(None);
    assert!(
        purpose(&fresh, &id(RIGHT, "gamma")).contains("[[left--alpha]]"),
        "{context}: a staged rewrite rode the next commit into right"
    );
    assert_eq!(
        view(&engine),
        view(&fresh),
        "{context}: running == fresh after the next write"
    );
    drop(fresh);
    run(op, &mut engine, sent_hash).unwrap_or_else(|e| {
        panic!(
            "{context}: the identical call is refused: {} {e:?}",
            e.code()
        )
    });
    assert_eq!(
        view(&engine),
        view(&ws.engine(None)),
        "{context}: running == fresh after completion"
    );
    let done = ws.engine(None);
    assert!(
        purpose(&done, &id(RIGHT, "gamma")).contains(new_link(op)),
        "{context}: completed"
    );
}

#[test]
fn a_failure_with_content_staged_leaves_nothing_staged() {
    for kinds in [
        [Kind::GitBranch, Kind::GitBranch, Kind::GitBranch],
        [Kind::Folder, Kind::Folder, Kind::Folder],
        [Kind::GitBranch, Kind::GitBranch, Kind::Folder],
        [Kind::Folder, Kind::GitBranch, Kind::GitBranch],
    ] {
        staged_scenario(kinds, Op::Rename);
        staged_scenario(kinds, Op::Sweep);
    }
}

// ---------------------------------------------------------------------
// Two faults: the commit into `right` fails, and the reload that
// levels the engine's view of `mid` (whose commit landed) fails too.
// ---------------------------------------------------------------------

fn levelling_scenario(kinds: [Kind; 3], op: Op) -> Vec<String> {
    let context = format!("{kinds:?} {op:?} commit+reload");
    let mut findings = Vec::new();
    let ws = Arc::new(Workspace::new(kinds));
    let stage = Arc::new(std::sync::Mutex::new((0u8, String::new())));
    let tap: Tap = {
        let stage = Arc::clone(&stage);
        Arc::new(move |mem: &str, method: &str| {
            let mut s = stage.lock().unwrap();
            match s.0 {
                0 if mem == RIGHT && matches!(method, "commit" | "commit_with_expected_parent") => {
                    s.0 = 1;
                    true
                }
                1 if mem == MID
                    && matches!(
                        method,
                        "list_entities" | "read_all_entities" | "read_entity"
                    ) =>
                {
                    s.0 = 2;
                    s.1 = method.to_string();
                    true
                }
                _ => false,
            }
        })
    };
    let mut engine = ws.engine(Some(tap));
    let sent_hash = engine
        .get_entity(&id(LEFT, "alpha"))
        .map(|e| e.content_hash.clone());
    let err = run(op, &mut engine, sent_hash.clone()).expect_err("the fault fails the call");
    let (faults, method) = stage.lock().unwrap().clone();
    eprintln!(
        "=== {context}: faults fired = {faults} (second on mid.{method}); code = {}",
        err.code()
    );
    let EngineError::RenamePartialFailure { mems, .. } = &err else {
        findings.push(format!("{context}: no envelope: {} {err}", err.code()));
        return findings;
    };
    for m in mems {
        eprintln!("    {} landed={} items={:?}", m.mem, m.landed, m.items);
    }
    let fresh = ws.engine(None);
    for item in mems.iter().flat_map(|m| &m.items) {
        let (mem, slug) = item.id.split_once("--").unwrap();
        let now = fresh
            .get_entity(&id(mem, slug))
            .filter(|e| !e.stub)
            .map(|e| e.content_hash.clone());
        if now != item.current_hash {
            findings.push(format!(
                "{context}: envelope item {} landed={} current_hash={:?}; storage (fresh engine) has {:?}",
                item.id, item.landed, item.current_hash, now
            ));
        }
    }
    let (running, fresh_view) = (view(&engine), view(&fresh));
    if running != fresh_view {
        let diff: Vec<String> = fresh_view
            .iter()
            .filter(|(k, v)| running.get(*k) != Some(*v))
            .map(|(k, _)| k.clone())
            .collect();
        findings.push(format!("{context}: running view != fresh view at {diff:?}"));
    }
    drop(fresh);
    match run(op, &mut engine, sent_hash) {
        Ok(done) => {
            eprintln!("    identical call: {done}");
            if view(&engine) != view(&ws.engine(None)) {
                findings.push(format!("{context}: running != fresh after completion"));
            }
        }
        Err(e) => findings.push(format!(
            "{context}: the identical call is refused: {} {e}",
            e.code()
        )),
    }
    findings
}

#[test]
fn a_failed_levelling_reload_after_a_partial_failure() {
    let mut findings = Vec::new();
    for kinds in [
        [Kind::GitBranch, Kind::GitBranch, Kind::GitBranch],
        [Kind::Folder, Kind::Folder, Kind::Folder],
    ] {
        findings.extend(levelling_scenario(kinds, Op::Rename));
        findings.extend(levelling_scenario(kinds, Op::Sweep));
    }
    assert!(findings.is_empty(), "{findings:#?}");
}

// ---------------------------------------------------------------------
// The batch forms over three mems: the second commit fails, the third
// was never tried.
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Create,
    Update,
    Relate,
}

fn relate_args(from: &EntityId, to: &EntityId) -> memstead_base::RelateEntityArgs {
    memstead_base::RelateEntityArgs {
        source: from.clone(),
        expected_hash: None,
        rel_type: "USES".to_string(),
        target: to.clone(),
        remove: false,
        description: None,
        dry_run: false,
    }
}

/// The batch's item for `mem`.
fn slug_of(form: Form, mem: &str) -> &'static str {
    match (form, mem) {
        (Form::Create, _) => "delta",
        (_, LEFT) => "beta",
        (_, MID) => "mu",
        _ => "gamma",
    }
}

fn batch(
    form: Form,
    engine: &mut Engine,
    mems: &[&str],
    hashes: &BTreeMap<String, Option<String>>,
) -> Result<memstead_base::ops::BatchResult, EngineError> {
    match form {
        Form::Create => engine.batch_create(
            mems.iter()
                .map(|m| (create_args(m, "Delta"), None))
                .collect(),
            Actor::Cli,
            None,
            false,
        ),
        Form::Update => {
            let updates = mems
                .iter()
                .map(|m| {
                    let target = id(m, slug_of(form, m));
                    let mut args = update_args(engine, &target, "batch text, see [[left--alpha]]");
                    if let Some(h) = hashes.get(&target.to_string()) {
                        args.expected_hash = memstead_base::ExpectedHash::from_token(h.clone());
                    }
                    (args, None)
                })
                .collect();
            engine.batch_update(updates, Actor::Cli, None, false)
        }
        Form::Relate => engine.batch_relate(
            mems.iter()
                .map(|m| {
                    (
                        relate_args(&id(m, slug_of(form, m)), &id(LEFT, "alpha")),
                        None,
                    )
                })
                .collect(),
            Actor::Cli,
            None,
            false,
        ),
    }
}

fn batch_scenario(kinds: [Kind; 3], form: Form) {
    let context = format!("{kinds:?} batch {form:?}");
    let none = BTreeMap::new();
    let undisturbed = {
        let ws = Workspace::new(kinds);
        let mut engine = ws.engine(None);
        let done = batch(form, &mut engine, &MEMS, &none).expect("lands");
        assert!(done.applied, "{context}: undisturbed");
        shape(&ws.engine(None))
    };
    let ws = Arc::new(Workspace::new(kinds));
    let fired = Arc::new(AtomicBool::new(false));
    let order = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let tap: Tap = {
        let (fired, order) = (Arc::clone(&fired), Arc::clone(&order));
        Arc::new(move |mem: &str, method: &str| {
            if !matches!(method, "commit" | "commit_with_expected_parent") {
                return false;
            }
            order.lock().unwrap().push(mem.to_string());
            mem == MID && !fired.swap(true, Ordering::SeqCst)
        })
    };
    let mut engine = ws.engine(Some(tap));
    let err = batch(form, &mut engine, &MEMS, &none).expect_err("the fault fails the batch");
    let EngineError::RenamePartialFailure {
        committed_mems,
        failed_mem,
        failure_cause,
        mems,
    } = &err
    else {
        panic!(
            "{context}: expected the envelope, got {} {err:?}",
            err.code()
        );
    };
    eprintln!(
        "=== {context}: order={:?} committed={committed_mems:?} failed={failed_mem} cause={failure_cause}",
        order.lock().unwrap()
    );
    for m in mems {
        eprintln!(
            "    {} landed={} error={} items={:?}",
            m.mem,
            m.landed,
            m.error.is_some(),
            m.items
        );
    }
    let fresh = ws.engine(None);
    let in_storage = |mem: &str| {
        let target = id(mem, slug_of(form, mem));
        match form {
            Form::Create => fresh.get_entity(&target).is_some_and(|e| !e.stub),
            Form::Update => purpose(&fresh, &target).contains("batch text"),
            Form::Relate => fresh
                .store()
                .outgoing(&target)
                .iter()
                .any(|e| e.rel_type == "USES"),
        }
    };
    let mut seen = Vec::new();
    let mut hashes = BTreeMap::new();
    let mut not_landed: Vec<&str> = Vec::new();
    for m in mems {
        let mem = *MEMS
            .iter()
            .find(|x| **x == m.mem)
            .expect("a mem of the batch");
        seen.push(mem);
        assert_eq!(m.landed, in_storage(mem), "{context}: {m:?}");
        assert_eq!(m.error.is_some(), mem == MID, "{context}: {m:?}");
        assert_eq!(m.items.len(), 1, "{context}: {m:?}");
        for item in &m.items {
            assert_eq!(
                item.id,
                format!("{mem}--{}", slug_of(form, mem)),
                "{context}"
            );
            assert_eq!(item.landed, in_storage(mem), "{context}: {item:?}");
            let (im, slug) = item.id.split_once("--").unwrap();
            assert_eq!(
                item.current_hash,
                fresh
                    .get_entity(&id(im, slug))
                    .filter(|e| !e.stub)
                    .map(|e| e.content_hash.clone()),
                "{context}: {item:?}"
            );
            hashes.insert(item.id.clone(), item.current_hash.clone());
            if !item.landed {
                not_landed.push(mem);
            }
        }
    }
    let mut sorted = seen.clone();
    sorted.sort();
    assert_eq!(sorted, MEMS.to_vec(), "{context}: every mem is named");
    assert_eq!(not_landed.len(), 2, "{context}: two mems did not land");
    assert_eq!(view(&engine), view(&fresh), "{context}: running == fresh");
    drop(fresh);

    // Nothing staged anywhere: an unrelated write into each mem lands
    // alone.
    for mem in MEMS {
        engine
            .create_entity(create_args(mem, "Probe"), Actor::Cli, None, None)
            .expect("an unrelated write lands");
    }
    let fresh = ws.engine(None);
    let check = |mem: &str| {
        let target = id(mem, slug_of(form, mem));
        match form {
            Form::Create => fresh.get_entity(&target).is_some_and(|e| !e.stub),
            Form::Update => purpose(&fresh, &target).contains("batch text"),
            Form::Relate => fresh
                .store()
                .outgoing(&target)
                .iter()
                .any(|e| e.rel_type == "USES"),
        }
    };
    for mem in &not_landed {
        assert!(
            !check(mem),
            "{context}: a staged write rode the next commit into {mem}"
        );
    }
    assert_eq!(
        view(&engine),
        view(&fresh),
        "{context}: running == fresh after the probes"
    );
    drop(fresh);

    // The items the envelope names as not landed, with its hashes.
    let rest = batch(form, &mut engine, &not_landed, &hashes)
        .unwrap_or_else(|e| panic!("{context}: the resend is refused: {} {e:?}", e.code()));
    assert!(
        rest.applied,
        "{context}: the resend applied: {:?}",
        rest.results
            .iter()
            .map(|r| r.error.as_ref().map(|e| e.code.clone()))
            .collect::<Vec<_>>()
    );
    let done = ws.engine(None);
    let mut done_shape = shape(&done);
    done_shape.retain(|k, _| !k.ends_with("--probe"));
    assert_eq!(
        done_shape, undisturbed,
        "{context}: completed == undisturbed"
    );
    assert_eq!(
        view(&engine),
        view(&done),
        "{context}: running == fresh after completion"
    );
}

#[test]
fn a_batch_over_three_mems_whose_second_commit_fails() {
    for kinds in [
        [Kind::GitBranch, Kind::GitBranch, Kind::GitBranch],
        [Kind::Folder, Kind::Folder, Kind::Folder],
        [Kind::Folder, Kind::GitBranch, Kind::Folder],
        [Kind::GitBranch, Kind::Folder, Kind::GitBranch],
    ] {
        for form in [Form::Create, Form::Update, Form::Relate] {
            batch_scenario(kinds, form);
        }
    }
}
