#![cfg(feature = "mem-repo")]
//! The rehearsal contract (agent-trust plan 07), asserted end to end
//! over the CLI write surface: a `--dry-run` call runs the IDENTICAL
//! validation the real call runs (same typed refusals, same details),
//! is observably side-effect-free (every byte under the workspace —
//! git refs, working tree, `.memstead/` — identical before and after),
//! and marks itself with the marker form (empty `commit_sha` plus the
//! prospective fields).
//!
//! Coverage: the pre-existing single-verb dry-run (create, update) is
//! asserted AGAINST the contract for the first time; the plan's new
//! surfaces (relate, batch-create, batch-update, batch-relate) are
//! asserted the same way; complements pin rehearsal against a
//! quarantined mem refusing exactly as the real call would.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use serde_json::Value;
use tempfile::TempDir;

/// Same fixture shape as `write_commands.rs`: a `cli-write/` mem with
/// a real gix-managed mem-repo so writes land on
/// `mem-repo/.git/refs/heads/cli-write`.
fn make_mem(tmp: &Path) -> PathBuf {
    let mem = tmp.join("cli-write");
    fs::create_dir_all(&mem).unwrap();
    let store = mem.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    init_real_mem_repo_from_disk(tmp, &[(&mem, "cli-write")]);
    mem
}

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn parse_json(stdout_bytes: &[u8]) -> Value {
    let body = std::str::from_utf8(stdout_bytes).expect("stdout must be UTF-8");
    serde_json::from_str(body.trim()).unwrap_or_else(|e| {
        panic!("--json output must parse as JSON: {e}\n--- stdout ---\n{body}\n--- end ---")
    })
}

/// Byte-level fingerprint of EVERYTHING under `root`, recursively:
/// relative path → file bytes. Git refs, loose objects, logs, index,
/// the working tree, and `.memstead/` state are all in scope — a
/// rehearsal that touches any byte anywhere fails the comparison.
fn tree_digest(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().to_string();
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Assert two digests are identical, naming the first divergent path.
fn assert_trees_identical(before: &BTreeMap<String, Vec<u8>>, after: &BTreeMap<String, Vec<u8>>) {
    for (path, bytes) in before {
        match after.get(path) {
            None => panic!("rehearsal DELETED {path}"),
            Some(b) if b != bytes => panic!("rehearsal MODIFIED {path}"),
            Some(_) => {}
        }
    }
    for path in after.keys() {
        assert!(before.contains_key(path), "rehearsal CREATED {path}");
    }
}

/// Boot once so any first-boot writes (state seeding, cursor files)
/// settle before the byte-identity window opens.
fn warm_up(ws: &Path) {
    memstead()
        .current_dir(ws)
        .args(["--json", "health"])
        .assert()
        .success();
}

/// The `{code, details}` pair from a refused `--json` invocation.
/// `message` rides along for the strongest practical parity check.
fn refusal_envelope(ws: &Path, args: &[&str]) -> Value {
    let out = memstead()
        .current_dir(ws)
        .args(args)
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let v = parse_json(&out);
    serde_json::json!({
        "code": v["code"],
        "message": v["message"],
        "details": v["details"],
    })
}

// ---------------------------------------------------------------------------
// Criterion 3 — the PRE-EXISTING dry-run surfaces, asserted against
// the contract: paired refusals, byte-identical workspace.
// ---------------------------------------------------------------------------

#[test]
fn create_rehearsal_refuses_identically_and_leaves_workspace_byte_identical() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_mem(tmp.path());
    warm_up(tmp.path());

    // Paired refusal: an unknown section refuses with the same typed
    // envelope, rehearsed or real.
    let illegal = |dry: bool| {
        let mut args = vec!["--json", "create", "--title", "Alpha", "--type", "spec"];
        args.extend(["--section", "bogus-section=nope"]);
        if dry {
            args.push("--dry-run");
        }
        args.into_iter().map(String::from).collect::<Vec<_>>()
    };
    let rehearsed = refusal_envelope(
        tmp.path(),
        &illegal(true).iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let real = refusal_envelope(
        tmp.path(),
        &illegal(false).iter().map(String::as_str).collect::<Vec<_>>(),
    );
    assert_eq!(rehearsed, real, "identical typed refusal, rehearsed vs real");

    // Byte-identity: a LEGAL rehearsed create touches nothing.
    let before = tree_digest(tmp.path());
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "create",
            "--title",
            "Alpha",
            "--type",
            "spec",
            "--section",
            "identity=The alpha entity.",
            "--section",
            "purpose=Rehearsal contract test.",
            "--dry-run",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body = parse_json(&out);
    assert_eq!(body["commit_sha"], "", "marker form: empty commit_sha");
    assert_eq!(body["id"], "cli-write--alpha", "prospective id reported");
    let after = tree_digest(tmp.path());
    assert_trees_identical(&before, &after);

    // Identical validation: the real call on the unchanged mem lands.
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "create",
            "--title",
            "Alpha",
            "--type",
            "spec",
            "--section",
            "identity=The alpha entity.",
            "--section",
            "purpose=Rehearsal contract test.",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let real_body = parse_json(&out);
    assert_ne!(real_body["commit_sha"], "", "the real create commits");
    // No cross-call hash-equality assertion: the auto-stamped
    // `created_date` (second-resolution wall clock) enters `_hash`,
    // so the rehearsed and real hashes diverge whenever a second
    // ticks between the two invocations — a legitimate timestamp
    // shift, not drift. The engine-level test pins the mutation
    // clock and asserts equality there.
    assert_eq!(real_body["id"], body["id"], "same prospective identity");
}

#[test]
fn update_rehearsal_refuses_identically_and_leaves_workspace_byte_identical() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_mem(tmp.path());
    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "Target",
            "--type",
            "spec",
            "--section",
            "identity=Before.",
            "--section",
            "purpose=Rehearsal target.",
        ])
        .assert()
        .success();
    warm_up(tmp.path());
    let hash = {
        let out = memstead()
            .current_dir(tmp.path())
            .args(["--json", "entity", "cli-write--target"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        parse_json(&out)["_hash"].as_str().unwrap().to_string()
    };

    // Paired refusal: unknown section, rehearsed vs real.
    let illegal = |dry: bool| {
        let mut v = vec![
            "--json".to_string(),
            "update".to_string(),
            "cli-write--target".to_string(),
            "--expected-hash".to_string(),
            hash.clone(),
            "--section".to_string(),
            "bogus-section=nope".to_string(),
        ];
        if dry {
            v.push("--dry-run".to_string());
        }
        v
    };
    let rehearsed = refusal_envelope(
        tmp.path(),
        &illegal(true).iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let real = refusal_envelope(
        tmp.path(),
        &illegal(false).iter().map(String::as_str).collect::<Vec<_>>(),
    );
    assert_eq!(rehearsed, real, "identical typed refusal, rehearsed vs real");

    // Byte-identity for the legal rehearsal.
    let before = tree_digest(tmp.path());
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "update",
            "cli-write--target",
            "--expected-hash",
            &hash,
            "--section",
            "identity=After.",
            "--dry-run",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body = parse_json(&out);
    assert_eq!(body["commit_sha"], "", "marker form: empty commit_sha");
    assert_eq!(
        body["_hash"].as_str().unwrap(),
        hash,
        "dry-run `_hash` is the CURRENT hash (the next expected_hash)"
    );
    let prospective = body["prospective_hash"].as_str().unwrap().to_string();
    assert_ne!(prospective, hash);
    let after = tree_digest(tmp.path());
    assert_trees_identical(&before, &after);

    // The real call with the SAME hash lands — a persisted rehearsal
    // would have moved the on-disk hash and refused here. This is the
    // load-bearing identical-validation proof; hash equality with the
    // rehearsed `prospective_hash` is NOT asserted, because the
    // auto-stamped `last_modified` (second-resolution wall clock)
    // enters the hash and legitimately diverges across a second tick.
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "update",
            "cli-write--target",
            "--expected-hash",
            &hash,
            "--section",
            "identity=After.",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let real_body = parse_json(&out);
    assert_ne!(real_body["commit_sha"], "");
    assert_ne!(
        real_body["_hash"].as_str().unwrap(),
        hash,
        "the real update moved the hash"
    );
}

// ---------------------------------------------------------------------------
// Criterion 1 — relate gains the same contract.
// ---------------------------------------------------------------------------

#[test]
fn relate_rehearsal_reports_would_be_stub_and_leaves_workspace_byte_identical() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_mem(tmp.path());
    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "Source",
            "--type",
            "spec",
            "--section",
            "identity=Src.",
            "--section",
            "purpose=Rehearsal source.",
        ])
        .assert()
        .success();
    warm_up(tmp.path());

    // Paired refusal: malformed target id, rehearsed vs real.
    let rehearsed = refusal_envelope(
        tmp.path(),
        &[
            "--json", "relate", "cli-write--source", "USES", "not a valid id", "--dry-run",
        ],
    );
    let real = refusal_envelope(
        tmp.path(),
        &["--json", "relate", "cli-write--source", "USES", "not a valid id"],
    );
    assert_eq!(rehearsed, real, "identical typed refusal, rehearsed vs real");

    // Legal rehearsal to an ABSENT target: would-be stub reported,
    // nothing written anywhere.
    let before = tree_digest(tmp.path());
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "relate",
            "cli-write--source",
            "USES",
            "cli-write--ghost",
            "--dry-run",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body = parse_json(&out);
    assert_eq!(body["commit_sha"], "", "marker form: empty commit_sha");
    let warnings = serde_json::to_string(&body["warnings"]).unwrap();
    assert!(
        warnings.contains("AUTO_STUB_CREATED"),
        "would-be stub must be reported: {warnings}"
    );
    let after = tree_digest(tmp.path());
    assert_trees_identical(&before, &after);
    // The stub was never created.
    memstead()
        .current_dir(tmp.path())
        .args(["--json", "entity", "cli-write--ghost"])
        .assert()
        .failure();

    // The real call on the unchanged mem lands.
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "relate",
            "cli-write--source",
            "USES",
            "cli-write--ghost",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let real_body = parse_json(&out);
    assert_ne!(real_body["commit_sha"], "", "the real relate commits");
    // No cross-call hash equality: the relate re-stamps
    // `last_modified` into the hash, so rehearsed vs real diverge
    // across a second tick (the clock-pinned engine test asserts
    // equality deterministically).
}

// ---------------------------------------------------------------------------
// Criterion 2 — the batch family's reversed contract.
// ---------------------------------------------------------------------------

#[test]
fn batch_create_rehearsal_reports_receipt_and_leaves_workspace_byte_identical() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_mem(tmp.path());
    warm_up(tmp.path());

    // Two entries; the first references the second — intra-batch
    // resolution is part of the rehearsed validation.
    let batch_file = tmp.path().join("batch.json");
    fs::write(
        &batch_file,
        serde_json::json!({
            "creates": [
                { "title": "Alpha", "entity_type": "spec",
                  "sections": { "identity": "A.", "purpose": "P." },
                  "relations": [ { "to": "cli-write--beta", "type": "USES" } ] },
                { "title": "Beta", "entity_type": "spec",
                  "sections": { "identity": "B.", "purpose": "P." } }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let before = tree_digest(tmp.path());
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "batch-create",
            "--from",
            batch_file.to_str().unwrap(),
            "--dry-run",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body = parse_json(&out);
    assert_eq!(body["applied"], true, "{body}");
    assert_eq!(body["commit_sha"], "", "marker form: empty commit_sha");
    assert_eq!(body["succeeded"], 2);
    assert_eq!(body["results"][0]["action"], "created");
    assert_eq!(body["results"][0]["id"], "cli-write--alpha");
    let after = tree_digest(tmp.path());
    assert_trees_identical(&before, &after);

    // Real run lands.
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "batch-create",
            "--from",
            batch_file.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let real_body = parse_json(&out);
    assert_eq!(real_body["applied"], true);
    assert_ne!(real_body["commit_sha"], "", "the real batch commits");
}

#[test]
fn batch_create_rehearsal_refuses_identically_to_real() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_mem(tmp.path());
    warm_up(tmp.path());

    // One illegal entry (bad title) refuses the whole batch — the
    // per-entry report must be identical, rehearsed vs real (both
    // refusals are side-effect-free, so they run back to back).
    let batch_file = tmp.path().join("batch.json");
    fs::write(
        &batch_file,
        serde_json::json!({
            "creates": [
                { "title": "Fine", "entity_type": "spec",
                  "sections": { "identity": "A.", "purpose": "P." } },
                { "title": "Bad/Title", "entity_type": "spec",
                  "sections": { "identity": "B.", "purpose": "P." } }
            ]
        })
        .to_string(),
    )
    .unwrap();
    let path = batch_file.to_str().unwrap();
    let rehearsed = refusal_envelope(
        tmp.path(),
        &["--json", "batch-create", "--from", path, "--dry-run"],
    );
    let real = refusal_envelope(tmp.path(), &["--json", "batch-create", "--from", path]);
    assert_eq!(rehearsed, real, "identical per-entry refusals");
    // Nothing landed either way.
    memstead()
        .current_dir(tmp.path())
        .args(["--json", "entity", "cli-write--fine"])
        .assert()
        .failure();
}

#[test]
fn batch_update_rehearsal_reports_receipt_and_leaves_workspace_byte_identical() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_mem(tmp.path());
    for title in ["One", "Two"] {
        memstead()
            .current_dir(tmp.path())
            .args([
                "create",
                "--title",
                title,
                "--type",
                "spec",
                "--section",
                "identity=Before.",
                "--section",
                "purpose=P.",
            ])
            .assert()
            .success();
    }
    warm_up(tmp.path());

    let batch_file = tmp.path().join("batch.json");
    fs::write(
        &batch_file,
        serde_json::json!({
            "updates": [
                { "id": "cli-write--one", "auto_hash": true,
                  "sections": { "identity": "After one." } },
                { "id": "cli-write--two", "auto_hash": true,
                  "sections": { "identity": "After two." } }
            ]
        })
        .to_string(),
    )
    .unwrap();
    let path = batch_file.to_str().unwrap();

    let before = tree_digest(tmp.path());
    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "batch-update", "--from", path, "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body = parse_json(&out);
    assert_eq!(body["applied"], true, "{body}");
    assert_eq!(body["commit_sha"], "", "marker form: empty commit_sha");
    assert!(
        body["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["action"] == "updated"),
        "{body}"
    );
    let after = tree_digest(tmp.path());
    assert_trees_identical(&before, &after);

    // Real run applies (auto_hash re-reads, so the pre-rehearsal
    // hashes being untouched is what lets this succeed).
    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "batch-update", "--from", path])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let real_body = parse_json(&out);
    assert_eq!(real_body["applied"], true);
    assert_ne!(real_body["commit_sha"], "");
}

#[test]
fn batch_update_rehearsal_refuses_identically_to_real() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_mem(tmp.path());
    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "One",
            "--type",
            "spec",
            "--section",
            "identity=X.",
            "--section",
            "purpose=P.",
        ])
        .assert()
        .success();
    warm_up(tmp.path());

    let batch_file = tmp.path().join("batch.json");
    fs::write(
        &batch_file,
        serde_json::json!({
            "updates": [
                { "id": "cli-write--one", "expected_hash": "wrong-hash",
                  "sections": { "identity": "Y." } },
                { "id": "cli-write--missing", "force": true,
                  "sections": { "identity": "Z." } }
            ]
        })
        .to_string(),
    )
    .unwrap();
    let path = batch_file.to_str().unwrap();
    let rehearsed = refusal_envelope(
        tmp.path(),
        &["--json", "batch-update", "--from", path, "--dry-run"],
    );
    let real = refusal_envelope(tmp.path(), &["--json", "batch-update", "--from", path]);
    assert_eq!(rehearsed, real, "identical per-entry refusals");
}

#[test]
fn batch_relate_rehearsal_reports_receipt_and_leaves_workspace_byte_identical() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_mem(tmp.path());
    for title in ["One", "Two"] {
        memstead()
            .current_dir(tmp.path())
            .args([
                "create",
                "--title",
                title,
                "--type",
                "spec",
                "--section",
                "identity=X.",
                "--section",
                "purpose=P.",
            ])
            .assert()
            .success();
    }
    warm_up(tmp.path());

    let batch_file = tmp.path().join("batch.json");
    fs::write(
        &batch_file,
        serde_json::json!({
            "relates": [
                { "from": "cli-write--one", "type": "USES", "to": "cli-write--two" },
                { "from": "cli-write--one", "type": "USES", "to": "cli-write--ghost" }
            ]
        })
        .to_string(),
    )
    .unwrap();
    let path = batch_file.to_str().unwrap();

    let before = tree_digest(tmp.path());
    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "batch-relate", "--from", path, "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body = parse_json(&out);
    assert_eq!(body["applied"], true, "{body}");
    assert_eq!(body["commit_sha"], "", "marker form: empty commit_sha");
    assert!(
        body["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["action"] == "added"),
        "{body}"
    );
    let after = tree_digest(tmp.path());
    assert_trees_identical(&before, &after);
    // No stub landed.
    memstead()
        .current_dir(tmp.path())
        .args(["--json", "entity", "cli-write--ghost"])
        .assert()
        .failure();

    // Real run lands both edges.
    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "batch-relate", "--from", path])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let real_body = parse_json(&out);
    assert_eq!(real_body["applied"], true);
    assert_ne!(real_body["commit_sha"], "");
}

#[test]
fn batch_relate_rehearsal_refuses_identically_to_real() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_mem(tmp.path());
    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "One",
            "--type",
            "spec",
            "--section",
            "identity=X.",
            "--section",
            "purpose=P.",
        ])
        .assert()
        .success();
    warm_up(tmp.path());

    let batch_file = tmp.path().join("batch.json");
    fs::write(
        &batch_file,
        serde_json::json!({
            "relates": [
                { "from": "cli-write--one", "type": "USES", "to": "not a valid id" }
            ]
        })
        .to_string(),
    )
    .unwrap();
    let path = batch_file.to_str().unwrap();
    let rehearsed = refusal_envelope(
        tmp.path(),
        &["--json", "batch-relate", "--from", path, "--dry-run"],
    );
    let real = refusal_envelope(tmp.path(), &["--json", "batch-relate", "--from", path]);
    assert_eq!(rehearsed, real, "identical refusals");
}

// ---------------------------------------------------------------------------
// Criterion 5 complement — rehearsal against a quarantined mem refuses
// exactly as the real call would.
// ---------------------------------------------------------------------------

/// A workspace whose `plenum` mem quarantines on an unresolvable
/// schema pin (same fixture shape as `repair_below_boot.rs`).
fn quarantined_workspace(tmp: &TempDir) -> PathBuf {
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    fs::create_dir_all(ws.join("plenum")).unwrap();
    let mounts = ws.join(".memstead").join("state").join("mounts.json");
    fs::create_dir_all(mounts.parent().unwrap()).unwrap();
    fs::write(
        &mounts,
        r#"{
  "format": "memstead-mounts-3",
  "mounts": [
    { "mem": "plenum", "schema": "ghost@1.0.0", "storage": { "type": "folder", "path": "plenum" }, "capability": "write", "lifecycle": "eager", "cross_linkable": true }
  ]
}"#,
    )
    .unwrap();
    ws
}

#[test]
fn rehearsal_against_quarantined_mem_refuses_identically_to_real() {
    let tmp = TempDir::new().unwrap();
    let ws = quarantined_workspace(&tmp);

    let illegal = |dry: bool| {
        let mut v = vec![
            "--json".to_string(),
            "create".to_string(),
            "--title".to_string(),
            "Doc".to_string(),
            "--type".to_string(),
            "note".to_string(),
            "--mem".to_string(),
            "plenum".to_string(),
        ];
        if dry {
            v.push("--dry-run".to_string());
        }
        v
    };
    let rehearsed = refusal_envelope(
        &ws,
        &illegal(true).iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let real = refusal_envelope(
        &ws,
        &illegal(false).iter().map(String::as_str).collect::<Vec<_>>(),
    );
    assert_eq!(rehearsed, real, "identical quarantine refusal");
    assert_eq!(rehearsed["code"], "MEM_QUARANTINED", "{rehearsed}");
}
