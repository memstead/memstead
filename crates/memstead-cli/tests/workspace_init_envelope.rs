#![cfg(feature = "vault-repo")]
//! Integration tests for the workspace-init / no-workspace edges.
//!
//! Covers two behaviours of the workspace-config CLI:
//!
//! - Round-trip: `memstead vault-repo init <path>` followed by
//!   `memstead stats` in the same workspace succeeds. Init writes
//!   `.memstead/workspace.toml`, so the second invocation no longer
//!   trips `StoreError::NotInitialised`.
//!
//! - Typed envelope: a workspace-affecting command run from a
//!   directory without `.memstead/workspace.toml` surfaces a JSON envelope
//!   carrying `code: "WORKSPACE_NOT_INITIALISED"` and a structured
//!   `hint.recovery_command` pointing at the right bootstrap command
//!   for this binary's flavour.

use std::fs;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// `--json` stdout is machine-only: exactly one JSON document, the
/// contract `--help` advertises (`memstead … --json | jq -r .<field>`).
/// `vault-repo init` runs the outer-repo `.gitignore`-ensure step; this
/// asserts that step's human provenance lands on stderr, never as a
/// trailing free-text line contaminating the JSON on stdout. The outer
/// `.git/` makes the gitignore step fire (otherwise `NoOuter`, no line).
#[test]
fn vault_repo_init_json_stdout_is_single_document() {
    let tmp = TempDir::new().unwrap();
    // Fake enclosing repo so `apply_outer_gitignore` appends rather than
    // returning `NoOuter` — the path that previously leaked onto stdout.
    fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let workspace = tmp.path().join("ws");

    let output = memstead()
        .args(["--json", "vault-repo", "init", workspace.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = std::str::from_utf8(&output.stdout).expect("stdout must be UTF-8");
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("--json stdout must be exactly one JSON document: {e}; stdout:\n{stdout}")
    });
    assert!(
        parsed.get("vault_repo_dir").and_then(|v| v.as_str()).is_some(),
        "--json envelope must carry `vault_repo_dir`, got: {parsed}",
    );

    // Provenance is preserved — relocated to stderr, not dropped.
    let stderr = std::str::from_utf8(&output.stderr).expect("stderr must be UTF-8");
    assert!(
        stderr.contains("outer:"),
        "outer-repo provenance must appear on stderr, got stderr:\n{stderr}",
    );
}

/// Round-trip: `vault-repo init` produces a workspace `memstead stats`
/// can boot against. Pro-only — `memstead vault-repo` is gated behind
/// the `vault-repo` feature.
#[test]
fn vault_repo_init_followed_by_stats_succeeds() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");

    memstead()
        .args(["vault-repo", "init", workspace.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();

    // Engine must load — init wrote `.memstead/workspace.toml`, so this
    // no longer fails with `StoreError::NotInitialised`.
    memstead()
        .current_dir(&workspace)
        .arg("stats")
        .assert()
        .success()
        .stdout(contains("# Graph stats"));
}

/// Typed envelope: stats from a directory without `.memstead/workspace.toml`
/// returns a typed JSON envelope. Asserts on `code` and the structured
/// `hint.recovery_command` field.
#[test]
fn missing_workspace_emits_typed_envelope_json() {
    let tmp = TempDir::new().unwrap();

    let output = memstead()
        .current_dir(tmp.path())
        .args(["--json", "stats"])
        .assert()
        .failure()
        .get_output()
        // Under `--json` the error envelope rides stdout.
        .stdout
        .clone();

    let body = std::str::from_utf8(&output).expect("stdout must be UTF-8");
    let parsed: serde_json::Value =
        serde_json::from_str(body.trim()).expect("--json error envelope must parse as JSON");

    assert_eq!(
        parsed["code"], "WORKSPACE_NOT_INITIALISED",
        "envelope must carry the symbolic workspace-not-init code, got: {parsed}",
    );

    // Pro CLI binary: recovery hint is always `memstead vault-repo init`.
    // Now lives under `details.hint.recovery_command` — the uniform
    // `{code, message, details}` envelope nests recovery payloads
    // under `details` rather than merging flat at top level.
    let expected_command = "memstead vault-repo init";
    assert_eq!(
        parsed["details"]["hint"]["recovery_command"], expected_command,
        "details.hint.recovery_command must name the bootstrap command for this flavour, got: {parsed}",
    );
    assert!(
        parsed["message"].as_str().unwrap_or("").contains(".memstead/workspace.toml"),
        "human-readable prose must still name the workspace marker, got: {parsed}",
    );
}

/// Typed envelope: the non-JSON path still prints actionable prose
/// on stderr. The `memstead: <message>` shape is the existing
/// `print_cli_error` contract — this path must not regress it.
#[test]
fn missing_workspace_prints_prose_to_stderr() {
    let tmp = TempDir::new().unwrap();

    memstead()
        .current_dir(tmp.path())
        .arg("stats")
        .assert()
        .failure()
        .stderr(contains(".memstead/workspace.toml"));
}
