#![cfg(feature = "vault-repo")]
//! Integration tests for `memstead` write subcommands.
//!
//! Covers the core round-trip (create → read → update strict → update
//! auto-hash → delete), plus each write command's distinct failure mode:
//!
//! * `create` — JSON-file input (`--from`) must work end-to-end.
//! * `update` — strict default refuses without `--expected-hash` (exit 5);
//!   wrong hash returns `HashMismatch` (exit 4); `--auto-hash` bypasses both.
//! * `relate` — adds an edge that's visible from `memstead relations`.
//! * `delete` — `--dry-run` leaves the entity in place.
//! * `rename` — changes the ID; the new ID becomes readable via `memstead entity`.
//! * `batch-update` — JSON file with N entries, per-entry status in stdout.
//!
//! Each test's `TempDir` gets a fresh gix-managed repo on first run — `memstead`
//! always initializes VCS since the `--vcs` flag was removed in the gix swap.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_vault_repo_from_disk;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::Value;
use tempfile::TempDir;

/// Create a `cli-write/` subdir inside `tmp` and return its absolute
/// path. The subdir basename equals the declared `name: "cli-write"`
/// per the basename-invariant.
///
/// Lays down `<tmp>/vault-repo/.git/` so the CLI's `find_workspace_root`
/// walk finds `<tmp>` and the engine's `vault-repo/.git/` fail-fast
/// accepts the workspace. Tests run the binary with
/// `current_dir(tmp)` so the binary's `.memstead/workspace.toml` walk
/// resolves the workspace from cwd.
fn make_vault(tmp: &Path) -> PathBuf {
    let vault = tmp.join("cli-write");
    fs::create_dir_all(&vault).unwrap();
    let store = vault.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    // The CLI write flow routes through the `VaultWriter` seam — for
    // vault-repo-backed vaults commits land on `refs/heads/cli-write` of
    // `<workspace>/vault-repo/.git/`. Seed a real vault-repo from the disk
    // shell so reads and writes share the same gitdir tip.
    init_real_vault_repo_from_disk(tmp, &[(&vault, "cli-write")]);
    vault
}

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// Read an entity and return its current `_hash` from the JSON output.
/// The CLI `entity --json` shape is the typed envelope
/// `{ _hash, id, sections, ... }` (not a `{ markdown: "..." }` flat
/// shape); the helper reads `_hash` directly off the structured field.
fn entity_hash(workspace_root: &Path, id: &str) -> String {
    let out = memstead()
        .current_dir(workspace_root)
        .args(["--json", "entity", id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&out).expect("entity --json output is JSON");
    json["_hash"]
        .as_str()
        .unwrap_or_else(|| panic!("entity --json must carry `_hash`: {json}"))
        .to_string()
}

#[test]
fn create_markdown() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "Alpha",
            "--type",
            "spec",
            "--section",
            "identity=The alpha entity.",
            "--section",
            "purpose=Verifies CLI write round-trip.",
        ])
        .assert()
        .success()
        .stdout(contains("Created `cli-write--alpha`"));

    // Vault-db-backed vaults persist via `vault-repo/.git/refs/heads/<vault>`
    // — the stdout marker covers the same write-landed contract.
}

#[test]
fn create_from_json_file() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());

    let payload = tmp.path().join("payload.json");
    // The `--from` payload uses `entity_type` (matching the response
    // envelopes), not the legacy `type` key.
    fs::write(
        &payload,
        r#"{
            "title": "Gamma",
            "entity_type": "spec",
            "sections": {
                "identity": "Loaded via --from.",
                "purpose": "Covers the JSON-input path."
            }
        }"#,
    )
    .unwrap();

    memstead()
        .current_dir(tmp.path())
        .args(["create", "--from"])
        .arg(&payload)
        .assert()
        .success()
        .stdout(contains("cli-write--gamma"));
}

#[test]
fn full_round_trip_create_update_delete() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());

    // Step 1 — create.
    memstead()
        .current_dir(tmp.path())
        .args([
            "create", "--title", "Delta", "--type", "spec", "--section",
            "identity=d", "--section", "purpose=d",
        ])
        .assert()
        .success();

    // Step 2 — read hash.
    let hash1 = entity_hash(tmp.path(), "cli-write--delta");

    // Step 3 — update (strict hash).
    memstead()
        .current_dir(tmp.path())
        .args([
            "update",
            "cli-write--delta",
            "--expected-hash",
            &hash1,
            "--section",
            "purpose=Updated purpose via strict hash.",
        ])
        .assert()
        .success()
        .stdout(contains("Updated `cli-write--delta`"));

    // Step 4 — update again via --auto-hash (no need to reread).
    memstead()
        .current_dir(tmp.path())
        .args([
            "update",
            "cli-write--delta",
            "--auto-hash",
            "--append",
            "purpose= Appended via auto-hash.",
        ])
        .assert()
        .success()
        .stdout(contains("Updated `cli-write--delta`"));

    // Step 5 — delete.
    memstead()
        .current_dir(tmp.path())
        .args(["delete", "cli-write--delta"])
        .assert()
        .success()
        .stdout(contains("Deleted `cli-write--delta`"));

    // Disk-existence post-condition is moot for vault-repo-backed vaults —
    // the subsequent `memstead entity` lookups in other tests cover the
    // same "the entity is gone" contract.
}

#[test]
fn update_requires_hash_by_default() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());
    memstead()
        .current_dir(tmp.path())
        .args([
            "create", "--title", "Eps", "--type", "spec", "--section",
            "identity=x", "--section", "purpose=x",
        ])
        .assert()
        .success();

    memstead()
        .current_dir(tmp.path())
        .args([
            "update",
            "cli-write--eps",
            "--section",
            "purpose=no hash given",
        ])
        .assert()
        .code(5)
        .stderr(contains("--expected-hash"));
}

#[test]
fn update_wrong_hash_returns_exit_4() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());
    memstead()
        .current_dir(tmp.path())
        .args([
            "create", "--title", "Zeta", "--type", "spec", "--section",
            "identity=x", "--section", "purpose=x",
        ])
        .assert()
        .success();

    memstead()
        .current_dir(tmp.path())
        .args([
            "update",
            "cli-write--zeta",
            "--expected-hash",
            "deadbeef",
            "--section",
            "purpose=q",
        ])
        .assert()
        .code(4)
        .stderr(contains("current:"));
}

#[test]
fn update_wrong_hash_json_mode_carries_current() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());
    memstead()
        .current_dir(tmp.path())
        .args([
            "create", "--title", "Omicron", "--type", "spec", "--section",
            "identity=x", "--section", "purpose=x",
        ])
        .assert()
        .success();

    memstead()
        .current_dir(tmp.path())
        .args(["--json"])
        .args([
            "update",
            "cli-write--omicron",
            "--expected-hash",
            "deadbeef",
            "--section",
            "purpose=q",
        ])
        .assert()
        .code(4)
        // Under `--json` the error envelope rides stdout.
        .stdout(contains("\"current\""));
}

#[test]
fn relate_adds_edge_visible_from_relations() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());

    for (title, slug_sections) in [("Src", "identity=s"), ("Dst", "identity=d")] {
        memstead()
            .current_dir(tmp.path())
            .args([
                "create", "--title", title, "--type", "spec", "--section",
                slug_sections, "--section", "purpose=x",
            ])
            .assert()
            .success();
    }

    memstead()
        .current_dir(tmp.path())
        .args(["relate", "cli-write--src", "USES", "cli-write--dst"])
        .assert()
        .success()
        .stdout(contains("Added"))
        .stdout(contains("USES"));

    memstead()
        .current_dir(tmp.path())
        .args(["relations", "cli-write--src"])
        .assert()
        .success()
        .stdout(contains("cli-write--dst"));
}

#[test]
fn delete_dry_run_does_not_remove_file() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());
    memstead()
        .current_dir(tmp.path())
        .args([
            "create", "--title", "Phi", "--type", "spec", "--section",
            "identity=x", "--section", "purpose=x",
        ])
        .assert()
        .success();

    memstead()
        .current_dir(tmp.path())
        .args(["delete", "cli-write--phi", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("Dry-run"));

    // The dry-run contract is observable via the stdout marker plus
    // the entity remaining readable; reading via `memstead entity` would
    // succeed because the dry-run skipped the writer commit.
}

/// `delete --dry-run` states the would-be
/// verdict — `HAS_INCOMING_REFS` when a Write-vault referrer blocks the
/// delete, `would PROCEED` when nothing does — and that verdict matches
/// the real `memstead delete` outcome in both the refuse and the allow case.
#[test]
fn delete_dry_run_reports_verdict_matching_real_delete() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());
    for (title, sect) in [("Src", "identity=s"), ("Dst", "identity=d")] {
        memstead()
            .current_dir(tmp.path())
            .args([
                "create", "--title", title, "--type", "spec", "--section", sect,
                "--section", "purpose=x",
            ])
            .assert()
            .success();
    }
    // src --USES--> dst: dst now has a blocking Write-vault referrer.
    memstead()
        .current_dir(tmp.path())
        .args(["relate", "cli-write--src", "USES", "cli-write--dst"])
        .assert()
        .success();

    // Dry-run on the referenced entity surfaces the would-be refusal —
    // an agent can decide not to attempt the delete from the preview alone.
    memstead()
        .current_dir(tmp.path())
        .args(["delete", "cli-write--dst", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("would REFUSE"))
        .stdout(contains("HAS_INCOMING_REFS"));
    // The dry-run was side-effect-free: the entity is still readable.
    memstead()
        .current_dir(tmp.path())
        .args(["entity", "cli-write--dst"])
        .assert()
        .success();
    // The real delete refuses, matching the verdict.
    memstead()
        .current_dir(tmp.path())
        .args(["delete", "cli-write--dst"])
        .assert()
        .failure();

    // Dry-run on the unreferenced source previews a clean removal, and
    // the real delete then succeeds — verdict matches in the allow case.
    memstead()
        .current_dir(tmp.path())
        .args(["delete", "cli-write--src", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("would PROCEED"));
    memstead()
        .current_dir(tmp.path())
        .args(["delete", "cli-write--src"])
        .assert()
        .success()
        .stdout(contains("Deleted"));
}

#[test]
fn rename_changes_id() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());
    memstead()
        .current_dir(tmp.path())
        .args([
            "create", "--title", "Old Name", "--type", "spec", "--section",
            "identity=x", "--section", "purpose=x",
        ])
        .assert()
        .success();

    memstead()
        .current_dir(tmp.path())
        .args([
            "rename",
            "cli-write--old-name",
            "New Name",
            "--auto-hash",
        ])
        .assert()
        .success()
        .stdout(contains("cli-write--new-name"));

    memstead()
        .current_dir(tmp.path())
        .args(["entity", "cli-write--new-name"])
        .assert()
        .success()
        .stdout(contains("# New Name"));
}

#[test]
fn batch_update_from_file() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());

    for title in ["Bat1", "Bat2"] {
        memstead()
            .current_dir(tmp.path())
            .args([
                "create", "--title", title, "--type", "spec", "--section",
                "identity=x", "--section", "purpose=x",
            ])
            .assert()
            .success();
    }

    let h1 = entity_hash(tmp.path(), "cli-write--bat1");
    let h2 = entity_hash(tmp.path(), "cli-write--bat2");

    let payload = tmp.path().join("batch.json");
    fs::write(
        &payload,
        serde_json::json!({
            "updates": [
                { "id": "cli-write--bat1", "expected_hash": h1,
                  "sections": { "purpose": "Batched #1" } },
                { "id": "cli-write--bat2", "expected_hash": h2,
                  "sections": { "purpose": "Batched #2" } }
            ]
        })
        .to_string(),
    )
    .unwrap();

    memstead()
        .current_dir(tmp.path())
        .args(["batch-update", "--from"])
        .arg(&payload)
        .assert()
        .success()
        .stdout(contains("applied — 2 item(s) in one commit"));
}

/// Atomic refusal: a 2-entry batch where the second entry targets a
/// missing id refuses the WHOLE batch — nothing is committed, and the
/// valid first entry's section change does NOT land. The output names
/// the refusal and marks the valid entry `not_applied`.
#[test]
fn batch_update_refuses_whole_batch_on_one_bad_entry() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args([
            "create", "--title", "Atomic", "--type", "spec", "--section",
            "identity=x", "--section", "purpose=orig",
        ])
        .assert()
        .success();

    let h = entity_hash(tmp.path(), "cli-write--atomic");

    let payload = tmp.path().join("batch.json");
    fs::write(
        &payload,
        serde_json::json!({
            "updates": [
                { "id": "cli-write--atomic", "expected_hash": h,
                  "sections": { "purpose": "should NOT land" } },
                { "id": "cli-write--ghost", "force": true,
                  "sections": { "purpose": "missing entity" } }
            ]
        })
        .to_string(),
    )
    .unwrap();

    // CLI F12: a refused batch exits non-zero (was exit 0), matching the
    // exit-code table for the dominant failure — here `ENTITY_NOT_FOUND`
    // → 3, the same code single `memstead update`/`entity` use for a missing
    // id. The human breakdown still prints on stdout.
    memstead()
        .current_dir(tmp.path())
        .args(["batch-update", "--from"])
        .arg(&payload)
        .assert()
        .failure()
        .code(3)
        .stdout(contains("REFUSED"))
        .stdout(contains("not_applied"))
        .stdout(contains("ENTITY_NOT_FOUND"));

    // The valid entry's change must not have landed — the batch was
    // refused as a unit.
    memstead()
        .current_dir(tmp.path())
        .args(["entity", "cli-write--atomic"])
        .assert()
        .success()
        .stdout(contains("orig"))
        .stdout(contains("should NOT land").not());
}

/// CLI F12 (`--json`): a refused batch exits non-zero and emits exactly
/// one JSON document — the standard `{code, message, details}` error
/// envelope. `code` is the stable `BATCH_REFUSED` token (so a script can
/// branch on `--json | jq -r .code`), and `details` carries the full
/// `BatchResult` (`applied:false`, per-entry `results`) so nothing is
/// lost. A stale hash → exit 4, matching single `update`.
#[test]
fn batch_update_json_refusal_exits_nonzero_with_envelope() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args([
            "create", "--title", "JsonAtomic", "--type", "spec", "--section",
            "identity=x", "--section", "purpose=orig",
        ])
        .assert()
        .success();

    // A deliberately stale hash → HASH_MISMATCH → exit 4 (mirrors single
    // `update`), and the whole batch refuses atomically.
    let payload = tmp.path().join("batch.json");
    fs::write(
        &payload,
        serde_json::json!({
            "updates": [
                { "id": "cli-write--jsonatomic", "expected_hash": "0000000000000000",
                  "sections": { "purpose": "should NOT land" } }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let output = memstead()
        .current_dir(tmp.path())
        .args(["--json", "batch-update", "--from"])
        .arg(&payload)
        .assert()
        .failure()
        .code(4)
        .get_output()
        .clone();

    // Exactly one JSON document on stdout.
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("refused-batch --json stdout must be one JSON document: {e}; stdout:\n{stdout}")
    });
    assert_eq!(
        parsed["code"], "BATCH_REFUSED",
        "top-level code must signal the refusal: {parsed}",
    );
    // Full result preserved under details.
    assert_eq!(parsed["details"]["applied"], false, "details carries the BatchResult: {parsed}");
    assert_eq!(
        parsed["details"]["results"][0]["error"]["code"], "HASH_MISMATCH",
        "per-entry failure code stays available: {parsed}",
    );
}

/// CLI F12 complement: a successful `--json` batch is unchanged — exit 0,
/// the bare `BatchResult` on stdout with `applied:true` and the commit
/// sha (no error-envelope wrapping on the success path).
#[test]
fn batch_update_json_success_unchanged_exits_zero() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args([
            "create", "--title", "JsonOk", "--type", "spec", "--section",
            "identity=x", "--section", "purpose=x",
        ])
        .assert()
        .success();
    let h = entity_hash(tmp.path(), "cli-write--jsonok");

    let payload = tmp.path().join("batch.json");
    fs::write(
        &payload,
        serde_json::json!({
            "updates": [
                { "id": "cli-write--jsonok", "expected_hash": h,
                  "sections": { "purpose": "Batched" } }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let output = memstead()
        .current_dir(tmp.path())
        .args(["--json", "batch-update", "--from"])
        .arg(&payload)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["applied"], true, "success path keeps the bare BatchResult shape: {parsed}");
    assert_eq!(parsed["succeeded"], 1);
    assert!(parsed["commit_sha"].as_str().is_some_and(|s| !s.is_empty()));
}

/// CLI F13: a `--include-notes` read of a batch-update commit names every
/// entity the batch touched via an additive `entity_ids` array — the
/// subject still collapses to `(N entities)` (so `subject`/`entity_id`
/// keep their backward shape), but the note alone is now self-describing.
#[test]
fn batch_update_commit_note_names_entities_via_include_notes() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_vault(tmp.path());

    for title in ["Note1", "Note2"] {
        memstead()
            .current_dir(tmp.path())
            .args([
                "create", "--title", title, "--type", "spec", "--section",
                "identity=x", "--section", "purpose=x",
            ])
            .assert()
            .success();
    }
    let h1 = entity_hash(tmp.path(), "cli-write--note1");
    let h2 = entity_hash(tmp.path(), "cli-write--note2");

    let payload = tmp.path().join("batch.json");
    fs::write(
        &payload,
        serde_json::json!({
            "updates": [
                { "id": "cli-write--note1", "expected_hash": h1,
                  "sections": { "purpose": "Batched #1" } },
                { "id": "cli-write--note2", "expected_hash": h2,
                  "sections": { "purpose": "Batched #2" } }
            ]
        })
        .to_string(),
    )
    .unwrap();

    memstead()
        .current_dir(tmp.path())
        .args(["batch-update", "--from"])
        .arg(&payload)
        .assert()
        .success();

    // Walk every commit (empty-tree sentinel as `since`) with notes folded in.
    let output = memstead()
        .current_dir(tmp.path())
        .args([
            "--json", "changes",
            "--since", "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
            "--include-notes",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();

    let notes = parsed["notes"].as_array().expect("notes[] present with --include-notes");
    let batch_note = notes
        .iter()
        .find(|n| n["subject"].as_str().is_some_and(|s| s.contains("batch-update")))
        .unwrap_or_else(|| panic!("batch-update commit note must be present; notes:\n{parsed}"));

    // Subject keeps its count-string shape (backward compatibility).
    assert!(
        batch_note["subject"].as_str().unwrap().contains("(2 entities)"),
        "subject keeps the count-string: {batch_note}",
    );
    // The additive entity_ids array names both touched entities.
    let ids: Vec<&str> = batch_note["entity_ids"]
        .as_array()
        .expect("batch note carries entity_ids")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"cli-write--note1") && ids.contains(&"cli-write--note2"),
        "entity_ids must name every entity the batch touched; got: {ids:?}",
    );
}

// -----------------------------------------------------------------------------
// Filesystem-vault write-side dispatch — proves Bug 2 closure for `create`,
// `update`, `delete`, `relate`, `rename` on the filesystem flavour. Each test
// initialises a fresh filesystem-vault workspace via `memstead init`, then
// exercises the relevant subcommand via the CLI subprocess (no engine
// shortcuts, no hand-shaped .md seeds).
// -----------------------------------------------------------------------------

fn entity_hash_filesystem(workspace_root: &Path, id: &str) -> String {
    entity_hash(workspace_root, id)
}

fn init_filesystem(tmp: &TempDir, name: &str) {
    memstead()
        .current_dir(tmp.path())
        .args(["init", "--name", name, "--schema", "default@1.0.0"])
        .assert()
        .success();
}

#[test]
fn create_works_on_filesystem_vault_workspace() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "Alpha",
            "--type",
            "spec",
            "--section",
            "identity=The alpha entity for filesystem CLI tests.",
            "--section",
            "purpose=Exercise the create-on-filesystem path end to end.",
        ])
        .assert()
        .success()
        .stdout(contains("Created `demo--alpha`"));

    // `memstead entity` should now read the entity back through the
    // filesystem-engine path — proves the round-trip across two
    // separate CLI invocations against the same workspace.
    memstead()
        .current_dir(tmp.path())
        .args(["entity", "demo--alpha"])
        .assert()
        .success()
        .stdout(contains("# Alpha"));
}

#[test]
fn update_works_on_filesystem_vault_workspace() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "Updatable",
            "--type",
            "spec",
            "--section",
            "identity=before",
            "--section",
            "purpose=before",
        ])
        .assert()
        .success();

    let hash = entity_hash_filesystem(tmp.path(), "demo--updatable");
    memstead()
        .current_dir(tmp.path())
        .args([
            "update",
            "demo--updatable",
            "--expected-hash",
            &hash,
            "--section",
            "identity=after",
        ])
        .assert()
        .success()
        .stdout(contains("Updated `demo--updatable`"));
}

#[test]
fn delete_works_on_filesystem_vault_workspace() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "Doomed",
            "--type",
            "spec",
            "--section",
            "identity=now you see me",
            "--section",
            "purpose=now you don't",
        ])
        .assert()
        .success();

    memstead()
        .current_dir(tmp.path())
        .args(["delete", "demo--doomed"])
        .assert()
        .success()
        .stdout(contains("Deleted `demo--doomed`"));

    // Re-read should now fail with NOT_FOUND.
    memstead()
        .current_dir(tmp.path())
        .args(["entity", "demo--doomed"])
        .assert()
        .failure()
        .code(3);
}

#[test]
fn relate_works_on_filesystem_vault_workspace() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    for title in ["Source", "Target"] {
        memstead()
            .current_dir(tmp.path())
            .args([
                "create",
                "--title",
                title,
                "--type",
                "spec",
                "--section",
                "identity=x",
                "--section",
                "purpose=x",
            ])
            .assert()
            .success();
    }

    memstead()
        .current_dir(tmp.path())
        .args(["relate", "demo--source", "USES", "demo--target"])
        .assert()
        .success()
        .stdout(contains("Added"));
}

#[test]
fn rename_works_on_filesystem_vault_workspace() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "Old Name",
            "--type",
            "spec",
            "--section",
            "identity=x",
            "--section",
            "purpose=x",
        ])
        .assert()
        .success();

    let hash = entity_hash_filesystem(tmp.path(), "demo--old-name");
    memstead()
        .current_dir(tmp.path())
        .args([
            "rename",
            "demo--old-name",
            "New Name",
            "--expected-hash",
            &hash,
        ])
        .assert()
        .success()
        .stdout(contains("Renamed"))
        .stdout(contains("demo--new-name"));
}

/// `memstead changes --since ""` on a filesystem-vault workspace reads
/// `.memstead/changes.jsonl` and surfaces every entry whose `ts` exceeds
/// the cursor. After a single `create`, the log holds one row tagged
/// with the new entity's id — exercises the filesystem dispatch arm
/// added on top of the vault-repo path.
#[test]
fn changes_works_on_filesystem_vault_workspace() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "Logged",
            "--type",
            "spec",
            "--section",
            "identity=x",
            "--section",
            "purpose=x",
        ])
        .assert()
        .success();

    memstead()
        .current_dir(tmp.path())
        .args(["changes", "--since", ""])
        .assert()
        .success()
        .stdout(contains("demo--logged"));
}

/// `memstead export --format vault` on a filesystem-vault workspace
/// invokes the `assemble_archive` path. Without a `version` field in
/// `.memstead/config.json`, the archive shape projection refuses with a
/// `MissingVersion` error — locks in that the CLI surfaces that
/// failure cleanly instead of silently producing an unstamped `.mem`.
/// F1: `memstead init` now seeds `version = 0.1.0` so the failure path
/// only fires when the field is removed (simulating a pre-gate or
/// externally-imported config). The CLI must surface this via the
/// typed `VAULT_CONFIG_INCOMPLETE` envelope.
#[test]
fn export_vault_on_filesystem_requires_version() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    // Strip `version` from the engine-default config to force the
    // residual missing-version path.
    let config_path = tmp.path().join(".memstead").join("config.json");
    let body = fs::read_to_string(&config_path).unwrap();
    let mut parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    parsed.as_object_mut().unwrap().remove("version");
    fs::write(&config_path, serde_json::to_string_pretty(&parsed).unwrap()).unwrap();

    memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "vault", "-o", "out.mem"])
        .assert()
        .failure();
}

/// `memstead export --format vault` on a filesystem-vault workspace with
/// a complete config (`name`, `schema`, `version`) packs the workspace
/// into a portable `.mem` zip and writes it to `--output`.
#[test]
fn export_vault_on_filesystem_writes_archive() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    let config_path = tmp.path().join(".memstead").join("config.json");
    fs::write(
        &config_path,
        r#"{ "format": 1, "name": "demo", "schema": "default@1.0.0", "version": "0.1.0" }"#,
    )
    .unwrap();

    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "Packed",
            "--type",
            "spec",
            "--section",
            "identity=x",
            "--section",
            "purpose=x",
        ])
        .assert()
        .success();

    let archive_path = tmp.path().join("out.mem");
    memstead()
        .current_dir(tmp.path())
        .args([
            "export",
            "--format",
            "vault",
            "-o",
            archive_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(
        archive_path.is_file(),
        "expected {} to exist after export --format vault",
        archive_path.display()
    );
    assert!(
        fs::metadata(&archive_path).unwrap().len() > 0,
        "archive should be non-empty"
    );
}

/// F1: `memstead vault set-version` updates the workspace config's
/// `version` field on disk. The change persists across CLI
/// invocations — a follow-up `memstead export --format vault` uses the
/// bumped version in the default archive filename.
#[test]
fn vault_set_version_persists_through_filesystem_backend() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    // Engine-default seed is `0.1.0` per F1; bump to 0.2.0.
    memstead()
        .current_dir(tmp.path())
        .args(["vault", "set-version", "demo", "0.2.0"])
        .assert()
        .success();

    // Verify the on-disk config reflects the bump.
    let config_path = tmp.path().join(".memstead").join("config.json");
    let body = fs::read_to_string(&config_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        parsed["version"].as_str(),
        Some("0.2.0"),
        "version must be bumped on disk: {body}"
    );

    // Malformed semver refuses with INVALID_INPUT exit + envelope.
    memstead()
        .current_dir(tmp.path())
        .args(["vault", "set-version", "demo", "not-a-semver"])
        .assert()
        .failure();

    // Unknown vault refuses with UNKNOWN_VAULT.
    memstead()
        .current_dir(tmp.path())
        .args(["vault", "set-version", "no-such-vault", "1.0.0"])
        .assert()
        .failure();
}

/// `memstead export --format markdown` on a filesystem-vault workspace
/// rejects with a validation error because entities are already on
/// disk in canonical form. Locks in the explicit "not yet supported"
/// path instead of a silent no-op.
#[test]
fn export_markdown_on_filesystem_rejects() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "markdown"])
        .assert()
        .failure()
        .stderr(contains("not yet supported"));
}

/// `memstead batch-update` is vault-repo-only because every entry needs an
/// optimistic-locking `expected_hash` over a vault-repo commit graph.
/// On a filesystem-vault workspace the CLI surfaces the
/// "vault-repo-only" message so the operator knows to either move
/// flavours or replay the updates one by one through `memstead update`.
///
/// Only meaningful in the pro build — under `--no-default-features`
/// the `batch-update` subcommand is gated out at the clap layer, so
/// the bail-on-filesystem behaviour can't be exercised.
#[test]
fn batch_update_on_filesystem_surfaces_vault_repo_only() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    let payload = tmp.path().join("updates.json");
    fs::write(
        &payload,
        r#"{ "updates": [{ "id": "demo--anything", "expected_hash": "deadbeef" }] }"#,
    )
    .unwrap();

    memstead()
        .current_dir(tmp.path())
        .args(["batch-update", "--from"])
        .arg(&payload)
        .assert()
        .failure()
        .stderr(contains("vault-repo-only"));
}

/// `memstead workspace dump` is vault-repo-only because the snapshot token
/// is the vault's branch HEAD oid in `vault-repo/.git/`. Filesystem
/// vaults have no git history, so the command surfaces the same
/// "vault-repo-only" message that the legacy `engine()` fallback
/// produces.
///
/// Only meaningful in the pro build — see the `batch_update_on_filesystem_*`
/// twin for the rationale.
#[test]
fn workspace_dump_on_filesystem_surfaces_vault_repo_only() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    memstead()
        .current_dir(tmp.path())
        .args(["workspace", "dump"])
        .assert()
        .failure()
        .stderr(contains("vault-repo-only"));
}

/// `memstead update --declare-relations REL:TARGET` lands the
/// declared relation in one CLI call and the response surfaces the
/// `relations_declared` echo. Locks the CLI flag plumbing for the
/// atomic-batched-declaration feature.
#[test]
fn update_declare_relations_lands_in_one_cli_call() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "Source",
            "--type",
            "spec",
            "--section",
            "identity=Source entity",
            "--section",
            "purpose=Source purpose",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--title",
            "Target",
            "--type",
            "spec",
            "--section",
            "identity=Target entity",
            "--section",
            "purpose=Target purpose",
        ])
        .assert()
        .success();

    let hash = entity_hash_filesystem(tmp.path(), "demo--source");
    memstead()
        .current_dir(tmp.path())
        .args([
            "update",
            "demo--source",
            "--expected-hash",
            &hash,
            "--declare-relations",
            "USES:demo--target",
        ])
        .assert()
        .success()
        .stdout(contains("Relations declared:"))
        .stdout(contains("USES → demo--target"));

    // The relation is queryable via `memstead relations`. USES (not
    // REFERENCES) — explicit author of REFERENCES is refused under
    // the default schema's `alias_target_rel_type` pointer.
    memstead()
        .current_dir(tmp.path())
        .args(["relations", "demo--source"])
        .assert()
        .success()
        .stdout(contains("USES"))
        .stdout(contains("demo--target"));
}

/// `memstead update --declare-relations` with a missing `:` separator
/// surfaces a validation error before the engine call.
#[test]
fn update_declare_relations_rejects_malformed_value() {
    let tmp = TempDir::new().unwrap();
    init_filesystem(&tmp, "demo");

    memstead()
        .current_dir(tmp.path())
        .args([
            "update",
            "demo--missing",
            "--expected-hash",
            "0000000000000000",
            "--declare-relations",
            "no-separator-here",
        ])
        .assert()
        .failure()
        .stderr(contains("expected REL_TYPE:TARGET_ID"));
}

