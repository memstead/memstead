//! Integration tests for `memstead conflicts` — the sanctioned door
//! for git merge conflicts in folder mems (backlog-sweep plan 07).
//! Runs the real binary; the conflicted file is written raw to disk,
//! simulating exactly the damage an ordinary `git merge` does to a
//! hand-committed folder mem.

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn stdout_of(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).expect("stdout is UTF-8")
}

const CONFLICTED: &str = "---\ntype: spec\n---\n# Torn\n\n## Identity\n\n\
<<<<<<< HEAD\nours line\n=======\ntheirs line\n>>>>>>> feature\n\n## Purpose\n\nshared\n";

/// End-to-end through the binary: a merge-conflicted entity file in a
/// quickstart (folder) workspace lists under `conflicts list`, resolves
/// to a side with a note, and afterwards the workspace serves the
/// entity cleanly. Complements: resolving it again refuses
/// `NOT_CONFLICTED`; an unknown side refuses `INVALID_INPUT`.
#[test]
fn conflicts_list_and_resolve_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("ws");
    memstead()
        .args(["quickstart", "--agent", "claude-code"])
        .arg(&root)
        .assert()
        .success();

    // Simulate git's damage: a merge wrote conflict markers into an
    // entity file. (Raw write is the SIMULATION of the outside event —
    // the repair below goes through the engine.)
    std::fs::write(root.join("torn.md"), CONFLICTED).unwrap();

    // List identifies exactly the conflicted entity.
    let out = stdout_of(
        memstead()
            .current_dir(&root)
            .args(["--json", "conflicts", "list"])
            .assert()
            .success(),
    );
    let listed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(listed["count"], 1, "got: {listed}");
    assert_eq!(listed["conflicts"][0]["id"], "ws--torn");
    assert_eq!(listed["conflicts"][0]["file_path"], "torn.md");

    // Criterion 3, agent-facing: the failure mode names the remedy on
    // a surface an agent actually reads — the CLI health report
    // carries the per-file load error whose message names the resolve
    // operation.
    let out = stdout_of(
        memstead()
            .current_dir(&root)
            .args(["--json", "health"])
            .assert()
            .success(),
    );
    let health: serde_json::Value = serde_json::from_str(&out).unwrap();
    let load_errors = health["load_errors"]
        .as_array()
        .expect("health carries load_errors for the conflicted file");
    assert!(
        load_errors.iter().any(|e| {
            e["file"].as_str().unwrap_or("").ends_with("torn.md")
                && e["error"]
                    .as_str()
                    .unwrap_or("")
                    .contains("memstead conflicts resolve")
        }),
        "the load failure names the remedy: {load_errors:?}"
    );

    // Resolve to theirs, with a note.
    let out = stdout_of(
        memstead()
            .current_dir(&root)
            .args([
                "--json",
                "conflicts",
                "resolve",
                "ws--torn",
                "--side",
                "theirs",
                "--note",
                "keeping upstream wording",
            ])
            .assert()
            .success(),
    );
    let resolved: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(resolved["id"], "ws--torn");
    assert_eq!(resolved["side"], "theirs");

    // The mem is fully readable again; the entity reads validly with
    // the kept side's content and no marker residue.
    let out = stdout_of(
        memstead()
            .current_dir(&root)
            .args(["--json", "entity", "ws--torn"])
            .assert()
            .success(),
    );
    assert!(out.contains("theirs line") && !out.contains("ours line"));
    let on_disk = std::fs::read_to_string(root.join("torn.md")).unwrap();
    assert!(!on_disk.contains("<<<<<<<"));

    // Provenance: the resolution is an attributed ledger entry with
    // the note — never an untracked file swap.
    let ledger = std::fs::read_to_string(root.join(".memstead").join("changes.jsonl")).unwrap();
    assert!(
        ledger.contains("ws--torn") && ledger.contains("keeping upstream wording"),
        "ledger records the resolution: {ledger}"
    );

    // After resolution the health report is clean — the stale load
    // error is replaced, not accumulated.
    let out = stdout_of(
        memstead()
            .current_dir(&root)
            .args(["--json", "health"])
            .assert()
            .success(),
    );
    let health: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        health.get("load_errors").is_none(),
        "clean workspace omits load_errors: {health}"
    );

    // Nothing conflicted remains.
    let out = stdout_of(
        memstead()
            .current_dir(&root)
            .args(["--json", "conflicts", "list"])
            .assert()
            .success(),
    );
    let listed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(listed["count"], 0);

    // Complement: resolving the now-clean entity refuses typed.
    let assert = memstead()
        .current_dir(&root)
        .args([
            "--json",
            "conflicts",
            "resolve",
            "ws--torn",
            "--side",
            "ours",
        ])
        .assert()
        .failure();
    let err = stdout_of(assert);
    assert!(err.contains("NOT_CONFLICTED"), "got: {err}");

    // Complement: an unknown side refuses INVALID_INPUT.
    let assert = memstead()
        .current_dir(&root)
        .args([
            "--json",
            "conflicts",
            "resolve",
            "ws--torn",
            "--side",
            "both",
        ])
        .assert()
        .failure();
    let err = stdout_of(assert);
    assert!(err.contains("INVALID_INPUT"), "got: {err}");
}
