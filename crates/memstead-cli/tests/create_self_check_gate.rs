//! A create may not land a self-check gated value on a record of a
//! placeholder stub. Such a record stores an empty `entity_hash`: it names
//! no bytes, so it never licenses the bytes the create writes (batch
//! create and a proposal merge's creates share the same gate).

use std::fs;
use std::path::Path;

use assert_cmd::Command;

fn cli(ws: &Path, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap()
}

fn ok(ws: &Path, args: &[&str]) {
    let out = cli(ws, args);
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_create_does_not_land_a_self_check_gated_value_on_a_record_of_a_stub() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path();
    ok(
        ws,
        &["init", "--name", "plans", "--schema", "default@1.3.0"],
    );
    let dir = ws.join(".memstead").join("schemas").join("gated@0.1.0");
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(
        dir.join("schema.yaml"),
        "name: gated\nversion: 0.1.0\ndescription: d\nwhen_to_use: t\ntypes:\n  - bundle\n  - note\nrelationships:\n  mode: strict\n  definitions:\n    - name: PART_OF\n      description: h\n      default_weight: 1.0\n      acyclic: true\n    - name: LINKS\n      description: l\n      default_weight: 1.0\n    - name: _default\n      description: f\n      default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n",
    )
    .unwrap();
    let ty = |name: &str, constraints: &str| {
        format!(
            "name: {name}\ndescription: t\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: status\n    description: s\n    field_type: string\n    required: false\n    enum_values: [draft, complete]\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n{constraints}"
        )
    };
    fs::write(
        dir.join("types").join("bundle.yaml"),
        ty("bundle", "constraints:\n  - kind: transition_requires_self_check\n    field: status\n    to_value: complete\n    check_kind: x-approval\n    severity: block\n"),
    )
    .unwrap();
    fs::write(dir.join("types").join("note.yaml"), ty("note", "")).unwrap();
    ok(ws, &["mem", "set-schema", "plans", "gated@0.1.0"]);

    // The id gets a writer history, then becomes a placeholder.
    ok(
        ws,
        &[
            "create",
            "--identity",
            "author-a",
            "--type",
            "bundle",
            "--title",
            "Lambda",
            "--section",
            "body=x",
            "--metadata",
            "status=draft",
        ],
    );
    ok(ws, &["delete", "plans--lambda", "--identity", "author-a"]);
    ok(
        ws,
        &[
            "create",
            "--identity",
            "author-a",
            "--type",
            "note",
            "--title",
            "Linker",
            "--section",
            "body=l",
            "--relation",
            "LINKS:plans--lambda",
        ],
    );
    // An independent check of the placeholder (no content).
    ok(
        ws,
        &[
            "check",
            "plans--lambda",
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

    let out = cli(
        ws,
        &[
            "create",
            "--identity",
            "author-a",
            "--type",
            "bundle",
            "--title",
            "Lambda",
            "--section",
            "body=text nobody checked",
            "--metadata",
            "status=complete",
            "--json",
        ],
    );
    assert!(
        !out.status.success(),
        "a create landed status=complete on text no record names: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!ws.join("lambda.md").exists());
}
