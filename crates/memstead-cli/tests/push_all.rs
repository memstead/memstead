#![cfg(feature = "mem-repo")]
// `memstead push --all` ships only in the full build.

//! Integration tests for `memstead push --all`: every mounted
//! git-branch mem's branch plus `__MEMSTEAD`, fast-forward only,
//! silent when in sync, one line per ref moved, a refused ref named
//! while the others still go.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn write_mem_dir(root: &Path, name: &str) {
    let dir = root.join(name);
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(store.join("config.json"), r#"{"schema": "default@1.0.0"}"#).unwrap();
    fs::write(
        dir.join("alpha.md"),
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nThe alpha entity.\n\n## Purpose\n\nSeed.\n",
    )
    .unwrap();
}

/// Workspace with three git-branch mems and a local bare remote
/// configured as `origin`. Returns `(workspace, remote gitdir)`.
fn seed() -> (TempDir, TempDir) {
    let ws = TempDir::new().unwrap();
    for m in ["alpha", "beta", "gamma"] {
        write_mem_dir(ws.path(), m);
    }
    let dirs: Vec<_> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|m| (ws.path().join(m), *m))
        .collect();
    let refs: Vec<(&Path, &str)> = dirs.iter().map(|(p, m)| (p.as_path(), *m)).collect();
    init_real_mem_repo_from_disk(ws.path(), &refs);

    let remote = TempDir::new().unwrap();
    let status = StdCommand::new("git")
        .args(["init", "-q", "--bare"])
        .arg(remote.path())
        .status()
        .unwrap();
    assert!(status.success());
    memstead()
        .current_dir(ws.path())
        .args(["mem-repo", "remote-add", "origin", "--quiet"])
        .arg(remote.path())
        .assert()
        .success();
    (ws, remote)
}

fn create_entity(ws: &Path, mem: &str, title: &str) {
    memstead()
        .current_dir(ws)
        .args([
            "create",
            "--quiet",
            "--mem",
            mem,
            "--title",
            title,
            "--type",
            "spec",
            "--metadata",
            "level=M0",
            "--section",
            "identity=x",
            "--section",
            "purpose=y",
        ])
        .assert()
        .success();
}

fn push_all(ws: &Path) -> std::process::Output {
    memstead()
        .current_dir(ws)
        .args(["push", "--all", "--quiet"])
        .output()
        .unwrap()
}

fn moved_refs(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|l| l.split(' ').next().unwrap_or_default().to_string())
        .collect()
}

/// A1 AC1, assertion: exactly the lagging refs move, one line each,
/// nothing for the synced ref, and a second run prints nothing.
#[test]
fn push_all_moves_exactly_the_lagging_refs_and_is_silent_when_in_sync() {
    let (ws, _remote) = seed();

    // First run: everything is new on the remote.
    let out = push_all(ws.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        moved_refs(&out.stdout),
        vec![
            "refs/heads/__MEMSTEAD",
            "refs/heads/alpha",
            "refs/heads/beta",
            "refs/heads/gamma"
        ]
    );

    // Second run: in sync, silent.
    let out = push_all(ws.path());
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Move alpha, beta and `__MEMSTEAD` (a mem description lives
    // there); gamma stays in sync.
    create_entity(ws.path(), "alpha", "Second alpha");
    create_entity(ws.path(), "beta", "Second beta");
    memstead()
        .current_dir(ws.path())
        .args(["mem", "set-description", "alpha", "moved", "--quiet"])
        .assert()
        .success();

    let out = push_all(ws.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        moved_refs(&out.stdout),
        vec![
            "refs/heads/__MEMSTEAD",
            "refs/heads/alpha",
            "refs/heads/beta"
        ]
    );
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        // `<ref> <previous> -> <new>`: three tokens around the arrow.
        let parts: Vec<&str> = line.split(' ').collect();
        assert_eq!(parts.len(), 4, "line = {line}");
        assert_eq!(parts[2], "->");
        assert_ne!(parts[1], "<new>", "the ref existed on the remote already");
    }

    let out = push_all(ws.path());
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

/// A1 AC1, refusal complement: a branch made non-fast-forward on the
/// remote is refused by name with `NON_FAST_FORWARD`, the other
/// lagging refs still go, the exit is non-zero; `--force` on `--all`
/// is refused at the parser.
#[test]
fn push_all_refuses_a_non_fast_forward_ref_and_still_pushes_the_rest() {
    let (ws, remote) = seed();
    assert!(push_all(ws.path()).status.success());

    // Diverge `beta` on the remote through a second clone.
    let other = TempDir::new().unwrap();
    let status = StdCommand::new("git")
        .args(["clone", "-q", "--branch", "beta"])
        .arg(remote.path())
        .arg(other.path())
        .status()
        .unwrap();
    assert!(status.success());
    for args in [
        vec![
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "remote-only",
        ],
        vec!["push", "-q", "origin", "beta"],
    ] {
        let status = StdCommand::new("git")
            .arg("-C")
            .arg(other.path())
            .args(&args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    create_entity(ws.path(), "beta", "Local beta");
    create_entity(ws.path(), "gamma", "Local gamma");

    let out = push_all(ws.path());
    assert_eq!(
        out.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // gamma still went (and `__MEMSTEAD`, which every create touches);
    // beta did not.
    let moved = moved_refs(&out.stdout);
    assert!(moved.contains(&"refs/heads/gamma".to_string()), "{moved:?}");
    assert!(!moved.contains(&"refs/heads/beta".to_string()), "{moved:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("NON_FAST_FORWARD"), "{stderr}");
    assert!(stderr.contains("refs/heads/beta"), "{stderr}");

    // The JSON surface carries the same outcome under `details` and
    // prints no second document.
    let out = memstead()
        .current_dir(ws.path())
        .args(["push", "--all", "--quiet", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let text = String::from_utf8_lossy(&out.stdout);
    let envelope: serde_json::Value = serde_json::from_str(text.trim()).expect("one JSON document");
    assert_eq!(envelope["code"], "NON_FAST_FORWARD");
    assert_eq!(
        envelope["details"]["refused"][0]["ref_name"],
        "refs/heads/beta"
    );
    assert_eq!(envelope["details"]["refused"][0]["mem"], "beta");

    // `--force` is a single-mem affordance.
    memstead()
        .current_dir(ws.path())
        .args(["push", "--all", "--force", "--quiet"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("cannot be used with"));
}
