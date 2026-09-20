//! `memstead mem fork`: the CLI face of `fork_mem`. One local fork
//! end to end (the JSON envelope, the `mem list` origin line in both
//! shapes, the help text naming source, sha, name and remote), one
//! typed refusal, and the folder-only workspace refusal.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use serde_json::Value;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn write_mem_dir(root: &Path, name: &str) {
    let dir = root.join(name);
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{"schema": "default@1.0.0", "version": "0.2.0", "description": "the alpha mem"}"#,
    )
    .unwrap();
    fs::write(
        dir.join("alpha.md"),
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nThe alpha entity.\n\n## Purpose\n\nSeed.\n",
    )
    .unwrap();
}

/// A mem-repo workspace with two mems, `alpha` and `beta`.
fn seed() -> TempDir {
    let ws = TempDir::new().unwrap();
    for m in ["alpha", "beta"] {
        write_mem_dir(ws.path(), m);
    }
    let dirs: Vec<_> = ["alpha", "beta"]
        .iter()
        .map(|m| (ws.path().join(m), *m))
        .collect();
    let refs: Vec<(&Path, &str)> = dirs.iter().map(|(p, m)| (p.as_path(), *m)).collect();
    init_real_mem_repo_from_disk(ws.path(), &refs);
    ws
}

fn json_stdout(out: &std::process::Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(text.trim())
        .unwrap_or_else(|e| panic!("one JSON document on stdout; got:\n{text}\n({e})"))
}

/// The local fork through the binary: the envelope names the fork,
/// its ancestor and its schema; `mem list` shows the origin in JSON
/// and in markdown, and shows none for the source.
#[test]
fn mem_fork_creates_the_fork_and_mem_list_shows_its_origin() {
    let ws = seed();
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "mem",
            "fork",
            "alpha",
            "alpha-fork",
            "--operator-mode",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let envelope = json_stdout(&out);
    assert_eq!(envelope["name"], "alpha-fork");
    assert_eq!(envelope["branch_ref"], "refs/heads/alpha-fork");
    assert_eq!(envelope["schema_ref"], "default@1.0.0");
    assert_eq!(envelope["forked_from"]["mem"], "alpha");
    let sha = envelope["forked_from"]["sha"].as_str().unwrap();
    assert_eq!(sha.len(), 40, "{sha}");
    assert!(sha.chars().all(|c| c.is_ascii_hexdigit()), "{sha}");
    assert!(envelope["forked_from"]["remote"].is_null());
    assert!(envelope["inherited_grants"].is_null());
    assert_eq!(envelope["warnings"], serde_json::json!([]));

    // The roster: the fork carries its origin, the source none.
    let out = memstead()
        .current_dir(ws.path())
        .args(["mem", "list", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let roster = json_stdout(&out);
    let rows = roster["mems"].as_array().unwrap();
    let fork = rows
        .iter()
        .find(|r| r["name"] == "alpha-fork")
        .expect("the fork is listed");
    assert_eq!(fork["forked_from"]["mem"], "alpha");
    assert_eq!(fork["forked_from"]["sha"], sha);
    assert!(fork["forked_from"]["remote"].is_null());
    assert_eq!(fork["description"], "the alpha mem");
    assert_eq!(fork["version"], "0.2.0");
    assert_eq!(fork["entity_count"], 1);
    let source = rows.iter().find(|r| r["name"] == "alpha").unwrap();
    assert!(source["forked_from"].is_null());

    let out = memstead()
        .current_dir(ws.path())
        .args(["mem", "list", "--quiet"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| l.contains("`alpha-fork`"))
        .unwrap_or_else(|| panic!("the fork's line: {text}"));
    assert!(
        line.contains(&format!("forked from `alpha`@{}", &sha[..12])),
        "{line}"
    );
    let source_line = text.lines().find(|l| l.contains("`alpha`")).unwrap();
    assert!(!source_line.contains("forked from"), "{source_line}");
}

/// A typed refusal rides the JSON envelope with its code; the
/// existing mem is untouched.
#[test]
fn mem_fork_refusal_is_typed() {
    let ws = seed();
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "mem",
            "fork",
            "alpha",
            "beta",
            "--operator-mode",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let envelope = json_stdout(&out);
    assert_eq!(envelope["code"], "MEM_NAME_COLLISION");

    // A malformed `<source>@<sha>` is the CLI's own refusal.
    let out = memstead()
        .current_dir(ws.path())
        .args(["mem", "fork", "alpha@", "gamma", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(json_stdout(&out)["code"], "INVALID_INPUT");
}

/// A folder-only workspace has no branch to fork: `INVALID_INPUT`
/// naming the reason, from the engine.
#[test]
fn mem_fork_refuses_a_folder_only_workspace() {
    let ws = TempDir::new().unwrap();
    memstead()
        .current_dir(ws.path())
        .args([
            "init",
            "--name",
            "notes",
            "--schema",
            "default@1.3.0",
            "--quiet",
        ])
        .assert()
        .success();
    let out = memstead()
        .current_dir(ws.path())
        .args(["mem", "fork", "notes", "notes-fork", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let envelope = json_stdout(&out);
    assert_eq!(envelope["code"], "INVALID_INPUT", "{envelope}");
    let message = envelope["message"].as_str().unwrap_or_default();
    assert!(message.contains("mem-repo"), "{message}");
}

/// The help text names source, sha, name and remote.
#[test]
fn mem_fork_help_names_source_sha_name_and_remote() {
    let out = memstead().args(["mem", "fork", "--help"]).output().unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for needle in ["<SOURCE>", "<sha>", "<NAME>", "--remote <REMOTE>"] {
        assert!(help.contains(needle), "help names {needle}: {help}");
    }
}
