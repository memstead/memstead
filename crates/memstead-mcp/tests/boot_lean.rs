#![cfg(not(feature = "mem-repo"))]
//! Boot smoke test for the lean MCP build (`memstead-mcp --no-default-features`).
//!
//! Spawns the binary as a subprocess against a tempdir workspace
//! carrying the post-rebuild markers (`.memstead/workspace.toml`
//! + `.memstead/state/mounts.json`). Sends one `initialize` JSON-RPC
//! request over stdin, reads the reply over stdout, asserts the
//! envelope is well-formed.
//!
//! The full equivalent (testing `memstead-mcp`) lives in
//! `boot.rs` (gated to the full build).

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
fn lean_binary_boots_against_new_layout_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_workspace(tmp.path());

    let mut child = Command::new(memstead_mcp_bin())
        .current_dir(tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn memstead-mcp — confirm the binary built before running tests");

    let mut stdin = child.stdin.take().expect("child stdin");
    writeln!(stdin, "{}", initialize_request()).expect("write initialize");
    stdin.flush().expect("flush initialize");
    drop(stdin);

    let stdout = child.stdout.take().expect("child stdout");
    let response = read_response_with_timeout(stdout, 1, Duration::from_secs(10))
        .expect("initialize response within 10s — binary did not boot or did not reply");

    assert_initialize_envelope(&response);

    let _ = child.kill();
    let _ = child.wait();
}
