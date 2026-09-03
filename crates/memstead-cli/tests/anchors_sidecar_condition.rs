//! An anchors sidecar the engine cannot read is a typed condition on
//! every read surface, never zero rows (`ANCHORS_SIDECAR_UNREADABLE`
//! with the mem and the parse reason); and the anchor-state vocabulary
//! is one name per state, `resolves`, across the sidecar, the entity
//! read, the verify surfaces and the health axis.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// A folder mem with one entity carrying one file anchor whose hash the
/// first verify backfills, so a second verify reads `resolves`.
fn seed() -> TempDir {
    let ws = TempDir::new().unwrap();
    memstead()
        .current_dir(ws.path())
        .args([
            "init",
            "--name",
            "notes",
            "--schema",
            "default@1.3.0",
            "--quiet",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args([
            "create",
            "--quiet",
            "--title",
            "Alpha",
            "--type",
            "concept",
            "--section",
            "definition=x",
            "--section",
            "explanation=y",
        ])
        .assert()
        .success();
    fs::write(ws.path().join("src.txt"), "hello\n").unwrap();
    memstead()
        .current_dir(ws.path())
        .args([
            "update",
            "notes--alpha",
            "--quiet",
            "--anchor",
            r#"{"artifact":"src.txt","grain":"file","class":"anchored","hash_stability":"stable"}"#,
        ])
        .assert()
        .success();
    // First pass backfills the observed hash; from here the row resolves.
    memstead()
        .current_dir(ws.path())
        .args(["verify-anchors", "--mem", "notes", "--quiet"])
        .assert()
        .success();
    ws
}

fn sidecar_path(ws: &Path) -> std::path::PathBuf {
    ws.join(".memstead").join("anchors.json")
}

fn json_of(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "not one JSON document: {e}\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// A2 AC1, refusal complement first: a valid version-2 sidecar carries no
/// condition anywhere and `verify-anchors` exits 0.
#[test]
fn valid_sidecar_carries_no_condition() {
    let ws = seed();
    let sidecar: serde_json::Value =
        serde_json::from_slice(&fs::read(sidecar_path(ws.path())).unwrap()).unwrap();
    assert_eq!(sidecar["version"], 2);

    let out = memstead()
        .current_dir(ws.path())
        .args(["verify-anchors", "--mem", "notes", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json_of(&out);
    assert_eq!(v["resolves"], 1, "{v}");
    assert_eq!(v["fully_adjudicated"], true, "{v}");
    assert!(v.get("condition").is_none());
    assert_eq!(v["anchors"][0]["state"], "resolves");

    let out = memstead()
        .current_dir(ws.path())
        .args(["anchors", "notes--alpha", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json_of(&out);
    assert_eq!(v["count"], 1);
    assert_eq!(v["anchors"][0]["state"], "resolves");
    assert_eq!(v["sidecar_unreadable"].as_array().map(Vec::len), Some(0));

    let out = memstead()
        .current_dir(ws.path())
        .args([
            "health",
            "--include",
            "anchors,integrity",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json_of(&out);
    assert!(v["anchors"]["notes"]["condition"].is_null(), "{v}");
    assert_eq!(v["anchors"]["notes"]["resolves"], 1, "{v}");
    assert_eq!(v["anchors"]["notes"]["fully_adjudicated"], true, "{v}");
    assert!(
        v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["code"] != "ANCHORS_SIDECAR_UNREADABLE"),
        "{v}"
    );
    // Rendering the axis does NOT examine it (C10, 2026-09-03). `anchors`
    // used to be promoted into `examined` for any pass that included it,
    // while `--strict` never folds anchor drift into its exit and its help
    // says so. The line now renders the registry declaration: `anchors` is
    // advisory on every pass, and the verify surfaces carry the drift
    // statement. This assertion was written against the promoted line and is
    // inverted with the behaviour it pins.
    let cov = v["verdict_coverage"].as_str().unwrap();
    let mut parts = cov.split(';');
    let examined = parts.next().unwrap();
    let advisory = parts.next().unwrap();
    assert!(
        !examined.contains("anchors"),
        "a rendered anchors axis is not an examined one: {cov}"
    );
    assert!(
        advisory.contains("anchors"),
        "and it is named advisory rather than dropped: {cov}"
    );

    let out = memstead()
        .current_dir(ws.path())
        .args(["entity", "notes--alpha", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(json_of(&out).get("anchors_sidecar_error").is_none());

    // The sidecar's own row spells the same name.
    let text = fs::read_to_string(sidecar_path(ws.path())).unwrap();
    assert!(!text.contains("\"resolved\""), "{text}");
}

/// A2 AC1, assertion: with the sidecar declaring version 3 every surface
/// carries `ANCHORS_SIDECAR_UNREADABLE` with the mem and the reason, the
/// integrity axis lists it as a finding, `verify-anchors` reports
/// `fully_adjudicated: false` and exits non-zero, and no surface reports
/// zero rows as a clean population.
#[test]
fn unreadable_sidecar_is_a_typed_condition_on_every_surface() {
    let ws = seed();
    let p = sidecar_path(ws.path());
    let mut sidecar: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    sidecar["version"] = serde_json::json!(3);
    fs::write(&p, serde_json::to_vec(&sidecar).unwrap()).unwrap();

    // verify-anchors: typed refusal, non-zero, fully_adjudicated false.
    let out = memstead()
        .current_dir(ws.path())
        .args(["verify-anchors", "--mem", "notes", "--quiet", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let v = json_of(&out);
    assert_eq!(v["code"], "ANCHORS_SIDECAR_UNREADABLE", "{v}");
    assert_eq!(v["details"]["mem"], "notes");
    assert!(
        v["details"]["reason"]
            .as_str()
            .unwrap()
            .contains("version 3"),
        "{v}"
    );
    assert_eq!(v["details"]["fully_adjudicated"], false);
    assert!(
        v["details"]["population"]
            .as_str()
            .unwrap()
            .contains("unknown"),
        "{v}"
    );

    // anchors by entity: refused, not empty.
    let out = memstead()
        .current_dir(ws.path())
        .args(["anchors", "notes--alpha", "--quiet", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let v = json_of(&out);
    assert_eq!(v["code"], "ANCHORS_SIDECAR_UNREADABLE");
    assert_eq!(v["details"]["mem"], "notes");

    // anchors by artifact: the condition rides along by mem.
    let out = memstead()
        .current_dir(ws.path())
        .args(["anchors", "--artifact", "src.txt", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json_of(&out);
    assert_eq!(
        v["sidecar_unreadable"][0]["code"],
        "ANCHORS_SIDECAR_UNREADABLE"
    );
    assert_eq!(v["sidecar_unreadable"][0]["mem"], "notes");

    // health: the anchors axis carries the condition with no clean
    // population; integrity lists the finding; strict refuses.
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "health",
            "--include",
            "anchors,integrity",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json_of(&out);
    let axis = &v["anchors"]["notes"];
    assert_eq!(
        axis["condition"]["code"], "ANCHORS_SIDECAR_UNREADABLE",
        "{v}"
    );
    assert_eq!(axis["condition"]["mem"], "notes");
    assert_eq!(axis["fully_adjudicated"], false);
    assert!(axis["population"].as_str().unwrap().contains("unknown"));
    let finding = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == "ANCHORS_SIDECAR_UNREADABLE")
        .unwrap_or_else(|| panic!("integrity finding missing: {v}"));
    assert_eq!(finding["id"], "notes");
    assert!(
        finding["detail"]["reason"]
            .as_str()
            .unwrap()
            .contains("version 3")
    );
    for include in ["integrity", "anchors"] {
        memstead()
            .current_dir(ws.path())
            .args(["health", "--include", include, "--strict", "--quiet"])
            .assert()
            .code(1);
    }

    // entity read: the condition beside the entity.
    let out = memstead()
        .current_dir(ws.path())
        .args(["entity", "notes--alpha", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json_of(&out);
    assert_eq!(
        v["anchors_sidecar_error"]["code"],
        "ANCHORS_SIDECAR_UNREADABLE"
    );
    assert_eq!(v["anchors_sidecar_error"]["mem"], "notes");
    let out = memstead()
        .current_dir(ws.path())
        .args(["entity", "notes--alpha", "--quiet"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("ANCHORS_SIDECAR_UNREADABLE"));
}

/// A2 AC2, refusal complement: the retired name `resolved` where a state
/// is read as input (a sidecar row's `last_observed.state`) refuses with
/// the vocabulary named, on every surface, as the same condition.
#[test]
fn retired_state_name_as_input_refuses_naming_the_vocabulary() {
    let ws = seed();
    let p = sidecar_path(ws.path());
    let mut sidecar: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    sidecar["entities"]["notes--alpha"][0]["last_observed"] = serde_json::json!({
        "at": "2026-09-01T00:00:00Z",
        "hash": "x",
        "state": "resolved",
    });
    fs::write(&p, serde_json::to_vec(&sidecar).unwrap()).unwrap();

    let out = memstead()
        .current_dir(ws.path())
        .args(["verify-anchors", "--mem", "notes", "--quiet", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let v = json_of(&out);
    assert_eq!(v["code"], "ANCHORS_SIDECAR_UNREADABLE");
    let reason = v["details"]["reason"].as_str().unwrap();
    assert!(reason.contains("`resolved`"), "{reason}");
    for name in ["resolves", "drifted", "recheck", "orphaned"] {
        assert!(reason.contains(name), "vocabulary not named: {reason}");
    }
}
