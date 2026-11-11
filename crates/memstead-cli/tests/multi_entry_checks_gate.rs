//! Grader repro: one write (a batch update, a proposal merge) that edits a
//! checked criterion AND moves its plan onto a `transition_requires_checks`
//! value lands the plan with its gate unmet on the write's result.

use std::fs;
use std::path::Path;

use assert_cmd::Command;

fn run(ws: &Path, args: &[&str]) -> std::process::Output {
    let out = Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn package(ws: &Path, version: &str, gate: bool) {
    let dir = ws
        .join(".memstead")
        .join("schemas")
        .join(format!("gated@{version}"));
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(
        dir.join("schema.yaml"),
        format!("name: gated\nversion: {version}\ndescription: d\nwhen_to_use: t\ntypes:\n  - plan\n  - criterion\nrelationships:\n  mode: strict\n  definitions:\n    - name: VERIFIES\n      description: v\n      default_weight: 3.0\n      acyclic: true\n    - name: PART_OF\n      description: h\n      default_weight: 1.0\n      acyclic: true\n    - name: _default\n      description: f\n      default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n"),
    )
    .unwrap();
    let constraints = if gate {
        "constraints:\n  - kind: transition_requires_checks\n    field: status\n    to_value: complete\n    relationships: [VERIFIES]\n    direction: in\n    min_related: 1\n    severity: block\n"
    } else {
        ""
    };
    let body = |name: &str, meta: &str, c: &str| {
        format!(
            "name: {name}\ndescription: t\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:{meta}\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n{c}"
        )
    };
    fs::write(
        dir.join("types").join("plan.yaml"),
        body("plan", "\n  - key: status\n    description: s\n    field_type: string\n    required: false\n    enum_values: [draft, complete]", constraints),
    )
    .unwrap();
    fs::write(
        dir.join("types").join("criterion.yaml"),
        body("criterion", " []", ""),
    )
    .unwrap();
}

fn setup(ws: &Path) {
    run(ws, &["mem-repo", "init"]);
    run(
        ws,
        &[
            "workspace",
            "allow-create",
            "plans",
            "--schema",
            "default@1.3.0",
        ],
    );
    run(ws, &["mem", "init", "plans", "--schema", "default@1.3.0"]);
    package(ws, "0.1.0", true);
    package(ws, "0.2.0", false);
    run(ws, &["mem", "set-schema", "plans", "gated@0.1.0"]);
    run(
        ws,
        &[
            "workspace",
            "allow-create",
            "fork*",
            "--schema",
            "gated@0.1.0",
        ],
    );
    run(
        ws,
        &[
            "create",
            "--identity",
            "author-a",
            "--type",
            "plan",
            "--title",
            "Beta",
            "--section",
            "body=original",
            "--metadata",
            "status=draft",
        ],
    );
    run(
        ws,
        &[
            "create",
            "--identity",
            "author-a",
            "--type",
            "criterion",
            "--title",
            "Crit",
            "--section",
            "body=checked criterion",
            "--relation",
            "VERIFIES:plans--beta",
        ],
    );
    run(
        ws,
        &[
            "check",
            "plans--crit",
            "--verdict",
            "ok",
            "--method",
            "t",
            "--identity",
            "checker-c",
            "--role",
            "checker",
        ],
    );
}

fn status_of(ws: &Path) -> String {
    String::from_utf8_lossy(&run(ws, &["entity", "plans--beta"]).stdout).to_string()
}

#[test]
fn batch_update_editing_the_criterion_cannot_close_its_plan() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path();
    setup(ws);
    let f = ws.join("b.json");
    fs::write(&f, r#"{"updates":[{"id":"plans--crit","auto_hash":true,"sections":{"body":"edited after the check"}},{"id":"plans--beta","auto_hash":true,"metadata":{"status":"complete"}}]}"#).unwrap();
    let out = Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args([
            "batch-update",
            "--from",
            f.to_str().unwrap(),
            "--identity",
            "author-b",
            "--quiet",
        ])
        .output()
        .unwrap();
    let ent = status_of(ws);
    assert!(
        !out.status.success() && ent.contains("status: draft"),
        "batch update landed status: complete while its only criterion's check is stale: {}\n{ent}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn proposal_merge_editing_the_criterion_cannot_close_its_plan() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path();
    setup(ws);
    run(
        ws,
        &["mem", "fork", "plans", "forkb", "--identity", "author-b"],
    );
    run(ws, &["mem", "set-schema", "forkb", "gated@0.2.0"]);
    run(
        ws,
        &[
            "update",
            "forkb--beta",
            "--metadata",
            "status=complete",
            "--auto-hash",
            "--identity",
            "author-b",
        ],
    );
    run(
        ws,
        &[
            "update",
            "forkb--crit",
            "--section",
            "body=edited after the check",
            "--auto-hash",
            "--identity",
            "author-b",
        ],
    );
    let disp = ws.join("disp.json");
    run(
        ws,
        &[
            "proposal",
            "brief",
            "forkb",
            "--out",
            disp.to_str().unwrap(),
        ],
    );
    let mut d: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&disp).unwrap()).unwrap();
    d["dispositions"]["beta"]["disposition"] = "adopt".into();
    d["dispositions"]["crit"]["disposition"] = "adopt".into();
    fs::write(&disp, d.to_string()).unwrap();
    let out = Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args([
            "proposal",
            "merge",
            "forkb",
            "--dispositions",
            disp.to_str().unwrap(),
            "--identity",
            "owner-o",
            "--quiet",
        ])
        .output()
        .unwrap();
    let ent = status_of(ws);
    assert!(
        !out.status.success() && ent.contains("status: draft"),
        "proposal merge landed status: complete while its only criterion's check is stale: {}\n{ent}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A batch create that creates a never-checked criterion beside the plan
/// it closes refuses, as the two creates do one after the other.
#[test]
fn batch_create_with_an_unchecked_new_criterion_cannot_close_its_plan() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path();
    run(ws, &["mem-repo", "init"]);
    run(
        ws,
        &[
            "workspace",
            "allow-create",
            "plans",
            "--schema",
            "default@1.3.0",
        ],
    );
    run(ws, &["mem", "init", "plans", "--schema", "default@1.3.0"]);
    package(ws, "0.1.0", true);
    run(ws, &["mem", "set-schema", "plans", "gated@0.1.0"]);
    run(
        ws,
        &[
            "create",
            "--identity",
            "author-a",
            "--type",
            "criterion",
            "--title",
            "Crit",
            "--section",
            "body=checked criterion",
            "--relation",
            "VERIFIES:plans--beta",
        ],
    );
    run(
        ws,
        &[
            "check",
            "plans--crit",
            "--verdict",
            "ok",
            "--method",
            "t",
            "--identity",
            "checker-c",
            "--role",
            "checker",
        ],
    );
    let f = ws.join("c.json");
    fs::write(
        &f,
        r#"{"creates":[{"title":"Fresh","entity_type":"criterion","mem":"plans","sections":{"body":"never checked"},"relations":[{"rel_type":"VERIFIES","target":"plans--beta"}]},{"title":"Beta","entity_type":"plan","mem":"plans","sections":{"body":"b"},"metadata":{"status":"complete"}}]}"#,
    )
    .unwrap();
    let out = Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args([
            "batch-create",
            "--from",
            f.to_str().unwrap(),
            "--identity",
            "author-a",
            "--quiet",
        ])
        .output()
        .unwrap();
    let ent = Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args(["entity", "plans--beta", "--quiet"])
        .output()
        .unwrap();
    let ent = String::from_utf8_lossy(&ent.stdout).to_string();
    assert!(
        !out.status.success() && !ent.contains("status: complete"),
        "batch create landed status: complete beside a never-checked criterion: {}\n{ent}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// The schema-language direction and relation set render in their one
/// spelling on every surface that shows a checks gate: the gates brief,
/// the `CONSTRAINT_UNSATISFIED` details and the health constraints axis.
#[test]
fn the_checks_gate_renders_in_and_relationships_on_every_surface() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path();
    setup(ws);
    let brief = String::from_utf8_lossy(&run(ws, &["gates", "--mem", "plans"]).stdout).to_string();
    assert!(brief.contains("related via [VERIFIES] (in)"), "{brief}");

    // A second, unchecked criterion: the transition refuses.
    run(
        ws,
        &[
            "create",
            "--identity",
            "author-a",
            "--type",
            "criterion",
            "--title",
            "Open",
            "--section",
            "body=never checked",
            "--relation",
            "VERIFIES:plans--beta",
        ],
    );
    let out = Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args([
            "update",
            "plans--beta",
            "--metadata",
            "status=complete",
            "--auto-hash",
            "--identity",
            "author-b",
            "--json",
            "--quiet",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let refusal: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let violation = refusal
        .pointer("/details/violations/0")
        .unwrap_or_else(|| panic!("{refusal}"));
    assert_eq!(violation["direction"], "in", "{violation}");
    assert_eq!(
        violation["relationships"],
        serde_json::json!(["VERIFIES"]),
        "{violation}"
    );

    // Check it, close the plan, then edit a criterion: the health axis
    // reports the standing violation in the same spelling.
    run(
        ws,
        &[
            "check",
            "plans--open",
            "--verdict",
            "ok",
            "--method",
            "t",
            "--identity",
            "checker-c",
            "--role",
            "checker",
        ],
    );
    run(
        ws,
        &[
            "update",
            "plans--beta",
            "--metadata",
            "status=complete",
            "--auto-hash",
            "--identity",
            "author-b",
        ],
    );
    run(
        ws,
        &[
            "update",
            "plans--open",
            "--section",
            "body=edited",
            "--auto-hash",
            "--identity",
            "author-a",
        ],
    );
    let health = run(ws, &["health", "--include", "constraints", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&health.stdout).unwrap();
    let standing = &v["constraints"][0]["violations"][0];
    assert_eq!(standing["direction"], "in", "{standing}");
    assert_eq!(
        standing["relationships"],
        serde_json::json!(["VERIFIES"]),
        "{standing}"
    );
    let text = v["constraints"].to_string();
    for retired in ["incoming", "outgoing", "rel_type"] {
        assert!(!text.contains(retired), "`{retired}` in {text}");
    }
}
