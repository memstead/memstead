#![cfg(feature = "mem-repo")]
// `memstead workspace dump` ships only in the full build.

//! Integration tests for `memstead workspace dump`.
//!
//! Each test seeds a mem-repo-git workspace under a temp dir, runs
//! `memstead workspace dump` against it, and asserts on the returned JSON
//! shape. The shape is the consumer-facing contract — every assertion
//! here corresponds to a documented field.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// Lay down a minimal mem directory with a `.memstead/config.json`. The
/// disk shape feeds `init_real_mem_repo_from_disk`, which reshapes it
/// into `__MEMSTEAD:mems/<name>/config.json` blobs and per-mem content
/// branches.
fn write_mem_dir(root: &Path, name: &str, schema: &str, description: Option<&str>) {
    let dir = root.join(name);
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    let config = match description {
        Some(d) => format!(
            r#"{{"schema": "{schema}", "description": "{d}", "writeGuidance": {{"goal_additions": "test goal"}}}}"#
        ),
        None => format!(r#"{{"schema": "{schema}"}}"#),
    };
    fs::write(store.join("config.json"), config).unwrap();

    fs::write(
        dir.join("alpha.md"),
        r#"---
type: spec
created_date: 2026-01-01
last_modified: 2026-01-01
level: M0
---
# Alpha

## Identity

The alpha entity.

## Purpose

Smoke-test entity for the dump tests.
"#,
    )
    .unwrap();
}

/// Seed a mem-repo with the named single mem and return the
/// workspace root.
fn seed_workspace_with(name: &str, schema: &str, description: Option<&str>) -> TempDir {
    let tmp = TempDir::new().unwrap();
    write_mem_dir(tmp.path(), name, schema, description);
    let dir = tmp.path().join(name);
    init_real_mem_repo_from_disk(tmp.path(), &[(&dir, name)]);
    tmp
}

#[test]
fn dump_format_is_v0() {
    let ws = seed_workspace_with("engine", "default@1.0.0", None);

    let output = memstead()
        .current_dir(ws.path())
        .args(["workspace", "dump"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(parsed["format"], "workspace-dump/v0");
}

#[test]
fn dump_lists_mem_with_schema_pin() {
    let ws = seed_workspace_with("engine", "default@1.0.0", Some("rust engine"));

    let output = memstead()
        .current_dir(ws.path())
        .args(["workspace", "dump"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");

    let mems = parsed["mems"].as_array().expect("mems is array");
    assert_eq!(mems.len(), 1);
    let v = &mems[0];
    assert_eq!(v["name"], "engine");
    assert_eq!(v["schema_ref"], "default@1.0.0");
    assert!(
        v.get("schema").is_none(),
        "per-mem pin is `schema_ref`, never `schema` (which is reserved for the inlined schema body)"
    );
    assert_eq!(v["description"], "rust engine");
    assert!(
        v["snapshot_token"].as_str().unwrap().len() == 40,
        "snapshot_token should be a 40-char git oid hex string, got {:?}",
        v["snapshot_token"]
    );
    // write_guidance is opaque pass-through; verify shape only.
    // The dump's wire key is snake_case `write_guidance`, consistent with
    // every other key in the envelope.
    assert!(
        v["write_guidance"].is_object(),
        "write_guidance is an object even when empty"
    );
    assert_eq!(
        v["write_guidance"]["goal_additions"], "test goal",
        "write_guidance from .memstead/config.json round-trips through the dump"
    );
    assert!(
        v.get("writeGuidance").is_none(),
        "camelCase `writeGuidance` key must not appear; only `write_guidance` is the documented dump shape"
    );
}

#[test]
fn dump_includes_schema_default_writing_guidance() {
    let ws = seed_workspace_with("engine", "default@1.0.0", None);

    let output = memstead()
        .current_dir(ws.path())
        .args(["workspace", "dump"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");

    // The schemas object is keyed by the same string the mem references.
    let schemas = parsed["schemas"].as_object().expect("schemas is object");
    assert!(
        schemas.contains_key("default@1.0.0"),
        "schemas key matches the mem's schema pin: {:?}",
        schemas.keys().collect::<Vec<_>>()
    );
    // `default_writing_guidance` is always present (possibly empty).
    let body = &schemas["default@1.0.0"];
    assert!(
        body["default_writing_guidance"].is_object(),
        "default_writing_guidance is always an object"
    );
}

#[test]
fn dump_snapshot_token_is_stable_across_runs() {
    let ws = seed_workspace_with("engine", "default@1.0.0", None);

    let run = || {
        let out = memstead()
            .current_dir(ws.path())
            .args(["workspace", "dump"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let parsed: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
        parsed["mems"][0]["snapshot_token"].as_str().unwrap().to_string()
    };

    let first = run();
    let second = run();
    assert_eq!(
        first, second,
        "snapshot_token must be stable across no-op runs"
    );
}

#[test]
fn dump_orders_mems_alphabetically() {
    // Three mems seeded out-of-order to verify the dump sorts them.
    let tmp = TempDir::new().unwrap();
    for name in ["zeta", "alpha", "mu"] {
        write_mem_dir(tmp.path(), name, "default@1.0.0", None);
    }
    let zeta = tmp.path().join("zeta");
    let alpha = tmp.path().join("alpha");
    let mu = tmp.path().join("mu");
    init_real_mem_repo_from_disk(
        tmp.path(),
        &[(&zeta, "zeta"), (&alpha, "alpha"), (&mu, "mu")],
    );

    let output = memstead()
        .current_dir(tmp.path())
        .args(["workspace", "dump"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    let names: Vec<&str> = parsed["mems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["alpha", "mu", "zeta"]);
}

#[test]
fn dump_accepts_local_json_flag() {
    // `memstead workspace dump --json` parses cleanly. The flag is a
    // forward-compat no-op (the dump is always JSON), but the literal
    // command must work.
    let ws = seed_workspace_with("engine", "default@1.0.0", None);

    let output = memstead()
        .current_dir(ws.path())
        .args(["workspace", "dump", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(parsed["format"], "workspace-dump/v0");
}

#[test]
fn dump_fails_outside_workspace_with_clear_error() {
    let tmp = TempDir::new().unwrap();
    // No mem-repo, no .memstead/workspace.toml — engine init will fail.
    let assert = memstead()
        .current_dir(tmp.path())
        .args(["--json", "workspace", "dump"])
        .assert()
        .failure();
    // Under `--json` the error envelope rides stdout.
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let envelope: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("JSON envelope on stdout");
    // Wire shape: `{code, message, details}` matching MCP. The
    // workspace-not-initialised path emits the typed
    // `WORKSPACE_NOT_INITIALISED` token; the process exit kind rides on
    // status, not in the JSON body.
    assert_eq!(envelope["code"], "WORKSPACE_NOT_INITIALISED");
    assert!(
        envelope["message"]
            .as_str()
            .unwrap()
            .contains("workspace dump"),
        "error envelope mentions the failing command: {envelope}"
    );
}

#[test]
fn dump_workspace_root_is_absolute() {
    let ws = seed_workspace_with("engine", "default@1.0.0", None);

    let output = memstead()
        .current_dir(ws.path())
        .args(["workspace", "dump"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    let root = parsed["workspace_root"].as_str().expect("workspace_root is string");
    assert!(
        Path::new(root).is_absolute(),
        "workspace_root is absolute: {root}"
    );
}

// --- sync-state round-trip (ingest source-cursor baseline) ---

/// Helper: run `memstead workspace dump` and return the first mem's
/// JSON object.
fn dump_first_mem(ws: &Path) -> serde_json::Value {
    let output = memstead()
        .current_dir(ws)
        .args(["workspace", "dump"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    parsed["mems"][0].clone()
}

#[test]
fn dump_omits_sync_state_when_unset() {
    // A mem that never had a sync-state token must not gain an empty
    // `sync_state` map on the wire — mirrors `write_guidance`'s
    // skip-if-empty contract so existing minimal dumps are unchanged.
    let ws = seed_workspace_with("engine", "default@1.0.0", None);
    let v = dump_first_mem(ws.path());
    assert!(
        v.get("sync_state").is_none(),
        "sync_state must be omitted from the wire when empty: {v}"
    );
}

#[test]
fn set_sync_state_surfaces_on_dump() {
    // The full write→persist→read pipe: `mem set-sync-state` commits
    // the opaque token into the per-mem config, and `workspace dump`
    // surfaces it verbatim for the ingest loop to diff against.
    let ws = seed_workspace_with("engine", "default@1.0.0", None);

    memstead()
        .current_dir(ws.path())
        .args([
            "mem",
            "set-sync-state",
            "engine",
            "engine-graph/source-files",
            "cafef00d",
        ])
        .assert()
        .success();

    let v = dump_first_mem(ws.path());
    assert_eq!(
        v["sync_state"]["engine-graph/source-files"], "cafef00d",
        "sync_state token round-trips through the dump: {v}"
    );
}

#[test]
fn set_sync_state_empty_token_clears_key() {
    // An empty token is the clear surface — the next ingest pass
    // re-seeds at the current source state. After clearing, the key is
    // gone and (being the only key) the whole map drops off the wire.
    let ws = seed_workspace_with("engine", "default@1.0.0", None);

    let set = |token: &str| {
        memstead()
            .current_dir(ws.path())
            .args([
                "mem",
                "set-sync-state",
                "engine",
                "engine-graph/source-files",
                token,
            ])
            .assert()
            .success();
    };

    set("cafef00d");
    set(""); // clear

    let v = dump_first_mem(ws.path());
    assert!(
        v.get("sync_state").is_none(),
        "cleared sync_state leaves no key, so the map drops off the wire: {v}"
    );
}

#[test]
fn set_sync_state_is_durable_across_cache_independent_reads() {
    // Durability proxy: the token lives in engine-held mem config
    // (a committed `__MEMSTEAD` blob), not skill cache. A fresh
    // `workspace dump` process — no shared in-memory state with the
    // writer — still sees it. This is the "survives a .memstead.cache
    // wipe" guarantee at the storage layer.
    let ws = seed_workspace_with("engine", "default@1.0.0", None);

    memstead()
        .current_dir(ws.path())
        .args([
            "mem",
            "set-sync-state",
            "engine",
            "engine-graph/source-files",
            "deadbeef",
        ])
        .assert()
        .success();

    // Two independent dump invocations, each its own process.
    let first = dump_first_mem(ws.path());
    let second = dump_first_mem(ws.path());
    assert_eq!(first["sync_state"], second["sync_state"]);
    assert_eq!(first["sync_state"]["engine-graph/source-files"], "deadbeef");
}
