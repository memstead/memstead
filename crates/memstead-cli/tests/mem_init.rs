#![cfg(feature = "mem-repo")]
// `memstead mem init` is part of the mem-repo subcommand surface; the
// basis build does not ship it.

//! Integration test for `memstead mem init`.
//!
//! The CLI calls the engine in-process via
//! `memstead_engine::mem_management::create_mem`. An earlier design
//! spawned `memstead-mcp` as a subprocess and the test mocked that
//! subprocess via a stub shell script — confirming the JSON-RPC plumbing
//! rather than the actual engine wiring.
//!
//! The test seeds a real `mem-repo/.git/` bare repo via
//! `memstead_git_branch::test_support::init_real_mem_repo` and exercises
//! the full create path, asserting the on-disk post-state: the new
//! mem's content branch is present in the gitdir, and the `.memstead`
//! mount manifest reflects the registration.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_mem_repo;
use tempfile::TempDir;

/// Build an `memstead` command primed with `MEMSTEAD_OPERATOR_MODE=1`.
/// The CLI default honours the workspace allowlist; tests in this file
/// were written assuming the historical silent bypass. The env-var
/// preserves that posture so each such test continues to exercise the
/// same code path — tests that explicitly verify the allowlist gate
/// clear the env-var before invoking.
fn memstead() -> Command {
    let mut cmd = Command::cargo_bin("memstead").expect("memstead binary must be built by cargo");
    cmd.env("MEMSTEAD_OPERATOR_MODE", "1");
    cmd
}

/// Lay down a workspace that the unified engine can boot against:
/// `.memstead/workspace.toml` plus a real bare `mem-repo/.git/` seeded
/// via `init_real_mem_repo`. Returns the workspace root path.
fn seed_workspace(tmp: &Path) -> PathBuf {
    let workspace = tmp.join("ws");
    fs::create_dir_all(&workspace).unwrap();
    let memstead_dir = workspace.join(".memstead");
    fs::create_dir_all(&memstead_dir).unwrap();
    fs::write(
        memstead_dir.join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    // Real bare repo with `main` + `__MEMSTEAD` refs — the engine's boot
    // path probes for these.
    init_real_mem_repo(&workspace, &[]);
    workspace
}

/// Inspect the workspace's `mounts.json` (post-rebuild persistence
/// adapter shape: `.memstead/state/mounts.json`) and assert the named
/// mem is registered.
fn assert_mem_in_mounts(workspace: &Path, mem_name: &str) {
    let mounts_path = workspace.join(".memstead").join("state").join("mounts.json");
    let raw = fs::read_to_string(&mounts_path).unwrap_or_else(|e| {
        panic!(
            "mounts.json must exist at {} after mem init: {e}",
            mounts_path.display(),
        )
    });
    assert!(
        raw.contains(&format!("\"{mem_name}\"")),
        "mounts.json must list `{mem_name}` post-init; got:\n{raw}",
    );
}

/// `memstead mem init <name>` against a real mem-repo workspace
/// registers the mem via the in-process engine call and prints a
/// markdown success block (no longer the stub's pre-rendered text
/// channel).
#[test]
fn memstead_mem_init_creates_mem_via_in_process_engine() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    let mut cmd = memstead();
    cmd.current_dir(&workspace)
        .args(["mem", "init", "test-mem", "--no-gitignore"]);
    let output = cmd.assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("test-mem"),
        "stdout must name the new mem; got:\n{stdout}",
    );
    assert!(
        stdout.contains("Mem `test-mem` created") || stdout.contains("Mem test-mem"),
        "stdout must render a mem-created success block; got:\n{stdout}",
    );
    assert_mem_in_mounts(&workspace, "test-mem");
}

/// Hierarchical paths are first-class. `memstead mem init team/sub-mem`
/// registers the mem under the FULL path — `mounts.json` carries
/// `"mem": "team/sub-mem"`, not the leaf. The legacy `--org-path` flag
/// is retired; the slashed path is the canonical hierarchical-mem input
/// shape.
#[test]
fn memstead_mem_init_hierarchical_name_lands_full_path() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    let mut cmd = memstead();
    cmd.current_dir(&workspace)
        .args(["mem", "init", "demo/engine", "--no-gitignore"]);
    cmd.assert().success();

    let mounts_path = workspace.join(".memstead").join("state").join("mounts.json");
    let raw = fs::read_to_string(&mounts_path).unwrap();
    assert!(
        raw.contains("\"demo/engine\""),
        "mounts.json must carry the full hierarchical name as the mem \
         identifier (not the bare leaf); got:\n{raw}",
    );
    assert!(
        raw.contains("demo/engine"),
        "branch ref must encode the hierarchical path; got:\n{raw}",
    );
}

/// The `--org-path` flag is retired alongside the `params.path` engine
/// field. The CLI refuses the legacy flag with a clap parse error (so
/// scripts that still pass it fail loudly rather than silently dropping
/// the path).
#[test]
fn memstead_mem_init_rejects_legacy_org_path_flag() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    let mut cmd = memstead();
    cmd.current_dir(&workspace).args([
        "mem",
        "init",
        "engine",
        "--org-path",
        "demo",
        "--no-gitignore",
    ]);
    let assertion = cmd.assert().failure();
    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--org-path")
            && (stderr.contains("unexpected") || stderr.contains("unrecognized")),
        "removed flag must surface as a clap parse error; got stderr:\n{stderr}",
    );
}

/// `memstead mem init <name> --json` emits a structured envelope
/// (matching what an agent reading `--json` expects) rather than the
/// earlier MCP subprocess's text-channel markdown.
#[test]
fn memstead_mem_init_json_mode_emits_structured_envelope() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    let mut cmd = memstead();
    cmd.current_dir(&workspace).args([
        "--json",
        "mem",
        "init",
        "json-mem",
        "--no-gitignore",
    ]);
    let output = cmd.assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("--json output must parse as JSON: {e}; stdout:\n{stdout}")
    });
    assert_eq!(
        parsed.get("name").and_then(|v| v.as_str()),
        Some("json-mem"),
        "--json envelope must carry `name`",
    );
    assert!(
        parsed.get("schema_ref").and_then(|v| v.as_str()).is_some(),
        "--json envelope must carry `schema_ref`",
    );
    assert!(
        parsed
            .get("seed_commit_sha")
            .and_then(|v| v.as_str())
            .is_some(),
        "--json envelope must carry `seed_commit_sha`",
    );
}

/// `--json` stdout is machine-only: exactly one JSON document, the
/// contract `--help` advertises (`memstead … --json | jq -r .<field>`). The
/// outer-repo `.gitignore`-ensure step's human provenance must land on
/// stderr, never appended as a free-text line after the JSON on stdout.
/// The existing envelope test passes `--no-gitignore`, which skips this
/// step entirely — so it never exercised the leak. Here an outer `.git/`
/// (the workspace's grandparent dir) makes the step fire.
#[test]
fn memstead_mem_init_json_stdout_is_single_document_with_gitignore_step() {
    let tmp = TempDir::new().unwrap();
    // `seed_workspace` lays the workspace at `<tmp>/ws`; the gitignore
    // walk starts at its parent (`<tmp>`), so an outer `.git/` here makes
    // `apply_outer_gitignore` append rather than return `NoOuter`.
    fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let workspace = seed_workspace(tmp.path());

    let mut cmd = memstead();
    cmd.current_dir(&workspace)
        .args(["--json", "mem", "init", "json-gi-mem"]);
    let output = cmd.assert().success().get_output().clone();

    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("--json stdout must be exactly one JSON document: {e}; stdout:\n{stdout}")
    });
    assert_eq!(
        parsed.get("name").and_then(|v| v.as_str()),
        Some("json-gi-mem"),
        "--json envelope must carry `name`, got: {parsed}",
    );

    // Provenance preserved on stderr, not dropped or leaked onto stdout.
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    assert!(
        stderr.contains("outer:"),
        "outer-repo provenance must appear on stderr, got stderr:\n{stderr}",
    );
}

// ---------------------------------------------------------------------------
// Verb-split lifecycle tests. The CLI
// gains `memstead mem unregister` (router-only, storage preserved) and
// repoints `memstead mem delete` to the storage-destruction shape (the
// `--delete-files` flag is gone). The four-case matrix below pins
// the per-verb post-state plus the `MEM_REFERENCED_BY_POLICY`
// gating: unregister is admitted regardless of cross-mem grants
// (the storage they rely on survives), delete refuses when grants
// point at the target.
// ---------------------------------------------------------------------------

/// Stamp a `cross_mem_links` block onto an already-seeded
/// workspace's `.memstead/workspace.toml`. The block grants `<from>`
/// permission to author cross-mem links into `<to>`, exercising
/// the engine-side `MEM_REFERENCED_BY_POLICY` check.
fn append_cross_link_grant(workspace: &Path, from: &str, to: &str) {
    let toml_path = workspace.join(".memstead").join("workspace.toml");
    let mut body = fs::read_to_string(&toml_path).unwrap();
    body.push_str(&format!(
        "\n[cross_mem_links]\n{from} = [\"{to}\"]\n",
    ));
    fs::write(&toml_path, body).unwrap();
}

/// Assert the `__MEMSTEAD` ref carries a `mems/<name>/config.json` blob
/// (residue check). The bare `mem-repo/.git/refs/heads/<name>`
/// branch existence is the other half of the storage-preservation
/// signal but the config blob is the simpler one to introspect from
/// the test (gix-free).
fn assert_branch_present(workspace: &Path, mem_name: &str) {
    let branch_ref = workspace
        .join("mem-repo")
        .join(".git")
        .join("refs")
        .join("heads")
        .join(mem_name);
    let packed_refs = workspace
        .join("mem-repo")
        .join(".git")
        .join("packed-refs");
    let loose_present = branch_ref.exists();
    let packed_present = packed_refs
        .exists()
        .then(|| fs::read_to_string(&packed_refs).unwrap_or_default())
        .map(|s| s.contains(&format!("refs/heads/{mem_name}")))
        .unwrap_or(false);
    assert!(
        loose_present || packed_present,
        "branch ref `refs/heads/{mem_name}` must be present after unregister; \
         loose={loose_present} packed={packed_present}",
    );
}

fn assert_branch_absent(workspace: &Path, mem_name: &str) {
    let branch_ref = workspace
        .join("mem-repo")
        .join(".git")
        .join("refs")
        .join("heads")
        .join(mem_name);
    let packed_refs = workspace
        .join("mem-repo")
        .join(".git")
        .join("packed-refs");
    let loose_present = branch_ref.exists();
    let packed_present = packed_refs
        .exists()
        .then(|| fs::read_to_string(&packed_refs).unwrap_or_default())
        .map(|s| s.contains(&format!("refs/heads/{mem_name}")))
        .unwrap_or(false);
    assert!(
        !loose_present && !packed_present,
        "branch ref `refs/heads/{mem_name}` must be pruned after delete; \
         loose={loose_present} packed={packed_present}",
    );
}

fn assert_mem_not_in_mounts(workspace: &Path, mem_name: &str) {
    let mounts_path = workspace.join(".memstead").join("state").join("mounts.json");
    let raw = fs::read_to_string(&mounts_path).unwrap();
    assert!(
        !raw.contains(&format!("\"{mem_name}\"")),
        "mounts.json must no longer list `{mem_name}`; got:\n{raw}",
    );
}

/// `memstead mem unregister <name>` against a grant-free
/// mem unregisters from the router and **preserves storage**.
/// Storage preservation is the verb's defining signal versus
/// `delete`.
#[test]
fn memstead_mem_unregister_preserves_storage() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    memstead()
        .current_dir(&workspace)
        .args(["mem", "init", "archive-target", "--no-gitignore"])
        .assert()
        .success();
    assert_mem_in_mounts(&workspace, "archive-target");

    let output = memstead()
        .current_dir(&workspace)
        .args(["mem", "unregister", "archive-target"])
        .assert()
        .success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("unregistered"),
        "stdout must render the unregister verb in its heading; got:\n{stdout}",
    );

    assert_mem_not_in_mounts(&workspace, "archive-target");
    // Storage survives: the content branch is still on disk for an
    // archive workflow / future re-init.
    assert_branch_present(&workspace, "archive-target");
}

/// `memstead mem delete <name>` against a grant-free mem
/// unregisters AND prunes the content branch. The previous
/// `--delete-files` flag is gone — `delete` is the verb that
/// destroys.
#[test]
fn memstead_mem_delete_prunes_storage() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    memstead()
        .current_dir(&workspace)
        .args(["mem", "init", "scratch", "--no-gitignore"])
        .assert()
        .success();

    let output = memstead()
        .current_dir(&workspace)
        .args(["mem", "delete", "scratch"])
        .assert()
        .success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("deleted"),
        "stdout must render the delete verb in its heading; got:\n{stdout}",
    );

    assert_mem_not_in_mounts(&workspace, "scratch");
    // Storage is gone — the branch ref is pruned.
    assert_branch_absent(&workspace, "scratch");
}

/// `memstead mem unregister <name>` succeeds even when
/// another mem has a `cross_mem_links` grant pointing at the
/// target. Storage survives so the grant remains valid against it;
/// the policy check is gated on `delete_files: true` and does NOT
/// fire for router-only unregister.
#[test]
fn memstead_mem_unregister_is_admitted_under_cross_link_grant() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    memstead()
        .current_dir(&workspace)
        .args(["mem", "init", "target", "--no-gitignore"])
        .assert()
        .success();
    memstead()
        .current_dir(&workspace)
        .args(["mem", "init", "referrer", "--no-gitignore"])
        .assert()
        .success();
    append_cross_link_grant(&workspace, "referrer", "target");

    memstead()
        .current_dir(&workspace)
        .args(["mem", "unregister", "target"])
        .assert()
        .success();
    assert_mem_not_in_mounts(&workspace, "target");
    assert_branch_present(&workspace, "target");
}

/// `memstead mem delete <name>` against a mem with cross-
/// mem grants pointing at it refuses with
/// `MEM_REFERENCED_BY_POLICY`. The operator must revoke the grant
/// first; this hard stop fires regardless of operator-mode (the CLI
/// is operator-mode by construction, so the test pins that the
/// safeguard survives the operator-mode bypass).
#[test]
fn memstead_mem_delete_refuses_under_cross_link_grant() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    memstead()
        .current_dir(&workspace)
        .args(["mem", "init", "target", "--no-gitignore"])
        .assert()
        .success();
    memstead()
        .current_dir(&workspace)
        .args(["mem", "init", "referrer", "--no-gitignore"])
        .assert()
        .success();
    append_cross_link_grant(&workspace, "referrer", "target");

    let assertion = memstead()
        .current_dir(&workspace)
        .args(["mem", "delete", "target"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("MEM_REFERENCED_BY_POLICY"),
        "policy refusal must carry MEM_REFERENCED_BY_POLICY; got stderr:\n{stderr}",
    );
    assert!(
        stderr.contains("referrer"),
        "policy refusal must name the referring mem; got stderr:\n{stderr}",
    );
    // The target survives the refused delete.
    assert_mem_in_mounts(&workspace, "target");
    assert_branch_present(&workspace, "target");
}

/// The previous `memstead mem delete --delete-files` flag is
/// gone. Invoking it must surface a clap parse error (not silently
/// accept the flag and proceed). The presence of `unrecognized` or
/// `unexpected` in stderr is the canonical clap message.
#[test]
fn memstead_mem_delete_rejects_legacy_delete_files_flag() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    memstead()
        .current_dir(&workspace)
        .args(["mem", "init", "scratch", "--no-gitignore"])
        .assert()
        .success();

    let assertion = memstead()
        .current_dir(&workspace)
        .args(["mem", "delete", "scratch", "--delete-files"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--delete-files")
            && (stderr.contains("unexpected") || stderr.contains("unrecognized")),
        "removed flag must surface as a clap parse error; got stderr:\n{stderr}",
    );
}

// ---------------------------------------------------------------------------
// Storage residue
// detection at `mem init`, plus tombstone-aware reattach when the residue
// originates from a deliberate `mem unregister`. These tests
// exercise the CLI end-to-end against a real bare mem-repo so
// the wire shape (engine call → CLI render) is pinned.
// ---------------------------------------------------------------------------

/// `memstead mem unregister` followed by `memstead mem init
/// <same-name>` in a fresh CLI process reattaches the preserved
/// storage. The reattach path emits no new seed commit — pinned via
/// the `--json` envelope's empty `seed_commit_sha`. Guards against
/// silent resurrection for the
/// deliberate-operator path (the operator's unregister stamped the
/// `unregistered_at` tombstone on the config blob; the re-init
/// reads it and routes to the `Reattach` recovery action).
#[test]
fn memstead_mem_init_reattaches_after_unregister() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    // Initial create.
    let initial = memstead()
        .current_dir(&workspace)
        .args(["--json", "mem", "init", "preserved", "--no-gitignore"])
        .assert()
        .success();
    let initial_stdout =
        String::from_utf8(initial.get_output().stdout.clone()).unwrap();
    let initial_envelope: serde_json::Value =
        serde_json::from_str(&initial_stdout).expect("--json envelope parses");
    let initial_seed = initial_envelope["seed_commit_sha"]
        .as_str()
        .expect("initial create must carry a non-empty seed_commit_sha")
        .to_string();
    assert!(
        !initial_seed.is_empty(),
        "initial create must produce a real seed commit SHA",
    );

    // Unregister — router-only removal, storage preserved, tombstone
    // written.
    memstead()
        .current_dir(&workspace)
        .args(["mem", "unregister", "preserved"])
        .assert()
        .success();
    assert_mem_not_in_mounts(&workspace, "preserved");
    assert_branch_present(&workspace, "preserved");

    // Re-init in a fresh CLI process. Engine boots from the
    // post-rebuild mount manifest (which no longer carries
    // `preserved`), encounters the storage residue at
    // `refs/heads/preserved`, reads the `unregistered_at` tombstone,
    // and routes to the reattach recovery action. The response
    // signal is an empty `seed_commit_sha` — the existing branch's
    // history is preserved.
    let reattach = memstead()
        .current_dir(&workspace)
        .args(["--json", "mem", "init", "preserved", "--no-gitignore"])
        .assert()
        .success();
    let reattach_stdout =
        String::from_utf8(reattach.get_output().stdout.clone()).unwrap();
    let reattach_envelope: serde_json::Value =
        serde_json::from_str(&reattach_stdout).expect("--json envelope parses");
    let reattach_seed = reattach_envelope["seed_commit_sha"]
        .as_str()
        .expect("reattach response must carry seed_commit_sha (even if empty)");
    assert!(
        reattach_seed.is_empty(),
        "reattach must skip the seed commit — empty `seed_commit_sha` is the \
         contract signal that the existing branch was adopted; got {reattach_seed:?}",
    );
    // The reattach branch surfaces a `MEM_REATTACHED_AFTER_UNREGISTER`
    // warning through the response envelope's `warnings` array. An
    // agent that didn't internalise the seed-SHA-as-signal convention
    // now has a typed-code path to detect the reattach.
    let warnings = reattach_envelope["warnings"]
        .as_array()
        .expect("reattach response carries a `warnings` array");
    let reattach_warning = warnings
        .iter()
        .find(|w| w.get("code").and_then(|c| c.as_str()) == Some("MEM_REATTACHED_AFTER_UNREGISTER"))
        .expect("MEM_REATTACHED_AFTER_UNREGISTER must appear in the warnings array");
    assert!(
        reattach_warning
            .get("message")
            .and_then(|m| m.as_str())
            .is_some_and(|s| !s.is_empty()),
        "MEM_REATTACHED_AFTER_UNREGISTER must carry a human-readable message"
    );
    // Mount manifest carries the mem again.
    assert_mem_in_mounts(&workspace, "preserved");
}

/// clap mutex group enforces that at most one recovery
/// flag is passed. Combining `--reattach` with `--force-overwrite`
/// (or any other pair) surfaces a parse error before the engine is
/// even reached. The error mentions the conflicting flag so the
/// operator knows which to drop.
#[test]
fn memstead_mem_init_rejects_multiple_recovery_flags() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    let assertion = memstead()
        .current_dir(&workspace)
        .args([
            "mem",
            "init",
            "scratch",
            "--no-gitignore",
            "--reattach",
            "--force-overwrite",
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--reattach") || stderr.contains("--force-overwrite"),
        "mutex error must name at least one conflicting flag; got stderr:\n{stderr}",
    );
}

/// Explicit `--reattach` against residue-without-tombstone
/// is the planned override path: the operator has verified the
/// residue is safe to adopt even though the tombstone-driven default
/// would refuse. Today this test pins that the flag is at minimum
/// *accepted* by clap (no parse error) — once the engine surface
/// gains a test fixture that manufactures no-tombstone residue, the
/// assertion can extend to "succeeds and adopts". For now we exercise
/// the simpler shape: `--reattach` against a fresh workspace with no
/// residue still creates cleanly (the flag is a no-op when there's
/// nothing to reattach to).
#[test]
fn memstead_mem_init_accepts_explicit_reattach_flag() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    memstead()
        .current_dir(&workspace)
        .args(["mem", "init", "fresh", "--no-gitignore", "--reattach"])
        .assert()
        .success();
    assert_mem_in_mounts(&workspace, "fresh");
    assert_branch_present(&workspace, "fresh");
}

/// `--force-overwrite` against residue prunes the existing
/// branch + `__MEMSTEAD` config blob and proceeds with a fresh create.
/// The post-state signal is a NEW `seed_commit_sha` distinct from
/// the pre-overwrite mem's seed (proves the branch was recreated,
/// not adopted). The prior session's entities are gone by design —
/// `force-overwrite` is the explicit destructive recovery path.
#[test]
fn memstead_mem_init_force_overwrite_prunes_and_recreates() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    // Initial create — capture the seed sha.
    let initial = memstead()
        .current_dir(&workspace)
        .args(["--json", "mem", "init", "scratch", "--no-gitignore"])
        .assert()
        .success();
    let initial_envelope: serde_json::Value = serde_json::from_str(
        &String::from_utf8(initial.get_output().stdout.clone()).unwrap(),
    )
    .unwrap();
    let initial_seed = initial_envelope["seed_commit_sha"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!initial_seed.is_empty());

    // Unregister leaves residue (and a tombstone).
    memstead()
        .current_dir(&workspace)
        .args(["mem", "unregister", "scratch"])
        .assert()
        .success();
    assert_branch_present(&workspace, "scratch");

    // Re-init with --force-overwrite — prunes the existing branch
    // and produces a fresh seed commit. The contract signal is a
    // non-empty `seed_commit_sha` (the reattach path returns empty);
    // assert that, plus the mount manifest shows the mem again.
    //
    // Note: the new seed SHA can be byte-identical to the prior one
    // — both seed commits have the same content (empty tree), the
    // same fixed author + committer, and `gix::date::Time` rounds
    // to seconds, so two creates within the same second collide
    // by design. The non-empty assertion is the recreate-vs-
    // reattach discriminator; a stronger pre-vs-post-prune check
    // is in `prune_residue_dispatch`'s integration in the engine
    // (the storage_memstead unit tests already cover the ref-edit
    // transaction).
    let overwritten = memstead()
        .current_dir(&workspace)
        .args([
            "--json",
            "mem",
            "init",
            "scratch",
            "--no-gitignore",
            "--force-overwrite",
        ])
        .assert()
        .success();
    let overwrite_envelope: serde_json::Value = serde_json::from_str(
        &String::from_utf8(overwritten.get_output().stdout.clone()).unwrap(),
    )
    .unwrap();
    let new_seed = overwrite_envelope["seed_commit_sha"]
        .as_str()
        .expect("overwrite must produce a non-null seed_commit_sha")
        .to_string();
    assert!(
        !new_seed.is_empty(),
        "force-overwrite must produce a fresh seed commit; got empty (which is \
         the reattach signal — means the orchestrator took the wrong recovery \
         branch)",
    );
    // Drop unused — silences a clippy hint for the previous-seed
    // capture pattern that the deterministic-commit comment above
    // explains.
    let _ = initial_seed;
    assert_mem_in_mounts(&workspace, "scratch");
}

/// `--hard-cleanup-first` against residue refuses with the
/// `MEM_STORAGE_RESIDUE_DETECTED` code, instructing the operator
/// to run `memstead mem delete` first. Hard barrier against
/// auto-recovery even when an explicit flag is passed.
#[test]
fn memstead_mem_init_hard_cleanup_first_refuses_against_residue() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    memstead()
        .current_dir(&workspace)
        .args(["mem", "init", "scratch", "--no-gitignore"])
        .assert()
        .success();
    memstead()
        .current_dir(&workspace)
        .args(["mem", "unregister", "scratch"])
        .assert()
        .success();

    let assertion = memstead()
        .current_dir(&workspace)
        .args([
            "mem",
            "init",
            "scratch",
            "--no-gitignore",
            "--hard-cleanup-first",
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("MEM_STORAGE_RESIDUE_DETECTED"),
        "hard-cleanup-first must surface MEM_STORAGE_RESIDUE_DETECTED; \
         got stderr:\n{stderr}",
    );
    // The residue survives — the refusal is exactly that.
    assert_branch_present(&workspace, "scratch");
}

/// Hierarchical-path isolation: residue at
/// `<org-a>/<leaf>` must NOT trigger the residue refusal for an
/// init at `<org-b>/<leaf>` (different composed branch_leaf, same
/// leaf segment). The probe is path-aware by design — this test
/// pins the isolation so a future refactor can't drift back to a
/// leaf-only match.
#[test]
fn memstead_mem_init_residue_isolation_is_path_aware() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());

    // Hierarchical paths are first-class. The slashed-path argument flows
    // verbatim through to the engine as the mem name; `--org-path` is
    // retired. Seed residue at `team-a/shared`.
    memstead()
        .current_dir(&workspace)
        .args(["mem", "init", "team-a/shared", "--no-gitignore"])
        .assert()
        .success();
    memstead()
        .current_dir(&workspace)
        .args(["mem", "unregister", "team-a/shared"])
        .assert()
        .success();
    assert_branch_present(&workspace, "team-a/shared");

    // Init `team-b/shared` — same leaf, different org path. Probe
    // is path-aware so the `team-a/shared` residue must NOT trigger
    // a refusal or a reattach here.
    let output = memstead()
        .current_dir(&workspace)
        .args(["--json", "mem", "init", "team-b/shared", "--no-gitignore"])
        .assert()
        .success();
    let envelope: serde_json::Value = serde_json::from_str(
        &String::from_utf8(output.get_output().stdout.clone()).unwrap(),
    )
    .unwrap();
    let seed = envelope["seed_commit_sha"].as_str().unwrap_or("");
    assert!(
        !seed.is_empty(),
        "fresh create at `team-b/shared` must produce a real seed commit — \
         empty seed signals a reattach, which would mean the probe leaked \
         across org paths",
    );
    // Both branches now coexist.
    assert_branch_present(&workspace, "team-a/shared");
    assert_branch_present(&workspace, "team-b/shared");
}

// ---------------------------------------------------------------------------
// CLI operator-mode opt-in. The allowlist applies by default; the operator
// opts into bypass via `--operator-mode` or `MEMSTEAD_OPERATOR_MODE=1`.
//
// These tests construct the `Command` directly (not via `memstead()`)
// because the helper sets `MEMSTEAD_OPERATOR_MODE=1` to preserve the
// historical posture for pre-pivot tests; here we explicitly clear
// it so the new gate fires.
// ---------------------------------------------------------------------------

fn memstead_no_env() -> Command {
    let mut cmd =
        Command::cargo_bin("memstead").expect("memstead binary must be built by cargo");
    cmd.env_remove("MEMSTEAD_OPERATOR_MODE");
    cmd
}

/// Seed a workspace with `[[mem_management.create]]` rules for
/// `test` and `other` only — the canonical allowlist-gate fixture.
fn seed_workspace_with_create_allowlist(tmp: &Path) -> PathBuf {
    let workspace = seed_workspace(tmp);
    let toml_path = workspace.join(".memstead").join("workspace.toml");
    let existing = fs::read_to_string(&toml_path).unwrap();
    fs::write(
        &toml_path,
        format!(
            "{existing}\n\
             [[mem_management.create]]\n\
             pattern = \"test\"\n\
             schemas = [\"default@1.0.0\"]\n\n\
             [[mem_management.create]]\n\
             pattern = \"other\"\n\
             schemas = [\"default@1.0.0\"]\n",
        ),
    )
    .unwrap();
    workspace
}

/// F6 — `memstead mem init forbidden` against an allow-create list of
/// `[test, other]` refuses with `MEM_PATH_NOT_ALLOWED`.
#[test]
fn mem_init_refuses_outside_allowlist_by_default() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace_with_create_allowlist(tmp.path());

    let output = memstead_no_env()
        .current_dir(&workspace)
        .args(["--json", "mem", "init", "forbidden", "--no-gitignore"])
        .assert()
        .failure()
        .get_output()
        // Under `--json` the error envelope rides stdout.
        .stdout
        .clone();
    let body = std::str::from_utf8(&output).expect("stdout UTF-8");
    let env: serde_json::Value =
        serde_json::from_str(body.trim()).expect("--json envelope parses");
    assert_eq!(
        env["code"], "MEM_PATH_NOT_ALLOWED",
        "expected typed refusal, got: {env}",
    );
}

/// Positive admission path — `memstead mem init test` against the same
/// allowlist succeeds. The pivot doesn't regress allowed names.
#[test]
fn mem_init_admits_allowlisted_name_by_default() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace_with_create_allowlist(tmp.path());

    memstead_no_env()
        .current_dir(&workspace)
        .args(["mem", "init", "test", "--no-gitignore"])
        .assert()
        .success();
    assert_mem_in_mounts(&workspace, "test");
}

/// `--operator-mode` flag overrides the allowlist — explicit opt-in.
#[test]
fn mem_init_operator_mode_flag_bypasses_allowlist() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace_with_create_allowlist(tmp.path());

    memstead_no_env()
        .current_dir(&workspace)
        .args([
            "mem",
            "init",
            "forbidden",
            "--no-gitignore",
            "--operator-mode",
        ])
        .assert()
        .success();
    assert_mem_in_mounts(&workspace, "forbidden");
}

/// `MEMSTEAD_OPERATOR_MODE=1` env var overrides the allowlist — script
/// convenience opt-in.
#[test]
fn mem_init_operator_mode_env_var_bypasses_allowlist() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace_with_create_allowlist(tmp.path());

    let mut cmd = memstead_no_env();
    cmd.env("MEMSTEAD_OPERATOR_MODE", "1")
        .current_dir(&workspace)
        .args(["mem", "init", "forbidden", "--no-gitignore"]);
    cmd.assert().success();
    assert_mem_in_mounts(&workspace, "forbidden");
}

/// CLI F7 regression. A
/// destructive `memstead mem delete` must NOT revoke the workspace's
/// `[[mem_management.create]]` / `[[mem_management.delete]]`
/// allowlist rules for the deleted name — they are forward-looking
/// permissions for the name, not references to the instance. So
/// re-creating a mem of the same name afterward succeeds without
/// re-running `allow-create`. The dangling `[cross_mem_links]` grant
/// FROM the deleted mem is still scrubbed (it named the gone
/// instance). The grant direction is `other → keep` so the deletion's
/// own `MEM_REFERENCED_BY_POLICY` gate (which fires only when *another*
/// mem grants the target) stays clear.
#[test]
fn mem_delete_preserves_allowlist_rules_so_recreate_succeeds() {
    let tmp = TempDir::new().unwrap();
    let workspace = seed_workspace(tmp.path());
    let toml_path = workspace.join(".memstead").join("workspace.toml");
    let existing = fs::read_to_string(&toml_path).unwrap();
    fs::write(
        &toml_path,
        format!(
            "{existing}\n\
             [cross_mem_links]\n\
             other = [\"keep\"]\n\n\
             [[mem_management.create]]\n\
             pattern = \"other\"\n\
             schemas = [\"default@1.0.0\"]\n\n\
             [[mem_management.delete]]\n\
             pattern = \"other\"\n",
        ),
    )
    .unwrap();

    // Create `other` (admitted by the create allowlist).
    memstead_no_env()
        .current_dir(&workspace)
        .args(["mem", "init", "other", "--no-gitignore"])
        .assert()
        .success();
    assert_mem_in_mounts(&workspace, "other");

    // Destructive delete — honours the delete allowlist (no operator
    // mode), so the surviving `[[mem_management.delete]]` rule is what
    // admits it.
    memstead_no_env()
        .current_dir(&workspace)
        .args(["mem", "delete", "other"])
        .assert()
        .success();

    // The create + delete allowlist rules for `other` survive the delete.
    let after = fs::read_to_string(&toml_path).unwrap();
    assert_eq!(
        after.matches("pattern = \"other\"").count(),
        2,
        "delete must preserve the create+delete allowlist rules for `other`; got:\n{after}",
    );
    // The deleted mem's own dangling cross-link grant is scrubbed.
    assert!(
        !after.contains("other = [\"keep\"]"),
        "delete must scrub the deleted mem's dangling cross-link grant; got:\n{after}",
    );

    // Re-create the same name — succeeds with no fresh `allow-create`,
    // because the create rule survived. This is the exact F7 reproduction.
    memstead_no_env()
        .current_dir(&workspace)
        .args(["mem", "init", "other", "--no-gitignore"])
        .assert()
        .success();
    assert_mem_in_mounts(&workspace, "other");
}
