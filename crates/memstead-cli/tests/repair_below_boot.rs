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

/// A mem-repo workspace with one folder-mount mem `plenum` pinned
/// (via the mount record — the mem is config-absent, so the mount pin
/// is settled) to a schema no source holds. Under the plan-04
/// quarantine posture the workspace BOOTS with `plenum` quarantined
/// (`SCHEMA_NOT_FOUND` reason) — the fixture asserts that state.
fn quarantined_workspace(tmp: &TempDir) -> std::path::PathBuf {
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
    // Confirm the fixture really quarantines `plenum` for the reason
    // this plan repairs (the workspace itself boots — plan 04).
    let health = health_json(&ws);
    assert_eq!(
        health["quarantined"][0]["reason_code"], "SCHEMA_NOT_FOUND",
        "fixture must quarantine on the bad pin: {health}"
    );
    ws
}

fn health_json(ws: &std::path::Path) -> serde_json::Value {
    let out = memstead()
        .current_dir(ws)
        .args(["--json", "health"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    parse_envelope(&out)
}

fn example_package_dir() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../memstead-schema/examples/minimal")
        .display()
        .to_string()
}

/// Criterion 1 (plan 03), re-shaped by plan 04's quarantine posture:
/// on a workspace whose broken-pin mem is quarantined, `mem
/// set-schema` to a resolvable ref succeeds — the booted engine's
/// quarantine-repair branch repins the retained mount and re-attaches
/// it in-process — and the workspace is fully green after.
#[test]
fn set_schema_repairs_quarantined_mem() {
    let tmp = TempDir::new().unwrap();
    let ws = quarantined_workspace(&tmp);

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "mem", "set-schema", "plenum", "default@1.0.0"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env = parse_envelope(&out);
    assert_eq!(env["schema_pin"], "default@1.0.0", "got: {env}");

    // The repair is durable: a fresh process boots with an empty
    // quarantine roster and serves the mem.
    let health = health_json(&ws);
    assert!(
        health["quarantined"].is_null(),
        "roster must be empty after repair: {health}"
    );
    memstead()
        .current_dir(&ws)
        .args(["--json", "status"])
        .assert()
        .success();
}

/// Criterion 2 + the criterion-4 fork-catcher: on the
/// quarantined-mem workspace, `schema install <package>` seals the
/// package onto the `__MEMSTEAD:schemas/` ref (never booting), and a
/// subsequent `set-schema` to the installed ref returns the mem to
/// full service — the plenum recovery path. If the resolver consulted
/// a narrower catalogue than boot (e.g. built-ins only), the
/// set-schema step would refuse the freshly ref-installed schema and
/// this test would fail.
#[test]
fn install_then_set_schema_recovers_end_to_end() {
    let tmp = TempDir::new().unwrap();
    let ws = quarantined_workspace(&tmp);

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

    let health = health_json(&ws);
    assert!(
        health["quarantined"].is_null(),
        "mem returns to service on the installed ref: {health}"
    );
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
    // Unresolvable target ref against the quarantined mem: same
    // SCHEMA_NOT_FOUND refusal, and the pin is never force-written.
    let tmp = TempDir::new().unwrap();
    let ws = quarantined_workspace(&tmp);
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
    // The pin was NOT force-written: the mem stays quarantined on the
    // original broken pin.
    let health = health_json(&ws);
    assert!(
        health["quarantined"][0]["reason_message"]
            .as_str()
            .unwrap()
            .contains("ghost@1.0.0"),
        "original broken pin must be untouched: {health}"
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
/// a workspace that (still) does not boot — under plan 04 that means
/// a workspace-LEVEL failure (here: a corrupt store; mem-level
/// failures now quarantine instead of blocking boot) — completes
/// without deadlocking: cursors are explicitly deferred with a typed
/// notice naming the follow-up, and the file survives (silent loss is
/// the refused shape). Complement: on a bootable workspace the
/// cursors are consumed and the file deleted, as today.
#[test]
fn projection_migrate_defers_cursor_seeding_when_boot_fails() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    let mounts = ws.join(".memstead").join("state").join("mounts.json");
    std::fs::create_dir_all(mounts.parent().unwrap()).unwrap();
    std::fs::write(&mounts, "this is not json {").unwrap();
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

/// Criteria 1 and 4: a workspace whose only mem is quarantined no longer
/// gets the built-in default's catalogue presented as its own. The command
/// names the quarantine and carries the engine's own reason and repair
/// rather than restating them in different words.
#[test]
fn type_names_the_quarantine_instead_of_printing_a_default_over_it() {
    let tmp = TempDir::new().unwrap();
    let ws = quarantined_workspace(&tmp);

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "type"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body: serde_json::Value = serde_json::from_slice(&out).expect("--json is JSON");

    assert_eq!(
        body["fallback"]["code"], "ALL_MEMS_QUARANTINED",
        "the condition must be machine-readable; got: {body}"
    );
    let detail = body["fallback"]["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("plenum") && detail.contains("SCHEMA_NOT_FOUND"),
        "the quarantined mem and the engine's typed reason must be named; got: {detail}"
    );
    assert!(
        detail.contains("memstead schema install"),
        "the engine's own repair command must be surfaced, not restated; got: {detail}"
    );
    let md = body["markdown"].as_str().unwrap_or_default();
    assert!(
        md.starts_with("**No mem is serving in this workspace**"),
        "the reader must meet the condition before the catalogue; got: {md}"
    );
}

/// Criterion 3, the arm the first attempt missed: the REFUSAL path needs
/// the condition more than the success paths do. Without it a user whose
/// own schema declares the type is told it does not exist, attributed to a
/// schema that is not theirs, with the quarantine unmentioned on both
/// channels.
#[test]
fn type_refusal_over_a_quarantine_states_the_condition_too() {
    let tmp = TempDir::new().unwrap();
    let ws = quarantined_workspace(&tmp);

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "type", "no-such-type"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let body: serde_json::Value = serde_json::from_slice(&out).expect("--json refusal is JSON");
    assert_eq!(body["code"], "UNKNOWN_ENTITY_TYPE");
    assert_eq!(
        body["details"]["fallback"]["code"], "ALL_MEMS_QUARANTINED",
        "the refusal must carry the condition machine-readably; got: {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("plenum") && message.contains("not this workspace's own"),
        "the human message must name the quarantine too; got: {message}"
    );

    // The healthy refusal is unchanged: a real schema, no condition.
    let healthy = tmp.path().join("healthy");
    memstead()
        .args([
            "init",
            healthy.to_str().unwrap(),
            "--name",
            "healthy-mem",
            "--schema",
            "default@1.0.0",
        ])
        .assert()
        .success();
    let out = memstead()
        .current_dir(&healthy)
        .args(["--json", "type", "no-such-type"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let body: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(
        body["details"]["fallback"].is_null(),
        "a healthy refusal carries no condition; got: {body}"
    );
}

/// Criterion 2's refusal complement: the genuine cold-start probe, run
/// where there is no workspace at all, still answers from the built-in
/// default with no warning. Here the default IS the answer, not a
/// stand-in, and a warning on the healthy path teaches readers to ignore
/// warnings.
#[test]
fn type_outside_a_workspace_stays_a_silent_cold_start_probe() {
    let tmp = TempDir::new().unwrap();
    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "type"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(
        body["fallback"].is_null(),
        "the cold-start probe must carry no fallback notice; got: {body}"
    );
    assert!(
        !body["markdown"]
            .as_str()
            .unwrap_or_default()
            .contains("built-in default, not"),
        "no condition line belongs on the healthy cold-start path"
    );
}

/// Criterion 5's refusal complement: a workspace with a healthy writable
/// mem is untouched — its own schema, and no notice.
#[test]
fn type_on_a_healthy_workspace_reports_its_own_schema_silently() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("healthy");
    memstead()
        .args([
            "init",
            ws.to_str().unwrap(),
            "--name",
            "healthy-mem",
            "--schema",
            "default@1.0.0",
        ])
        .assert()
        .success();

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "type"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(
        body["fallback"].is_null(),
        "a healthy workspace carries no fallback notice; got: {body}"
    );
    assert_eq!(body["schema"], "default@1.0.0");
}

/// The `--mem` arm of the same command, one path over (the backlog
/// residual plan 06's closing grade named): a user who explicitly names
/// the mem they know about must be told it is quarantined — with the
/// engine's reason and repair — not `UNKNOWN_MEM` in the exact phrasing
/// an empty workspace produces.
#[test]
fn type_mem_names_the_quarantine_instead_of_unknown_mem() {
    let tmp = TempDir::new().unwrap();
    let ws = quarantined_workspace(&tmp);

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "type", "--mem", "plenum"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let body = parse_envelope(&out);
    assert_eq!(
        body["code"], "MEM_QUARANTINED",
        "naming a quarantined mem is not an unknown-mem condition; got: {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("SCHEMA_NOT_FOUND") && message.contains("memstead schema install"),
        "the refusal must carry the engine's reason and repair; got: {message}"
    );
}

/// `status` is the roster surface the filesystem-workspace `mem list`
/// refusal points at, so it must carry the quarantine roster in both
/// output forms — the same never-behind-an-opt-in rule `health` follows.
/// Without it a quarantined mem simply vanished: the graph counts read
/// as a small healthy workspace.
#[test]
fn status_carries_the_quarantine_roster_in_both_forms() {
    let tmp = TempDir::new().unwrap();
    let ws = quarantined_workspace(&tmp);

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body = parse_envelope(&out);
    let roster = body["quarantined"].as_array().expect("quarantined roster");
    assert_eq!(roster.len(), 1, "got: {body}");
    assert_eq!(roster[0]["mem"], "plenum");
    assert_eq!(roster[0]["reason_code"], "SCHEMA_NOT_FOUND");

    let md_bytes = memstead()
        .current_dir(&ws)
        .arg("status")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let md = String::from_utf8(md_bytes).expect("stdout is UTF-8");
    assert!(
        md.contains("Quarantined mems (1)")
            && md.contains("plenum")
            && md.contains("SCHEMA_NOT_FOUND"),
        "markdown status must render the quarantine roster; got:\n{md}"
    );
}
