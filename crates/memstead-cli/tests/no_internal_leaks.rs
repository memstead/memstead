#![cfg(feature = "mem-repo")]
//! Regression test: recoverable CLI errors never leak `code: "INTERNAL"`.
//!
//! The structural fix (non-optional `CliError.code` and explicit `code`
//! arg on `tool_error_with_payload`) makes "default to INTERNAL"
//! compile-impossible. This test is the runtime cross-check: walks
//! the seven probe-finding commands plus a handful of other
//! recoverable-error CLI invocations and asserts that none of them
//! ships `code: "INTERNAL"` on the JSON wire.
//!
//! Adding a new command that leaks INTERNAL on a recoverable path
//! fails here, forcing the implementer either to type the code or
//! explicitly opt the new path into the genuinely-systemic list
//! (which lives in the source code, not in this test).

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn parse_envelope(stdout_bytes: &[u8]) -> serde_json::Value {
    // Under `--json` the error envelope rides stdout so the documented
    // `… --json | jq -r .code` recipe works on the error path.
    let body = std::str::from_utf8(stdout_bytes).expect("stdout must be UTF-8");
    serde_json::from_str(body.trim()).unwrap_or_else(|e| {
        panic!("--json error must parse as JSON: {e}\n--- stdout ---\n{body}\n--- end stdout ---")
    })
}

fn assert_typed_code(envelope: &serde_json::Value, ctx: &str) {
    let code = envelope["code"]
        .as_str()
        .unwrap_or_else(|| panic!("{ctx}: envelope must carry a string `code`, got: {envelope}"));
    assert_ne!(
        code, "INTERNAL",
        "{ctx}: recoverable path must not leak code=INTERNAL — got envelope: {envelope}",
    );
}

/// F5/F15 — `memstead update <missing-id>` returns ENTITY_NOT_FOUND.
#[test]
fn update_missing_entity_returns_typed_code() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    memstead()
        .args([
            "mem-repo",
            "init",
            workspace.to_str().unwrap(),
            "--no-gitignore",
        ])
        .assert()
        .success();

    let output = memstead()
        .current_dir(&workspace)
        .args([
            "--json",
            "update",
            "nope--does-not-exist",
            "--section",
            "Body=hi",
            "--auto-hash",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&output);
    assert_typed_code(&env, "update missing entity");
}

/// F13 — `memstead update <id>` with no hash flag returns HASH_FLAG_REQUIRED.
#[test]
fn update_missing_hash_flag_returns_typed_code() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    memstead()
        .args([
            "mem-repo",
            "init",
            workspace.to_str().unwrap(),
            "--no-gitignore",
        ])
        .assert()
        .success();

    let output = memstead()
        .current_dir(&workspace)
        .args(["--json", "update", "anything--here", "--section", "Body=hi"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&output);
    assert_typed_code(&env, "update missing hash flag");
    assert_eq!(
        env["code"], "HASH_FLAG_REQUIRED",
        "expected HASH_FLAG_REQUIRED, got: {env}",
    );
}

/// F21 — `memstead overview --chunk <past-end>` returns CHUNK_OUT_OF_RANGE.
#[test]
fn overview_chunk_past_end_returns_typed_code() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    memstead()
        .args([
            "mem-repo",
            "init",
            workspace.to_str().unwrap(),
            "--no-gitignore",
        ])
        .assert()
        .success();

    let output = memstead()
        .current_dir(&workspace)
        .args(["--json", "overview", "--chunk", "99"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&output);
    assert_typed_code(&env, "overview chunk past end");
    assert_eq!(
        env["code"], "CHUNK_OUT_OF_RANGE",
        "expected CHUNK_OUT_OF_RANGE, got: {env}",
    );
}

/// F28 — `memstead init <non-empty>` returns TARGET_NOT_EMPTY.
#[test]
fn init_non_empty_returns_typed_code() {
    let tmp = TempDir::new().unwrap();
    // Drop a file so the target is non-empty before init runs.
    std::fs::write(tmp.path().join("preexisting.md"), "hi").unwrap();

    let output = memstead()
        .args([
            "--json",
            "init",
            "--name",
            "tmp",
            "--schema",
            "default@1.0.0",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&output);
    assert_typed_code(&env, "init non-empty target");
    assert_eq!(
        env["code"], "TARGET_NOT_EMPTY",
        "expected TARGET_NOT_EMPTY, got: {env}",
    );
}

/// F31 — `memstead type <unknown>` returns UNKNOWN_ENTITY_TYPE.
#[test]
fn type_unknown_returns_typed_code() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    memstead()
        .args([
            "mem-repo",
            "init",
            workspace.to_str().unwrap(),
            "--no-gitignore",
        ])
        .assert()
        .success();

    let output = memstead()
        .current_dir(&workspace)
        .args(["--json", "type", "nonexistent-type"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&output);
    assert_typed_code(&env, "type unknown");
}

/// Cold-start probe — running a workspace command outside a workspace
/// returns WORKSPACE_NOT_INITIALISED, not INTERNAL.
#[test]
fn cold_start_returns_typed_code() {
    let tmp = TempDir::new().unwrap();
    let output = memstead()
        .current_dir(tmp.path())
        .args(["--json", "stats"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&output);
    assert_typed_code(&env, "stats outside workspace");
}

/// `projection migrate` outside a workspace returns the single-sourced
/// WORKSPACE_NOT_INITIALISED code, not INTERNAL.
#[test]
fn projection_migrate_outside_workspace_returns_typed_code() {
    let tmp = TempDir::new().unwrap();
    let output = memstead()
        .current_dir(tmp.path())
        .args(["--json", "projection", "migrate"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&output);
    assert_typed_code(&env, "projection migrate outside workspace");
    assert_eq!(env["code"], "WORKSPACE_NOT_INITIALISED", "got: {env}");
}

/// `projection init` outside a workspace returns the single-sourced
/// WORKSPACE_NOT_INITIALISED code, not INTERNAL.
#[test]
fn projection_init_outside_workspace_returns_typed_code() {
    let tmp = TempDir::new().unwrap();
    let output = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "projection",
            "init",
            "--mem",
            "m",
            "--source",
            "../x",
            "--medium-type",
            "codebase",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&output);
    assert_typed_code(&env, "projection init outside workspace");
    assert_eq!(env["code"], "WORKSPACE_NOT_INITIALISED", "got: {env}");
}

/// `projection init` on an existing binding id returns the typed
/// PROJECTION_EXISTS code, not INTERNAL.
#[test]
fn projection_init_existing_binding_returns_typed_code() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    memstead()
        .args([
            "mem-repo",
            "init",
            workspace.to_str().unwrap(),
            "--no-gitignore",
        ])
        .assert()
        .success();

    let init_args = [
        "projection",
        "init",
        "--mem",
        "engine",
        "--source",
        "../public",
        "--medium-type",
        "codebase",
        "--name",
        "graph",
    ];
    memstead()
        .current_dir(&workspace)
        .args(init_args)
        .assert()
        .success();

    // Second init on the same id refuses.
    let output = memstead()
        .current_dir(&workspace)
        .args(
            ["--json"]
                .iter()
                .chain(init_args.iter())
                .copied()
                .collect::<Vec<_>>(),
        )
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&output);
    assert_typed_code(&env, "projection init existing binding");
    assert_eq!(env["code"], "PROJECTION_EXISTS", "got: {env}");
}

/// `projection enable` on a missing binding returns the typed
/// PROJECTION_NOT_FOUND code (NotFound exit), not INTERNAL.
#[test]
fn projection_enable_missing_binding_returns_typed_code() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    memstead()
        .args([
            "mem-repo",
            "init",
            workspace.to_str().unwrap(),
            "--no-gitignore",
        ])
        .assert()
        .success();

    let output = memstead()
        .current_dir(&workspace)
        .args(["--json", "projection", "enable", "sync", "engine/nope"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&output);
    assert_typed_code(&env, "projection enable missing binding");
    assert_eq!(env["code"], "PROJECTION_NOT_FOUND", "got: {env}");
}

/// `projection migrate` over a dangling ingest→projection ref returns the
/// typed PROJECTION_MIGRATE_DANGLING_REF code, not INTERNAL.
#[test]
fn projection_migrate_dangling_ref_returns_typed_code() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    memstead()
        .args([
            "mem-repo",
            "init",
            workspace.to_str().unwrap(),
            "--no-gitignore",
        ])
        .assert()
        .success();

    // A flat ingest naming a projection that does not exist — a dangling ref.
    let ingests = workspace.join(".memstead").join("ingests");
    std::fs::create_dir_all(&ingests).unwrap();
    std::fs::write(
        ingests.join("engine-graph.json"),
        r#"{"projection":"engine/missing","mode":"discovery","trigger":"loop","batch_size":20,"deny_paths":[]}"#,
    )
    .unwrap();

    let output = memstead()
        .current_dir(&workspace)
        .args(["--json", "projection", "migrate"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&output);
    assert_typed_code(&env, "projection migrate dangling ref");
    assert_eq!(env["code"], "PROJECTION_MIGRATE_DANGLING_REF", "got: {env}");
}
