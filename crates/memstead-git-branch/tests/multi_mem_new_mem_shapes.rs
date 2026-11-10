//! The ways a write can refer to another mem's entity, each tried from
//! a mem that a second process creates while a delete, a rename, a
//! retype or a mem rename of that entity's mem is held at its commit;
//! and a fork of a referring mem in the same window. None may be
//! written inside the window: the writer holds the lock of every mem
//! it refers to, so it waits for the verb.

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
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n\n[cross_mem_links]\nleft = \"*\"\nlate = \"*\"\nright = \"*\"\n",
    )
    .unwrap();
    init_real_mem_repo(root, &[(LEFT, SCHEMA)]);
}

#[derive(Debug, Clone, Copy)]
enum LinkBy {
    CreateRelation,
    CreateBody,
    Append,
    DeclareRel,
    Relate,
    BatchCreateRel,
    BatchCreateBody,
    BatchUpdateText,
    BatchUpdateDeclare,
    BatchRelate,
    RelDescription,
    Fork,
    Nothing,
    Spelling(&'static str),
    RelTarget(&'static str),
}

fn rel(target: EntityId) -> memstead_base::ops::RelateArg {
    memstead_base::ops::RelateArg {
        target,
        rel_type: "USES".to_string(),
        description: None,
    }
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

fn second_process(root: &Path, by: LinkBy) -> Vec<String> {
    use memstead_base::mem_management;
    let mut out = Vec::new();
    let mut second = engine_from_workspace_root(root).expect("second boots");
    if matches!(by, LinkBy::Nothing) {
        out.push("link (none)".to_string());
        return out;
    }
    if matches!(by, LinkBy::Fork) {
        let forked = mem_management::fork_mem(
            &mut second,
            mem_management::MemForkParams {
                source: "right".to_string(),
                sha: None,
                name: LATE.to_string(),
                remote: None,
                adopt: false,
                note: None,
                operator_mode: true,
                actor: Actor::Cli,
                client: None,
            },
        );
        out.push(format!(
            "link (fork right -> late): {:?}",
            forked.map(|r| r.name).map_err(|e| format!("{e}"))
        ));
        return out;
    }
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
            storage: Some(if std::env::var("HELD_LATE_FOLDER").is_ok() {
                mem_management::StorageKind::Folder
            } else {
                mem_management::StorageKind::GitBranch
            }),
            actor: Actor::Cli,
            client: None,
        },
    );
    out.push(format!("create_mem ok={}", made.is_ok()));
    if let Err(e) = &made {
        out.push(format!("create_mem error: {e}"));
    }
    let alpha = id(LEFT, "alpha");
    let delta = id(LATE, "delta");
    let linked: Report = match by {
        LinkBy::Fork | LinkBy::Nothing => unreachable!(),
        LinkBy::CreateRelation => {
            let mut args = create_args(LATE, "Delta");
            args.relations = vec![rel(alpha.clone())];
            report(
                second
                    .create_entity(args, Actor::Cli, None, None)
                    .map(|o| o.warnings),
            )
        }
        LinkBy::CreateBody => {
            let mut args = create_args(LATE, "Delta");
            args.sections.insert(
                "purpose".to_string(),
                "Delta purpose, see [[left--alpha]]".to_string(),
            );
            report(
                second
                    .create_entity(args, Actor::Cli, None, None)
                    .map(|o| o.warnings),
            )
        }
        LinkBy::BatchCreateRel => {
            let mut args = create_args(LATE, "Delta");
            args.relations = vec![rel(alpha.clone())];
            batch_report(second.batch_create(
                vec![(args, None), (create_args(LATE, "Eps"), None)],
                Actor::Cli,
                None,
                false,
            ))
        }
        LinkBy::BatchCreateBody => {
            let mut args = create_args(LATE, "Delta");
            args.sections.insert(
                "purpose".to_string(),
                "Delta purpose, see [[left--alpha]]".to_string(),
            );
            batch_report(second.batch_create(
                vec![(create_args(LATE, "Eps"), None), (args, None)],
                Actor::Cli,
                None,
                false,
            ))
        }
        _ => {
            let created = report(
                second
                    .create_entity(create_args(LATE, "Delta"), Actor::Cli, None, None)
                    .map(|o| o.warnings),
            );
            out.push(format!("create late--delta: {created:?}"));
            match by {
                LinkBy::Spelling(s) => {
                    let args = update_args(&second, &delta, &format!("Delta purpose, see {s} end"));
                    report(
                        second
                            .update_entity(args, Actor::Cli, None, None)
                            .map(|o| o.warnings),
                    )
                }
                LinkBy::RelTarget(s) => {
                    let mut args = update_args(&second, &delta, "Delta purpose two");
                    args.declare_relations = vec![rel(EntityId(s.to_string()))];
                    report(
                        second
                            .update_entity(args, Actor::Cli, None, None)
                            .map(|o| o.warnings),
                    )
                }
                LinkBy::Append => {
                    let mut args = update_args(&second, &delta, "x");
                    args.sections.clear();
                    args.append_sections.insert(
                        "purpose".to_string(),
                        "also see [[left--alpha]]".to_string(),
                    );
                    report(
                        second
                            .update_entity(args, Actor::Cli, None, None)
                            .map(|o| o.warnings),
                    )
                }
                LinkBy::DeclareRel => {
                    let mut args = update_args(&second, &delta, "Delta purpose two");
                    args.declare_relations = vec![rel(alpha.clone())];
                    report(
                        second
                            .update_entity(args, Actor::Cli, None, None)
                            .map(|o| o.warnings),
                    )
                }
                LinkBy::RelDescription => {
                    // a relation to an own-mem entity whose description holds the link
                    let c = report(
                        second
                            .create_entity(create_args(LATE, "Eps"), Actor::Cli, None, None)
                            .map(|o| o.warnings),
                    );
                    out.push(format!("create late--eps: {c:?}"));
                    let mut args = update_args(&second, &delta, "Delta purpose two");
                    args.declare_relations = vec![memstead_base::ops::RelateArg {
                        target: id(LATE, "eps"),
                        rel_type: "USES".to_string(),
                        description: Some("as in [[left--alpha]]".to_string()),
                    }];
                    report(
                        second
                            .update_entity(args, Actor::Cli, None, None)
                            .map(|o| o.warnings),
                    )
                }
                LinkBy::Relate => report(
                    second
                        .relate_entity(
                            memstead_base::RelateEntityArgs {
                                source: delta.clone(),
                                expected_hash: None,
                                rel_type: "USES".to_string(),
                                target: alpha.clone(),
                                remove: false,
                                description: None,
                                dry_run: false,
                            },
                            Actor::Cli,
                            None,
                            None,
                        )
                        .map(|o| o.warnings),
                ),
                LinkBy::BatchRelate => batch_report(second.batch_relate(
                    vec![(
                        memstead_base::RelateEntityArgs {
                            source: delta.clone(),
                            expected_hash: None,
                            rel_type: "USES".to_string(),
                            target: alpha.clone(),
                            remove: false,
                            description: None,
                            dry_run: false,
                        },
                        None,
                    )],
                    Actor::Cli,
                    None,
                    false,
                )),
                LinkBy::BatchUpdateText => {
                    let args = update_args(&second, &delta, "Delta purpose, see [[left--alpha]]");
                    batch_report(second.batch_update(vec![(args, None)], Actor::Cli, None, false))
                }
                LinkBy::BatchUpdateDeclare => {
                    let mut args = update_args(&second, &delta, "Delta purpose two");
                    args.declare_relations = vec![rel(alpha.clone())];
                    batch_report(second.batch_update(vec![(args, None)], Actor::Cli, None, false))
                }
                _ => unreachable!(),
            }
        }
    };
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
    tap_mem(engine, LEFT, tap)
}
fn tap_mem(engine: &mut Engine, mem: &'static str, tap: Tap) {
    let mount = engine.mount(mem).unwrap().clone();
    engine.unregister_writable_mem(mem).unwrap();
    engine
        .register_writable_mem(
            mount.clone(),
            Box::new(Tapped {
                inner: instantiate_full_backend(&mount).unwrap(),
                mem,
                tap,
            }) as Box<dyn MemBackend>,
            memstead_base::MemOrigin::RuntimeCreated {
                at: std::time::SystemTime::now(),
                by_tool: "test",
            },
        )
        .unwrap();
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

thread_local! { static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

fn held_verb(verb: &str, by: LinkBy) -> Held {
    ARMED.with(|a| a.set(false));
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
            if !lands(method) || !ARMED.with(|a| a.get()) {
                return false;
            }
            let mut stage = stage.lock().unwrap();
            if stage.is_some() {
                return false;
            }
            eprintln!("HELD first landing call: {_mem} {method}");
            let locked = !lock_is_free(&lock_path);
            let (done, is_done) = std::sync::mpsc::channel::<Vec<String>>();
            let root = root.clone();
            std::thread::spawn(move || {
                let _ = done.send(second_process(&root, by));
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
    tap_left(&mut engine, Arc::clone(&tap));
    if matches!(by, LinkBy::Fork | LinkBy::Nothing) || verb == "memrename" {
        use memstead_base::mem_management;
        mem_management::create_mem(
            &mut engine,
            mem_management::MemCreateParams {
                name: "right".to_string(),
                location: PathBuf::from("right"),
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
        .expect("right created");
        tap_mem(&mut engine, "right", Arc::clone(&tap));
        let mut args = create_args("right", "Gamma");
        args.relations = vec![rel(id(LEFT, "alpha"))];
        engine
            .create_entity(args, Actor::Cli, None, None)
            .expect("seed gamma");
        eprintln!(
            "HELD before tapping right: incoming(left--alpha)={:?}",
            engine
                .store()
                .incoming(&id(LEFT, "alpha"))
                .iter()
                .map(|e| format!("{}<{}", e.rel_type, e.from))
                .collect::<Vec<_>>()
        );
        eprintln!(
            "HELD after tapping right: incoming(left--alpha)={:?} ids={:?}",
            engine
                .store()
                .incoming(&id(LEFT, "alpha"))
                .iter()
                .map(|e| format!("{}<{}", e.rel_type, e.from))
                .collect::<Vec<_>>(),
            engine
                .store()
                .all_entities()
                .map(|e| e.id.to_string())
                .collect::<Vec<_>>()
        );
    }
    ARMED.with(|a| a.set(true));
    eprintln!(
        "HELD after tapping left: incoming(left--alpha)={:?} ids={:?}",
        engine
            .store()
            .incoming(&id(LEFT, "alpha"))
            .iter()
            .map(|e| format!("{}<{}", e.rel_type, e.from))
            .collect::<Vec<_>>(),
        engine
            .store()
            .all_entities()
            .map(|e| format!("{}{}", e.id, if e.stub { "(stub)" } else { "" }))
            .collect::<Vec<_>>()
    );

    let alpha = id(LEFT, "alpha");
    let hash = engine.get_entity(&alpha).map(|e| e.content_hash.clone());
    let outcome = match verb {
        "delete" => engine
            .delete_entity(
                DeleteEntityArgs {
                    id: alpha.clone(),
                    expected_hash: memstead_base::ExpectedHash::from_token(hash),
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
                    expected_hash: memstead_base::ExpectedHash::from_token(hash),
                },
                Actor::Cli,
                None,
                None,
            )
            .map(|_| ())
            .map_err(|e| e.code().to_string()),
        "memrename" => memstead_base::mem_management::rename_mem(
            &mut engine,
            memstead_base::mem_management::MemRenameParams {
                old: LEFT.to_string(),
                new: "lefty".to_string(),
                operator_mode: true,
                note: None,
            },
        )
        .map(|_| ())
        .map_err(|e| format!("{e:?}")),
        "retype" => engine
            .retype_entity(
                memstead_base::RetypeEntityArgs {
                    id: alpha.clone(),
                    expected_hash: memstead_base::ExpectedHash::from_token(hash),
                    target_type: "concept".to_string(),
                    section_map: IndexMap::from_iter([
                        ("identity".to_string(), "definition".to_string()),
                        ("purpose".to_string(), "explanation".to_string()),
                    ]),
                    drop_metadata: vec!["level".to_string()],
                    dry_run: false,
                },
                Actor::Cli,
                None,
                None,
            )
            .map(|_| ())
            .map_err(|e| e.code().to_string()),
        _ => unreachable!(),
    };
    eprintln!("HELD verb outcome: {outcome:?}");
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

const ALL: [LinkBy; 11] = [
    LinkBy::CreateRelation,
    LinkBy::CreateBody,
    LinkBy::Append,
    LinkBy::DeclareRel,
    LinkBy::Relate,
    LinkBy::BatchCreateRel,
    LinkBy::BatchCreateBody,
    LinkBy::BatchUpdateText,
    LinkBy::BatchUpdateDeclare,
    LinkBy::BatchRelate,
    LinkBy::RelDescription,
];

fn run(verb: &str) {
    let mut bad = Vec::new();
    for by in ALL {
        let (outcome, second, locked, inside, real, from, ids) = held_verb(verb, by);
        let line = format!(
            "HELD verb={verb} by={by:?} outcome={outcome:?} left_locked={locked} inside={inside} second={second:?} | fresh: target_real={real} incoming={from:?} ids={ids:?}"
        );
        eprintln!("{line}");
        // A reference to the verb's entity must not be written inside
        // the verb's window: its writer waits for the verb and writes
        // against the entity as the verb left it.
        if inside && !from.is_empty() {
            bad.push(line);
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn delete() {
    run("delete");
}
#[test]
fn rename() {
    run("rename");
}
#[test]
fn retype() {
    run("retype");
}

#[test]
fn memrename() {
    run("memrename");
}

#[test]
fn nothing_during_rename_control() {
    let (outcome, second, locked, inside, real, from, ids) = held_verb("rename", LinkBy::Nothing);
    eprintln!(
        "HELD-CONTROL rename outcome={outcome:?} left_locked={locked} inside={inside} second={second:?} | fresh: left--alpha real={real} incoming={from:?} ids={ids:?}"
    );
    assert!(
        from.is_empty(),
        "a reference to the old id is left: {from:?} {ids:?}"
    );
}

#[test]
fn fork_during_rename() {
    let (outcome, second, locked, inside, real, from, ids) = held_verb("rename", LinkBy::Fork);
    eprintln!(
        "HELD-FORK rename outcome={outcome:?} left_locked={locked} inside={inside} second={second:?} | fresh: left--alpha real={real} incoming={from:?} ids={ids:?}"
    );
    assert!(
        from.is_empty(),
        "a reference to the old id is left: {from:?} {ids:?}"
    );
}

#[test]
fn sequential_control() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    workspace(&root);
    let mut engine = engine_from_workspace_root(&root).expect("engine boots");
    let mut args = create_args(LEFT, "Delta");
    args.relations = vec![rel(id(LEFT, "ghost"))];
    let r = engine
        .create_entity(args, Actor::Cli, None, None)
        .map(|o| o.warnings);
    eprintln!(
        "HELD-SEQ create with relation to a missing id: {:?}",
        report(r)
    );
    let mut args = update_args(&engine, &id(LEFT, "delta"), "p2");
    args.declare_relations = vec![rel(id(LEFT, "ghost2"))];
    let r = engine
        .update_entity(args, Actor::Cli, None, None)
        .map(|o| o.warnings);
    eprintln!(
        "HELD-SEQ update declaring relation to a missing id: {:?}",
        report(r)
    );
    let args = update_args(&engine, &id(LEFT, "delta"), "see [[left--ghost3]]");
    let r = engine
        .batch_update(vec![(args, None)], Actor::Cli, None, false)
        .unwrap();
    eprintln!(
        "HELD-SEQ batch update body link to a missing id: applied={} warnings={:?} entries={:?}",
        r.applied,
        r.warnings
            .iter()
            .map(|w| w.code().to_string())
            .collect::<Vec<_>>(),
        r.results
            .iter()
            .map(|e| format!("{:?}", e))
            .collect::<Vec<_>>()
    );
}

#[test]
fn spellings() {
    let mut bad = Vec::new();
    for by in [
        LinkBy::Spelling("[[ left--alpha]]"),
        LinkBy::Spelling("[[left--alpha ]]"),
        LinkBy::Spelling("[[left:alpha]]"),
        LinkBy::Spelling("[[left--alpha|label]]"),
        LinkBy::Spelling("[[left--alpha#purpose]]"),
        LinkBy::Spelling("[[left--alpha.md]]"),
        LinkBy::Spelling("[[LEFT--alpha]]"),
        LinkBy::Spelling("[[left--Alpha]]"),
        LinkBy::Spelling("[[left::alpha]]"),
        LinkBy::Spelling("[[left/alpha]]"),
        LinkBy::Spelling("[[left--alpha\n]]"),
        LinkBy::Spelling("\\[[left--alpha]]"),
        LinkBy::Spelling("<!-- x --> [[left--alpha]]"),
        LinkBy::RelTarget("left:alpha"),
        LinkBy::RelTarget("LEFT--alpha"),
        LinkBy::RelTarget(" left--alpha"),
        LinkBy::RelTarget("left--alpha.md"),
        LinkBy::RelTarget("alpha"),
    ] {
        let (outcome, second, locked, inside, real, from, ids) = held_verb("delete", by);
        let line = format!(
            "HELD-SPELL by={by:?} outcome={outcome:?} left_locked={locked} inside={inside} second={second:?} | fresh: target_real={real} incoming={from:?} ids={ids:?}"
        );
        eprintln!("{line}");
        if inside && !from.is_empty() {
            bad.push(line);
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
