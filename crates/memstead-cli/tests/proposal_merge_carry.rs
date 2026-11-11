//! A proposal merge of an existing entity that changes
//! nothing but the self-check gated field, on a fresh independent record
//! of the stored content, must commit AND carry the record (the plan's
//! verb table row "update, batch update, proposal merge of an existing
//! entity: by the rule above").

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
        format!("name: gated\nversion: {version}\ndescription: d\nwhen_to_use: t\ntypes:\n  - plan\nrelationships:\n  mode: strict\n  definitions:\n    - name: PART_OF\n      description: h\n      default_weight: 1.0\n      acyclic: true\n    - name: _default\n      description: f\n      default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n"),
    )
    .unwrap();
    let constraints = if gate {
        "constraints:\n  - kind: transition_requires_self_check\n    field: status\n    to_value: complete\n    check_kind: x-approval\n    severity: block\n"
    } else {
        ""
    };
    fs::write(
        dir.join("types").join("plan.yaml"),
        format!("name: plan\ndescription: t\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: status\n    description: s\n    field_type: string\n    required: false\n    enum_values: [draft, complete]\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n{constraints}"),
    )
    .unwrap();
}

#[test]
fn proposal_merge_of_a_status_flip_carries_the_licensing_record() {
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
            "check",
            "plans--beta",
            "--verdict",
            "ok",
            "--method",
            "t",
            "--identity",
            "checker-c",
            "--role",
            "checker",
            "--kind",
            "x-approval",
        ],
    );
    run(
        ws,
        &["mem", "fork", "plans", "forkb", "--identity", "author-b"],
    );
    // The fork drops the gate so the proposer can flip the status there.
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
    fs::write(&disp, d.to_string()).unwrap();
    run(
        ws,
        &[
            "proposal",
            "merge",
            "forkb",
            "--dispositions",
            disp.to_str().unwrap(),
            "--identity",
            "owner-o",
        ],
    );

    // The merge landed the gated value on a record of the stored content
    // and changed nothing else: the record is carried, the gate stands.
    let out = run(ws, &["health", "--include", "constraints", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let violations = v["constraints"].to_string();
    assert!(
        !violations.contains("plans--beta"),
        "the merged status flip stands violated (record not carried): {violations}"
    );
}

/// A merge that adopts the status flip and amends the body in the same
/// call reaches the gated value on text no record names: it refuses, and
/// the entity and the ledger stay as they were.
#[test]
fn proposal_merge_adopting_with_changes_does_not_land_a_gated_value_on_unchecked_text() {
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
            "check",
            "plans--beta",
            "--verdict",
            "ok",
            "--method",
            "t",
            "--identity",
            "checker-c",
            "--role",
            "checker",
            "--kind",
            "x-approval",
        ],
    );
    run(
        ws,
        &["mem", "fork", "plans", "forkb", "--identity", "author-b"],
    );
    // The fork drops the gate so the proposer can flip the status there.
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
    d["dispositions"]["beta"]["disposition"] = "adopt_with_changes".into();
    d["dispositions"]["beta"]["reason"] = "the owner rewords the body".into();
    d["dispositions"]["beta"]["body"] = serde_json::json!({
        "sections": {"body": "amended by the owner, never checked"},
        "metadata": {"status": "complete"}
    });
    fs::write(&disp, d.to_string()).unwrap();
    let ledger_before = run(ws, &["checks", "--mem", "plans", "--json"]).stdout;
    let entity_before = run(ws, &["entity", "plans--beta", "--json"]).stdout;
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
            "--json",
            "--quiet",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("transition_requires_self_check"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        run(ws, &["checks", "--mem", "plans", "--json"]).stdout,
        ledger_before
    );
    assert_eq!(
        run(ws, &["entity", "plans--beta", "--json"]).stdout,
        entity_before
    );
}
