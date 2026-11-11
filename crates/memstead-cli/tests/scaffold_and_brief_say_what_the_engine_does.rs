//! The scaffold, help and error text say what the engine does: every key
//! the scaffolded type leaves unmarked can be deleted and the package
//! still validates (a missing catch-all section still refuses); the gates
//! brief lists both gated-transition forms; a `number` field's pattern
//! refusal names `number`; `due --help` lists `--today`.

use std::fs;
use std::path::Path;

use assert_cmd::Command;

fn run(ws: &Path, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap()
}

/// Top-level keys of a type file whose comment block carries no
/// REQUIRED marker, each with the line range its block spans.
fn unmarked_top_level_keys(text: &str) -> Vec<(String, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(key) = line.split(':').next().filter(|k| {
            !k.is_empty() && !line.starts_with(' ') && !line.starts_with('#') && line.contains(':')
        }) else {
            continue;
        };
        let mut j = i;
        let mut marked = false;
        while j > 0 && lines[j - 1].trim_start().starts_with('#') {
            j -= 1;
            marked |= lines[j].contains("REQUIRED");
        }
        if marked {
            continue;
        }
        let mut end = i + 1;
        while end < lines.len() && (lines[end].starts_with(' ') || lines[end].trim().is_empty()) {
            end += 1;
        }
        out.push((key.to_string(), i, end));
    }
    out
}

#[test]
fn every_unmarked_scaffold_key_is_optional_and_a_catch_all_is_required() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path();
    assert!(run(ws, &["schema", "new", "probe"]).status.success());
    let type_file = ws.join("probe").join("types").join("note.yaml");
    let original = fs::read_to_string(&type_file).unwrap();
    let unmarked = unmarked_top_level_keys(&original);
    assert!(
        unmarked
            .iter()
            .any(|(k, _, _)| k == "no_self_loop_relationships"),
        "{unmarked:?}"
    );
    for (key, start, end) in &unmarked {
        let lines: Vec<&str> = original.lines().collect();
        let without: Vec<&str> = lines[..*start]
            .iter()
            .chain(&lines[*end..])
            .copied()
            .collect();
        fs::write(&type_file, without.join("\n") + "\n").unwrap();
        let out = run(ws, &["schema", "validate", "probe"]);
        assert!(
            out.status.success(),
            "deleting the unmarked `{key}` refuses: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    for marked in [
        "catch_all: true",
        "write_rules:\n  - \"Notes are placeholders",
    ] {
        assert!(original.contains(marked), "{marked}");
    }
    fs::write(&type_file, original.replace("    catch_all: true\n", "")).unwrap();
    assert!(
        !run(ws, &["schema", "validate", "probe"]).status.success(),
        "a type without a catch-all section still refuses"
    );
    // The scaffold names no count of constraint forms.
    for count in ["four-form", "five-form", "six-form", "seven-form"] {
        assert!(!original.contains(count), "{count}");
    }
}

const MANIFEST: &str = "name: gated\nversion: 0.1.0\ndescription: d\nwhen_to_use: t\ntypes:\n  - item\nrelationships:\n  mode: strict\n  definitions:\n    - name: VERIFIES\n      description: v\n      default_weight: 1.0\n    - name: _default\n      description: f\n      default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n";

fn item_type(extra_field: &str, constraints: &str) -> String {
    format!(
        "name: item\ndescription: t\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: status\n    description: s\n    field_type: string\n    enum_values: [draft, done]\n{extra_field}title_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: _default\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\n  - status\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n{constraints}"
    )
}

fn workspace(type_text: &str) -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path();
    assert!(
        run(
            ws,
            &["init", "--name", "items", "--schema", "default@1.3.0"]
        )
        .status
        .success()
    );
    let dir = ws.join(".memstead").join("schemas").join("gated@0.1.0");
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(dir.join("schema.yaml"), MANIFEST).unwrap();
    fs::write(dir.join("types").join("item.yaml"), type_text).unwrap();
    assert!(
        run(ws, &["mem", "set-schema", "items", "gated@0.1.0"])
            .status
            .success()
    );
    tmp
}

#[test]
fn the_gates_brief_lists_both_gated_transition_forms() {
    let ws = workspace(&item_type(
        "",
        "constraints:\n  - kind: transition_requires_checks\n    field: status\n    to_value: done\n    relationships: [VERIFIES]\n    direction: in\n  - kind: transition_requires_self_check\n    field: status\n    to_value: done\n    check_kind: x-review\n",
    ));
    let root = ws.path();
    assert!(
        run(
            root,
            &[
                "create",
                "--type",
                "item",
                "--title",
                "One",
                "--section",
                "body=b",
                "--metadata",
                "status=draft"
            ]
        )
        .status
        .success()
    );
    let brief =
        String::from_utf8_lossy(&run(root, &["gates", "--mem", "items"]).stdout).to_string();
    assert!(brief.contains("related via [VERIFIES] (in)"), "{brief}");
    assert!(
        brief.contains("confirming `x-review` check record on the entity itself"),
        "{brief}"
    );
    assert!(
        brief.contains(
            "`items--one` — status: draft — own `x-review` check unconfirmed (never_checked)"
        ),
        "{brief}"
    );
}

#[test]
fn a_number_field_pattern_refusal_names_number() {
    let ws = workspace(&item_type(
        "  - key: score\n    description: a score\n    field_type: number\n    value_pattern: \"[0-9]{2}\"\n",
        "",
    ));
    let out = run(
        ws.path(),
        &[
            "create",
            "--type",
            "item",
            "--title",
            "One",
            "--section",
            "body=b",
            "--metadata",
            "status=draft",
            "--metadata",
            "score=7",
            "--json",
        ],
    );
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(text.contains("number"), "{text}");
    assert!(!text.contains("String"), "{text}");
}

#[test]
fn due_help_lists_today() {
    let tmp = tempfile::TempDir::new().unwrap();
    let help = String::from_utf8_lossy(&run(tmp.path(), &["due", "--help"]).stdout).to_string();
    assert!(help.contains("--today"), "{help}");
}
