//! A git-branch mem keeps its check rows in the store that holds it, on
//! one ref per mem beside its branch: every workspace that mounts the mem
//! reads the same rows, concurrent recorders lose none, a copy of the
//! mem-repo and an adopted mem carry them, and the rows a workspace's
//! ledger file held before move into the store once.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn run(ws: &Path, args: &[&str]) -> (bool, String) {
    let out = memstead()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        ),
    )
}

fn ok(ws: &Path, args: &[&str]) -> String {
    let (succeeded, printed) = run(ws, args);
    assert!(
        succeeded,
        "`memstead {}` failed:\n{printed}",
        args.join(" ")
    );
    printed
}

fn json(ws: &Path, args: &[&str]) -> serde_json::Value {
    let out = memstead()
        .current_dir(ws)
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "`memstead --json {}` failed:\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn git(gitdir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(gitdir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn sha(gitdir: &Path, ref_name: &str) -> String {
    git(gitdir, &["rev-parse", "--verify", "--quiet", ref_name])
}

fn spec(ws: &Path, mem: &str, title: &str, identity: &str) {
    ok(
        ws,
        &[
            "create",
            "--mem",
            mem,
            "--title",
            title,
            "--type",
            "spec",
            "--metadata",
            "level=M0",
            "--section",
            "identity=i",
            "--section",
            "purpose=p",
            "--identity",
            identity,
        ],
    );
}

/// A store whose mem-repo holds the mem `kept` with the entities named.
fn store(root: &Path, name: &str, titles: &[&str]) -> (PathBuf, PathBuf) {
    let store = root.join(name);
    fs::create_dir_all(&store).unwrap();
    ok(&store, &["mem-repo", "init", "."]);
    ok(&store, &["workspace", "allow-create", "--schema", "*", "*"]);
    ok(
        &store,
        &["mem", "init", "kept", "--schema", "default@1.3.0"],
    );
    for title in titles {
        spec(&store, "kept", title, "author-a");
    }
    let gitdir = store.join("mem-repo/.git").canonicalize().unwrap();
    (store, gitdir)
}

/// A view: a workspace of engine state only mounting `kept` of `gitdir`.
fn view(root: &Path, name: &str, gitdir: &Path, capability: &str) -> PathBuf {
    let view = root.join(name);
    fs::create_dir_all(&view).unwrap();
    ok(
        &view,
        &[
            "mem",
            "mount",
            "kept",
            "--branch",
            "kept",
            "--gitdir",
            gitdir.to_str().unwrap(),
            "--capability",
            capability,
            "--new-workspace",
            "--operator-mode",
        ],
    );
    view
}

fn rows(ws: &Path) -> Vec<serde_json::Value> {
    json(ws, &["checks", "--mem", "kept"])["checks"]
        .as_array()
        .unwrap()
        .clone()
}

/// A check recorded through one view reads the same, by every reader of
/// the list, in every workspace of the list: another writable view, a
/// read-only view, the store's own workspace, and a workspace of another
/// store that mounts the mem read-only. An engine already running there
/// reads it at its next operation. The check moves no branch, no
/// registry ref and no entity hash.
#[test]
fn a_check_recorded_in_one_workspace_reads_the_same_in_every_workspace() {
    let tmp = TempDir::new().unwrap();
    let (store, gitdir) = store(tmp.path(), "store", &["Thing"]);
    let c = view(tmp.path(), "c", &gitdir, "write");
    let d = view(tmp.path(), "d", &gitdir, "write");
    let r = view(tmp.path(), "r", &gitdir, "read-only");
    // Another store's workspace: its own mem-repo, `kept` mounted
    // read-only from the first store's git directory.
    let other = tmp.path().join("other");
    fs::create_dir_all(&other).unwrap();
    ok(&other, &["mem-repo", "init", "."]);
    ok(
        &other,
        &[
            "mem",
            "mount",
            "kept",
            "--branch",
            "kept",
            "--gitdir",
            gitdir.to_str().unwrap(),
            "--capability",
            "read-only",
            "--operator-mode",
        ],
    );
    let running = memstead_git_branch::workspace_store::engine_from_workspace_root(&d).unwrap();
    assert!(running.check_records("kept").0.is_empty());

    let branch_before = sha(&gitdir, "refs/heads/kept");
    let registry_before = sha(&gitdir, "refs/heads/__MEMSTEAD");
    let hash_before = json(&c, &["entity", "kept--thing"])["_hash"].clone();
    ok(
        &c,
        &[
            "check",
            "kept--thing",
            "--verdict",
            "ok",
            "--identity",
            "checker-c",
            "--role",
            "checker",
        ],
    );
    assert_eq!(sha(&gitdir, "refs/heads/kept"), branch_before);
    assert_eq!(sha(&gitdir, "refs/heads/__MEMSTEAD"), registry_before);
    assert_eq!(json(&c, &["entity", "kept--thing"])["_hash"], hash_before);
    assert!(!sha(&gitdir, "refs/memstead/checks/kept").is_empty());

    let recorded = rows(&c);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    for ws in [&d, &r, &store, &other] {
        assert_eq!(rows(ws), recorded, "{}", ws.display());
        let prov = &json(ws, &["entity", "kept--thing", "--provenance"])["mutation_provenance"];
        assert_eq!(
            prov["check_state"],
            "checked_ok",
            "{}: {prov}",
            ws.display()
        );
        assert_eq!(prov["last_check"]["identity"], "checker-c", "{prov}");
        assert_eq!(prov["last_check"]["role"], "checker", "{prov}");
        assert_eq!(
            prov["last_check"]["checked_write"], recorded[0]["checked_write"],
            "{prov}"
        );
    }
    // The running engine reads the row at its next operation.
    let seen = running.check_records("kept").0;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].identity.as_deref(), Some("checker-c"));
    assert!(
        running
            .latest_check_record("kept", "kept--thing", "verification")
            .is_some()
    );
}

/// Four processes in two views record 16 checks each on one mem, started
/// together: all 64 rows land, each once, and the mem-repo stays sound.
/// Five rounds.
#[test]
fn concurrent_checks_from_several_processes_are_all_kept() {
    for round in 0..5 {
        let tmp = TempDir::new().unwrap();
        let (_store, gitdir) = store(tmp.path(), "store", &["One", "Two", "Three", "Four"]);
        let views = [
            view(tmp.path(), "v1", &gitdir, "write"),
            view(tmp.path(), "v2", &gitdir, "write"),
        ];
        let mut children = Vec::new();
        for p in 0..4 {
            let ws = &views[p % 2];
            let entries: Vec<serde_json::Value> = (0..16)
                .map(|i| {
                    let entity = ["kept--one", "kept--two", "kept--three", "kept--four"][i % 4];
                    serde_json::json!({
                        "id": entity,
                        "verdict": "ok",
                        "kind": format!("x-k{i}")
                    })
                })
                .collect();
            let file = ws.join(format!("batch-{p}.json"));
            fs::write(&file, serde_json::json!({ "checks": entries }).to_string()).unwrap();
            let bin = assert_cmd::cargo::cargo_bin("memstead");
            children.push(
                std::process::Command::new(bin)
                    .current_dir(ws)
                    .args([
                        "check",
                        "--from",
                        file.to_str().unwrap(),
                        "--identity",
                        &format!("p{p}"),
                        "--quiet",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
        }
        for child in children {
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "round {round}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        for ws in &views {
            let seen = rows(ws);
            assert_eq!(seen.len(), 64, "round {round}");
            let distinct: std::collections::BTreeSet<String> =
                seen.iter().map(|r| r.to_string()).collect();
            assert_eq!(distinct.len(), 64, "round {round}: a row twice");
        }
        let fsck = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&gitdir)
            .args(["fsck", "--no-progress"])
            .output()
            .unwrap();
        assert!(
            fsck.status.success(),
            "round {round}: {}",
            String::from_utf8_lossy(&fsck.stderr)
        );
    }
}

/// A mirror of the mem-repo carries the checks; a mem adopted by
/// `mem mount --remote` brings them, with the same standing.
#[test]
fn the_mems_checks_travel_with_a_copy_and_with_an_adopted_mem() {
    let tmp = TempDir::new().unwrap();
    let (store, gitdir) = store(tmp.path(), "store", &["Thing"]);
    ok(
        &store,
        &[
            "check",
            "kept--thing",
            "--verdict",
            "ok",
            "--identity",
            "checker-c",
            "--role",
            "checker",
        ],
    );
    let recorded = rows(&store);
    assert_eq!(recorded.len(), 1);

    // (a) A mirror, mounted by a fresh view.
    let mirror = tmp.path().join("mirror.git");
    let status = std::process::Command::new("git")
        .args(["clone", "--mirror", "--quiet"])
        .arg(&gitdir)
        .arg(&mirror)
        .status()
        .unwrap();
    assert!(status.success());
    let mirror = mirror.canonicalize().unwrap();
    let on_mirror = view(tmp.path(), "on-mirror", &mirror, "read-only");
    assert_eq!(rows(&on_mirror), recorded);

    // (b) Adopted into another store from a remote.
    let target = tmp.path().join("target");
    fs::create_dir_all(&target).unwrap();
    ok(&target, &["mem-repo", "init", "."]);
    ok(
        &target,
        &["workspace", "allow-create", "--schema", "*", "*"],
    );
    ok(
        &target,
        &["mem-repo", "remote-add", "src", gitdir.to_str().unwrap()],
    );
    ok(
        &target,
        &[
            "mem",
            "mount",
            "kept",
            "--remote",
            "src",
            "--capability",
            "write",
            "--operator-mode",
        ],
    );
    assert_eq!(rows(&target), recorded);
    let prov = &json(&target, &["entity", "kept--thing", "--provenance"])["mutation_provenance"];
    assert_eq!(prov["check_state"], "checked_ok", "{prov}");
    let target_gitdir = target.join("mem-repo/.git");
    assert!(!sha(&target_gitdir, "refs/memstead/checks/kept").is_empty());
}

/// The rows a workspace's ledger file holds for a writable git-branch mem
/// move into the mem's store when an engine first opens the workspace;
/// the explicit migration adds nothing after that, and rows of a mem the
/// workspace does not mount stay in the file and nowhere else.
#[test]
fn rows_held_in_a_workspace_ledger_reach_the_store_once() {
    let tmp = TempDir::new().unwrap();
    let (_store, gitdir) = store(tmp.path(), "store", &["Thing"]);
    let v = view(tmp.path(), "v", &gitdir, "write");
    let hash = json(&v, &["entity", "kept--thing"])["_hash"]
        .as_str()
        .unwrap()
        .to_string();
    // A ledger file as 0.28.0 wrote it: a row of `kept` and a row of a
    // mem this workspace does not mount.
    let checks = v.join(".memstead/state/checks");
    fs::create_dir_all(&checks).unwrap();
    let line = |entity: &str, h: &str| {
        format!(
            "{{\"ts\":1790000000,\"entity\":\"{entity}\",\"verdict\":\"ok\",\"entity_hash\":\"{h}\",\
             \"actor\":\"cli\",\"role\":\"checker\",\"identity\":\"checker-old\"}}\n"
        )
    };
    fs::write(
        checks.join("checks.jsonl"),
        format!(
            "{}{}",
            line("kept--thing", &hash),
            line("ghost--gone", "h0")
        ),
    )
    .unwrap();
    let _ = fs::remove_file(checks.join("store-migrated"));
    assert!(sha(&gitdir, "refs/memstead/checks/kept").is_empty());

    // The first opening moves the row of `kept`.
    let seen = rows(&v);
    assert_eq!(seen.len(), 1, "{seen:?}");
    let in_store = git(&gitdir, &["show", "refs/memstead/checks/kept:checks.jsonl"]);
    assert_eq!(in_store.lines().count(), 1, "{in_store}");
    assert!(in_store.contains("checker-old"));
    let prov = &json(&v, &["entity", "kept--thing", "--provenance"])["mutation_provenance"];
    assert_eq!(prov["check_state"], "checked_ok", "{prov}");

    // The explicit migration and a second opening add nothing.
    let moved = json(&v, &["checks", "--migrate"]);
    assert!(
        moved["migrated"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["moved"] == 0),
        "{moved}"
    );
    let in_store = git(&gitdir, &["show", "refs/memstead/checks/kept:checks.jsonl"]);
    assert_eq!(in_store.lines().count(), 1, "{in_store}");
    assert_eq!(rows(&v).len(), 1);
    // The unmounted mem's row stays in the file and nowhere else.
    assert!(sha(&gitdir, "refs/memstead/checks/ghost").is_empty());
    assert!(
        fs::read_to_string(checks.join("checks.jsonl"))
            .unwrap()
            .contains("ghost--gone")
    );
    let ghost = json(&v, &["checks", "--mem", "ghost"]);
    assert_eq!(ghost["checks"].as_array().unwrap().len(), 1, "{ghost}");

    // Another view of the same store reads the moved row at once.
    let w = view(tmp.path(), "w", &gitdir, "read-only");
    assert_eq!(rows(&w).len(), 1);
}
