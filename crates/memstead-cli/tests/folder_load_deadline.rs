//! A fresh `memstead` process on a folder mem that holds a commit which
//! is decided and not applied, while another process holds the mem's
//! write lock: the load waits for that lock no longer than the
//! workspace's write-lock deadline.
//!
//! The fixture: a workspace with a folder mem `papers` and a second mem
//! `other`. The test lays a commit record by hand (staged content plus
//! the record that names it, the state a writer leaves when it stops
//! between its commit point and the end of its commit) and holds the
//! mem's write lock itself, as the stopped writer would.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn run(ws: &Path, args: &[&str]) -> (Duration, std::process::Output) {
    let started = Instant::now();
    let out = memstead()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap();
    (started.elapsed(), out)
}

fn ok(ws: &Path, args: &[&str]) -> String {
    let (_, out) = run(ws, args);
    assert!(
        out.status.success(),
        "`memstead {}` failed:\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn printed(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
}

fn spec(ws: &Path, mem: &str, title: &str) {
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
        ],
    );
}

/// A workspace with the folder mem `papers` (one entity) and the mem
/// `other` (one entity), under the given `[mutations]` lines.
fn workspace(root: &Path, mutations: &str) -> PathBuf {
    let ws = root.join("ws");
    fs::create_dir_all(&ws).unwrap();
    ok(&ws, &["mem-repo", "init", "."]);
    ok(&ws, &["workspace", "allow-create", "--schema", "*", "*"]);
    ok(
        &ws,
        &[
            "mem",
            "init",
            "papers",
            "--storage",
            "folder",
            "--schema",
            "default@1.3.0",
        ],
    );
    ok(&ws, &["mem", "init", "other", "--schema", "default@1.3.0"]);
    spec(&ws, "papers", "Held");
    spec(&ws, "other", "Elsewhere");
    if !mutations.is_empty() {
        let toml = ws.join(".memstead/workspace.toml");
        let mut text = fs::read_to_string(&toml).unwrap();
        text.push_str("\n[mutations]\n");
        text.push_str(mutations);
        text.push('\n');
        fs::write(&toml, text).unwrap();
    }
    ws
}

/// Lay a commit that is decided and not applied into the folder mem: the
/// staged content of a new entity `late`, and the record that names it.
fn lay_unapplied_commit(mem: &Path) {
    let staged = fs::read_to_string(mem.join("held.md"))
        .unwrap()
        .replace("# Held", "# Late");
    fs::write(mem.join("late.md.tmp"), staged).unwrap();
    fs::write(
        mem.join(".memstead/commit.json"),
        r#"{"v":1,"ops":[{"op":"upsert","path":"late.md","tmp":"late.md.tmp"}],"ledger":[]}"#,
    )
    .unwrap();
}

/// Hold the mem's write lock, as another process would.
fn hold_write_lock(mem: &Path) -> fs::File {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(mem.join(".memstead/locks/write.lock"))
        .expect("the mem has a write lock file");
    lock.lock().expect("the lock is free");
    lock
}

/// Every path under a directory with its bytes.
fn every_file(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    found.sort();
    found
}

/// Under a deadline of one second a fresh process that names the mem
/// returns within the deadline and a margin, with the mem out of service
/// for the reason `COMMIT_NOT_APPLIED`, which states the wait and the
/// setting; the mem's files are untouched and another mem is served.
/// Once the lock is free the next process completes the commit, reports
/// it and serves the mem.
#[test]
fn a_fresh_process_behind_a_held_lock_returns_at_the_deadline() {
    let root = TempDir::new().unwrap();
    let ws = workspace(root.path(), "write_lock_timeout_secs = 1");
    let mem = ws.join("papers");
    lay_unapplied_commit(&mem);
    let before = every_file(&mem);
    let held = hold_write_lock(&mem);

    let within = Duration::from_secs(3);
    for args in [
        vec!["entity", "papers--held", "--json"],
        vec!["changes", "--mem", "papers", "--since", "", "--json"],
        vec![
            "create",
            "--mem",
            "papers",
            "--title",
            "Not Now",
            "--type",
            "spec",
            "--metadata",
            "level=M0",
            "--section",
            "identity=i",
            "--section",
            "purpose=p",
            "--json",
        ],
    ] {
        let (took, out) = run(&ws, &args);
        let said = printed(&out);
        assert!(!out.status.success(), "{}: {said}", args.join(" "));
        assert!(took < within, "{}: {took:?}", args.join(" "));
        assert!(
            said.contains("MEM_QUARANTINED") && said.contains("COMMIT_NOT_APPLIED"),
            "{}: {said}",
            args.join(" ")
        );
        assert!(said.contains("papers"), "{said}");
        assert!(
            said.contains("write_lock_timeout_secs") && said.contains("1 s"),
            "{}: {said}",
            args.join(" ")
        );
    }
    // The way back for a mem that is out of service is a reload, and a
    // reload behind the lock that is still held waits the workspace's
    // deadline too (once at boot, once for its own load), not a default.
    let (took, out) = run(&ws, &["reload", "--mem", "papers", "--json"]);
    let said = printed(&out);
    assert!(!out.status.success(), "{said}");
    assert!(took < Duration::from_secs(5), "reload: {took:?}");
    assert!(
        said.contains("COMMIT_NOT_APPLIED") && said.contains("1 s"),
        "{said}"
    );
    assert_eq!(every_file(&mem), before, "nothing of the mem was touched");

    // Another mem of the same workspace is answered, and the roster
    // names the reason.
    let (took, out) = run(&ws, &["entity", "other--elsewhere"]);
    assert!(out.status.success(), "{}", printed(&out));
    assert!(took < within, "{took:?}");
    let listed = ok(&ws, &["mem", "list"]);
    assert!(
        listed.contains("`other`") && listed.contains("[COMMIT_NOT_APPLIED]"),
        "{listed}"
    );

    // The lock is free: the next process completes the commit, says so
    // and serves the mem with the commit applied.
    drop(held);
    let health = ok(&ws, &["health", "--json"]);
    assert!(health.contains("INTERRUPTED_COMMIT_COMPLETED"), "{health}");
    assert!(!mem.join(".memstead/commit.json").exists());
    assert!(ok(&ws, &["entity", "papers--late"]).contains("# Late"));
    assert!(ok(&ws, &["entity", "papers--held"]).contains("# Held"));
    assert!(!ok(&ws, &["mem", "list"]).contains("Quarantined"));
}

/// With the deadline set to `0` the same process is still waiting after
/// three seconds, and completes the commit once the lock is released.
#[test]
fn without_a_deadline_a_fresh_process_waits_until_the_lock_is_free() {
    let root = TempDir::new().unwrap();
    let ws = workspace(root.path(), "write_lock_timeout_secs = 0");
    let mem = ws.join("papers");
    lay_unapplied_commit(&mem);
    let held = hold_write_lock(&mem);

    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("memstead"))
        .current_dir(&ws)
        .args(["entity", "papers--late", "--quiet"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the process is still waiting for the lock"
    );
    drop(held);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", printed(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("# Late"));
    assert!(!mem.join(".memstead/commit.json").exists());
}

/// A writer that finishes its own commit within the deadline lets the
/// load through: the waiting process serves the mem with the commit
/// applied and reports no commit it completed itself.
#[test]
fn a_writer_that_finishes_within_the_deadline_lets_the_load_through() {
    let root = TempDir::new().unwrap();
    let ws = workspace(root.path(), "write_lock_timeout_secs = 20");
    let mem = ws.join("papers");
    lay_unapplied_commit(&mem);
    let held = hold_write_lock(&mem);

    let child = StdCommand::new(assert_cmd::cargo::cargo_bin("memstead"))
        .current_dir(&ws)
        .args(["health", "--json", "--quiet"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(700));
    // The writer finishes: its staged content moves into place, its
    // record goes, and it lets the lock go.
    fs::rename(mem.join("late.md.tmp"), mem.join("late.md")).unwrap();
    fs::remove_file(mem.join(".memstead/commit.json")).unwrap();
    drop(held);

    let started = Instant::now();
    let out = child.wait_with_output().unwrap();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(out.status.success(), "{}", printed(&out));
    assert!(
        !printed(&out).contains("INTERRUPTED_COMMIT_COMPLETED"),
        "the load completed nothing itself: {}",
        printed(&out)
    );
    assert!(ok(&ws, &["entity", "papers--late"]).contains("# Late"));
}

/// A mem with no commit record is loaded while its lock is held, at
/// once, under the default deadline: nothing waits where there is no
/// unapplied commit.
#[test]
fn a_whole_mem_is_loaded_under_a_held_lock_without_waiting() {
    let root = TempDir::new().unwrap();
    let ws = workspace(root.path(), "");
    let mem = ws.join("papers");
    let held = hold_write_lock(&mem);
    let (took, out) = run(&ws, &["entity", "papers--held"]);
    assert!(out.status.success(), "{}", printed(&out));
    assert!(took < Duration::from_secs(3), "{took:?}");
    drop(held);
}
