//! A due axis with an offset from an engine-stamped timestamp and a roster
//! gate: a complete container is due N days after its last change, and
//! never while a member is still open. The shape the planning schema's
//! bundle type declares, exercised on a schema of its own.

use std::fs;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

const MANIFEST: &str = r#"name: rosters
version: 0.1.0
description: due-gate fixture
when_to_use: tests
types:
  - bundle
  - plan
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: membership
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;

const BUNDLE: &str = "name: bundle\ndescription: a campaign\nwhen_to_use: tests\nsections:\n  - key: purpose\n    heading: Purpose\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: status\n    description: s\n    field_type: string\n    default_value: draft\n    enum_values: [draft, in_progress, complete]\ndue:\n  date_field: last_modified\n  offset_days: 14\n  status_field: status\n  open_values: [complete]\n  unless_open_via:\n    relationships: [PART_OF]\n    direction: in\n    status_field: status\n    open_values: [draft, in_progress]\ntitle_weight: 100.0\ntext_fields: [purpose]\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: [PART_OF]\nupdatable_fields: [title, purpose, status]\nhealth_required_fields: [purpose]\nstaleness_threshold_days: 180\nwrite_rules: []\n";

const PLAN: &str = "name: plan\ndescription: a unit of work\nwhen_to_use: tests\nsections:\n  - key: goal\n    heading: Goal\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: status\n    description: s\n    field_type: string\n    default_value: draft\n    enum_values: [draft, in_progress, complete]\ntitle_weight: 100.0\ntext_fields: [goal]\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: [PART_OF]\nupdatable_fields: [title, goal, status]\nhealth_required_fields: [goal]\nstaleness_threshold_days: 180\nwrite_rules: []\n";

fn entity(kind: &str, title: &str, status: &str, section: &str, body: &str, rels: &str) -> String {
    format!(
        "---\ntype: {kind}\ncreated_date: 2026-08-01T00:00:00Z\nlast_modified: 2026-08-21T09:00:00Z\nstatus: {status}\n---\n# {title}\n\n## {section}\n\n{body}\n{rels}"
    )
}

/// A folder-mount workspace with the rosters schema installed, one bundle
/// last modified on 2026-08-21 and one plan PART_OF it.
fn workspace(bundle_status: &str, plan_status: &str) -> TempDir {
    let ws = TempDir::new().unwrap();
    let root = ws.path();
    let store = root.join(".memstead");
    fs::create_dir_all(store.join("state")).unwrap();
    fs::write(
        store.join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    let pkg = store.join("schemas").join("rosters@0.1.0");
    fs::create_dir_all(pkg.join("types")).unwrap();
    fs::write(pkg.join("schema.yaml"), MANIFEST).unwrap();
    fs::write(pkg.join("types/bundle.yaml"), BUNDLE).unwrap();
    fs::write(pkg.join("types/plan.yaml"), PLAN).unwrap();
    fs::write(
        store.join("state/mounts.json"),
        r#"{"format":"memstead-mounts-3","mounts":[{"mem":"camp","schema":"rosters@0.1.0","storage":{"type":"folder","path":"camp-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}]}"#,
    )
    .unwrap();
    let mem = root.join("camp-mem");
    fs::create_dir_all(mem.join(".memstead")).unwrap();
    fs::write(
        mem.join(".memstead/config.json"),
        r#"{"format":1,"schema":"rosters@0.1.0"}"#,
    )
    .unwrap();
    fs::write(
        mem.join("the-campaign.md"),
        entity(
            "bundle",
            "The campaign",
            bundle_status,
            "Purpose",
            "Deliver.",
            "",
        ),
    )
    .unwrap();
    fs::write(
        mem.join("first-plan.md"),
        entity(
            "plan",
            "First plan",
            plan_status,
            "Goal",
            "Reach it.",
            "\n## Relationships\n\n- **PART_OF**: [[the-campaign]]\n",
        ),
    )
    .unwrap();
    ws
}

fn due_json(root: &std::path::Path) -> Value {
    let out = memstead()
        .current_dir(root)
        .args(["--json", "due", "--today", "2026-09-05", "--quiet"])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}\n{}", String::from_utf8_lossy(&out.stdout)))
}

/// Complete bundle, complete plan, last change 15 days back: overdue by
/// one day on `memstead due`, on the health due axis, and in both
/// markdown renderings; the plan itself declares no due axis and never
/// appears.
#[test]
fn a_complete_bundle_is_due_fourteen_days_after_its_last_change() {
    let ws = workspace("complete", "complete");
    let root = ws.path();
    let due = due_json(root);
    assert_eq!(due["mems"], serde_json::json!(["camp"]), "{due}");
    let overdue = due["overdue"].as_array().unwrap();
    assert_eq!(overdue.len(), 1, "{due}");
    assert_eq!(overdue[0]["id"], "camp--the-campaign");
    assert_eq!(
        overdue[0]["date"], "2026-09-04",
        "last change plus fourteen days"
    );
    assert_eq!(overdue[0]["days_past"], 1);
    assert_eq!(overdue[0]["status"], "complete");

    let md = memstead()
        .current_dir(root)
        .args(["due", "--today", "2026-09-05", "--quiet"])
        .output()
        .unwrap();
    let md = String::from_utf8(md.stdout).unwrap();
    assert!(
        md.contains("camp--the-campaign") && md.contains("OVERDUE** (1 days past)"),
        "{md}"
    );

    // The health due axis carries the same row (today is the real date
    // here, so only the id and the presence of days_past are pinned).
    let out = memstead()
        .current_dir(root)
        .args(["--json", "health", "--include", "due", "--quiet"])
        .output()
        .unwrap();
    let health: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = health["due"]["overdue"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{}", health["due"]);
    assert_eq!(rows[0]["id"], "camp--the-campaign");
    assert!(rows[0]["days_past"].as_u64().unwrap() >= 1);
    let md = memstead()
        .current_dir(root)
        .args(["health", "--include", "due", "--quiet"])
        .output()
        .unwrap();
    let md = String::from_utf8(md.stdout).unwrap();
    assert!(
        md.contains("## Due (1 overdue") && md.contains("camp--the-campaign"),
        "{md}"
    );
}

/// The roster gate: the same bundle with one plan still in progress is not
/// due, however old its last change; a bundle not yet complete is not due
/// either.
#[test]
fn a_bundle_with_an_open_plan_is_never_due() {
    let ws = workspace("complete", "in_progress");
    let due = due_json(ws.path());
    assert_eq!(
        due["mems"],
        serde_json::json!(["camp"]),
        "the schema still declares the axis"
    );
    assert!(due["overdue"].as_array().unwrap().is_empty(), "{due}");
    assert!(due["due_soon"].as_array().unwrap().is_empty(), "{due}");

    let ws = workspace("in_progress", "complete");
    let due = due_json(ws.path());
    assert!(due["overdue"].as_array().unwrap().is_empty(), "{due}");
}
