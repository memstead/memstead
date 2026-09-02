//! `software@0.5.0` (backlog-decisions plan B6): a `contract` with
//! `protocol: engine_state` validates with and without `version_axes`
//! (the field is optional); a `version_axes` member that is not a
//! `name=constant` pair refuses `INVALID_FIELD_VALUE` naming the member
//! and the pattern; `spec` carries an optional `notes` section; and a
//! schema package whose `value_pattern` does not compile refuses at
//! `schema validate` naming the field.

use std::fs;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// A folder workspace whose single mem pins the built-in `software@0.5.0`.
fn workspace() -> TempDir {
    let ws = TempDir::new().unwrap();
    memstead()
        .current_dir(ws.path())
        .args([
            "init",
            "--name",
            "code",
            "--schema",
            "software@0.5.0",
            "--quiet",
        ])
        .assert()
        .success();
    ws
}

fn contract(ws: &TempDir, title: &str, axes: Option<&str>) -> std::process::Output {
    let mut args: Vec<String> = vec![
        "create".into(),
        "--quiet".into(),
        "--type".into(),
        "contract".into(),
        "--title".into(),
        title.into(),
        "--section".into(),
        "summary=An engine-owned sidecar the next engine reads back.".into(),
        "--metadata".into(),
        "protocol=engine_state".into(),
        "--metadata".into(),
        "version=2".into(),
        "--metadata".into(),
        "stable_since=2026-09-02".into(),
        "--identity".into(),
        "author-one".into(),
        "--role".into(),
        "author".into(),
    ];
    if let Some(a) = axes {
        args.push("--metadata".into());
        args.push(format!("version_axes={a}"));
    }
    memstead()
        .current_dir(ws.path())
        .args(&args)
        .output()
        .unwrap()
}

/// B6 AC2 refusal complement, first half: `engine_state` with no
/// `version_axes` validates; a well-formed pair list validates too.
#[test]
fn an_engine_state_contract_validates_with_and_without_version_axes() {
    let ws = workspace();
    let out = contract(&ws, "Anchors sidecar", None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = contract(
        &ws,
        "Binding record",
        Some("BINDING_RECORD_VERSION=2,ANCHOR_SIDECAR_VERSION=2"),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(
        &memstead()
            .current_dir(ws.path())
            .args(["entity", "code--binding-record", "--quiet"])
            .output()
            .unwrap()
            .stdout,
    )
    .to_string();
    assert!(text.contains("engine_state"), "{text}");
    assert!(text.contains("ANCHOR_SIDECAR_VERSION=2"), "{text}");
}

/// B6 AC2 refusal complement, second half: a member that is not a
/// `name=constant` pair refuses `INVALID_FIELD_VALUE` naming it.
#[test]
fn a_malformed_version_axis_refuses_invalid_field_value() {
    let ws = workspace();
    let out = contract(
        &ws,
        "Broken axes",
        Some("ANCHOR_SIDECAR_VERSION=2,just a note"),
    );
    assert!(!out.status.success());
    let text =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("INVALID_FIELD_VALUE"), "{text}");
    assert!(text.contains("just a note"), "{text}");
    assert!(text.contains("version_axes"), "{text}");
    assert!(text.contains("pattern"), "{text}");
}

/// `spec` gains an optional Notes section: a spec with the historical
/// marker in Notes and a one-sentence Identity validates.
#[test]
fn a_spec_carries_its_standing_remark_in_notes() {
    let ws = workspace();
    memstead()
        .current_dir(ws.path())
        .args([
            "create",
            "--quiet",
            "--type",
            "spec",
            "--title",
            "Pipeline store",
            "--section",
            "identity=The gen-1 pipeline store that persisted four primitives to the workspace.",
            "--section",
            "purpose=Kept for its citations from the knowledge mem.",
            "--section",
            "notes=Historical record: migrated into v2 single-record bindings on 2026-08-23; stability frozen.",
            "--metadata",
            "stability=frozen",
            "--identity",
            "author-one",
            "--role",
            "author",
        ])
        .assert()
        .success();
}

/// A package whose `value_pattern` does not compile refuses at
/// `schema validate` naming the type, the field and the pattern.
#[test]
fn a_value_pattern_that_does_not_compile_refuses_at_validate() {
    let ws = TempDir::new().unwrap();
    let pkg = ws.path().join("bad");
    fs::create_dir_all(pkg.join("types")).unwrap();
    fs::write(
        pkg.join("schema.yaml"),
        "name: bad\nversion: 0.1.0\ndescription: pattern fixture\nwhen_to_use: tests\ntypes:\n  - item\nrelationships:\n  mode: strict\n  definitions:\n    - name: _default\n      description: fallback\n      default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n",
    )
    .unwrap();
    fs::write(
        pkg.join("types").join("item.yaml"),
        "name: item\ndescription: an item\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: axes\n    description: pairs\n    field_type: string\n    serialization: csv_array\n    value_pattern: \"[A-Z\"\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: _default\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\n  - axes\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n",
    )
    .unwrap();
    let out = memstead()
        .current_dir(ws.path())
        .args(["schema", "validate", pkg.to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("value_pattern"), "{text}");
    assert!(text.contains("axes"), "{text}");
    assert!(text.contains("does not compile"), "{text}");
}
