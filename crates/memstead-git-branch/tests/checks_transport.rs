//! The check ledger travels with a pushed branch
//! (`engine::checks_transport`): a push publishes the mem's ledger
//! rows on the mem-repo's `__MEMSTEAD_CHECKS` ref, a pull on another
//! machine unions them into its own ledger, an export made there seals
//! them, two machines' rows merge by set union, and a round trip adds
//! nothing. The remote is a plain bare repository.

use std::path::{Path, PathBuf};

use memstead_base::backend::MemBackend;
use memstead_base::check::{CheckKind, Verdict};
use memstead_base::vcs::{Actor, Role};
use memstead_base::{CreateEntityArgs, EntityId};
use memstead_git_branch::ops::transport::resolve_ref_in_gitdir;
use memstead_git_branch::storage::git_tree::GitTreeBackend;
use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

const CHECKS_REF: &str = "refs/heads/__MEMSTEAD_CHECKS";
const MEMBER: &str = "mems/specs/checks.jsonl";

fn gitdir_of(root: &Path) -> PathBuf {
    root.join("mem-repo").join(".git").canonicalize().unwrap()
}

fn sha_of(gitdir: &Path, ref_name: &str) -> Option<String> {
    resolve_ref_in_gitdir(gitdir, ref_name).unwrap()
}

fn create(engine: &mut memstead_base::Engine, title: &str) -> EntityId {
    let mut sections = indexmap::IndexMap::new();
    sections.insert("identity".to_string(), format!("{title} identity"));
    sections.insert("purpose".to_string(), "seed".to_string());
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
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
        .unwrap()
        .id
}

/// A check under `identity` in the checker's role.
fn check(engine: &mut memstead_base::Engine, identity: &str, id: &EntityId) {
    engine.set_identity(Some(identity.to_string()));
    engine.set_role(Role::Checker);
    engine
        .record_check(
            "specs",
            id.as_ref(),
            Verdict::Ok,
            CheckKind::Verification,
            Some("read against the source"),
            Actor::Cli,
            None,
        )
        .unwrap();
    engine.set_role(Role::Unspecified);
    engine.set_identity(None);
}

fn ledger_lines(root: &Path) -> Vec<String> {
    std::fs::read_to_string(memstead_base::check::check_ledger_path(root))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn member_at(gitdir: &Path, ref_name: &str) -> Option<String> {
    GitTreeBackend::new(gitdir.to_path_buf(), ref_name.to_string())
        .read_entity(Path::new(MEMBER))
        .unwrap()
        .map(|b| String::from_utf8(b).unwrap())
}

/// Workspace A with mem `specs` (two entities) and a plain bare remote
/// `hub`; A's `specs` is on the remote, nothing else.
fn fixture() -> (TempDir, TempDir) {
    let a = TempDir::new().unwrap();
    init_real_mem_repo(a.path(), &[("specs", "default@1.0.0")]);
    // A version on the config, so an export of the mem is possible.
    memstead_git_branch::mem_repo_config::commit_config_at_gitdir(
        &gitdir_of(a.path()),
        "specs",
        br#"{"schema": "default@1.0.0", "version": "0.1.0", "description": "the owner's"}"#,
        &memstead_git_branch::vcs::CommitContext::internal(),
        "seed",
    )
    .unwrap();
    let mut engine = engine_from_workspace_root(a.path()).expect("A boots");
    create(&mut engine, "Alpha");
    create(&mut engine, "Beta");
    let remote = TempDir::new().unwrap();
    gix::init_bare(remote.path()).unwrap();
    engine
        .remote_add("hub", remote.path().to_str().unwrap())
        .expect("remote added");
    engine.push("specs", "hub", false).expect("A pushes specs");
    drop(engine);
    (a, remote)
}

/// A second machine: a copy of A's workspace with no ledger of its own.
fn clone_workspace(a: &Path) -> TempDir {
    let b = TempDir::new().unwrap();
    let status = std::process::Command::new("cp")
        .arg("-R")
        .arg(format!("{}/.", a.display()))
        .arg(b.path())
        .status()
        .unwrap();
    assert!(status.success());
    let ledger = memstead_base::check::check_ledger_path(b.path());
    if ledger.exists() {
        std::fs::remove_file(&ledger).unwrap();
    }
    b
}

/// A push with no check records publishes nothing; the first check
/// then rides the next push as one commit on the transport ref, and a
/// push that changes nothing leaves the ref where it is.
#[test]
fn a_push_publishes_the_mems_rows_once() {
    let (a, remote) = fixture();
    let gitdir = gitdir_of(a.path());
    let remote_gitdir = remote.path().to_path_buf();
    assert_eq!(
        sha_of(&remote_gitdir, CHECKS_REF),
        None,
        "nothing to publish yet"
    );

    let mut engine = engine_from_workspace_root(a.path()).unwrap();
    let alpha = EntityId::new("specs", "alpha");
    check(&mut engine, "checker-c", &alpha);
    let outcome = engine.push("specs", "hub", false).unwrap();
    let published = outcome.checks_published.expect("the rows were published");
    assert_eq!(
        sha_of(&remote_gitdir, CHECKS_REF).as_deref(),
        Some(published.as_str())
    );
    assert_eq!(
        sha_of(&gitdir, CHECKS_REF).as_deref(),
        Some(published.as_str())
    );
    let member = member_at(&remote_gitdir, CHECKS_REF).expect("the member is on the remote");
    assert_eq!(member.lines().count(), 1);
    assert!(member.contains("\"identity\":\"checker-c\""), "{member}");
    assert!(member.contains("\"role\":\"checker\""), "{member}");

    // Nothing new: the ref stays.
    let again = engine.push("specs", "hub", false).unwrap();
    assert_eq!(again.checks_published, None);
    assert_eq!(
        sha_of(&remote_gitdir, CHECKS_REF).as_deref(),
        Some(published.as_str())
    );
    assert_eq!(
        ledger_lines(a.path()).len(),
        1,
        "a publish imports nothing new"
    );

    // `push --all` carries the ref too, and reports it in sync when it is.
    let all = engine.push_all("hub").unwrap();
    assert!(all.refused.is_empty(), "{all:?}");
    assert!(all.in_sync.contains(&CHECKS_REF.to_string()), "{all:?}");
}

/// The other machine pulls the branch and gets the rows: its ledger
/// holds A's record, its checks axis reads it, and an export made there
/// seals it. Its own check then rides back: A pulls and holds both
/// rows, once each, in timestamp order.
#[test]
fn a_pull_imports_the_rows_and_two_machines_merge_by_union() {
    let (a, remote) = fixture();
    let remote_gitdir = remote.path().to_path_buf();
    let alpha = EntityId::new("specs", "alpha");
    let beta = EntityId::new("specs", "beta");
    {
        let mut engine = engine_from_workspace_root(a.path()).unwrap();
        check(&mut engine, "checker-c", &alpha);
        engine.push("specs", "hub", false).unwrap();
    }

    let b = clone_workspace(a.path());
    assert!(ledger_lines(b.path()).is_empty());
    let mut engine_b = engine_from_workspace_root(b.path()).expect("B boots");
    let pulled = engine_b.pull("specs", "hub").expect("B pulls");
    assert_eq!(pulled.checks_imported, 1);
    let lines = ledger_lines(b.path());
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("checker-c"), "{lines:?}");
    let axis = memstead_base::ops::health::health_checks_axis(&engine_b, Some("specs"));
    assert_eq!(axis["specs"]["checked_ok"], 1, "{axis}");
    assert_eq!(
        axis["specs"]["independence"]["readings"]["specs--alpha"][0]["identity"], "checker-c",
        "{axis}"
    );
    // An export made on B seals the record A wrote.
    let bytes = engine_b.export_mem_to_bytes("specs").unwrap();
    let mounted = memstead_base::Engine::from_archive_bytes(bytes).unwrap();
    let sealed = memstead_base::ops::health::health_checks_axis(&mounted, Some("specs"));
    assert_eq!(sealed["specs"]["checked_ok"], 1, "{sealed}");

    // B checks beta and pushes: the remote member holds both rows.
    check(&mut engine_b, "checker-d", &beta);
    let outcome = engine_b.push("specs", "hub", false).unwrap();
    assert!(outcome.checks_published.is_some());
    let member = member_at(&remote_gitdir, CHECKS_REF).unwrap();
    assert_eq!(member.lines().count(), 2, "{member}");

    // A pulls: its ledger gains beta's row and keeps its own once.
    let mut engine_a = engine_from_workspace_root(a.path()).unwrap();
    let pulled = engine_a.pull("specs", "hub").unwrap();
    assert_eq!(pulled.checks_imported, 1);
    let lines = ledger_lines(a.path());
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("checker-c")));
    assert!(lines.iter().any(|l| l.contains("checker-d")));
    // A second pull adds nothing; a fetch neither.
    assert_eq!(engine_a.pull("specs", "hub").unwrap().checks_imported, 0);
    assert_eq!(
        engine_a.fetch("specs", "hub", &[]).unwrap().checks_imported,
        0
    );
    assert_eq!(ledger_lines(a.path()).len(), 2);
    // And A's next push changes nothing on the remote.
    assert_eq!(
        engine_a
            .push("specs", "hub", false)
            .unwrap()
            .checks_published,
        None
    );
}

/// Two machines that both checked before either published: the second
/// publisher lands on top of the first's commit, and the member holds
/// the union.
#[test]
fn concurrent_publishers_union_instead_of_overwriting() {
    let (a, remote) = fixture();
    let remote_gitdir = remote.path().to_path_buf();
    let b = clone_workspace(a.path());
    let alpha = EntityId::new("specs", "alpha");
    let beta = EntityId::new("specs", "beta");

    let mut engine_a = engine_from_workspace_root(a.path()).unwrap();
    let mut engine_b = engine_from_workspace_root(b.path()).unwrap();
    check(&mut engine_a, "checker-c", &alpha);
    check(&mut engine_b, "checker-d", &beta);
    let first = engine_a
        .push("specs", "hub", false)
        .unwrap()
        .checks_published
        .unwrap();
    let second = engine_b
        .push("specs", "hub", false)
        .unwrap()
        .checks_published
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(
        sha_of(&remote_gitdir, CHECKS_REF).as_deref(),
        Some(second.as_str())
    );
    let member = member_at(&remote_gitdir, CHECKS_REF).unwrap();
    assert_eq!(member.lines().count(), 2, "{member}");
    // B's publish imported A's row on the way.
    assert_eq!(ledger_lines(b.path()).len(), 2);
    // The remote's status report never classifies the transport ref.
    let status = engine_a.remote_status("hub");
    assert!(
        status.refs.iter().all(|r| r.ref_name != CHECKS_REF),
        "{status:?}"
    );
}
