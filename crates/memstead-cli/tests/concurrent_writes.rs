//! Concurrent writers in separate processes, all carrying the same stale
//! `expected_hash`: exactly one may land, every other one must be refused
//! with `HASH_MISMATCH`. Before the cross-process write lock, the hash
//! compare and the commit were not atomic across processes, so on a
//! folder mem several writers could be told "success" while all but the
//! last were silently overwritten, and on a git-branch mem a writer that
//! staged after a sibling's commit landed on top of it. The race needs
//! real processes, so these tests spawn the binary; the mixed case adds a
//! long-lived in-process engine (the shape of a running MCP server) that
//! was booted before any of the processes wrote.

use std::path::Path;
use std::process::{Child, Command, Stdio};

use indexmap::IndexMap;
use memstead_base::vcs::Actor;
use memstead_base::{EngineError, EntityId, UpdateEntityArgs};
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

const WRITERS: usize = 6;
const TRIALS: usize = 25;

fn bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("memstead")
}

fn run(ws: &Path, args: &[&str]) -> String {
    let out = Command::new(bin())
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "memstead {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A mem-repo workspace with one mem of the given storage.
fn workspace(storage: &str) -> TempDir {
    let ws = TempDir::new().unwrap();
    run(ws.path(), &["mem-repo", "init", ".", "--no-gitignore"]);
    run(
        ws.path(),
        &[
            "workspace",
            "allow-create",
            "*",
            "--schema",
            "default@1.3.0",
        ],
    );
    run(ws.path(), &["mem", "init", "notes", "--storage", storage]);
    ws
}

fn create(ws: &Path, title: &str) -> String {
    run(
        ws,
        &[
            "create",
            "--mem",
            "notes",
            "--type",
            "assertion",
            "--title",
            title,
            "--section",
            "claim=A thing holds.",
            "--section",
            "evidence=- a test",
        ],
    );
    format!("notes--{}", title.to_lowercase().replace(' ', "-"))
}

fn hash(ws: &Path, id: &str) -> String {
    let out = run(ws, &["entity", id, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    v["_hash"]
        .as_str()
        .or_else(|| v["entity"]["_hash"].as_str())
        .expect("entity --json carries _hash")
        .to_string()
}

fn spawn_writer(ws: &Path, id: &str, hash: &str, n: usize) -> Child {
    Command::new(bin())
        .current_dir(ws)
        .args([
            "update",
            id,
            "--expected-hash",
            hash,
            "--section",
            &format!("conditions=writer {n}"),
            "--quiet",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Wait for every writer; return (successes, refusals with HASH_MISMATCH).
fn settle(children: Vec<Child>) -> (usize, usize) {
    let mut ok = 0;
    let mut mismatch = 0;
    for child in children {
        let out = child.wait_with_output().unwrap();
        if out.status.success() {
            ok += 1;
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(
                err.contains("HASH_MISMATCH"),
                "a refused writer must be refused with HASH_MISMATCH, got: {err}"
            );
            mismatch += 1;
        }
    }
    (ok, mismatch)
}

fn race_between_processes(storage: &str) {
    let ws = workspace(storage);
    for trial in 0..TRIALS {
        let id = create(ws.path(), &format!("Race {trial}"));
        let stale = hash(ws.path(), &id);
        let children: Vec<Child> = (0..WRITERS)
            .map(|n| spawn_writer(ws.path(), &id, &stale, n))
            .collect();
        let (ok, mismatch) = settle(children);
        assert_eq!(
            (ok, mismatch),
            (1, WRITERS - 1),
            "{storage} trial {trial}: exactly one writer lands, the rest are refused"
        );
        assert_ne!(hash(ws.path(), &id), stale, "the one accepted write landed");
    }
}

#[test]
fn racing_processes_on_a_folder_mem_land_exactly_one_write() {
    race_between_processes("folder");
}

#[test]
fn racing_processes_on_a_git_branch_mem_land_exactly_one_write() {
    race_between_processes("git-branch");
}

/// A long-lived engine, booted before the race (as an MCP server is),
/// writes with the same stale hash while CLI processes race it.
fn race_with_a_long_lived_engine(storage: &str) {
    let ws = workspace(storage);
    for trial in 0..TRIALS {
        let id = create(ws.path(), &format!("Mixed {trial}"));
        let stale = hash(ws.path(), &id);
        let mut engine = engine_from_workspace_root(ws.path()).expect("engine boots");
        let children: Vec<Child> = (0..WRITERS - 1)
            .map(|n| spawn_writer(ws.path(), &id, &stale, n))
            .collect();
        // Process start-up takes tens of milliseconds; spread the
        // engine's write across that window so it lands before, among
        // and after the processes' commits over the trials.
        std::thread::sleep(std::time::Duration::from_millis((trial as u64 * 9) % 120));
        let mut sections = IndexMap::new();
        sections.insert("conditions".to_string(), "server writer".to_string());
        let in_process = engine.update_entity(
            UpdateEntityArgs {
                anchors: Vec::new(),
                id: EntityId::canonical(&id),
                expected_hash: Some(stale.clone()),
                sections,
                append_sections: IndexMap::new(),
                patch_sections: IndexMap::new(),
                sections_unset: Vec::new(),
                metadata: IndexMap::new(),
                metadata_unset: Vec::new(),
                dry_run: false,
                declare_relations: Vec::new(),
                relations_unset: Vec::new(),
                anchors_unset: Vec::new(),
            },
            Actor::Cli,
            None,
            None,
        );
        let server_ok = match in_process {
            Ok(_) => 1,
            Err(EngineError::HashMismatch { .. }) => 0,
            Err(e) => panic!("{storage} trial {trial}: unexpected engine error {e}"),
        };
        let (ok, mismatch) = settle(children);
        assert_eq!(
            ok + server_ok,
            1,
            "{storage} trial {trial}: exactly one of the server and {} processes lands \
             (processes: {ok} landed, {mismatch} refused)",
            WRITERS - 1
        );
    }
}

#[test]
fn a_long_lived_engine_and_racing_processes_on_a_folder_mem_land_exactly_one_write() {
    race_with_a_long_lived_engine("folder");
}

#[test]
fn a_long_lived_engine_and_racing_processes_on_a_git_branch_mem_land_exactly_one_write() {
    race_with_a_long_lived_engine("git-branch");
}
