#![cfg(feature = "mem-repo")]
// `memstead mem set-schema` ships only in the full build.

//! The due brief reads `overdue` (backlog-decisions plan B4): with the
//! clock pinned, an open entity dated before today lists under `overdue`
//! with the days past, one dated inside the window under `due_soon` with
//! the days until, a closed one under neither; a type without a `due`
//! declaration yields no reading and no error; the stale axis is
//! untouched. The fixture is a workspace-local schema whose `obligation`
//! type declares `due` over `due_date` and `status` (the `obligation`
//! built-in declares the same axis but also requires an outgoing edge,
//! which is not what this test is about).

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

const MANIFEST: &str = r#"name: duties
version: 0.1.0
description: due-axis fixture
when_to_use: tests
types:
  - obligation
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;

const OBLIGATION: &str = "name: obligation\ndescription: dated duty\nwhen_to_use: Here\nsections:\n  - key: duty\n    heading: Duty\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\n  - key: consequence\n    heading: Consequence\n    required: true\n    search_weight: 5.0\n    catch_all: false\n    write_rules: []\n  - key: lead_time\n    heading: Lead time\n    required: false\n    search_weight: 3.0\n    catch_all: false\n    write_rules: []\nmetadata_fields:\n  - key: due_date\n    description: d\n    field_type: date\n  - key: status\n    description: s\n    field_type: string\n    default_value: open\n    enum_values: [open, in_progress, done]\n  - key: criticality\n    description: c\n    field_type: string\n    default_value: low\n    enum_values: [low, high]\ndue:\n  date_field: due_date\n  status_field: status\n  open_values: [open, in_progress]\n  lead_section: lead_time\ntitle_weight: 100.0\ntext_fields:\n  - duty\nhierarchy_relationship: _default\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - duty\n  - status\n  - due_date\nhealth_required_fields:\n  - duty\nstaleness_threshold_days: 90\nwrite_rules: []\n";

fn workspace() -> TempDir {
    let ws = TempDir::new().unwrap();
    memstead()
        .current_dir(ws.path())
        .args([
            "init",
            "--name",
            "duties",
            "--schema",
            "default@1.3.0",
            "--quiet",
        ])
        .assert()
        .success();
    let dir = ws
        .path()
        .join(".memstead")
        .join("schemas")
        .join("duties@0.1.0");
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(dir.join("schema.yaml"), MANIFEST).unwrap();
    fs::write(dir.join("types").join("obligation.yaml"), OBLIGATION).unwrap();
    memstead()
        .current_dir(ws.path())
        .args(["mem", "set-schema", "duties", "duties@0.1.0", "--quiet"])
        .assert()
        .success();
    ws
}

fn obligation(ws: &Path, title: &str, due_date: &str, status: &str) {
    memstead()
        .current_dir(ws)
        .args([
            "create",
            "--quiet",
            "--type",
            "obligation",
            "--title",
            title,
            "--section",
            "duty=file the return",
            "--section",
            "consequence=a fine",
            "--section",
            "lead_time=two weeks of bookkeeping",
            "--metadata",
            &format!("due_date={due_date}"),
            "--metadata",
            &format!("status={status}"),
            "--metadata",
            "criticality=low",
        ])
        .assert()
        .success();
}

fn due_json(ws: &Path) -> serde_json::Value {
    let out = memstead()
        .current_dir(ws)
        .args(["due", "--today", "2026-09-02", "--json", "--quiet"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// B4 AC1: overdue with days past, due_soon with days until, closed absent.
#[test]
fn overdue_and_due_soon_read_from_the_declaration_with_the_clock_pinned() {
    let ws = workspace();
    obligation(ws.path(), "Overdue filing", "2026-07-09", "open");
    obligation(ws.path(), "Soon filing", "2026-09-10", "in_progress");
    obligation(ws.path(), "Closed filing", "2026-07-09", "done");
    let v = due_json(ws.path());
    assert_eq!(v["today"], "2026-09-02");
    assert_eq!(v["mems"], serde_json::json!(["duties"]));
    let overdue = v["overdue"].as_array().unwrap();
    assert_eq!(overdue.len(), 1, "{v}");
    assert_eq!(overdue[0]["id"], "duties--overdue-filing");
    assert_eq!(overdue[0]["days_past"], 55);
    assert!(overdue[0].get("days_until").is_none());
    assert_eq!(overdue[0]["lead"]["section"], "lead_time");
    // No severity, no recommendation: the row names the reading only.
    for key in ["severity", "level", "recommendation", "action", "verdict"] {
        assert!(overdue[0].get(key).is_none(), "{key} must not appear: {v}");
    }
    let soon = v["due_soon"].as_array().unwrap();
    assert_eq!(soon.len(), 1, "{v}");
    assert_eq!(soon[0]["id"], "duties--soon-filing");
    assert_eq!(soon[0]["days_until"], 8);
    let text = v.to_string();
    assert!(!text.contains("closed-filing"), "{v}");
    let brief = v["brief"].as_str().unwrap();
    assert!(brief.contains("**OVERDUE** (55 days past)"), "{brief}");
    assert!(brief.contains("(in 8 days)"), "{brief}");
}

/// B4 AC1 refusal complement, first half: a mem whose schema declares no
/// `due` yields no reading and no error.
#[test]
fn a_schema_without_a_due_declaration_yields_no_reading_and_no_error() {
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
    let v = due_json(ws.path());
    assert_eq!(v["mems"], serde_json::json!([]));
    assert_eq!(v["overdue"], serde_json::json!([]));
    assert_eq!(v["due_soon"], serde_json::json!([]));
    assert!(
        v["brief"]
            .as_str()
            .unwrap()
            .contains("No mounted mem's schema declares a due axis"),
        "{v}"
    );
}

/// B4 AC1 refusal complement, second half: the stale axis does not read
/// the due date. An overdue but freshly edited entity is not stale.
#[test]
fn the_stale_axis_ignores_the_due_date() {
    let ws = workspace();
    obligation(ws.path(), "Overdue filing", "2026-07-09", "open");
    let out = memstead()
        .current_dir(ws.path())
        .args(["health", "--include", "stale", "--json", "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["stale"], serde_json::json!([]), "{v}");
    assert_eq!(v["summary"]["total_stale"], 0, "{v}");
}
