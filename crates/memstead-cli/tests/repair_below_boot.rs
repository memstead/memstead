#![cfg(feature = "mem-repo")]
//! Repair-below-boot: the verbs a boot-failure message names must run
//! on exactly the workspace whose boot they repair (agent-trust plan
//! 03). Fixtures follow the plenum outage shape — a mem-repo workspace
//! whose boot fails on an unresolvable schema pin — and drive
//! failure → repair → green end to end.

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn parse_envelope(stdout_bytes: &[u8]) -> serde_json::Value {
    let body = std::str::from_utf8(stdout_bytes).expect("stdout must be UTF-8");
    serde_json::from_str(body.trim()).unwrap_or_else(|e| {
        panic!("--json output must parse as JSON: {e}\n--- stdout ---\n{body}\n--- end ---")
    })
}

/// A mem-repo workspace whose boot fails with `SCHEMA_NOT_FOUND`: one
/// folder-mount mem `plenum` pinned (via the mount record — the mem is
/// config-absent, so the mount pin is settled) to a schema no source
/// holds.
fn unbootable_workspace(tmp: &TempDir) -> std::path::PathBuf {
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    std::fs::create_dir_all(ws.join("plenum")).unwrap();
    let mounts = ws.join(".memstead").join("state").join("mounts.json");
    std::fs::create_dir_all(mounts.parent().unwrap()).unwrap();
    std::fs::write(
        &mounts,
        r#"{
  "format": "memstead-mounts-3",
  "mounts": [
    { "mem": "plenum", "schema": "ghost@1.0.0", "storage": { "type": "folder", "path": "plenum" }, "capability": "write", "lifecycle": "eager", "cross_linkable": true }
  ]
}"#,
    )
    .unwrap();
    // Confirm the fixture is really unbootable, and for the reason
    // this plan repairs.
    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "status"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    assert_eq!(parse_envelope(&out)["code"], "SCHEMA_NOT_FOUND");
    ws
}

fn example_package_dir() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../memstead-schema/examples/minimal")
        .display()
        .to_string()
}

/// Criterion 1: on the unbootable workspace, `mem set-schema` to a
/// resolvable ref succeeds below boot, and the next boot is green.
#[test]
fn set_schema_repairs_unbootable_workspace() {
    let tmp = TempDir::new().unwrap();
    let ws = unbootable_workspace(&tmp);

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "mem", "set-schema", "plenum", "default@1.0.0"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&out);
    assert_eq!(env["below_boot"], true, "got: {env}");
    assert_eq!(env["schema_pin"], "default@1.0.0");
    assert_eq!(
        env["conformance_checked"], false,
        "below-boot repair states plainly that the conformance gate did not run: {env}"
    );

    memstead()
        .current_dir(&ws)
        .args(["--json", "status"])
        .assert()
        .success();
}

/// Criterion 2 + the criterion-4 fork-catcher: on the unbootable
/// workspace, `schema install <package>` seals the package onto the
/// `__MEMSTEAD:schemas/` ref below boot, and a subsequent `set-schema`
/// to the installed ref boots green — the full plenum recovery path.
/// If the below-boot resolver consulted a different catalogue than the
/// booted path (e.g. built-ins only), the set-schema step would refuse
/// the freshly ref-installed schema and this test would fail.
#[test]
fn install_then_set_schema_recovers_end_to_end() {
    let tmp = TempDir::new().unwrap();
    let ws = unbootable_workspace(&tmp);

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "schema", "install", &example_package_dir()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&out);
    assert_eq!(env["ok"], true, "got: {env}");
    assert_eq!(env["schema"], "recipe@0.1.0");

    memstead()
        .current_dir(&ws)
        .args(["--json", "mem", "set-schema", "plenum", "recipe@0.1.0"])
        .assert()
        .success();

    memstead()
        .current_dir(&ws)
        .args(["--json", "status"])
        .assert()
        .success();
}

/// Criterion 3: typed refusals below boot — a corrupt store, an
/// invalid package, and an unresolvable target ref; repair never
/// force-writes a pin that resolves nowhere.
#[test]
fn repair_refuses_typed_on_corrupt_store_bad_package_and_bad_ref() {
    // Unresolvable target ref on the unbootable workspace: same
    // SCHEMA_NOT_FOUND the booted path produces (shared resolver).
    let tmp = TempDir::new().unwrap();
    let ws = unbootable_workspace(&tmp);
    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "mem", "set-schema", "plenum", "ghost2@1.0.0"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&out);
    assert_eq!(env["code"], "SCHEMA_NOT_FOUND", "got: {env}");
    // The pin was NOT force-written: boot still fails on the original pin.
    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "status"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    assert!(
        parse_envelope(&out)["message"]
            .as_str()
            .unwrap()
            .contains("ghost@1.0.0"),
        "original broken pin must be untouched"
    );

    // Invalid package refuses typed.
    let bad_pkg = tmp.path().join("badpkg");
    std::fs::create_dir_all(&bad_pkg).unwrap();
    std::fs::write(bad_pkg.join("schema.yaml"), "not: [valid").unwrap();
    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "schema", "install", bad_pkg.to_str().unwrap()])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&out);
    assert_eq!(env["code"], "SCHEMA_VALIDATION_FAILED", "got: {env}");

    // Genuinely corrupt store: both verbs refuse typed, never INTERNAL.
    let tmp2 = TempDir::new().unwrap();
    let ws2 = tmp2.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws2.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    let mounts = ws2.join(".memstead").join("state").join("mounts.json");
    std::fs::create_dir_all(mounts.parent().unwrap()).unwrap();
    std::fs::write(&mounts, "this is not json {").unwrap();
    let pkg = example_package_dir();
    for args in [
        vec!["--json", "mem", "set-schema", "plenum", "default@1.0.0"],
        vec!["--json", "schema", "install", pkg.as_str()],
    ] {
        let out = memstead()
            .current_dir(&ws2)
            .args(&args)
            .assert()
            .failure()
            .get_output()
            .stdout
            .clone();
        let env = parse_envelope(&out);
        assert_eq!(
            env["code"], "WORKSPACE_STORE_PARSE",
            "corrupt store must refuse typed for {args:?}: {env}"
        );
    }
}

/// Criterion 5: `projection migrate` with pending reconcile cursors on
/// a workspace that (still) does not boot completes without
/// deadlocking — cursors are explicitly deferred with a typed notice
/// naming the follow-up, and the file survives (silent loss is the
/// refused shape). Complement: on a bootable workspace the cursors are
/// consumed and the file deleted, as today.
#[test]
fn projection_migrate_defers_cursor_seeding_when_boot_fails() {
    let tmp = TempDir::new().unwrap();
    let ws = unbootable_workspace(&tmp);
    let cursor_path = ws.join(".memstead").join("reconcile-cursors.json");
    std::fs::write(&cursor_path, r#"{"plenum:/abs/somewhere": "abc123"}"#).unwrap();

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "projection", "migrate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&out);
    let notice = env["cursors_deferred"]
        .as_str()
        .expect("deferral notice must be present");
    assert!(
        notice.contains("RECONCILE_CURSORS_DEFERRED"),
        "typed notice: {notice}"
    );
    assert!(
        notice.contains("projection migrate"),
        "notice names the follow-up: {notice}"
    );
    assert!(
        cursor_path.exists(),
        "deferral keeps the cursor file — silent loss is the refused shape"
    );

    // Complement: bootable workspace consumes and deletes.
    let tmp2 = TempDir::new().unwrap();
    let ws2 = tmp2.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws2.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    let cursor_path2 = ws2.join(".memstead").join("reconcile-cursors.json");
    std::fs::write(&cursor_path2, r#"{"m:/abs/x": "abc123"}"#).unwrap();
    let out = memstead()
        .current_dir(&ws2)
        .args(["--json", "projection", "migrate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&out);
    assert!(env["cursors_deferred"].is_null(), "no deferral: {env}");
    assert!(
        !cursor_path2.exists(),
        "bootable workspace consumes and retires the cursor file as today"
    );
}
