#![cfg(feature = "mem-repo")]
// `memstead mem set-schema` ships only in the full build.

//! The checks-axis independence reading compares a check's identity with
//! every identity that mutated the verified plan, its criteria or its
//! session-log notes since the criterion was written — never with the
//! criterion's author alone — and the `transition_requires_checks` gate
//! consumes that reading, so a plan cannot complete on the executor's own
//! checks (A5).

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// A folder workspace with one mem `plans` pinned to a gated schema:
/// `plan` completes only when every incoming VERIFIES criterion carries a
/// fresh, independent ok check; `criterion` and `note` (hierarchy PART_OF)
/// are plain typed entities.
fn seed() -> TempDir {
    let ws = TempDir::new().unwrap();
    memstead()
        .current_dir(ws.path())
        .args([
            "init",
            "--name",
            "plans",
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
        .join("gated@0.1.0");
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(
        dir.join("schema.yaml"),
        r#"name: gated
version: 0.1.0
description: transition_requires_checks test schema
when_to_use: tests
types:
  - plan
  - criterion
  - note
relationships:
  mode: strict
  definitions:
    - name: VERIFIES
      description: v
      default_weight: 3.0
      acyclic: true
    - name: PART_OF
      description: hier
      default_weight: 1.0
      acyclic: true
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
    )
    .unwrap();
    let body = |name: &str, extra_meta: &str, constraints: &str| {
        format!(
            "name: {name}\ndescription: t\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:{extra_meta}\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n{constraints}"
        )
    };
    fs::write(
        dir.join("types").join("plan.yaml"),
        body(
            "plan",
            "\n  - key: status\n    description: s\n    field_type: string\n    default_value: draft\n    enum_values: [draft, complete]",
            "constraints:\n  - kind: transition_requires_checks\n    field: status\n    to_value: complete\n    relationships: [VERIFIES]\n    direction: incoming\n    severity: block\n",
        ),
    )
    .unwrap();
    fs::write(
        dir.join("types").join("criterion.yaml"),
        body("criterion", " []", ""),
    )
    .unwrap();
    fs::write(dir.join("types").join("note.yaml"), body("note", " []", "")).unwrap();
    memstead()
        .current_dir(ws.path())
        .args(["mem", "set-schema", "plans", "gated@0.1.0", "--quiet"])
        .assert()
        .success();
    ws
}

fn create(ws: &Path, identity: &str, ty: &str, title: &str, relation: Option<&str>) {
    let mut args = vec![
        "create",
        "--quiet",
        "--identity",
        identity,
        "--role",
        "author",
        "--type",
        ty,
        "--title",
        title,
        "--section",
        "body=x",
    ];
    if let Some(r) = relation {
        args.push("--relation");
        args.push(r);
    }
    memstead().current_dir(ws).args(&args).assert().success();
}

fn check(ws: &Path, identity: Option<&str>, entity: &str) {
    let mut args = vec![
        "check",
        entity,
        "--verdict",
        "ok",
        "--role",
        "checker",
        "--method",
        "test",
        "--quiet",
    ];
    if let Some(i) = identity {
        args.push("--identity");
        args.push(i);
    }
    memstead().current_dir(ws).args(&args).assert().success();
}

fn independence(ws: &Path) -> serde_json::Value {
    let out = memstead()
        .current_dir(ws)
        .args(["health", "--include", "checks", "--json", "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["checks"]["plans"]["independence"].clone()
}

fn complete(ws: &Path, identity: &str) -> std::process::Output {
    memstead()
        .current_dir(ws)
        .args([
            "update",
            "plans--the-plan",
            "--metadata",
            "status=complete",
            "--auto-hash",
            "--identity",
            identity,
            "--role",
            "author",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap()
}

/// A5 AC1: identity A authors the plan and the criterion; identity B
/// mutates the plan (a note) and checks: `self_checked`, the transition
/// refuses; identity C, which mutated nothing, checks: the criterion reads
/// `confirmed_independent` and the transition succeeds. Refusal
/// complement: a check without an identity stays `unconfirmable` and
/// refuses.
#[test]
fn executors_own_checks_read_self_checked_and_do_not_close_the_gate() {
    let ws = seed();
    let root = ws.path();
    create(root, "identity-a", "plan", "The Plan", None);
    create(
        root,
        "identity-a",
        "criterion",
        "The Criterion",
        Some("VERIFIES:plans--the-plan"),
    );
    // B executes: a session-log note under the plan.
    create(
        root,
        "identity-b",
        "note",
        "Session One",
        Some("PART_OF:plans--the-plan"),
    );

    // Refusal complement first: no identity, no promotion.
    check(root, None, "plans--the-criterion");
    let ind = independence(root);
    assert_eq!(
        ind["unconfirmable"]["items"],
        serde_json::json!(["plans--the-criterion"]),
        "{ind}"
    );
    let out = complete(root, "identity-b");
    assert!(
        !out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let env: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(env["code"], "CONSTRAINT_UNSATISFIED", "{env}");
    assert!(env.to_string().contains("unconfirmable"), "{env}");

    // The executor's own check: self_checked, and the gate stays shut.
    check(root, Some("identity-b"), "plans--the-criterion");
    let ind = independence(root);
    assert_eq!(
        ind["self_checked"]["items"],
        serde_json::json!(["plans--the-criterion"]),
        "{ind}"
    );
    let executors = ind["executors"]["plans--the-criterion"].as_array().unwrap();
    let names: Vec<&str> = executors.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        names.contains(&"identity-a") && names.contains(&"identity-b"),
        "{ind}"
    );
    let out = complete(root, "identity-b");
    assert!(!out.status.success());
    let env: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(env["code"], "CONSTRAINT_UNSATISFIED", "{env}");
    assert!(env.to_string().contains("self_checked"), "{env}");

    // An identity that mutated nothing since the criterion was written.
    check(root, Some("identity-c"), "plans--the-criterion");
    let ind = independence(root);
    assert_eq!(
        ind["confirmed_independent"]["items"],
        serde_json::json!(["plans--the-criterion"]),
        "{ind}"
    );
    let out = complete(root, "identity-b");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // The gates brief agrees: the plan is closed.
    let out = memstead()
        .current_dir(root)
        .args(["gates", "--mem", "plans", "--quiet"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Closed (at `complete`): `plans--the-plan`"),
        "{text}"
    );
}

/// A5 AC2, first half: the ledger line shape is unchanged — a row written
/// before the plan (no new field) derives under the new comparator. The
/// row is appended by hand in the exact pre-plan shape.
#[test]
fn old_ledger_rows_derive_under_the_new_comparator() {
    let ws = seed();
    let root = ws.path();
    create(root, "identity-a", "plan", "The Plan", None);
    create(
        root,
        "identity-a",
        "criterion",
        "The Criterion",
        Some("VERIFIES:plans--the-plan"),
    );
    let out = memstead()
        .current_dir(root)
        .args(["entity", "plans--the-criterion", "--json", "--quiet"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let hash = v["_hash"].as_str().unwrap().to_string();
    let ledger = root
        .join(".memstead")
        .join("state")
        .join("checks")
        .join("checks.jsonl");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 60;
    // The pre-plan line shape: ts, entity, verdict, method, entity_hash,
    // actor, client, role, identity — nothing new.
    fs::write(
        &ledger,
        format!(
            "{{\"ts\":{ts},\"entity\":\"plans--the-criterion\",\"verdict\":\"ok\",\"method\":\"old row\",\"entity_hash\":\"{hash}\",\"actor\":\"cli\",\"client\":\"memstead-cli@0.15.0\",\"role\":\"checker\",\"identity\":\"identity-a\"}}\n"
        ),
    )
    .unwrap();
    let ind = independence(root);
    // identity-a authored the criterion: an executor, so self_checked
    // under the new comparator, and the old row parsed as-is.
    assert_eq!(
        ind["self_checked"]["items"],
        serde_json::json!(["plans--the-criterion"]),
        "{ind}"
    );
    assert!(
        ind["comparator"]
            .as_str()
            .unwrap()
            .contains("since the criterion was written")
    );
}
