//! An anchors-only update replaces the row on folder mems (backlog-decisions
//! plan B10), exactly as git-branch mems do: a `memstead update --anchor`
//! naming a stored (artifact, grain, class) triple with a differing field
//! rewrites the sidecar row hash-less, the response reports
//! `anchors_changed: true`, and the next verify backfills the hash; a row
//! that restates what is stored returns the no-op response
//! (`anchors_changed: false`, `UPDATE_NOOP`, empty `write_id`) and leaves the
//! sidecar bytes untouched; an update that also changes content re-baselines
//! a restated row. The same test runs on both mount kinds.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use tempfile::TempDir;

const STABLE: &str =
    r#"{"artifact":"src.txt","grain":"file","class":"anchored","hash_stability":"stable"}"#;
const UNSTABLE: &str =
    r#"{"artifact":"src.txt","grain":"file","class":"anchored","hash_stability":"unstable"}"#;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// A folder workspace: `notes` on plain files, the sidecar at
/// `.memstead/anchors.json`.
fn folder_workspace() -> (TempDir, String) {
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
    memstead()
        .current_dir(ws.path())
        .args([
            "create",
            "--quiet",
            "--type",
            "concept",
            "--title",
            "Alpha",
            "--section",
            "definition=x",
            "--section",
            "explanation=y",
        ])
        .assert()
        .success();
    (ws, "notes--alpha".to_string())
}

/// A mem-repo workspace: `notes` on a git branch, the sidecar in the
/// branch tree.
fn branch_workspace() -> (TempDir, String) {
    let ws = TempDir::new().unwrap();
    let dir = ws.path().join("notes-mem");
    fs::create_dir_all(dir.join(".memstead")).unwrap();
    fs::write(
        dir.join(".memstead").join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    fs::write(
        dir.join("alpha.md"),
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nThe alpha surface.\n\n## Purpose\n\nExists.\n",
    )
    .unwrap();
    init_real_mem_repo_from_disk(ws.path(), &[(&dir, "notes")]);
    (ws, "notes--alpha".to_string())
}

fn update_anchor_json(root: &Path, id: &str, anchor: &str) -> serde_json::Value {
    let out = memstead()
        .current_dir(root)
        .args(["update", id, "--quiet", "--json", "--anchor", anchor])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn verify(root: &Path) -> serde_json::Value {
    let out = memstead()
        .current_dir(root)
        .args(["--json", "verify-anchors", "--mem", "notes", "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success());
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The entity's first anchor row as the engine reads it back.
fn stored_row(root: &Path, id: &str) -> serde_json::Value {
    let out = memstead()
        .current_dir(root)
        .args(["anchors", id, "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["anchors"][0].clone()
}

fn exercise(root: &Path, id: &str, repair_section: &str) {
    fs::write(root.join("src.txt"), "hello\n").unwrap();
    let first = update_anchor_json(root, id, STABLE);
    assert_eq!(first["anchors_changed"], true, "{first}");
    let v = verify(root);
    assert_eq!(v["hash_backfilled"], 1, "{v}");
    assert!(stored_row(root, id)["hash"].is_string());

    // Refusal complement: a restated row is a truthful no-op.
    let noop = update_anchor_json(root, id, STABLE);
    assert_eq!(noop["anchors_changed"], false, "{noop}");
    assert_eq!(noop["write_id"], "", "{noop}");
    let codes: Vec<&str> = noop["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w["code"].as_str())
        .collect();
    assert!(codes.contains(&"UPDATE_NOOP"), "{noop}");
    assert!(stored_row(root, id)["hash"].is_string(), "baseline kept");

    // Assertion: the same triple with a differing field replaces the row
    // hash-less; the next verify backfills it.
    let changed = update_anchor_json(root, id, UNSTABLE);
    assert_eq!(changed["anchors_changed"], true, "{changed}");
    assert_ne!(changed["write_id"], "", "{changed}");
    let row = stored_row(root, id);
    assert_eq!(row["hash_stability"], "unstable");
    assert!(row["hash"].is_null(), "rewritten hash-less: {row}");
    let v = verify(root);
    assert_eq!(v["hash_backfilled"], 1, "{v}");
    assert!(stored_row(root, id)["hash"].is_string());

    // The sync brief's one-update repair: content change plus the same
    // anchors re-baselines the restated row.
    let out = memstead()
        .current_dir(root)
        .args([
            "update",
            id,
            "--quiet",
            "--json",
            "--auto-hash",
            "--section",
            &format!("{repair_section}=repaired from the source"),
            "--anchor",
            UNSTABLE,
        ])
        .output()
        .unwrap();
    let repaired: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(repaired["anchors_changed"], true, "{repaired}");
    assert!(stored_row(root, id)["hash"].is_null(), "re-baselined");
}

/// B10 AC1 on a folder mem, sidecar bytes compared around the no-op.
#[test]
fn an_anchors_only_update_replaces_the_row_on_a_folder_mem() {
    let (ws, id) = folder_workspace();
    exercise(ws.path(), &id, "explanation");
    let sidecar = ws.path().join(".memstead").join("anchors.json");
    let before = fs::read(&sidecar).unwrap();
    let noop = update_anchor_json(ws.path(), &id, UNSTABLE);
    assert_eq!(noop["anchors_changed"], false);
    assert_eq!(
        fs::read(&sidecar).unwrap(),
        before,
        "sidecar bytes untouched"
    );
}

/// B10 AC1 on a git-branch mem: the same behaviour, and a restated row
/// leaves no commit behind.
#[test]
fn an_anchors_only_update_replaces_the_row_on_a_git_branch_mem() {
    let (ws, id) = branch_workspace();
    exercise(ws.path(), &id, "purpose");
    let head = |root: &Path| -> String {
        let out = std::process::Command::new("git")
            .args([
                "-C",
                root.join("mem-repo").to_str().unwrap(),
                "rev-parse",
                "notes",
            ])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let before = head(ws.path());
    let noop = update_anchor_json(ws.path(), &id, UNSTABLE);
    assert_eq!(noop["anchors_changed"], false);
    assert_eq!(head(ws.path()), before, "no commit for a restated row");
}
