#![cfg(feature = "mem-repo")]
//! Boot smoke test for the full MCP binary (`memstead-mcp`).
//!
//! Spawns the binary as a subprocess against a tempdir workspace
//! carrying the post-rebuild markers (`.memstead/workspace.toml` +
//! `.memstead/state/mounts.json`, plus a stub `mem-repo/.git/`).
//! Sends one `initialize` JSON-RPC request over stdin, reads the
//! reply over stdout, asserts the envelope is well-formed.
//!
//! The lean equivalent (testing the --no-default-features build) lives in
//! `memstead-mcp/tests/boot.rs`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

const WORKSPACE_TOML_BODY: &str = "format = \"memstead-git-branch-2\"\n\n\
[persistence_adapter]\nname = \"file-two-layer\"\n";

const MOUNTS_JSON_BODY: &str = r#"{ "format": "memstead-mounts-3", "mounts": [] }"#;

fn memstead_mcp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_memstead-mcp")
}

fn seed_workspace(root: &std::path::Path) {
    let memstead = root.join(".memstead");
    std::fs::create_dir_all(memstead.join("state")).unwrap();
    std::fs::write(memstead.join("workspace.toml"), WORKSPACE_TOML_BODY).unwrap();
    std::fs::write(memstead.join("state").join("mounts.json"), MOUNTS_JSON_BODY).unwrap();
}

fn initialize_request() -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "boot-smoke-test", "version": "0" }
        }
    }))
    .unwrap()
}

fn read_response_with_timeout(
    stdout: std::process::ChildStdout,
    want_id: i64,
    timeout: Duration,
) -> Option<serde_json::Value> {
    let mut reader = BufReader::new(stdout);
    let deadline = Instant::now() + timeout;
    let mut line = String::new();
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return None,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let value: serde_json::Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if value.get("id").and_then(|v| v.as_i64()) == Some(want_id) {
                    return Some(value);
                }
            }
            Err(_) => return None,
        }
    }
}

fn assert_initialize_envelope(response: &serde_json::Value) {
    let result = response
        .get("result")
        .expect("initialize response must carry a `result` field");
    assert!(
        result.get("protocolVersion").is_some(),
        "initialize result missing `protocolVersion`: {response}"
    );
    assert!(
        result.get("capabilities").is_some(),
        "initialize result missing `capabilities`: {response}"
    );
    let server_info = result
        .get("serverInfo")
        .expect("initialize result missing `serverInfo`");
    assert!(
        server_info.get("name").is_some(),
        "serverInfo missing `name`: {response}"
    );
}

#[test]
fn full_binary_boots_against_new_layout_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_workspace(tmp.path());

    // The full binary checks `<workspace>/mem-repo/.git` shape on
    // boot — `init_real_mem_repo` lays down a real bare repo with
    // `main` + `__MEMSTEAD` refs so the engine accepts it.
    memstead_git_branch::test_support::init_real_mem_repo(tmp.path(), &[]);

    let mut child = Command::new(memstead_mcp_bin())
        .current_dir(tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn memstead-mcp (full) — confirm the binary built before running tests");

    let mut stdin = child.stdin.take().expect("child stdin");
    writeln!(stdin, "{}", initialize_request()).expect("write initialize");
    stdin.flush().expect("flush initialize");
    drop(stdin);

    let stdout = child.stdout.take().expect("child stdout");
    let response = read_response_with_timeout(stdout, 1, Duration::from_secs(15))
        .expect("initialize response within 15s — binary did not boot or did not reply");

    assert_initialize_envelope(&response);

    let _ = child.kill();
    let _ = child.wait();
}

/// Boot-failure parity: the full binary's stderr diagnostic for an
/// unbootable workspace carries the same typed code and the same
/// `BootError::surface_message` string the CLI ships for the
/// identical fixture (`memstead-cli/tests/boot_typed_errors.rs` pins
/// the CLI side to the same renderer). Fixture: a corrupt workspace
/// store — under the plan-04 quarantine posture, the only class that
/// still fails the whole boot (mem-level failures quarantine; legacy
/// projection configs quarantine their binding). The server no longer
/// exits — it prints the typed line, then serves the diagnostic
/// shell; stdin EOF ends it cleanly.
#[test]
fn full_binary_boot_failure_prints_typed_code_and_shared_message() {
    let tmp = TempDir::new().unwrap();
    seed_workspace(tmp.path());
    memstead_git_branch::test_support::init_real_mem_repo(tmp.path(), &[]);
    std::fs::write(
        tmp.path()
            .join(".memstead")
            .join("state")
            .join("mounts.json"),
        "this is not json {",
    )
    .unwrap();

    // The child resolves its workspace root through cwd, which the OS
    // canonicalizes (`/var` → `/private/var` on macOS) — compute the
    // expected line against the same base path.
    let ws = tmp.path().canonicalize().unwrap();
    let boot_err = memstead_git_branch::workspace_store::engine_from_workspace_root(&ws)
        .err()
        .expect("fixture must fail the in-process boot");
    assert_eq!(boot_err.code(), "WORKSPACE_STORE_PARSE");
    let expected = format!(
        "memstead-mcp: ERROR [{}]: {}",
        boot_err.code(),
        boot_err.surface_message(&ws)
    );

    let output = Command::new(memstead_mcp_bin())
        .current_dir(tmp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn memstead-mcp (full) — confirm the binary built before running tests");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&expected),
        "stderr must carry the typed boot diagnostic\nexpected line: {expected}\n--- stderr ---\n{stderr}"
    );
}

fn tools_call_request(id: i64, name: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": {} }
    }))
    .unwrap()
}

/// Criterion 4, partial half (agent-trust plan 04): a workspace with a
/// broken-pin mem STARTS the server (the mem quarantines inside the
/// engine) instead of dying into `-32000 Connection closed`.
#[test]
fn full_binary_serves_partially_broken_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_workspace(tmp.path());
    memstead_git_branch::test_support::init_real_mem_repo(tmp.path(), &[]);
    std::fs::create_dir_all(tmp.path().join("plenum")).unwrap();
    std::fs::write(
        tmp.path().join(".memstead").join("state").join("mounts.json"),
        r#"{ "format": "memstead-mounts-3", "mounts": [
            { "mem": "plenum", "schema": "ghost@1.0.0", "storage": { "type": "folder", "path": "plenum" }, "capability": "write", "lifecycle": "eager", "cross_linkable": true }
        ] }"#,
    )
    .unwrap();

    let mut child = Command::new(memstead_mcp_bin())
        .current_dir(tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn memstead-mcp (full)");
    let mut stdin = child.stdin.take().expect("child stdin");
    writeln!(stdin, "{}", initialize_request()).unwrap();
    stdin.flush().unwrap();
    drop(stdin);
    let stdout = child.stdout.take().expect("child stdout");
    let response = read_response_with_timeout(stdout, 1, Duration::from_secs(15))
        .expect("server must start and answer initialize despite the broken mem");
    assert_initialize_envelope(&response);
    let _ = child.kill();
    let _ = child.wait();
}

/// Criterion 4, wholly-unbootable half: a corrupt workspace store no
/// longer kills the server — it starts as a diagnostic shell and
/// `memstead_health` answers with the typed boot diagnosis, so a
/// session can always ask why the graph is gone.
#[test]
fn full_binary_serves_boot_diagnosis_on_unbootable_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_workspace(tmp.path());
    memstead_git_branch::test_support::init_real_mem_repo(tmp.path(), &[]);
    std::fs::write(
        tmp.path()
            .join(".memstead")
            .join("state")
            .join("mounts.json"),
        "this is not json {",
    )
    .unwrap();

    let mut child = Command::new(memstead_mcp_bin())
        .current_dir(tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn memstead-mcp (full)");
    let mut stdin = child.stdin.take().expect("child stdin");
    writeln!(stdin, "{}", initialize_request()).unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        }))
        .unwrap()
    )
    .unwrap();
    writeln!(stdin, "{}", tools_call_request(2, "memstead_health")).unwrap();
    stdin.flush().unwrap();
    drop(stdin);

    let stdout = child.stdout.take().expect("child stdout");
    let response = read_response_with_timeout(stdout, 2, Duration::from_secs(15))
        .expect("diagnostic shell must answer memstead_health");
    let text = serde_json::to_string(&response).unwrap();
    assert!(
        text.contains("boot_diagnosis") && text.contains("WORKSPACE_STORE_PARSE"),
        "health must carry the typed boot diagnosis: {text}"
    );
    let _ = child.kill();
    let _ = child.wait();
}
