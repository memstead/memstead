//! `fork_mem` on the git-branch backend: a mem forks from another
//! mem's branch at a recorded ancestor.
//!
//! AC1 (local form): the branch starts at the source's tip or at a
//! given sha the source reaches, the config is the source's with
//! `forkedFrom` written in and the source's cursors left behind, the
//! entities read identical, the source's cross-link grants are
//! inherited, and every refusal lands nothing.
//!
//! AC2 (remote form): the source branch and config come from a second
//! bare mem-repo declared as a remote, the fetched tree is validated
//! before the mount exists, no grant is inherited, the local
//! `__MEMSTEAD` is never fetched over, and an unresolvable pin refuses
//! naming `memstead schema install`.
//!
//! The fork commit (plan-proposal 01): the fork's branch carries
//! exactly one commit above the ancestor, made by the fork itself,
//! that moves the anchors and derivations sidecar ids and the
//! mem-qualified self-links from the source's name to the fork's, with
//! hashes, spans and observations unchanged; its sha is
//! `forkedFrom.base`, the ancestor stays `forkedFrom.sha`. A tree with
//! nothing to move still gets the commit; a failure after it rolls
//! everything back; a fork recorded without a base reads as based on
//! its ancestor.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use memstead_base::anchor::{AnchorInput, AnchorSidecar, ObservedArtifactHash};
use memstead_base::backend::MemBackend;
use memstead_base::check::{CheckKind, Verdict};
use memstead_base::derivation::{DERIVATION_SIDECAR_PATH, DerivationSidecar};
use memstead_base::mem_management::{self, MemForkParams, StorageKind};
use memstead_base::vcs::Actor;
use memstead_base::{CreateEntityArgs, EntityId, FullEngineError};
use memstead_git_branch::mem_repo_config::{commit_config_at_gitdir, read_config_at_gitdir};
use memstead_git_branch::ops::transport::{
    push_in_gitdir, read_md_blobs_at_ref, remote_add_in_gitdir, resolve_ref_in_gitdir,
};
use memstead_git_branch::storage::git_tree::GitTreeBackend;
use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::vcs::CommitContext;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use memstead_schema::workspace_config::CrossLinkValue;
use tempfile::TempDir;

const WORKSPACE_HEAD: &str = "format = \"memstead-git-branch-2\"\n\n\
     [persistence_adapter]\nname = \"file-two-layer\"\n\n";

fn seed_sections() -> indexmap::IndexMap<String, String> {
    let mut sections = indexmap::IndexMap::new();
    sections.insert("identity".to_string(), "seed identity".to_string());
    sections.insert("purpose".to_string(), "seed purpose".to_string());
    sections
}

fn create_entity_in(engine: &mut memstead_base::Engine, mem: &str, title: &str) {
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: mem.to_string(),
                title: title.to_string(),
                entity_type: "spec".to_string(),
                sections: seed_sections(),
                metadata: Default::default(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("create entity {title:?} in mem {mem:?}: {e:?}"));
}

/// An entity with the given `identity` section text and anchors.
fn create_entity_with(
    engine: &mut memstead_base::Engine,
    mem: &str,
    title: &str,
    identity: &str,
    anchors: Vec<AnchorInput>,
) {
    let mut sections = seed_sections();
    sections.insert("identity".to_string(), identity.to_string());
    engine
        .create_entity(
            CreateEntityArgs {
                anchors,
                mem: mem.to_string(),
                title: title.to_string(),
                entity_type: "spec".to_string(),
                sections,
                metadata: Default::default(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("create entity {title:?} in mem {mem:?}: {e:?}"));
}

/// The parents of a commit, the fork commit's contract being exactly
/// one: the ancestor.
fn parents_of(gitdir: &Path, sha: &str) -> Vec<String> {
    let repo = gix::open(gitdir).unwrap();
    let id = repo.rev_parse_single(sha).unwrap();
    id.object()
        .unwrap()
        .try_into_commit()
        .unwrap()
        .parent_ids()
        .map(|p| p.to_string())
        .collect()
}

/// A commit's full message and its committer name.
fn commit_message_and_committer(gitdir: &Path, sha: &str) -> (String, String) {
    let repo = gix::open(gitdir).unwrap();
    let id = repo.rev_parse_single(sha).unwrap();
    let commit = id.object().unwrap().try_into_commit().unwrap();
    let message = String::from_utf8_lossy(commit.message_raw().expect("message")).to_string();
    let committer = commit.committer().expect("committer").name.to_string();
    (message, committer)
}

/// A blob of the tree at `ref_name`, by path.
fn blob_at(gitdir: &Path, ref_name: &str, path: &str) -> Option<Vec<u8>> {
    GitTreeBackend::new(gitdir.to_path_buf(), ref_name.to_string())
        .read_entity(Path::new(path))
        .unwrap()
}

fn relate(engine: &mut memstead_base::Engine, from: (&str, &str), to: (&str, &str)) {
    engine
        .relate_entity(
            memstead_base::RelateEntityArgs {
                source: memstead_base::EntityId::new(from.0, from.1),
                expected_hash: None,
                rel_type: "DEPENDS_ON".to_string(),
                target: memstead_base::EntityId::new(to.0, to.1),
                remove: false,
                description: None,
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("relate {from:?} -> {to:?}: {e:?}"));
}

fn gitdir_of(root: &Path) -> PathBuf {
    root.join("mem-repo").join(".git").canonicalize().unwrap()
}

fn sha_of(gitdir: &Path, ref_name: &str) -> Option<String> {
    resolve_ref_in_gitdir(gitdir, ref_name).unwrap()
}

fn workspace_toml(root: &Path) -> String {
    std::fs::read_to_string(root.join(".memstead/workspace.toml")).unwrap()
}

fn params(source: &str, name: &str) -> MemForkParams {
    MemForkParams {
        source: source.to_string(),
        sha: None,
        name: name.to_string(),
        remote: None,
        note: None,
        operator_mode: true,
        actor: Actor::Cli,
        client: None,
    }
}

/// One entity's shape with the mem prefix stripped from its own and
/// its targets' ids, so a source and its fork compare as equal.
type Shape = (String, String, Vec<(String, String)>, Vec<(String, String)>);

fn shapes(engine: &memstead_base::Engine, mem: &str) -> Vec<Shape> {
    let mut out: Vec<Shape> = engine
        .store()
        .all_entities()
        .filter(|e| e.id.mem() == mem && !e.stub)
        .map(|e| {
            let sections = e
                .sections
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let rels = e
                .relationships
                .iter()
                .map(|r| {
                    let target = if r.target.mem() == mem {
                        r.target.path().to_string()
                    } else {
                        r.target.to_string()
                    };
                    (r.rel_type.clone(), target)
                })
                .collect();
            (e.file_path.clone(), e.entity_type.clone(), sections, rels)
        })
        .collect();
    out.sort();
    out
}

/// Every blob of the tree at `ref_name`, path to object id.
fn tree_blobs(gitdir: &Path, ref_name: &str) -> BTreeMap<String, String> {
    let repo = gix::open(gitdir).unwrap();
    let id = repo.rev_parse_single(ref_name).unwrap();
    let tree = id
        .object()
        .unwrap()
        .try_into_commit()
        .unwrap()
        .tree()
        .unwrap();
    let entries = tree.traverse().breadthfirst.files().unwrap();
    entries
        .into_iter()
        .filter(|e| e.mode.is_blob())
        .map(|e| {
            (
                String::from_utf8(e.filepath.to_vec()).unwrap(),
                e.oid.to_string(),
            )
        })
        .collect()
}

/// Nothing of `name` exists: no branch, no config blob, no policy line,
/// no mount.
fn assert_nothing_landed(engine: &memstead_base::Engine, root: &Path, name: &str) {
    let gitdir = gitdir_of(root);
    assert_eq!(
        sha_of(&gitdir, &format!("refs/heads/{name}")),
        None,
        "no branch for {name}"
    );
    assert!(
        read_config_at_gitdir(&gitdir, name).is_err(),
        "no config blob for {name}"
    );
    assert!(
        !workspace_toml(root).contains(name),
        "no policy line for {name}: {}",
        workspace_toml(root)
    );
    assert!(
        !engine.mem_names().contains(&name),
        "no mount for {name}: {:?}",
        engine.mem_names()
    );
}

fn code_of(err: &FullEngineError) -> &'static str {
    err.code()
}

// ---------------------------------------------------------------------
// AC1: the local form
// ---------------------------------------------------------------------

/// A source with two entities, a cross-mem edge under a grant, a
/// description and a version, forked at its tip under the create
/// rules: the fork's branch is the source's commit, its config is the
/// source's plus the origin, its entities read identical, it carries
/// the source's grant under its own name (on disk and in the running
/// engine), it survives a reboot, and a later write moves only its
/// own branch.
#[test]
fn local_fork_starts_at_the_sources_tip_with_its_config_entities_and_grants() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
    std::fs::write(
        tmp.path().join(".memstead/workspace.toml"),
        format!(
            "{WORKSPACE_HEAD}[[mem_management.create]]\npattern = \"*\"\nschemas = [\"*\"]\n\n\
             [cross_mem_links]\nspecs = [\"plans\"]\n"
        ),
    )
    .unwrap();
    init_real_mem_repo(
        tmp.path(),
        &[("specs", "default@1.0.0"), ("plans", "default@1.0.0")],
    );
    let gitdir = gitdir_of(tmp.path());
    // The source's config carries a description and a version the
    // fork must copy.
    commit_config_at_gitdir(
        &gitdir,
        "specs",
        br#"{"schema": "default@1.0.0", "version": "0.4.0", "description": "the specs"}"#,
        &CommitContext::internal(),
        "seed",
    )
    .unwrap();
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    create_entity_in(&mut engine, "plans", "Roadmap");
    create_entity_in(&mut engine, "specs", "Alpha");
    create_entity_in(&mut engine, "specs", "Beta");
    relate(&mut engine, ("specs", "beta"), ("specs", "alpha"));
    relate(&mut engine, ("specs", "alpha"), ("plans", "roadmap"));
    let source_tip = sha_of(&gitdir, "refs/heads/specs").unwrap();
    let plans_tip = sha_of(&gitdir, "refs/heads/plans").unwrap();

    let mut p = params("specs", "specs-fork");
    p.operator_mode = false;
    let response = mem_management::fork_mem(&mut engine, p).expect("local fork lands");

    // The response names what was created and where it came from.
    assert_eq!(response.name, "specs-fork");
    assert_eq!(response.branch_ref, "refs/heads/specs-fork");
    assert_eq!(response.forked_from.mem, "specs");
    assert_eq!(response.forked_from.sha, source_tip);
    assert_eq!(response.forked_from.remote, None);
    assert_eq!(response.schema_ref.to_string(), "default@1.0.0");
    assert_eq!(
        response.inherited_grants,
        Some(CrossLinkValue::List(vec!["plans".to_string()]))
    );

    // The branch is the fork commit right above the source's commit:
    // one commit, whose only parent is the ancestor, and the base the
    // response and the config record.
    let fork_tip = sha_of(&gitdir, "refs/heads/specs-fork").unwrap();
    assert_ne!(fork_tip, source_tip, "the fork commit is the fork's own");
    assert_eq!(parents_of(&gitdir, &fork_tip), vec![source_tip.clone()]);
    assert_eq!(
        response.forked_from.base.as_deref(),
        Some(fork_tip.as_str())
    );
    // The config on __MEMSTEAD is the source's plus the origin.
    let cfg = read_config_at_gitdir(&gitdir, "specs-fork").unwrap();
    assert_eq!(cfg.schema.unwrap().to_string(), "default@1.0.0");
    assert_eq!(cfg.version.unwrap().to_string(), "0.4.0");
    assert_eq!(cfg.description.as_deref(), Some("the specs"));
    let origin = cfg.forked_from.expect("origin recorded");
    assert_eq!(origin.mem, "specs");
    assert_eq!(origin.sha, source_tip);
    assert_eq!(origin.remote, None);
    assert_eq!(origin.base.as_deref(), Some(fork_tip.as_str()));
    assert_eq!(origin.base_sha(), fork_tip);
    // The source's own config is untouched by the fork.
    let source_cfg = read_config_at_gitdir(&gitdir, "specs").unwrap();
    assert!(source_cfg.forked_from.is_none());

    // The entities, relationships included, read identical.
    assert_eq!(shapes(&engine, "specs-fork"), shapes(&engine, "specs"));
    assert_eq!(shapes(&engine, "specs-fork").len(), 2);

    // The grant rides under the fork's name: on disk through the
    // policy writer, and in the running engine.
    let toml = workspace_toml(tmp.path());
    assert!(
        toml.contains("specs-fork = [\"plans\"]"),
        "inherited grant written: {toml}"
    );
    assert!(
        toml.contains("specs = [\"plans\"]"),
        "source keeps its grant"
    );
    assert!(engine.cross_mem_link_allowed("specs-fork", "plans"));
    assert!(
        engine.mem_names().contains(&"specs-fork"),
        "{:?}",
        engine.mem_names()
    );

    // A later write to the fork moves only the fork's branch.
    create_entity_in(&mut engine, "specs-fork", "Gamma");
    assert_ne!(
        sha_of(&gitdir, "refs/heads/specs-fork").unwrap(),
        fork_tip,
        "the fork's branch moved"
    );
    assert_eq!(
        sha_of(&gitdir, "refs/heads/specs").unwrap(),
        source_tip,
        "the source's branch did not"
    );
    assert_eq!(sha_of(&gitdir, "refs/heads/plans").unwrap(), plans_tip);
    drop(engine);

    // A reboot loads the fork from the persisted mount with its origin
    // and its edge conformant under the inherited grant.
    let engine = engine_from_workspace_root(tmp.path()).expect("reboot");
    let cfg = engine.mem_config_for("specs-fork").expect("config loaded");
    assert_eq!(cfg.forked_from.as_ref().unwrap().sha, source_tip);
    assert_eq!(
        cfg.forked_from.as_ref().unwrap().base.as_deref(),
        Some(fork_tip.as_str())
    );
    assert_eq!(shapes(&engine, "specs-fork").len(), 3);
    assert!(engine.cross_mem_link_allowed("specs-fork", "plans"));
}

/// `<source>@<sha>`: the fork starts at the named commit, which the
/// source branch reaches, and carries only what that commit carried.
#[test]
fn local_fork_at_a_given_sha_starts_there() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let gitdir = gitdir_of(tmp.path());
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    create_entity_in(&mut engine, "specs", "Alpha");
    let first = sha_of(&gitdir, "refs/heads/specs").unwrap();
    create_entity_in(&mut engine, "specs", "Beta");
    let tip = sha_of(&gitdir, "refs/heads/specs").unwrap();
    assert_ne!(first, tip);

    let mut p = params("specs", "specs-early");
    // An abbreviated sha resolves like a full one.
    p.sha = Some(first[..12].to_string());
    let response = mem_management::fork_mem(&mut engine, p).expect("fork at sha lands");
    assert_eq!(response.forked_from.sha, first);
    // The fork commit sits right above the named commit, not the tip.
    let fork_tip = sha_of(&gitdir, "refs/heads/specs-early").unwrap();
    assert_eq!(parents_of(&gitdir, &fork_tip), vec![first.clone()]);
    assert_eq!(
        response.forked_from.base.as_deref(),
        Some(fork_tip.as_str())
    );
    let names: Vec<String> = shapes(&engine, "specs-early")
        .into_iter()
        .map(|s| s.0)
        .collect();
    assert_eq!(names, vec!["alpha.md".to_string()]);
    assert_eq!(shapes(&engine, "specs").len(), 2, "the source keeps both");
}

/// A source with no grants forks with none; the source's sync state
/// and review mark are its own cursors and are not copied.
#[test]
fn fork_of_a_mem_without_grants_adds_none_and_copies_no_cursor() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("plans", "default@1.0.0")]);
    let gitdir = gitdir_of(tmp.path());
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    create_entity_in(&mut engine, "plans", "Roadmap");
    let tip = sha_of(&gitdir, "refs/heads/plans").unwrap();
    engine
        .set_mem_sync_state("plans", "docs/tree#synced", "abc123", None)
        .unwrap();
    engine.set_review_mark("plans", Some(&tip), None).unwrap();
    let source_cfg = read_config_at_gitdir(&gitdir, "plans").unwrap();
    assert!(!source_cfg.sync_state.is_empty());
    assert_eq!(source_cfg.review_mark.as_deref(), Some(tip.as_str()));
    let toml_before = workspace_toml(tmp.path());

    let response =
        mem_management::fork_mem(&mut engine, params("plans", "plans-fork")).expect("fork lands");
    assert_eq!(response.inherited_grants, None);
    assert_eq!(
        workspace_toml(tmp.path()),
        toml_before,
        "no policy line for a fork of a mem without grants"
    );
    let cfg = read_config_at_gitdir(&gitdir, "plans-fork").unwrap();
    assert!(
        cfg.sync_state.is_empty(),
        "sync state is the source's cursor"
    );
    assert!(
        cfg.review_mark.is_none(),
        "the review mark is the source's cursor"
    );
    assert!(cfg.unregistered_at.is_none());
    assert_eq!(cfg.forked_from.unwrap().mem, "plans");
}

/// Every local refusal is typed and lands nothing: no branch, no
/// config blob, no policy line, no mount.
#[test]
fn local_fork_refusals_land_nothing() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
    std::fs::write(
        tmp.path().join(".memstead/workspace.toml"),
        format!(
            "{WORKSPACE_HEAD}[[mem_management.create]]\npattern = \"proposals/*\"\n\
             schemas = [\"*\"]\n\n[[mem_management.create]]\npattern = \"planning-only/*\"\n\
             schemas = [\"planning@0.1.0\"]\n\n[cross_mem_links]\nspecs = \"*\"\n"
        ),
    )
    .unwrap();
    init_real_mem_repo(
        tmp.path(),
        &[("specs", "default@1.0.0"), ("plans", "default@1.0.0")],
    );
    let gitdir = gitdir_of(tmp.path());
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    create_entity_in(&mut engine, "specs", "Alpha");
    create_entity_in(&mut engine, "plans", "Roadmap");
    let plans_tip = sha_of(&gitdir, "refs/heads/plans").unwrap();
    // A folder mem beside the git-branch mems, for the source-kind refusal.
    mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            name: "notes".to_string(),
            location: PathBuf::from("notes"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            vcs: None,
            note: None,
            operator_mode: true,
            recovery: None,
            write_guidance: Default::default(),
            storage: Some(StorageKind::Folder),
            actor: Actor::Cli,
            client: None,
        },
    )
    .expect("folder mem");
    let memstead_before = sha_of(&gitdir, "refs/heads/__MEMSTEAD").unwrap();

    // A sha not reachable from the source branch.
    let mut p = params("specs", "proposals/wrong-sha");
    p.sha = Some(plans_tip.clone());
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "UNKNOWN_REF", "{err}");
    assert!(err.to_string().contains("refs/heads/specs"), "{err}");
    assert_nothing_landed(&engine, tmp.path(), "proposals/wrong-sha");

    // A sha nothing resolves.
    let mut p = params("specs", "proposals/no-sha");
    p.sha = Some("deadbeef".to_string());
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "UNKNOWN_REF", "{err}");
    assert_nothing_landed(&engine, tmp.path(), "proposals/no-sha");

    // A full 40-hex sha that names no object: git's rev-parse accepts
    // the spelling, the object store does not hold it. Typed, naming
    // the sha and the source branch.
    let mut p = params("specs", "proposals/no-object");
    p.sha = Some("0000000000000000000000000000000000000001".to_string());
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "UNKNOWN_REF", "{err}");
    let msg = err.to_string();
    assert!(
        msg.contains("0000000000000000000000000000000000000001")
            && msg.contains("refs/heads/specs"),
        "{msg}"
    );
    assert_nothing_landed(&engine, tmp.path(), "proposals/no-object");

    // A malformed name.
    let err = mem_management::fork_mem(&mut engine, params("specs", "Specs Fork")).unwrap_err();
    assert_eq!(code_of(&err), "INVALID_MEM_NAME", "{err}");

    // A name the create rules do not admit (agent posture).
    let mut p = params("specs", "elsewhere");
    p.operator_mode = false;
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "MEM_PATH_NOT_ALLOWED", "{err}");
    assert_nothing_landed(&engine, tmp.path(), "elsewhere");

    // A source pin the matched rule's allowlist does not admit.
    let mut p = params("specs", "planning-only/copy");
    p.operator_mode = false;
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "MEM_SCHEMA_NOT_ALLOWED", "{err}");
    assert_nothing_landed(&engine, tmp.path(), "planning-only/copy");

    // A name a branch sits above.
    let err = mem_management::fork_mem(&mut engine, params("specs", "specs/child")).unwrap_err();
    assert_eq!(code_of(&err), "MEM_NAME_REF_CONFLICT", "{err}");
    assert_nothing_landed(&engine, tmp.path(), "specs/child");

    // An existing mem name.
    let err = mem_management::fork_mem(&mut engine, params("specs", "plans")).unwrap_err();
    assert_eq!(code_of(&err), "MEM_NAME_COLLISION", "{err}");
    assert_eq!(
        sha_of(&gitdir, "refs/heads/plans").as_deref(),
        Some(plans_tip.as_str())
    );

    // The source is a folder mem.
    let err = mem_management::fork_mem(&mut engine, params("notes", "notes-fork")).unwrap_err();
    assert_eq!(code_of(&err), "INVALID_INPUT", "{err}");
    assert!(err.to_string().contains("folder"), "{err}");
    assert_nothing_landed(&engine, tmp.path(), "notes-fork");

    // The source is not mounted.
    let err = mem_management::fork_mem(&mut engine, params("ghost", "ghost-fork")).unwrap_err();
    assert_eq!(code_of(&err), "UNKNOWN_MEM", "{err}");
    assert_nothing_landed(&engine, tmp.path(), "ghost-fork");

    // A fork under its own name.
    let err = mem_management::fork_mem(&mut engine, params("specs", "specs")).unwrap_err();
    assert_eq!(code_of(&err), "INVALID_INPUT", "{err}");

    // Residue at the name (an unregistered mem's branch and config).
    let stale = GitTreeBackend::new(gitdir.clone(), "refs/heads/stale".to_string());
    stale
        .write_entity(
            Path::new("old.md"),
            b"---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n---\n# Old\n\n## Identity\n\nold\n\n## Purpose\n\nold\n",
        )
        .unwrap();
    let stale_tip = stale.commit("stale", &CommitContext::internal()).unwrap();
    commit_config_at_gitdir(
        &gitdir,
        "stale",
        br#"{"schema": "default@1.0.0"}"#,
        &CommitContext::internal(),
        "stale config",
    )
    .unwrap();
    let err = mem_management::fork_mem(&mut engine, params("specs", "stale")).unwrap_err();
    assert_eq!(code_of(&err), "MEM_STORAGE_RESIDUE_DETECTED", "{err}");
    assert_eq!(
        sha_of(&gitdir, "refs/heads/stale").as_deref(),
        Some(stale_tip.as_str()),
        "the residue is neither adopted nor destroyed"
    );

    // Across every refusal: the source's wildcard grant never leaked
    // under another name, the mount roster is unchanged, and the
    // registry ref moved only for the residue fixture and the folder
    // create, never for a refused fork.
    let toml = workspace_toml(tmp.path());
    assert_eq!(toml.matches("= \"*\"").count(), 1, "{toml}");
    let mut names = engine.mem_names();
    names.sort_unstable();
    assert_eq!(names, vec!["notes", "plans", "specs"]);
    let _ = memstead_before;
}

// ---------------------------------------------------------------------
// The fork commit: sidecars and self-links carry the fork's name
// ---------------------------------------------------------------------

/// The trustwork case shape: a source whose entity carries a url span
/// row, a derived entity-grain row and a file row with an observation,
/// whose derivations sidecar holds baselines keyed by its ids, and whose
/// bodies qualify self-links with the mem's own name beside links to
/// another mem and a code span. After the fork: exactly one commit
/// above the ancestor, by the engine, with the caller's note and
/// identity; every sidecar id and self-link names the fork; rows keep
/// their hashes, spans and observations; the other mem's links and the
/// code span are untouched; a body with nothing to move keeps its blob;
/// the anchors read through the engine under the fork's ids and count
/// the same; the check ledger has no line for the fork; the source is
/// untouched; a reboot reads it all back.
#[test]
fn fork_commit_retargets_sidecars_and_self_links_and_records_the_base() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
    std::fs::write(
        tmp.path().join(".memstead/workspace.toml"),
        format!(
            "{WORKSPACE_HEAD}[[mem_management.create]]\npattern = \"*\"\nschemas = [\"*\"]\n\n\
             [cross_mem_links]\nspecs = [\"plans\"]\n"
        ),
    )
    .unwrap();
    init_real_mem_repo(
        tmp.path(),
        &[("specs", "default@1.0.0"), ("plans", "default@1.0.0")],
    );
    let gitdir = gitdir_of(tmp.path());
    // The file row's artifact: the write path's existence gate reads it.
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    create_entity_in(&mut engine, "plans", "Roadmap");
    // Alpha: a url span row, a derived entity-grain row, a file row.
    create_entity_with(
        &mut engine,
        "specs",
        "Alpha",
        "Alpha stands alone.",
        vec![
            AnchorInput {
                artifact: Some("https://example.org/doc".to_string()),
                grain: Some("url".to_string()),
                class: Some("anchored".to_string()),
                span: Some("the quoted words".to_string()),
                ..Default::default()
            },
            AnchorInput {
                artifact: Some("plans--roadmap".to_string()),
                grain: Some("entity".to_string()),
                class: Some("derived".to_string()),
                derived_from: Some(vec!["plans--roadmap".to_string()]),
                ..Default::default()
            },
            AnchorInput {
                artifact: Some("src/lib.rs".to_string()),
                grain: Some("file".to_string()),
                class: Some("anchored".to_string()),
                ..Default::default()
            },
        ],
    );
    // An observed baseline on the file row (`hash_source: backfill`):
    // the fork must carry it.
    let written = engine
        .record_anchor_observed_hashes(
            "specs",
            &[ObservedArtifactHash {
                entity: "specs--alpha".to_string(),
                artifact: "src/lib.rs".to_string(),
                hash: "0123456789abcdef".to_string(),
            }],
            Some("observed"),
        )
        .unwrap();
    assert_eq!(written, 1);
    // Beta: self-links in both spellings (one labelled), links to the
    // other mem in both spellings, and a self-link inside a code span.
    create_entity_with(
        &mut engine,
        "specs",
        "Beta",
        "Beta builds on [[specs--alpha]] and [[specs:alpha|the alpha]]; see [[plans--roadmap]] \
         and [[plans:roadmap]]; the literal `[[specs--alpha]]` stays.",
        Vec::new(),
    );
    // A check on the source's entity: the ledger line the fork must not
    // inherit.
    engine
        .record_check(
            "specs",
            "specs--alpha",
            Verdict::Ok,
            CheckKind::Verification,
            Some("read by hand"),
            Actor::Cli,
            None,
        )
        .unwrap();
    // The derivations sidecar, keyed by the source's ids with a
    // same-mem target and a cross-mem target.
    let specs_writer = GitTreeBackend::new(gitdir.clone(), "refs/heads/specs".to_string());
    let mut derivations = DerivationSidecar::default();
    derivations.set("specs--beta", "DERIVED_FROM", "specs--alpha", "aaaa1111");
    derivations.set("specs--beta", "DERIVED_FROM", "plans--roadmap", "bbbb2222");
    specs_writer
        .write_entity(Path::new(DERIVATION_SIDECAR_PATH), &derivations.to_bytes())
        .unwrap();
    specs_writer
        .commit("derivation baselines", &CommitContext::internal())
        .unwrap();
    let source_tip = sha_of(&gitdir, "refs/heads/specs").unwrap();
    let source_rows = engine.entity_anchors(&EntityId::new("specs", "alpha"));
    assert_eq!(source_rows.len(), 3, "{source_rows:?}");
    assert!(
        source_rows
            .iter()
            .any(|a| a.hash.as_deref() == Some("0123456789abcdef")
                && a.hash_source == Some(memstead_base::anchor::AnchorHashSource::Backfill)),
        "the observed baseline is on the source's row: {source_rows:?}"
    );
    let source_sidecar = blob_at(&gitdir, "refs/heads/specs", ".memstead/anchors.json").unwrap();
    let source_alpha_blob = tree_blobs(&gitdir, "refs/heads/specs")["alpha.md"].clone();

    engine.set_identity(Some("proposer-p1".to_string()));
    let mut p = params("specs", "proposals/specs-001");
    p.note = Some("the proposal fork".to_string());
    let response = mem_management::fork_mem(&mut engine, p).expect("fork lands");

    // Exactly one commit above the ancestor, by the engine, naming the
    // fork, with the caller's note, tool, actor and identity.
    let fork_tip = sha_of(&gitdir, "refs/heads/proposals/specs-001").unwrap();
    assert_eq!(parents_of(&gitdir, &fork_tip), vec![source_tip.clone()]);
    assert_eq!(response.forked_from.sha, source_tip);
    assert_eq!(
        response.forked_from.base.as_deref(),
        Some(fork_tip.as_str())
    );
    let (message, committer) = commit_message_and_committer(&gitdir, &fork_tip);
    assert_eq!(committer, "engine");
    let subject = message.lines().next().unwrap_or_default();
    assert_eq!(
        subject,
        format!(
            "memstead: fork mem proposals/specs-001 from specs@{}",
            &source_tip[..12]
        )
    );
    for needle in [
        "the proposal fork",
        "Tool: memstead_mem_fork",
        "Actor: cli",
        "Identity: proposer-p1",
    ] {
        assert!(message.contains(needle), "{needle} in:\n{message}");
    }

    // The anchors sidecar: every key names the fork, the rows are the
    // source's rows (hashes, spans, observations included), and the
    // engine reads them under the fork's id.
    let fork_sidecar_bytes = blob_at(
        &gitdir,
        "refs/heads/proposals/specs-001",
        ".memstead/anchors.json",
    )
    .unwrap();
    let fork_sidecar = AnchorSidecar::from_bytes(&fork_sidecar_bytes).unwrap();
    let keys: Vec<&String> = fork_sidecar.entities.keys().collect();
    assert_eq!(keys, vec!["proposals/specs-001--alpha"], "{keys:?}");
    assert_eq!(
        fork_sidecar.get("proposals/specs-001--alpha"),
        &source_rows[..]
    );
    assert_eq!(
        fork_sidecar.version,
        AnchorSidecar::from_bytes(&source_sidecar).unwrap().version
    );
    assert_eq!(
        engine.entity_anchors(&EntityId::new("proposals/specs-001", "alpha")),
        source_rows
    );
    assert!(
        engine
            .entity_anchors(&EntityId::new("proposals/specs-001", "alpha"))
            .iter()
            .any(|a| a.span.as_deref() == Some("the quoted words")),
        "the span row came along"
    );
    assert_eq!(
        engine.mem_anchors_resolved("proposals/specs-001").len(),
        engine.mem_anchors_resolved("specs").len(),
        "the fork counts the rows the source counts"
    );

    // The derivations sidecar: the source key and the same-mem target
    // name the fork, the cross-mem target and the hashes stay.
    let fork_derivations = DerivationSidecar::from_bytes(
        &blob_at(
            &gitdir,
            "refs/heads/proposals/specs-001",
            DERIVATION_SIDECAR_PATH,
        )
        .unwrap(),
    )
    .unwrap();
    let keys: Vec<&String> = fork_derivations.baselines.keys().collect();
    assert_eq!(keys, vec!["proposals/specs-001--beta"], "{keys:?}");
    assert_eq!(
        fork_derivations.get(
            "proposals/specs-001--beta",
            "DERIVED_FROM",
            "proposals/specs-001--alpha"
        ),
        Some("aaaa1111")
    );
    assert_eq!(
        fork_derivations.get(
            "proposals/specs-001--beta",
            "DERIVED_FROM",
            "plans--roadmap"
        ),
        Some("bbbb2222")
    );
    assert_eq!(
        fork_derivations.baselines["proposals/specs-001--beta"].len(),
        2
    );

    // The bodies: both self-link spellings name the fork (label kept),
    // the other mem's links and the code span are untouched, and the
    // link-free body is the same blob.
    let beta =
        String::from_utf8(blob_at(&gitdir, "refs/heads/proposals/specs-001", "beta.md").unwrap())
            .unwrap();
    assert!(beta.contains("[[proposals/specs-001--alpha]]"), "{beta}");
    assert!(
        beta.contains("[[proposals/specs-001:alpha|the alpha]]"),
        "{beta}"
    );
    assert!(beta.contains("[[plans--roadmap]]"), "{beta}");
    assert!(beta.contains("[[plans:roadmap]]"), "{beta}");
    assert!(beta.contains("`[[specs--alpha]]`"), "the code span: {beta}");
    assert_eq!(beta.matches("[[specs--alpha]]").count(), 1, "{beta}");
    assert_eq!(beta.matches("[[specs:").count(), 0, "{beta}");
    let fork_blobs = tree_blobs(&gitdir, "refs/heads/proposals/specs-001");
    assert_eq!(
        fork_blobs["alpha.md"], source_alpha_blob,
        "nothing to move: same blob"
    );
    // The fork loads with its two entities, the retargeted link read
    // as its own.
    assert_eq!(shapes(&engine, "proposals/specs-001").len(), 2);
    let fork_beta = engine
        .store()
        .get(&EntityId::new("proposals/specs-001", "beta"))
        .expect("beta loaded");
    assert!(
        fork_beta.sections["identity"].contains("[[proposals/specs-001--alpha]]"),
        "{}",
        fork_beta.sections["identity"]
    );

    // The check ledger: the source's check stands, the fork has none.
    assert!(
        engine
            .latest_check_record("specs", "specs--alpha", "verification")
            .is_some()
    );
    assert!(
        engine
            .latest_check_record(
                "proposals/specs-001",
                "proposals/specs-001--alpha",
                "verification"
            )
            .is_none(),
        "a fork starts unchecked"
    );
    let ledger = std::fs::read_to_string(memstead_base::check::check_ledger_path(tmp.path()))
        .unwrap_or_default();
    assert!(!ledger.contains("proposals/specs-001--"), "{ledger}");

    // The source is untouched: branch, sidecar and rows.
    assert_eq!(sha_of(&gitdir, "refs/heads/specs").unwrap(), source_tip);
    assert_eq!(
        blob_at(&gitdir, "refs/heads/specs", ".memstead/anchors.json").unwrap(),
        source_sidecar
    );
    assert_eq!(
        engine.entity_anchors(&EntityId::new("specs", "alpha")),
        source_rows
    );
    drop(engine);

    // A reboot reads the base and the retargeted rows back.
    let engine = engine_from_workspace_root(tmp.path()).expect("reboot");
    let origin = engine
        .mem_config_for("proposals/specs-001")
        .unwrap()
        .forked_from
        .clone()
        .unwrap();
    assert_eq!(origin.base.as_deref(), Some(fork_tip.as_str()));
    assert_eq!(origin.sha, source_tip);
    assert_eq!(
        engine.entity_anchors(&EntityId::new("proposals/specs-001", "alpha")),
        source_rows
    );
}

/// A source with no sidecar and no self-qualified link, only links
/// naming another mem: the fork still gets its one commit (an empty
/// retarget is still the base), the tree is the ancestor's tree
/// blob for blob, and the other mem's links are untouched.
#[test]
fn fork_of_a_tree_with_nothing_to_retarget_still_gets_its_base_commit() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
    std::fs::write(
        tmp.path().join(".memstead/workspace.toml"),
        format!("{WORKSPACE_HEAD}[cross_mem_links]\nspecs = [\"plans\"]\n"),
    )
    .unwrap();
    init_real_mem_repo(
        tmp.path(),
        &[("specs", "default@1.0.0"), ("plans", "default@1.0.0")],
    );
    let gitdir = gitdir_of(tmp.path());
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    create_entity_in(&mut engine, "plans", "Roadmap");
    create_entity_with(
        &mut engine,
        "specs",
        "Gamma",
        "Gamma follows [[plans--roadmap]] and [[plans:roadmap]] and [[delta]].",
        Vec::new(),
    );
    create_entity_in(&mut engine, "specs", "Delta");
    let source_tip = sha_of(&gitdir, "refs/heads/specs").unwrap();
    let source_blobs = tree_blobs(&gitdir, "refs/heads/specs");
    assert!(
        !source_blobs.contains_key(".memstead/anchors.json"),
        "the fixture has no sidecar: {source_blobs:?}"
    );

    let response =
        mem_management::fork_mem(&mut engine, params("specs", "specs-fork")).expect("fork lands");
    let fork_tip = sha_of(&gitdir, "refs/heads/specs-fork").unwrap();
    assert_ne!(fork_tip, source_tip, "the base is the fork's own commit");
    assert_eq!(parents_of(&gitdir, &fork_tip), vec![source_tip.clone()]);
    assert_eq!(
        response.forked_from.base.as_deref(),
        Some(fork_tip.as_str())
    );
    assert_eq!(
        tree_blobs(&gitdir, "refs/heads/specs-fork"),
        source_blobs,
        "an empty retarget leaves the ancestor's tree blob for blob"
    );
    let (message, committer) = commit_message_and_committer(&gitdir, &fork_tip);
    assert_eq!(committer, "engine");
    assert!(
        message.starts_with("memstead: fork mem specs-fork from specs@"),
        "{message}"
    );
    assert_eq!(shapes(&engine, "specs-fork").len(), 2);
}

/// A failure after the fork commit (here: the workspace policy file
/// cannot be written when the grant is inherited) rolls the branch and
/// the fork commit with it, the config and the policy line back; the
/// name is free again afterwards.
#[cfg(unix)]
#[test]
fn fork_rolls_back_the_fork_commit_when_a_later_step_fails() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
    std::fs::write(
        tmp.path().join(".memstead/workspace.toml"),
        format!("{WORKSPACE_HEAD}[cross_mem_links]\nspecs = [\"plans\"]\n"),
    )
    .unwrap();
    init_real_mem_repo(
        tmp.path(),
        &[("specs", "default@1.0.0"), ("plans", "default@1.0.0")],
    );
    let gitdir = gitdir_of(tmp.path());
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    create_entity_in(&mut engine, "plans", "Roadmap");
    create_entity_with(
        &mut engine,
        "specs",
        "Alpha",
        "Alpha names [[specs--alpha]] itself.",
        Vec::new(),
    );
    let source_tip = sha_of(&gitdir, "refs/heads/specs").unwrap();
    let toml_path = tmp.path().join(".memstead/workspace.toml");
    let toml_before = std::fs::read_to_string(&toml_path).unwrap();

    // Seal the policy file: the grant inheritance, which runs after the
    // fork commit and the config write, fails.
    std::fs::set_permissions(&toml_path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let result = mem_management::fork_mem(&mut engine, params("specs", "specs-fork"));
    std::fs::set_permissions(&toml_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = result.expect_err("the sealed policy file refuses the grant write");
    assert!(err.to_string().contains("grant"), "{err}");

    assert_nothing_landed(&engine, tmp.path(), "specs-fork");
    assert_eq!(std::fs::read_to_string(&toml_path).unwrap(), toml_before);
    assert_eq!(sha_of(&gitdir, "refs/heads/specs").unwrap(), source_tip);

    // The name is free: the same fork lands once the file is writable.
    let response =
        mem_management::fork_mem(&mut engine, params("specs", "specs-fork")).expect("fork lands");
    let fork_tip = sha_of(&gitdir, "refs/heads/specs-fork").unwrap();
    assert_eq!(parents_of(&gitdir, &fork_tip), vec![source_tip]);
    assert_eq!(
        response.forked_from.base.as_deref(),
        Some(fork_tip.as_str())
    );
}

/// A fork recorded before the fork commit existed carries no `base`:
/// it reads as based on its ancestor, loads as before, and forks again
/// (the new fork gets a base of its own).
#[test]
fn a_fork_recorded_without_a_base_reads_as_based_on_its_ancestor() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(
        tmp.path(),
        &[
            ("specs", "default@1.0.0"),
            ("specs-legacy", "default@1.0.0"),
        ],
    );
    let gitdir = gitdir_of(tmp.path());
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    create_entity_in(&mut engine, "specs", "Alpha");
    create_entity_in(&mut engine, "specs-legacy", "Alpha");
    let legacy_tip = sha_of(&gitdir, "refs/heads/specs-legacy").unwrap();
    drop(engine);
    // The older engine's origin: mem and sha, no base.
    commit_config_at_gitdir(
        &gitdir,
        "specs-legacy",
        format!(
            r#"{{"schema": "default@1.0.0", "forkedFrom": {{"mem": "specs", "sha": "{legacy_tip}"}}}}"#
        )
        .as_bytes(),
        &CommitContext::internal(),
        "legacy origin",
    )
    .unwrap();

    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    let origin = engine
        .mem_config_for("specs-legacy")
        .unwrap()
        .forked_from
        .clone()
        .expect("the origin loads");
    assert_eq!(origin.base, None);
    assert_eq!(
        origin.base_sha(),
        legacy_tip,
        "read as based on the ancestor"
    );
    assert_eq!(shapes(&engine, "specs-legacy").len(), 1);

    let response = mem_management::fork_mem(&mut engine, params("specs-legacy", "specs-next"))
        .expect("a legacy fork forks");
    assert_eq!(response.forked_from.mem, "specs-legacy");
    assert_eq!(response.forked_from.sha, legacy_tip);
    let next_tip = sha_of(&gitdir, "refs/heads/specs-next").unwrap();
    assert_eq!(
        response.forked_from.base.as_deref(),
        Some(next_tip.as_str())
    );
    assert_eq!(parents_of(&gitdir, &next_tip), vec![legacy_tip]);
}

// ---------------------------------------------------------------------
// AC2: the remote form, with a second bare mem-repo as the remote
// ---------------------------------------------------------------------

/// Workspace A with mem `specs` (two entities, a description and a
/// version), pushed to a bare remote; workspace B declares that remote
/// as `origin`. Returns `(a, b, remote, specs tip sha)`.
fn remote_fixture() -> (TempDir, TempDir, TempDir, String) {
    let a = TempDir::new().unwrap();
    init_real_mem_repo(a.path(), &[("specs", "default@1.0.0")]);
    let a_gitdir = gitdir_of(a.path());
    commit_config_at_gitdir(
        &a_gitdir,
        "specs",
        br#"{"schema": "default@1.0.0", "version": "0.4.0", "description": "from A"}"#,
        &CommitContext::internal(),
        "seed",
    )
    .unwrap();
    let mut engine_a = engine_from_workspace_root(a.path()).expect("A boots");
    create_entity_in(&mut engine_a, "specs", "Alpha");
    // Beta qualifies a self-link with the mem's own name: what the
    // remote form's fork commit has to retarget.
    create_entity_with(
        &mut engine_a,
        "specs",
        "Beta",
        "Beta follows [[specs--alpha]].",
        Vec::new(),
    );
    relate(&mut engine_a, ("specs", "beta"), ("specs", "alpha"));
    drop(engine_a);
    let tip = sha_of(&a_gitdir, "refs/heads/specs").unwrap();

    let remote = TempDir::new().unwrap();
    gix::init_bare(remote.path()).unwrap();
    remote_add_in_gitdir(&a_gitdir, "origin", remote.path().to_str().unwrap()).unwrap();
    push_in_gitdir(&a_gitdir, "origin", "specs", "specs", false).unwrap();
    push_in_gitdir(&a_gitdir, "origin", "__MEMSTEAD", "__MEMSTEAD", false).unwrap();

    let b = TempDir::new().unwrap();
    init_real_mem_repo(b.path(), &[("local", "default@1.0.0")]);
    let b_gitdir = gitdir_of(b.path());
    remote_add_in_gitdir(&b_gitdir, "origin", remote.path().to_str().unwrap()).unwrap();
    (a, b, remote, tip)
}

/// Push one extra branch of A (with its config blob on A's
/// `__MEMSTEAD`) to the remote: `entries` are `(path, content)` pairs.
fn push_branch_from_a(a: &Path, name: &str, config: &[u8], entries: &[(&str, &[u8])]) -> String {
    let gitdir = gitdir_of(a);
    let writer = GitTreeBackend::new(gitdir.clone(), format!("refs/heads/{name}"));
    for (path, content) in entries {
        writer.write_entity(Path::new(path), content).unwrap();
    }
    let sha = writer.commit("seed", &CommitContext::internal()).unwrap();
    commit_config_at_gitdir(&gitdir, name, config, &CommitContext::internal(), "seed").unwrap();
    push_in_gitdir(&gitdir, "origin", name, name, false).unwrap();
    push_in_gitdir(&gitdir, "origin", "__MEMSTEAD", "__MEMSTEAD", true).unwrap();
    sha
}

/// The remote fork: the branch is the remote's commit, the config is
/// the remote's with the origin naming source, sha and remote, the
/// entities load, no grant is inherited, the local `__MEMSTEAD` gained
/// exactly the fork's config blob and nothing else, and the fork
/// survives a reboot.
#[test]
fn remote_fork_fetches_the_source_branch_and_config() {
    let (_a, b, _remote, tip) = remote_fixture();
    let b_gitdir = gitdir_of(b.path());
    let mut engine = engine_from_workspace_root(b.path()).expect("B boots");
    let registry_before = tree_blobs(&b_gitdir, "refs/heads/__MEMSTEAD");
    let toml_before = workspace_toml(b.path());

    let mut p = params("specs", "specs-copy");
    p.remote = Some("origin".to_string());
    let response = mem_management::fork_mem(&mut engine, p).expect("remote fork lands");
    assert_eq!(response.forked_from.mem, "specs");
    assert_eq!(response.forked_from.sha, tip);
    assert_eq!(response.forked_from.remote.as_deref(), Some("origin"));
    assert_eq!(response.inherited_grants, None);
    // The fork commit sits right above the fetched commit, and it did
    // the same retarget the local form does: the self-link names the
    // fork.
    let fork_tip = sha_of(&b_gitdir, "refs/heads/specs-copy").unwrap();
    assert_eq!(parents_of(&b_gitdir, &fork_tip), vec![tip.clone()]);
    assert_eq!(
        response.forked_from.base.as_deref(),
        Some(fork_tip.as_str())
    );
    let beta =
        String::from_utf8(blob_at(&b_gitdir, "refs/heads/specs-copy", "beta.md").unwrap()).unwrap();
    assert!(beta.contains("[[specs-copy--alpha]]"), "{beta}");
    assert!(!beta.contains("[[specs--alpha]]"), "{beta}");

    let cfg = read_config_at_gitdir(&b_gitdir, "specs-copy").unwrap();
    assert_eq!(cfg.description.as_deref(), Some("from A"));
    assert_eq!(cfg.version.unwrap().to_string(), "0.4.0");
    let origin = cfg.forked_from.unwrap();
    assert_eq!(
        (origin.mem.as_str(), origin.remote.as_deref()),
        ("specs", Some("origin"))
    );
    assert_eq!(origin.sha, tip);
    assert_eq!(origin.base.as_deref(), Some(fork_tip.as_str()));

    // The entities and their edge came with the branch.
    let fork_shapes = shapes(&engine, "specs-copy");
    assert_eq!(fork_shapes.len(), 2);
    assert!(
        fork_shapes.iter().any(|s| s
            .3
            .contains(&("DEPENDS_ON".to_string(), "alpha".to_string()))),
        "{fork_shapes:?}"
    );

    // No grant from the remote's policy: the workspace file is untouched.
    assert_eq!(workspace_toml(b.path()), toml_before);

    // The remote's registry was fetched to a tracking ref; the local
    // __MEMSTEAD gained exactly the fork's config blob.
    assert!(sha_of(&b_gitdir, "refs/remotes/origin/__MEMSTEAD").is_some());
    assert!(sha_of(&b_gitdir, "refs/remotes/origin/specs").is_some());
    let mut expected = registry_before;
    let registry_after = tree_blobs(&b_gitdir, "refs/heads/__MEMSTEAD");
    let fork_blob = registry_after
        .get("mems/specs-copy/config.json")
        .expect("the fork's config blob");
    expected.insert("mems/specs-copy/config.json".to_string(), fork_blob.clone());
    assert_eq!(registry_after, expected, "no other blob moved");
    // A's own config never appears under the source's name locally.
    assert!(read_config_at_gitdir(&b_gitdir, "specs").is_err());
    drop(engine);

    let engine = engine_from_workspace_root(b.path()).expect("reboot");
    assert_eq!(shapes(&engine, "specs-copy").len(), 2);
    assert_eq!(
        engine
            .mem_config_for("specs-copy")
            .unwrap()
            .forked_from
            .as_ref()
            .unwrap()
            .remote
            .as_deref(),
        Some("origin")
    );
}

/// `<source>@<sha> --remote`: the fork starts at the named commit of
/// the fetched branch.
#[test]
fn remote_fork_at_a_given_sha_starts_there() {
    let (a, b, _remote, first) = remote_fixture();
    // A moves on; the remote follows.
    let a_gitdir = gitdir_of(a.path());
    let mut engine_a = engine_from_workspace_root(a.path()).expect("A boots");
    create_entity_in(&mut engine_a, "specs", "Gamma");
    drop(engine_a);
    push_in_gitdir(&a_gitdir, "origin", "specs", "specs", false).unwrap();
    let tip = sha_of(&a_gitdir, "refs/heads/specs").unwrap();
    assert_ne!(first, tip);

    let mut engine = engine_from_workspace_root(b.path()).expect("B boots");
    let mut p = params("specs", "specs-early");
    p.remote = Some("origin".to_string());
    p.sha = Some(first.clone());
    let response = mem_management::fork_mem(&mut engine, p).expect("remote fork at sha");
    assert_eq!(response.forked_from.sha, first);
    assert_eq!(shapes(&engine, "specs-early").len(), 2);
}

/// Every remote refusal is typed and lands nothing; the local
/// `__MEMSTEAD` is never fetched over.
#[test]
fn remote_fork_refusals_land_nothing() {
    let (a, b, _remote, _tip) = remote_fixture();
    let b_gitdir = gitdir_of(b.path());
    // A branch whose config pins a schema B cannot resolve.
    let valid = b"---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n---\n# Valid\n\n## Identity\n\nv\n\n## Purpose\n\nv\n";
    push_branch_from_a(
        a.path(),
        "weird",
        br#"{"schema": "nowhere@1.0.0"}"#,
        &[("valid.md", valid)],
    );
    // A branch whose tree fails the schema.
    push_branch_from_a(
        a.path(),
        "broken",
        br#"{"schema": "default@1.0.0"}"#,
        &[("broken.md", b"# Broken\n\nno frontmatter, no sections\n")],
    );

    let mut engine = engine_from_workspace_root(b.path()).expect("B boots");
    let registry_before = sha_of(&b_gitdir, "refs/heads/__MEMSTEAD").unwrap();
    let local_tip = sha_of(&b_gitdir, "refs/heads/local").unwrap();

    // An unknown remote.
    let mut p = params("specs", "from-nowhere");
    p.remote = Some("nowhere".to_string());
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "UNKNOWN_REMOTE", "{err}");
    assert_nothing_landed(&engine, b.path(), "from-nowhere");

    // A branch the remote does not have, named with the remote.
    let mut p = params("ghost", "ghost-copy");
    p.remote = Some("origin".to_string());
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "UNKNOWN_REF", "{err}");
    let msg = err.to_string();
    assert!(
        msg.contains("origin") && msg.contains("refs/heads/ghost"),
        "{msg}"
    );
    assert_nothing_landed(&engine, b.path(), "ghost-copy");

    // A sha not on the fetched branch (B's own local tip).
    let mut p = params("specs", "specs-off");
    p.remote = Some("origin".to_string());
    p.sha = Some(local_tip);
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "UNKNOWN_REF", "{err}");
    assert_nothing_landed(&engine, b.path(), "specs-off");

    // A full 40-hex sha naming no object, after the fetch: typed.
    let mut p = params("specs", "specs-no-object");
    p.remote = Some("origin".to_string());
    p.sha = Some("0000000000000000000000000000000000000001".to_string());
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "UNKNOWN_REF", "{err}");
    assert!(
        err.to_string()
            .contains("0000000000000000000000000000000000000001"),
        "{err}"
    );
    assert_nothing_landed(&engine, b.path(), "specs-no-object");

    // A source pin this workspace cannot resolve: typed, naming the
    // pin and the remedy, landing no branch and no config.
    let mut p = params("weird", "weird-copy");
    p.remote = Some("origin".to_string());
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "SCHEMA_NOT_FOUND", "{err}");
    let msg = err.to_string();
    assert!(msg.contains("nowhere@1.0.0"), "{msg}");
    assert!(msg.contains("memstead schema install"), "{msg}");
    assert_nothing_landed(&engine, b.path(), "weird-copy");

    // A fetched tree failing the schema: the pull path's code.
    let mut p = params("broken", "broken-copy");
    p.remote = Some("origin".to_string());
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "SCHEMA_VIOLATION_IN_FETCH", "{err}");
    assert_nothing_landed(&engine, b.path(), "broken-copy");

    // The local registry ref never moved for a refused fork, and the
    // fetches landed on tracking refs only.
    assert_eq!(
        sha_of(&b_gitdir, "refs/heads/__MEMSTEAD").unwrap(),
        registry_before
    );
    assert!(sha_of(&b_gitdir, "refs/remotes/origin/__MEMSTEAD").is_some());
    assert!(read_md_blobs_at_ref(&b_gitdir, "refs/heads/local").is_ok());
}

/// The trustwork case: the source pins a workspace-local newer
/// generation of a builtin-named schema (`planning@0.7.0`, where the
/// forking workspace holds only the builtin generations). A fork
/// copies the pin and never re-pins, so the refusal names the pin and
/// `memstead schema install`, never `mem set-schema` against a mem that
/// does not exist, and lands nothing.
#[test]
fn remote_fork_refuses_a_pin_newer_than_the_installed_generations_naming_install() {
    let (a, b, _remote, _tip) = remote_fixture();
    let b_gitdir = gitdir_of(b.path());
    let valid = b"---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n---\n# Valid\n\n## Identity\n\nv\n\n## Purpose\n\nv\n";
    push_branch_from_a(
        a.path(),
        "trust",
        br#"{"schema": "planning@0.7.0"}"#,
        &[("valid.md", valid)],
    );
    let mut engine = engine_from_workspace_root(b.path()).expect("B boots");
    // The forking workspace resolves the builtin planning generations
    // and not the source's newer one.
    assert!(
        engine
            .builtin_schemas()
            .iter()
            .any(|s| s.manifest.name == "planning"),
        "the fixture needs a builtin named planning"
    );
    assert!(
        !engine
            .builtin_schemas()
            .iter()
            .any(|s| s.manifest.name == "planning" && s.version.to_string() == "0.7.0"),
        "the fixture needs planning@0.7.0 to be absent here"
    );
    let registry_before = sha_of(&b_gitdir, "refs/heads/__MEMSTEAD").unwrap();

    let mut p = params("trust", "trust-copy");
    p.remote = Some("origin".to_string());
    let err = mem_management::fork_mem(&mut engine, p).unwrap_err();
    assert_eq!(code_of(&err), "SCHEMA_NOT_FOUND", "{err}");
    let msg = err.to_string();
    assert!(msg.contains("planning@0.7.0"), "{msg}");
    assert!(msg.contains("memstead schema install"), "{msg}");
    assert!(!msg.contains("set-schema"), "a fork never re-pins: {msg}");
    assert_nothing_landed(&engine, b.path(), "trust-copy");
    assert_eq!(
        sha_of(&b_gitdir, "refs/heads/__MEMSTEAD").unwrap(),
        registry_before
    );
}

/// A workspace without a mem-repo cannot fork: typed `INVALID_INPUT`
/// naming the reason.
#[test]
fn fork_refuses_a_folder_only_workspace() {
    let tmp = TempDir::new().unwrap();
    let mem = tmp.path().join("notes");
    std::fs::create_dir_all(mem.join(".memstead")).unwrap();
    std::fs::write(
        mem.join(".memstead/config.json"),
        r#"{"schema": "default@1.0.0"}"#,
    )
    .unwrap();
    let mut engine = memstead_base::Engine::from_mounts(vec![(
        memstead_base::Mount {
            mem: "notes".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: memstead_base::MountStorage::Folder { path: mem },
            capability: memstead_base::MountCapability::Write,
            lifecycle: memstead_base::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        },
        memstead_base::instantiate_local_backend(&memstead_base::Mount {
            mem: "notes".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: memstead_base::MountStorage::Folder {
                path: tmp.path().join("notes"),
            },
            capability: memstead_base::MountCapability::Write,
            lifecycle: memstead_base::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        })
        .unwrap(),
    )])
    .expect("folder engine");
    engine.set_workspace_root(tmp.path().to_path_buf());
    let err = mem_management::fork_mem(&mut engine, params("notes", "notes-fork")).unwrap_err();
    assert_eq!(code_of(&err), "INVALID_INPUT", "{err}");
    assert!(err.to_string().contains("mem-repo"), "{err}");
}
