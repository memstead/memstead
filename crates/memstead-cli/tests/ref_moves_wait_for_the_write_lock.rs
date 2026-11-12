//! A branch move and a pin switch wait for a write in progress in
//! another mem: with that mem's write lock held from a thread, each
//! returns only after the lock is released, and then succeeds.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use assert_cmd::Command;

fn run(ws: &Path, args: &[&str]) -> std::process::Output {
    let out = Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

fn head(ws: &Path, mem: &str) -> String {
    let out = run(
        ws,
        &["changes", "--mem", mem, "--since", EMPTY_TREE, "--json"],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["head"].as_str().unwrap().to_string()
}

fn memo(ws: &Path, mem: &str, title: &str) {
    run(
        ws,
        &[
            "create",
            "--mem",
            mem,
            "--type",
            "memo",
            "--title",
            title,
            "--section",
            "claim=c",
            "--section",
            "context=x",
        ],
    );
}

/// Two git-branch mems `a` and `b`, each with one entity.
fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path();
    run(ws, &["mem-repo", "init"]);
    run(
        ws,
        &[
            "workspace",
            "allow-create",
            "*",
            "--schema",
            "default@1.3.0",
        ],
    );
    run(ws, &["mem", "init", "a", "--schema", "default@1.3.0"]);
    run(ws, &["mem", "init", "b", "--schema", "default@1.3.0"]);
    memo(ws, "a", "One");
    memo(ws, "b", "Other");
    tmp
}

fn lock_of(ws: &Path, mem: &str) -> PathBuf {
    std::fs::canonicalize(ws.join("mem-repo").join(".git"))
        .unwrap()
        .join("memstead-locks")
        .join(format!("refs%heads%{mem}.lock"))
}

/// Run `args`, while a thread holds `held`'s lock for 1.5 s, and return
/// how long the call took.
fn while_held(ws: &Path, held: PathBuf, args: &[&str]) -> Duration {
    std::fs::create_dir_all(held.parent().unwrap()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&held)
            .unwrap();
        f.lock().unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        f.unlock().unwrap();
    });
    rx.recv().unwrap();
    let started = Instant::now();
    run(ws, args);
    let waited = started.elapsed();
    holder.join().unwrap();
    waited
}

#[test]
fn a_branch_reset_waits_for_a_write_in_another_mem() {
    let tmp = workspace();
    let ws = tmp.path();
    let before = head(ws, "a");
    memo(ws, "a", "Two");
    let waited = while_held(ws, lock_of(ws, "b"), &["branch-reset", "a", &before]);
    assert!(
        waited >= Duration::from_millis(1000),
        "did not wait ({waited:?})"
    );
    assert_eq!(
        head(ws, "a"),
        before,
        "the reset landed after the lock was free"
    );
}

#[test]
fn a_pin_switch_waits_for_a_write_in_another_mem() {
    let tmp = workspace();
    let ws = tmp.path();
    let waited = while_held(
        ws,
        lock_of(ws, "b"),
        &["mem", "set-schema", "a", "default@1.3.0"],
    );
    assert!(
        waited >= Duration::from_millis(1000),
        "did not wait ({waited:?})"
    );
}

const CX_MANIFEST: &str = "name: cx\nversion: 0.1.0\ndescription: d\nwhen_to_use: t\ntypes:\n  - thing\nrelationships:\n  mode: strict\n  definitions:\n    - name: REFERENCES\n      description: r\n      default_weight: 1.0\n      acyclic: true\n    - name: _default\n      description: f\n      default_weight: 1.0\nalias_target_rel_type: REFERENCES\ncross_mem_relationships:\n  - to_schema: cx\n    definitions:\n      - name: REFERENCES\n        description: r\n        default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n";

const CX_THING: &str = "name: thing\ndescription: t\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: _default\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";

/// Two git-branch mems on a schema whose body-link relationship is
/// acyclic, cross-linkable both ways; `b--back` links to `a--x` (and
/// to `a--new`, not yet created).
fn cycle_workspace() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path();
    run(ws, &["mem-repo", "init"]);
    let pkg = ws.join("cx-pkg");
    std::fs::create_dir_all(pkg.join("types")).unwrap();
    std::fs::write(pkg.join("schema.yaml"), CX_MANIFEST).unwrap();
    std::fs::write(pkg.join("types").join("thing.yaml"), CX_THING).unwrap();
    run(ws, &["schema", "install", pkg.to_str().unwrap()]);
    run(
        ws,
        &["workspace", "allow-create", "*", "--schema", "cx@0.1.0"],
    );
    run(ws, &["mem", "init", "a", "--schema", "cx@0.1.0"]);
    run(ws, &["mem", "init", "b", "--schema", "cx@0.1.0"]);
    run(ws, &["workspace", "grant-cross-link", "a", "*"]);
    run(ws, &["workspace", "grant-cross-link", "b", "*"]);
    run(
        ws,
        &[
            "create",
            "--mem",
            "a",
            "--type",
            "thing",
            "--title",
            "X",
            "--section",
            "body=plain",
        ],
    );
    run(
        ws,
        &[
            "create",
            "--mem",
            "b",
            "--type",
            "thing",
            "--title",
            "Back",
            "--section",
            "body=see [[a--x]] and [[a--new]]",
        ],
    );
    tmp
}

fn refused_while_held(ws: &Path, args: &[&str]) -> (Duration, String) {
    let held = lock_of(ws, "b");
    std::fs::create_dir_all(held.parent().unwrap()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&held)
            .unwrap();
        f.lock().unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        f.unlock().unwrap();
    });
    rx.recv().unwrap();
    let started = Instant::now();
    let out = Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args(args)
        .args(["--json", "--quiet"])
        .output()
        .unwrap();
    let waited = started.elapsed();
    holder.join().unwrap();
    assert!(
        !out.status.success(),
        "{args:?} committed a cycle-closing body link"
    );
    (waited, String::from_utf8_lossy(&out.stdout).to_string())
}

/// For create, batch create, update and batch update: a body link in
/// `a` that closes a cycle through `b` waits for `b`'s lock, then
/// refuses `RELATIONSHIP_CYCLE`; a body link that closes none commits.
#[test]
fn a_body_link_closing_a_cycle_across_two_mems_waits_and_refuses() {
    let tmp = cycle_workspace();
    let ws = tmp.path();
    let batch_create = ws.join("bc.json");
    std::fs::write(
        &batch_create,
        r#"{"creates":[{"mem":"a","entity_type":"thing","title":"New","sections":{"body":"back to [[b--back]]"}}]}"#,
    )
    .unwrap();
    let batch_update = ws.join("bu.json");
    std::fs::write(
        &batch_update,
        r#"{"updates":[{"id":"a--x","auto_hash":true,"sections":{"body":"back to [[b--back]]"}}]}"#,
    )
    .unwrap();
    let calls: Vec<Vec<String>> = vec![
        vec![
            "create",
            "--mem",
            "a",
            "--type",
            "thing",
            "--title",
            "New",
            "--section",
            "body=back to [[b--back]]",
        ],
        vec!["batch-create", "--from", batch_create.to_str().unwrap()],
        vec![
            "update",
            "a--x",
            "--section",
            "body=back to [[b--back]]",
            "--auto-hash",
        ],
        vec!["batch-update", "--from", batch_update.to_str().unwrap()],
    ]
    .into_iter()
    .map(|v| v.into_iter().map(String::from).collect())
    .collect();
    for call in &calls {
        let args: Vec<&str> = call.iter().map(String::as_str).collect();
        let (waited, out) = refused_while_held(ws, &args);
        assert!(
            waited >= Duration::from_millis(1000),
            "{args:?} did not wait ({waited:?})"
        );
        assert!(out.contains("RELATIONSHIP_CYCLE"), "{args:?}: {out}");
    }
    // A body link that closes no cycle commits.
    run(
        ws,
        &[
            "create",
            "--mem",
            "b",
            "--type",
            "thing",
            "--title",
            "Leaf",
            "--section",
            "body=leaf",
        ],
    );
    run(
        ws,
        &[
            "update",
            "a--x",
            "--section",
            "body=to [[b--leaf]]",
            "--auto-hash",
        ],
    );
}
