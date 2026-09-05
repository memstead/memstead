//! `memstead status --remote`: the engine's read-only remote-staleness
//! report. Against a bare fixture remote it classifies every mounted mem
//! branch and the schemas ref (in sync, local-ahead, behind, forked,
//! missing locally, an unmounted remote branch, a branch not on the
//! remote), exits 6 (`REMOTE_STALE`) only when a ref is behind, forked or
//! missing locally, moves no ref, and fails open with a named notice when
//! no remote is configured or the remote cannot be reached.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;

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
    fs::write(store.join("config.json"), r#"{"schema": "default@1.0.0"}"#).unwrap();
    fs::write(
        dir.join("alpha.md"),
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nThe alpha entity.\n\n## Purpose\n\nSeed.\n",
    )
    .unwrap();
}

/// A mem-repo workspace with two mems and a bare `origin`, fully pushed.
fn seed() -> (TempDir, TempDir) {
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
    memstead()
        .current_dir(ws.path())
        .args(["push", "--all", "--quiet"])
        .assert()
        .success();
    (ws, remote)
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
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

/// `status --remote --json`: exit code, the report, the envelope line.
fn status_remote(ws: &Path) -> (i32, Value, String) {
    let out = memstead()
        .current_dir(ws)
        .args(["--json", "status", "--remote", "--quiet"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let split = stdout.find("\n{\"code\":\"REMOTE_STALE\"");
    let (report, envelope) = match split {
        Some(i) => (&stdout[..i], stdout[i..].trim().to_string()),
        None => (stdout.as_str(), String::new()),
    };
    let json: Value = serde_json::from_str(report)
        .unwrap_or_else(|e| panic!("status --json prints a report: {e}\n{stdout}"));
    (out.status.code().unwrap_or(-1), json, envelope)
}

fn state_of(json: &Value, ref_name: &str) -> String {
    json["remote"]["refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["ref_name"] == ref_name)
        .unwrap_or_else(|| panic!("no row for {ref_name}: {json}"))["state"]
        .as_str()
        .unwrap()
        .to_string()
}

fn local_heads(mem_repo: &Path) -> String {
    git(mem_repo, &["for-each-ref", "refs/heads", "refs/remotes"])
}

/// The classification walk, one workspace: in sync, local-ahead, behind,
/// forked, an unmounted remote branch, a missing local ref — with the exit
/// code following the stale set and no local ref moved by the report.
#[test]
fn status_remote_classifies_every_ref_and_exits_six_only_when_stale() {
    let (ws, remote) = seed();
    let root = ws.path();
    let mem_repo = root.join("mem-repo");

    // (1) Fully pushed: every ref in sync, exit 0.
    let before = local_heads(&mem_repo);
    let (code, json, envelope) = status_remote(root);
    assert_eq!(code, 0, "{envelope}");
    assert_eq!(json["remote"]["remote"], "origin");
    assert_eq!(json["remote"]["stale"], false);
    for r in [
        "refs/heads/__MEMSTEAD",
        "refs/heads/alpha",
        "refs/heads/beta",
    ] {
        assert_eq!(state_of(&json, r), "in_sync", "{json}");
    }
    assert_eq!(
        json["remote"]["refs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["ref_name"] == "refs/heads/__MEMSTEAD")
            .unwrap()["schemas_ref"],
        true
    );
    assert_eq!(local_heads(&mem_repo), before, "the report moves no ref");

    // (2) A local commit on alpha: local-ahead, never staleness.
    create_entity(root, "alpha", "Local work");
    let (code, json, _) = status_remote(root);
    assert_eq!(code, 0);
    assert_eq!(state_of(&json, "refs/heads/alpha"), "local_ahead");
    // The write also advanced the `__MEMSTEAD` ref, so at least alpha is ahead.
    assert!(json["remote"]["counts"]["local_ahead"].as_u64().unwrap() >= 1);

    // (3) Push it, then rewind the local branch to the seed commit: behind.
    memstead()
        .current_dir(root)
        .args(["push", "--all", "--quiet"])
        .assert()
        .success();
    let pushed = git(&mem_repo, &["rev-parse", "refs/heads/alpha"]);
    let seed_sha = git(&mem_repo, &["rev-parse", "refs/heads/alpha~1"]);
    git(
        &mem_repo,
        &["update-ref", "refs/heads/alpha", seed_sha.trim()],
    );
    let before = local_heads(&mem_repo);
    let (code, json, envelope) = status_remote(root);
    assert_eq!(code, 6, "behind is staleness: {envelope}");
    assert_eq!(state_of(&json, "refs/heads/alpha"), "behind");
    assert_eq!(state_of(&json, "refs/heads/beta"), "in_sync");
    assert_eq!(json["remote"]["stale"], true);
    assert!(envelope.contains("REMOTE_STALE"), "{envelope}");
    assert!(envelope.contains("refs/heads/alpha (behind)"), "{envelope}");
    assert_eq!(
        local_heads(&mem_repo),
        before,
        "a stale report moves no ref either"
    );

    // (4) A new local commit on the rewound branch: forked.
    create_entity(root, "alpha", "Diverging work");
    let (code, json, _) = status_remote(root);
    assert_eq!(code, 6);
    assert_eq!(state_of(&json, "refs/heads/alpha"), "forked");

    // (5) Restore the pushed tip locally: in sync again; then a remote branch
    //     nothing mounts is a notice, not staleness.
    git(
        &mem_repo,
        &["update-ref", "refs/heads/alpha", pushed.trim()],
    );
    git(
        &mem_repo,
        &["push", "-q", "origin", "refs/heads/alpha:refs/heads/ghost"],
    );
    let (code, json, _) = status_remote(root);
    assert_eq!(code, 0, "{json}");
    assert_eq!(state_of(&json, "refs/heads/ghost"), "unmounted_remote");
    assert_eq!(json["remote"]["stale"], false);

    // (6) A mounted branch the remote carries and this clone lacks:
    //     missing locally, staleness.
    let beta_sha = git(&mem_repo, &["rev-parse", "refs/heads/beta"]);
    git(&mem_repo, &["update-ref", "-d", "refs/heads/beta"]);
    let (code, json, envelope) = status_remote(root);
    assert_eq!(code, 6, "{envelope}");
    assert_eq!(state_of(&json, "refs/heads/beta"), "missing_local");
    assert!(
        envelope.contains("refs/heads/beta (missing_local)"),
        "{envelope}"
    );

    // (7) A branch nothing mounts, with a local ref of the same name, that
    //     another machine advanced: unmounted_remote, never staleness — the
    //     mount table decides, not the presence of a local ref.
    git(
        &mem_repo,
        &["update-ref", "refs/heads/beta", beta_sha.trim()],
    );
    git(
        &mem_repo,
        &["update-ref", "refs/heads/scratch", pushed.trim()],
    );
    git(
        &mem_repo,
        &[
            "push",
            "-q",
            "origin",
            "refs/heads/scratch:refs/heads/scratch",
        ],
    );
    let clone = TempDir::new().unwrap();
    git(
        clone.path(),
        &["clone", "-q", remote.path().to_str().unwrap(), "."],
    );
    git(clone.path(), &["checkout", "-q", "scratch"]);
    fs::write(clone.path().join("elsewhere.md"), "not graph state\n").unwrap();
    git(clone.path(), &["add", "elsewhere.md"]);
    git(
        clone.path(),
        &["commit", "-qm", "advance scratch elsewhere"],
    );
    git(clone.path(), &["push", "-q", "origin", "scratch"]);
    let (code, json, envelope) = status_remote(root);
    assert_eq!(code, 0, "an unmounted branch never gates: {envelope}");
    assert_eq!(state_of(&json, "refs/heads/scratch"), "unmounted_remote");
    assert_eq!(json["remote"]["stale"], false);

    // (8) Another machine pushes onto a MOUNTED branch: the remote head is
    //     not in this clone's object store, so the honest label is
    //     unfetched (stale), not a guessed forked; a fetch refines it to
    //     behind.
    git(clone.path(), &["checkout", "-q", "beta"]);
    fs::write(clone.path().join("other-machine.md"), "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Other\n\n## Identity\n\nPushed elsewhere.\n\n## Purpose\n\nSeed.\n").unwrap();
    git(clone.path(), &["add", "other-machine.md"]);
    git(
        clone.path(),
        &["commit", "-qm", "work from another machine"],
    );
    git(clone.path(), &["push", "-q", "origin", "beta"]);
    let (code, json, envelope) = status_remote(root);
    assert_eq!(code, 6, "{envelope}");
    assert_eq!(state_of(&json, "refs/heads/beta"), "unfetched");
    assert!(
        envelope.contains("refs/heads/beta (unfetched)"),
        "{envelope}"
    );
    memstead()
        .current_dir(root)
        .args(["fetch", "beta", "--quiet"])
        .assert()
        .success();
    let (code, json, _) = status_remote(root);
    assert_eq!(code, 6);
    assert_eq!(state_of(&json, "refs/heads/beta"), "behind");

    // The markdown carries the same classification and the reconcile line.
    let md = memstead()
        .current_dir(root)
        .args(["status", "--remote", "--quiet"])
        .output()
        .unwrap();
    let md = String::from_utf8(md.stdout).unwrap();
    assert!(md.contains("## Remote `origin`"), "{md}");
    assert!(md.contains("behind"), "{md}");
    assert!(md.contains("memstead fetch"), "{md}");
    drop(remote);
}

/// Fail open: no remote configured, and an unreachable remote, each exit 0
/// with a named notice and no refs compared.
#[test]
fn status_remote_fails_open_without_a_reachable_remote() {
    let ws = TempDir::new().unwrap();
    write_mem_dir(ws.path(), "alpha");
    let dir = ws.path().join("alpha");
    init_real_mem_repo_from_disk(ws.path(), &[(&dir, "alpha")]);

    let (code, json, _) = status_remote(ws.path());
    assert_eq!(code, 0);
    assert_eq!(json["remote"]["stale"], false);
    assert!(json["remote"]["refs"].as_array().unwrap().is_empty());
    let notices = json["remote"]["notices"].as_array().unwrap();
    assert!(
        notices.iter().any(|n| n
            .as_str()
            .unwrap()
            .contains("not configured or not reachable")),
        "{notices:?}"
    );

    // A remote that points nowhere reads the same way.
    memstead()
        .current_dir(ws.path())
        .args(["mem-repo", "remote-add", "origin", "--quiet"])
        .arg(ws.path().join("no-such-remote"))
        .assert()
        .success();
    let (code, json, _) = status_remote(ws.path());
    assert_eq!(code, 0, "{json}");
    assert!(!json["remote"]["notices"].as_array().unwrap().is_empty());
    assert!(json["remote"]["refs"].as_array().unwrap().is_empty());

    // Without --remote the payload carries no remote block at all.
    let out = memstead()
        .current_dir(ws.path())
        .args(["--json", "status", "--quiet"])
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.get("remote").is_none());
}
