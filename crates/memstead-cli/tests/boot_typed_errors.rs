#![cfg(feature = "mem-repo")]
//! Boot-path complement of `no_internal_leaks`: every boot failure
//! class surfaces a typed code — never `INTERNAL` — and a message whose
//! final clause names the repair command (or states plainly that no
//! mechanical remedy exists).
//!
//! Each test builds a healthy workspace, breaks it in one specific
//! way, and asserts the `--json` envelope of a booting command
//! (`memstead status`) carries the class's typed code. Where the boot
//! error is reproducible in-process, the envelope message is compared
//! against `BootError::surface_message` verbatim — the same string the
//! MCP server prints on its boot-failure diagnostics (parity test in
//! `memstead-mcp/tests/boot.rs`), so the two surfaces are pinned to one
//! renderer.

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn parse_envelope(stdout_bytes: &[u8]) -> serde_json::Value {
    let body = std::str::from_utf8(stdout_bytes).expect("stdout must be UTF-8");
    serde_json::from_str(body.trim()).unwrap_or_else(|e| {
        panic!("--json error must parse as JSON: {e}\n--- stdout ---\n{body}\n--- end stdout ---")
    })
}

/// Init a filesystem-mem workspace pinned to the built-in
/// `default@1.0.0`, then rewrite the pin in `.memstead/config.json`.
fn filesystem_workspace_with_pin(tmp: &TempDir, broken_pin: &str) -> std::path::PathBuf {
    let ws = tmp.path().join("ws");
    memstead()
        .args([
            "init",
            "--name",
            "mem1",
            "--schema",
            "default@1.0.0",
            ws.to_str().unwrap(),
        ])
        .assert()
        .success();
    let config = ws.join(".memstead").join("config.json");
    let body = std::fs::read_to_string(&config).unwrap();
    assert!(body.contains("default@1.0.0"), "fixture: pin must be present");
    std::fs::write(&config, body.replace("default@1.0.0", broken_pin)).unwrap();
    ws
}

fn status_error_envelope(ws: &std::path::Path) -> serde_json::Value {
    let output = memstead()
        .current_dir(ws)
        .args(["--json", "status"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    parse_envelope(&output)
}

/// Criterion 1, both trail shapes: an unresolvable schema pin refuses
/// with `SCHEMA_NOT_FOUND`, and the message's final clause names the
/// repair command the source trail calls for — `mem set-schema` with
/// the concrete mem and version when the name exists at other
/// versions, the `schema install` path when no source knows the name.
/// The envelope message equals `BootError::surface_message` for the
/// same workspace — the string the MCP boot diagnostics print.
#[test]
fn unresolvable_pin_refuses_typed_with_repair_command_for_both_trails() {
    // Trail shape 1: right name, wrong version (the plenum outage's
    // disappeared-built-in class).
    let tmp = TempDir::new().unwrap();
    let ws = filesystem_workspace_with_pin(&tmp, "default@99.0.0");
    let env = status_error_envelope(&ws);
    assert_eq!(env["code"], "SCHEMA_NOT_FOUND", "got: {env}");
    let msg = env["message"].as_str().unwrap();
    assert!(
        msg.contains("memstead mem set-schema mem1 default@1.0.0"),
        "wrong-version trail must end in the concrete repin command: {msg}"
    );
    // Canonicalize: the CLI process resolves cwd through macOS's
    // `/var` → `/private/var` symlink; the renderer comparison needs
    // the same base path.
    let ws_canon = ws.canonicalize().unwrap();
    let boot_err = memstead_base::Engine::from_workspace_root(&ws_canon)
        .err()
        .expect("in-process boot must fail the same way");
    assert_eq!(boot_err.code(), "SCHEMA_NOT_FOUND");
    assert_eq!(
        msg,
        boot_err.surface_message(&ws_canon),
        "CLI envelope message must be the shared boot-failure renderer verbatim"
    );

    // Trail shape 2: name unknown everywhere.
    let tmp2 = TempDir::new().unwrap();
    let ws2 = filesystem_workspace_with_pin(&tmp2, "ghost@1.0.0");
    let env2 = status_error_envelope(&ws2);
    assert_eq!(env2["code"], "SCHEMA_NOT_FOUND", "got: {env2}");
    let msg2 = env2["message"].as_str().unwrap();
    assert!(
        msg2.contains("memstead schema install"),
        "unknown-name trail must name the install path: {msg2}"
    );
    assert!(
        !msg2.contains("mem set-schema"),
        "unknown-name trail must not suggest repinning: {msg2}"
    );
}

/// Criterion 2: a legacy pre-v2 projection config refuses boot with a
/// typed code naming `memstead projection migrate` — the previously
/// `INTERNAL`-leaking backlog item.
#[test]
fn legacy_projection_config_refuses_typed_naming_migrate() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    let proj_dir = ws.join(".memstead").join("projections").join("engine");
    std::fs::create_dir_all(&proj_dir).unwrap();
    // Version-less record — the gen-2 shape the v2 loader refuses.
    std::fs::write(proj_dir.join("graph.json"), "{}").unwrap();

    let env = status_error_envelope(&ws);
    assert_eq!(env["code"], "PROJECTION_STORE_LEGACY", "got: {env}");
    let msg = env["message"].as_str().unwrap();
    assert!(
        msg.contains("memstead projection migrate"),
        "message must name the migrate command: {msg}"
    );
}

/// Duplicate mem name in the mount list refuses typed.
#[test]
fn duplicate_mem_name_refuses_typed() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    for dir in ["a", "b"] {
        std::fs::create_dir_all(ws.join(dir)).unwrap();
    }
    let mounts = ws.join(".memstead").join("state").join("mounts.json");
    std::fs::create_dir_all(mounts.parent().unwrap()).unwrap();
    std::fs::write(
        &mounts,
        r#"{
  "format": "memstead-mounts-3",
  "mounts": [
    { "mem": "dup", "schema": "default@1.0.0", "storage": { "type": "folder", "path": "a" }, "capability": "write", "lifecycle": "eager", "cross_linkable": true },
    { "mem": "dup", "schema": "default@1.0.0", "storage": { "type": "folder", "path": "b" }, "capability": "write", "lifecycle": "eager", "cross_linkable": true }
  ]
}"#,
    )
    .unwrap();

    let env = status_error_envelope(&ws);
    assert_eq!(env["code"], "DUPLICATE_MEM", "got: {env}");
}

/// A mount with no schema pin anywhere (no mount pin, no backend
/// config) refuses typed — whatever the class resolves to, it is not
/// `INTERNAL`.
#[test]
fn missing_pin_refuses_typed() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    std::fs::create_dir_all(ws.join("bare")).unwrap();
    let mounts = ws.join(".memstead").join("state").join("mounts.json");
    std::fs::create_dir_all(mounts.parent().unwrap()).unwrap();
    std::fs::write(
        &mounts,
        r#"{
  "format": "memstead-mounts-3",
  "mounts": [
    { "mem": "bare", "storage": { "type": "folder", "path": "bare" }, "capability": "write", "lifecycle": "eager", "cross_linkable": true }
  ]
}"#,
    )
    .unwrap();

    let env = status_error_envelope(&ws);
    let code = env["code"].as_str().unwrap_or_default();
    assert_ne!(code, "INTERNAL", "missing pin must not leak INTERNAL: {env}");
    assert!(!code.is_empty(), "missing pin must carry a code: {env}");
}

/// An unparseable workspace store refuses typed and — having no
/// mechanical remedy — says so plainly instead of inventing a command.
#[test]
fn unparseable_workspace_store_refuses_typed_and_states_no_remedy() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    let mounts = ws.join(".memstead").join("state").join("mounts.json");
    std::fs::create_dir_all(mounts.parent().unwrap()).unwrap();
    std::fs::write(&mounts, "this is not json {").unwrap();

    let env = status_error_envelope(&ws);
    assert_eq!(env["code"], "WORKSPACE_STORE_PARSE", "got: {env}");
    let msg = env["message"].as_str().unwrap();
    assert!(
        msg.contains("no memstead command repairs this"),
        "no-remedy class must state that plainly: {msg}"
    );
}

/// Complement (criterion 5): a healthy workspace still boots — the
/// typed-boot-error seam changes failure output only.
#[test]
fn healthy_workspace_boots_successfully() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    memstead()
        .current_dir(&ws)
        .args(["--json", "status"])
        .assert()
        .success();
}
