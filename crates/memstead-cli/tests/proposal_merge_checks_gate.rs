//! A proposal merge of an existing entity onto a
//! `transition_requires_checks` gated value whose related criterion is
//! unconfirmed must refuse, whether the owner adopts or adopts with changes.

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

fn attempt(disposition: &str) -> (bool, String) {
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
            "create",
            "--identity",
            "author-a",
            "--type",
            "criterion",
            "--title",
            "Crit",
            "--section",
            "body=unchecked criterion",
            "--relation",
            "VERIFIES:plans--beta",
        ],
    );
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
    d["dispositions"]["beta"]["disposition"] = disposition.into();
    if disposition == "adopt_with_changes" {
        d["dispositions"]["beta"]["reason"] = "owner restates".into();
        d["dispositions"]["beta"]["body"] = serde_json::json!({
            "sections": {"body": "original"},
            "metadata": {"status": "complete"}
        });
    }
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
            "--json",
            "--quiet",
        ])
        .output()
        .unwrap();
    let ent = run(ws, &["entity", "plans--beta"]).stdout;
    (
        out.status.success(),
        format!(
            "{}{}\n--- entity:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&ent)
        ),
    )
}

#[test]
fn adopt_onto_unmet_checks_gate_refuses() {
    let (ok, text) = attempt("adopt");
    assert!(!ok, "plain adopt landed an unmet checks gate: {text}");
}

#[test]
fn adopt_with_changes_onto_unmet_checks_gate_refuses() {
    let (ok, text) = attempt("adopt_with_changes");
    assert!(
        !ok,
        "adopt_with_changes landed an unmet checks gate: {text}"
    );
}
