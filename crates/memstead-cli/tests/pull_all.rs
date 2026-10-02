//! Integration tests for `memstead pull --all`, the inverse of
//! `push --all`: a workspace whose mem-repo lacks or lags the remote's
//! refs is brought to the remote's state, the schema-and-config ref
//! first, then every mounted mem's branch, fast-forward only.
//!
//! The fixture is two workspaces sharing one bare remote. Workspace A
//! authors a schema no built-in provides (`widgets`), pins one mem to
//! it, writes, checks and pushes. Workspace B carries A's tracked
//! engine state and a mem-repo that is a plain `git clone` of the
//! remote: only remote-tracking refs, no local mem branch and no local
//! schema-and-config ref, so the `widgets` mem starts quarantined.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn run(ws: &Path, args: &[&str]) -> std::process::Output {
    memstead()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap()
}

fn ok(ws: &Path, args: &[&str]) -> String {
    let out = run(ws, args);
    assert!(
        out.status.success(),
        "`memstead {}` failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    _root: TempDir,
    remote: PathBuf,
    a: PathBuf,
    b: PathBuf,
}

/// Workspace A with a `widgets@0.1.0` mem (`knowledge`) and a
/// built-in-schema mem (`kitchen`), one entity each, a check record on
/// the widgets entity, everything pushed to a bare remote.
fn workspace_a() -> Fixture {
    workspace_a_seeded(false)
}

/// `older_seed`: the backup's schema-and-config ref starts at a root
/// commit other than the one this engine's `mem-repo init` writes, as a
/// workspace bootstrapped by an earlier engine version has.
fn workspace_a_seeded(older_seed: bool) -> Fixture {
    let root = TempDir::new().unwrap();
    let remote = root.path().join("remote.git");
    let a = root.path().join("a");
    let b = root.path().join("b");
    fs::create_dir_all(&a).unwrap();
    git(root.path(), &["init", "-q", "--bare", "remote.git"]);

    ok(&a, &["mem-repo", "init", "."]);
    if older_seed {
        let repo = a.join("mem-repo");
        let empty_tree = git(&repo, &["hash-object", "-t", "tree", "/dev/null"]);
        let seed = git(
            &repo,
            &[
                "-c",
                "user.name=older-cli",
                "-c",
                "user.email=older@cli",
                "commit-tree",
                &empty_tree,
                "-m",
                "an older engine's seed",
            ],
        );
        git(&repo, &["update-ref", "refs/heads/__MEMSTEAD", &seed]);
    }
    ok(&a, &["workspace", "allow-create", "--schema", "*", "k*"]);
    ok(&a, &["schema", "new", "widgets"]);
    ok(&a, &["schema", "install", "widgets"]);
    ok(
        &a,
        &["mem", "init", "knowledge", "--schema", "widgets@0.1.0"],
    );
    ok(&a, &["mem", "init", "kitchen", "--schema", "default@1.3.0"]);
    ok(
        &a,
        &[
            "create",
            "--mem",
            "knowledge",
            "--title",
            "First note",
            "--type",
            "note",
            "--section",
            "summary=Hello.",
        ],
    );
    ok(
        &a,
        &[
            "create",
            "--mem",
            "kitchen",
            "--title",
            "Stove",
            "--type",
            "spec",
            "--metadata",
            "level=M0",
            "--section",
            "identity=x",
            "--section",
            "purpose=y",
        ],
    );
    ok(
        &a,
        &[
            "check",
            "knowledge--first-note",
            "--verdict",
            "ok",
            "--method",
            "read it",
            "--identity",
            "reader",
        ],
    );
    ok(
        &a,
        &["mem-repo", "remote-add", "origin", remote.to_str().unwrap()],
    );
    ok(&a, &["push", "--all"]);
    Fixture {
        _root: root,
        remote,
        a,
        b,
    }
}

/// Workspace B: A's tracked engine state without its check ledger, and
/// a mem-repo that is a plain clone of the remote.
fn clone_b(f: &Fixture) {
    fs::create_dir_all(&f.b).unwrap();
    copy_dir(&f.a.join(".memstead"), &f.b.join(".memstead"));
    let _ = fs::remove_dir_all(f.b.join(".memstead/state/checks"));
    let out = StdCommand::new("git")
        .args(["clone", "-q"])
        .arg(&f.remote)
        .arg(f.b.join("mem-repo"))
        .output()
        .unwrap();
    assert!(out.status.success());
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn status_json(ws: &Path) -> serde_json::Value {
    let out = run(ws, &["status", "--json"]);
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "status --json did not parse ({e}):\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn quarantined(ws: &Path) -> Vec<String> {
    status_json(ws)["quarantined"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|q| q["mem"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn entity_hash(ws: &Path, id: &str) -> String {
    let out = ok(ws, &["entity", id, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    v["_hash"].as_str().unwrap().to_string()
}

/// 01-a, assertion: a pushed workspace round-trips through
/// `pull --all` into a plain clone, the schema-only-on-the-remote mem
/// included, check rows and all, and is in sync afterwards.
#[test]
fn a_plain_clone_is_restored_by_pull_all() {
    let f = workspace_a();
    clone_b(&f);
    assert_eq!(quarantined(&f.b), vec!["knowledge".to_string()]);

    let out = ok(&f.b, &["pull", "--all"]);
    let moved: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("refs/"))
        .map(|l| l.split(' ').next().unwrap())
        .collect();
    // The schema-and-config ref first, then the mem branches.
    assert_eq!(moved.first(), Some(&"refs/heads/__MEMSTEAD"), "{out}");
    assert!(moved.contains(&"refs/heads/knowledge"), "{out}");
    assert!(moved.contains(&"refs/heads/kitchen"), "{out}");
    assert_eq!(moved.len(), 3, "{out}");
    assert!(out.contains("mem `knowledge` serves again"), "{out}");

    assert!(quarantined(&f.b).is_empty());
    for id in ["knowledge--first-note", "kitchen--stove"] {
        assert_eq!(entity_hash(&f.a, id), entity_hash(&f.b, id), "{id}");
    }
    let ledger = fs::read_to_string(f.b.join(".memstead/state/checks/checks.jsonl"))
        .expect("the check rows arrive with the pull");
    assert!(ledger.contains("knowledge--first-note"), "{ledger}");

    // In sync both ways now: nothing to push, nothing stale.
    let push = run(&f.b, &["push", "--all"]);
    assert!(push.status.success());
    assert!(
        push.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&push.stdout)
    );
    let remote = run(&f.b, &["status", "--remote"]);
    assert!(
        remote.status.success(),
        "{}",
        String::from_utf8_lossy(&remote.stdout)
    );
    // A second pull has nothing to move.
    let again = ok(&f.b, &["pull", "--all"]);
    assert!(!again.lines().any(|l| l.starts_with("refs/")), "{again}");
}

/// 01-a, refusal complement: a ref with local commits the remote lacks
/// is never moved; it is refused by name while the other refs still
/// move, the exit is non-zero, and the local commit stays reachable.
#[test]
fn a_diverged_branch_is_refused_by_name_and_the_rest_still_moves() {
    let f = workspace_a();
    clone_b(&f);
    ok(&f.b, &["pull", "--all"]);

    ok(
        &f.b,
        &[
            "create",
            "--mem",
            "knowledge",
            "--title",
            "Local only",
            "--type",
            "note",
            "--section",
            "summary=B.",
        ],
    );
    let local_tip = git(
        &f.b.join("mem-repo"),
        &["rev-parse", "refs/heads/knowledge"],
    );
    ok(
        &f.a,
        &[
            "create",
            "--mem",
            "knowledge",
            "--title",
            "Remote only",
            "--type",
            "note",
            "--section",
            "summary=A.",
        ],
    );
    ok(
        &f.a,
        &[
            "create",
            "--mem",
            "kitchen",
            "--title",
            "Oven",
            "--type",
            "spec",
            "--metadata",
            "level=M0",
            "--section",
            "identity=x",
            "--section",
            "purpose=y",
        ],
    );
    ok(&f.a, &["push", "--all"]);

    let out = run(&f.b, &["pull", "--all"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("LOCAL_DIVERGENCE"), "{stderr}");
    assert!(stderr.contains("refs/heads/knowledge"), "{stderr}");
    assert_eq!(
        git(
            &f.b.join("mem-repo"),
            &["rev-parse", "refs/heads/knowledge"]
        ),
        local_tip,
        "the diverged branch must not move"
    );
    assert_eq!(
        git(&f.b.join("mem-repo"), &["rev-parse", "refs/heads/kitchen"]),
        git(&f.a.join("mem-repo"), &["rev-parse", "refs/heads/kitchen"]),
        "the other branch still moves"
    );
    ok(&f.b, &["entity", "kitchen--oven"]);
}

/// 01-b, assertion: content written under a newer schema generation
/// that only the remote's schema ref carries is pulled, not refused
/// (the pin moves with the schema ref, the gate validates against it).
#[test]
fn a_newer_schema_generation_arrives_with_its_content() {
    let f = workspace_a();
    clone_b(&f);
    ok(&f.b, &["pull", "--all"]);

    // A: widgets@0.2.0 adds an optional metadata field, the mem moves
    // to it, an entity uses the field.
    let pkg = f.a.join("widgets");
    let manifest = fs::read_to_string(pkg.join("schema.yaml")).unwrap();
    fs::write(
        pkg.join("schema.yaml"),
        manifest.replace("version: 0.1.0", "version: 0.2.0"),
    )
    .unwrap();
    let note = fs::read_to_string(pkg.join("types/note.yaml")).unwrap();
    fs::write(
        pkg.join("types/note.yaml"),
        note.replace(
            "metadata_fields:\n",
            "metadata_fields:\n  - key: color\n    required: false\n    description: A colour.\n    field_type: string\n",
        ),
    )
    .unwrap();
    ok(&f.a, &["schema", "install", "widgets"]);
    ok(&f.a, &["mem", "set-schema", "knowledge", "widgets@0.2.0"]);
    ok(
        &f.a,
        &[
            "create",
            "--mem",
            "knowledge",
            "--title",
            "Red note",
            "--type",
            "note",
            "--metadata",
            "color=red",
            "--section",
            "summary=Red.",
        ],
    );
    ok(&f.a, &["push", "--all"]);

    // The single-mem pull refuses: B's local pin is still 0.1.0.
    let single = run(&f.b, &["pull", "knowledge"]);
    assert!(!single.status.success());
    assert!(
        String::from_utf8_lossy(&single.stderr).contains("SCHEMA_VIOLATION_IN_FETCH"),
        "{}",
        String::from_utf8_lossy(&single.stderr)
    );

    ok(&f.b, &["pull", "--all"]);
    let red = ok(&f.b, &["entity", "knowledge--red-note"]);
    assert!(red.contains("red"), "{red}");
    assert!(quarantined(&f.b).is_empty());
}

/// 01-b, refusal complement: a mem whose pin the remote does not carry
/// either stays quarantined and is named with its reason, and content
/// that violates the schema it resolves to is refused with its branch
/// left where it was.
#[test]
fn quarantines_the_pull_cannot_repair_stay_and_invalid_content_is_refused() {
    let f = workspace_a();
    clone_b(&f);

    // A roster entry whose pin resolves nowhere, local or remote.
    let roster_path = f.b.join(".memstead/state/mounts.json");
    let mut roster: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&roster_path).unwrap()).unwrap();
    roster["mounts"].as_array_mut().unwrap().push(serde_json::json!({
        "mem": "phantom",
        "schema": "nowhere@1.0.0",
        "storage": {"type": "git-branch", "gitdir": "mem-repo/.git", "branch": "refs/heads/phantom"},
        "capability": "write",
        "lifecycle": "eager",
        "cross_linkable": true
    }));
    fs::write(&roster_path, serde_json::to_string_pretty(&roster).unwrap()).unwrap();

    let out = ok(&f.b, &["pull", "--all"]);
    assert!(
        out.contains("mem `phantom` is still quarantined [SCHEMA_NOT_FOUND]"),
        "{out}"
    );
    assert!(out.contains("mem `knowledge` serves again"), "{out}");
    assert_eq!(quarantined(&f.b), vec!["phantom".to_string()]);

    // Content on the remote that violates the widgets schema: a note
    // with an undeclared frontmatter key, committed straight onto the
    // remote's branch from a scratch clone.
    let scratch = f.b.parent().unwrap().join("scratch");
    let o = StdCommand::new("git")
        .args(["clone", "-q", "--branch", "knowledge"])
        .arg(&f.remote)
        .arg(&scratch)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    fs::write(
        scratch.join("bad-note.md"),
        "---\ntype: note\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nstatus: active\nnot_a_field: x\n---\n# Bad note\n\n## Summary\n\nBad.\n",
    )
    .unwrap();
    git(&scratch, &["add", "bad-note.md"]);
    git(
        &scratch,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@e",
            "commit",
            "-q",
            "-m",
            "bad",
        ],
    );
    git(&scratch, &["push", "-q", "origin", "knowledge"]);

    let before = git(
        &f.b.join("mem-repo"),
        &["rev-parse", "refs/heads/knowledge"],
    );
    let out = run(&f.b, &["pull", "--all"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("SCHEMA_VIOLATION_IN_FETCH"), "{stderr}");
    assert_eq!(
        git(
            &f.b.join("mem-repo"),
            &["rev-parse", "refs/heads/knowledge"]
        ),
        before
    );
}

/// The guide's two recovery flows, read from the published page so the
/// test executes exactly what a reader types.
fn guide_block(heading: &str) -> Vec<String> {
    let guide = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs-site/src/content/docs/guides/back-up-a-mem-repo.md"),
    )
    .unwrap();
    let after = &guide[guide.find(heading).expect("guide heading")..];
    let block = &after[after.find("```sh\n").expect("sh block") + 6..];
    let block = &block[..block.find("```").unwrap()];
    block
        .lines()
        .map(|l| match l.find("  #") {
            Some(i) => l[..i].trim().to_string(),
            None => l.trim().to_string(),
        })
        .filter(|l| !l.is_empty())
        .collect()
}

/// Split a guide line into arguments, honouring single quotes.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '\'' => quoted = !quoted,
            ' ' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Run a guide flow in `dir` against `remote`; returns the directory
/// the flow ends in (it may `cd` into one it creates).
fn run_guide_flow(lines: &[String], dir: &Path, remote: &Path) -> PathBuf {
    let mut cwd = dir.to_path_buf();
    for line in lines {
        assert!(
            !line.starts_with("git "),
            "a recovery flow must not run raw git: {line}"
        );
        if let Some(rest) = line.strip_prefix("mkdir ") {
            let name = rest.split("&&").next().unwrap().trim();
            fs::create_dir_all(cwd.join(name)).unwrap();
            if rest.contains("cd ") {
                cwd = cwd.join(name);
            }
            continue;
        }
        let line = line.replace(
            "git@github.com:you/mem-backup.git",
            remote.to_str().unwrap(),
        );
        let args = words(&line);
        assert_eq!(args[0], "memstead", "unexpected guide line: {line}");
        let rest: Vec<&str> = args[1..].iter().map(String::as_str).collect();
        ok(&cwd, &rest);
    }
    cwd
}

/// 01-c and 01-d (the cloned case): a workspace with its tracked
/// engine state but no mem-repo directory is restored by the guide's
/// two commands, and is in sync afterwards.
#[test]
fn the_guides_cloned_workspace_flow_restores_in_two_commands() {
    let f = workspace_a();
    fs::create_dir_all(&f.b).unwrap();
    copy_dir(&f.a.join(".memstead"), &f.b.join(".memstead"));
    let _ = fs::remove_dir_all(f.b.join(".memstead/state/checks"));

    let flow = guide_block("### The workspace's engine state came with a clone");
    assert!(flow.len() <= 2, "{flow:?}");
    let end = run_guide_flow(&flow, &f.b, &f.remote);

    assert!(quarantined(&end).is_empty());
    for id in ["knowledge--first-note", "kitchen--stove"] {
        assert_eq!(entity_hash(&f.a, id), entity_hash(&end, id), "{id}");
    }
    let push = run(&end, &["push", "--all"]);
    assert!(push.status.success() && push.stdout.is_empty());
    assert!(run(&end, &["status", "--remote"]).status.success());
}

/// 01-c, refusal complement: a wrong remote URL refuses typed and leaves
/// nothing that blocks a retry with the right one.
#[test]
fn a_wrong_remote_refuses_and_a_retry_succeeds() {
    let f = workspace_a();
    fs::create_dir_all(&f.b).unwrap();
    copy_dir(&f.a.join(".memstead"), &f.b.join(".memstead"));
    let wrong = f.b.parent().unwrap().join("no-such-backup.git");
    ok(
        &f.b,
        &["mem-repo", "init", "--remote", wrong.to_str().unwrap()],
    );
    let out = run(&f.b, &["pull", "--all"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("UNKNOWN_REMOTE"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // An unreachable network remote refuses with the same typed code.
    ok(
        &f.b,
        &[
            "mem-repo",
            "init",
            "--remote",
            "https://127.0.0.1:1/backup.git",
        ],
    );
    let out = run(&f.b, &["pull", "--all"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("UNKNOWN_REMOTE"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The documented command again, with the right URL.
    ok(
        &f.b,
        &["mem-repo", "init", "--remote", f.remote.to_str().unwrap()],
    );
    ok(&f.b, &["pull", "--all"]);
    assert!(quarantined(&f.b).is_empty());
    let push = run(&f.b, &["push", "--all"]);
    assert!(push.status.success() && push.stdout.is_empty());
}

/// 01-d (the roster-less case): the guide's flow from nothing but the
/// backup ends with the mem restored, the schema ref on the backup's
/// history, and nothing to push.
#[test]
fn the_guides_backup_only_flow_restores_without_forking_the_schema_ref() {
    let f = workspace_a();
    let flow = guide_block("### Nothing but the backup");
    let end = run_guide_flow(&flow, f.b.parent().unwrap(), &f.remote);

    assert_eq!(
        entity_hash(&f.a, "knowledge--first-note"),
        entity_hash(&end, "knowledge--first-note")
    );
    let push = run(&end, &["push", "--all"]);
    assert!(
        push.status.success() && push.stdout.is_empty(),
        "{}{}",
        String::from_utf8_lossy(&push.stdout),
        String::from_utf8_lossy(&push.stderr)
    );
    assert!(run(&end, &["status", "--remote"]).status.success());
}

/// 01-e: a fetch refspec that writes into a local branch is refused
/// before git runs; fetching into remote-tracking refs keeps working;
/// and no single-mem verb moves the local schema ref, only `pull --all`.
#[test]
fn only_pull_all_moves_local_refs_from_a_remote() {
    let f = workspace_a();
    clone_b(&f);
    ok(&f.b, &["pull", "--all"]);
    let repo = f.b.join("mem-repo");

    // The remote's schema ref moves ahead (a mem description).
    ok(&f.a, &["mem", "set-description", "kitchen", "moved"]);
    ok(&f.a, &["push", "--all"]);
    let schema_before = git(&repo, &["rev-parse", "refs/heads/__MEMSTEAD"]);
    let kitchen_before = git(&repo, &["rev-parse", "refs/heads/kitchen"]);

    for spec in [
        "+refs/heads/kitchen:refs/heads/kitchen",
        "refs/heads/__MEMSTEAD:refs/heads/__MEMSTEAD",
        "kitchen:kitchen",
        // A case-insensitive filesystem stores these as local heads.
        "+refs/heads/kitchen:refs/HEADS/kitchen",
        "+refs/heads/__MEMSTEAD:refs/Heads/__MEMSTEAD",
        "refs/heads/kitchen:refs/tags/kitchen",
    ] {
        let out = run(&f.b, &["fetch", "kitchen", spec]);
        assert!(!out.status.success(), "{spec}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("INVALID_INPUT"),
            "{spec}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    ok(
        &f.b,
        &[
            "fetch",
            "kitchen",
            "+refs/heads/kitchen:refs/remotes/origin/kitchen",
        ],
    );
    ok(&f.b, &["fetch", "kitchen"]);
    ok(&f.b, &["pull", "kitchen"]);
    assert_eq!(
        git(&repo, &["rev-parse", "refs/heads/kitchen"]),
        kitchen_before
    );
    assert_eq!(
        git(&repo, &["rev-parse", "refs/heads/__MEMSTEAD"]),
        schema_before,
        "fetch and the single-mem pull never move the schema ref"
    );

    // The pushing verbs and branch-reset never move it from the remote
    // either (`push --all` refuses the lagging schema ref by name).
    ok(&f.b, &["push", "kitchen"]);
    let _ = run(&f.b, &["push", "--all"]);
    ok(
        &f.b,
        &["branch-reset", "kitchen", "refs/remotes/origin/kitchen"],
    );
    assert_eq!(
        git(&repo, &["rev-parse", "refs/heads/__MEMSTEAD"]),
        schema_before,
        "push, push --all and branch-reset never move the schema ref"
    );

    // A refspec that would parse as a git option is refused, and the
    // option never reaches git.
    let marker = f.b.parent().unwrap().join("injected");
    let spec = format!("--upload-pack=touch {}; false", marker.display());
    let out = run(&f.b, &["fetch", "kitchen", "--", &spec]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("INVALID_INPUT"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!marker.exists(), "the refspec ran as a git option");

    ok(&f.b, &["pull", "--all"]);
    assert_eq!(
        git(&repo, &["rev-parse", "refs/heads/__MEMSTEAD"]),
        git(
            &f.a.join("mem-repo"),
            &["rev-parse", "refs/heads/__MEMSTEAD"]
        ),
        "pull --all moves it"
    );
}

/// 01-a: unpushed local work is reported and left alone, never a
/// refusal, and the run exits zero.
#[test]
fn unpushed_local_work_is_left_alone_without_failing_the_run() {
    let f = workspace_a();
    clone_b(&f);
    ok(&f.b, &["pull", "--all"]);
    ok(
        &f.b,
        &[
            "create",
            "--mem",
            "kitchen",
            "--title",
            "Sink",
            "--type",
            "spec",
            "--metadata",
            "level=M0",
            "--section",
            "identity=x",
            "--section",
            "purpose=y",
        ],
    );
    let tip = git(&f.b.join("mem-repo"), &["rev-parse", "refs/heads/kitchen"]);
    let out = ok(&f.b, &["pull", "--all"]);
    assert!(
        out.contains("refs/heads/kitchen has local commits the remote lacks"),
        "{out}"
    );
    assert_eq!(
        git(&f.b.join("mem-repo"), &["rev-parse", "refs/heads/kitchen"]),
        tip
    );
}

/// 01-b refusal complement: a mem whose mem-repo cannot be read is
/// named with its reason, and every other ref is still restored.
#[test]
fn a_broken_mem_repo_is_named_and_the_rest_still_restores() {
    let f = workspace_a();
    clone_b(&f);
    let roster_path = f.b.join(".memstead/state/mounts.json");
    let mut roster: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&roster_path).unwrap()).unwrap();
    roster["mounts"].as_array_mut().unwrap().push(serde_json::json!({
        "mem": "elsewhere",
        "schema": "default@1.3.0",
        "storage": {"type": "git-branch", "gitdir": "no-such-repo/.git", "branch": "refs/heads/elsewhere"},
        "capability": "write",
        "lifecycle": "eager",
        "cross_linkable": true
    }));
    fs::write(&roster_path, serde_json::to_string_pretty(&roster).unwrap()).unwrap();

    let out = run(&f.b, &["pull", "--all"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("no-such-repo"), "{stderr}");
    assert!(
        stdout.contains("mem `elsewhere` is still quarantined"),
        "{stdout}"
    );
    for id in ["knowledge--first-note", "kitchen--stove"] {
        assert_eq!(entity_hash(&f.a, id), entity_hash(&f.b, id), "{id}");
    }
}

/// 01-c and 01-d: a backup whose schema ref was seeded by an older
/// engine restores through the documented two commands; the fresh
/// local seed carries no state and gives way.
#[test]
fn a_backup_with_an_older_seed_restores_in_two_commands() {
    let f = workspace_a_seeded(true);
    fs::create_dir_all(&f.b).unwrap();
    copy_dir(&f.a.join(".memstead"), &f.b.join(".memstead"));
    let _ = fs::remove_dir_all(f.b.join(".memstead/state/checks"));
    ok(
        &f.b,
        &["mem-repo", "init", "--remote", f.remote.to_str().unwrap()],
    );
    ok(&f.b, &["pull", "--all"]);
    assert!(quarantined(&f.b).is_empty());
    assert_eq!(
        git(
            &f.b.join("mem-repo"),
            &["rev-parse", "refs/heads/__MEMSTEAD"]
        ),
        git(
            &f.a.join("mem-repo"),
            &["rev-parse", "refs/heads/__MEMSTEAD"]
        )
    );
    let push = run(&f.b, &["push", "--all"]);
    assert!(push.status.success() && push.stdout.is_empty());
}

/// The seed rule is narrow: a local schema ref that holds real state
/// and shares no history with the backup is refused, never replaced.
#[test]
fn a_schema_ref_with_state_is_never_replaced() {
    let f = workspace_a_seeded(true);
    let other = workspace_a();
    ok(
        &other.a,
        &[
            "mem-repo",
            "remote-add",
            "backup",
            f.remote.to_str().unwrap(),
        ],
    );
    let before = git(
        &other.a.join("mem-repo"),
        &["rev-parse", "refs/heads/__MEMSTEAD"],
    );
    let out = run(&other.a, &["pull", "--all", "--remote", "backup"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("LOCAL_DIVERGENCE"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        git(
            &other.a.join("mem-repo"),
            &["rev-parse", "refs/heads/__MEMSTEAD"]
        ),
        before
    );
}
