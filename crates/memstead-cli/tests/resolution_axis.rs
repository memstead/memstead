#![cfg(feature = "mem-repo")]
// `memstead mem set-schema` ships only in the full build.

//! A type's `resolution` declaration (backlog-decisions plan B5): the
//! `open_questions` health axis lists, per mem, the open entities whose
//! condition section is empty (`resolution_missing`) and the open entities
//! whose condition nobody has checked under the declared kind
//! (`resolution_unchecked`), byte-identical between the CLI JSON and the
//! MCP `structured_content`; a closed entity is never listed; a check of
//! another `x-` kind does not count; and a declaration naming a section
//! the type does not declare refuses at `schema install` naming it.

use std::fs;
use std::io::{BufRead, BufReader, Write as _};
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

const MANIFEST: &str = r#"name: inquiry
version: 0.1.0
description: resolution fixture
when_to_use: tests
types:
  - question
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

fn question_type(condition_section: &str, check_kind: &str) -> String {
    format!(
        "name: question\ndescription: an open unknown\nwhen_to_use: Here\nsections:\n  - key: question\n    heading: Question\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\n  - key: condition\n    heading: Condition\n    required: false\n    search_weight: 5.0\n    catch_all: false\n    write_rules: []\nmetadata_fields:\n  - key: status\n    description: s\n    field_type: string\n    default_value: open\n    enum_values: [open, answered]\nresolution:\n  status_field: status\n  open_values: [open]\n  condition_section: {condition_section}\n  check_kind: {check_kind}\ntitle_weight: 100.0\ntext_fields:\n  - question\nhierarchy_relationship: _default\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - question\n  - condition\n  - status\nhealth_required_fields:\n  - question\nstaleness_threshold_days: 90\nwrite_rules: []\n"
    )
}

fn write_package(dir: &Path, condition_section: &str, check_kind: &str) {
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(dir.join("schema.yaml"), MANIFEST).unwrap();
    fs::write(
        dir.join("types").join("question.yaml"),
        question_type(condition_section, check_kind),
    )
    .unwrap();
}

/// A folder workspace whose mem `qs` pins a schema declaring `resolution`
/// on `question` (condition section `condition`, check kind `check_kind`).
fn workspace(check_kind: &str) -> TempDir {
    let ws = TempDir::new().unwrap();
    memstead()
        .current_dir(ws.path())
        .args([
            "init",
            "--name",
            "qs",
            "--schema",
            "default@1.3.0",
            "--quiet",
        ])
        .assert()
        .success();
    write_package(
        &ws.path()
            .join(".memstead")
            .join("schemas")
            .join("inquiry@0.1.0"),
        "condition",
        check_kind,
    );
    memstead()
        .current_dir(ws.path())
        .args(["mem", "set-schema", "qs", "inquiry@0.1.0", "--quiet"])
        .assert()
        .success();
    ws
}

fn question(ws: &Path, title: &str, status: &str, condition: Option<&str>) {
    let mut args: Vec<String> = vec![
        "create".into(),
        "--quiet".into(),
        "--type".into(),
        "question".into(),
        "--title".into(),
        title.into(),
        "--section".into(),
        "question=what?".into(),
        "--metadata".into(),
        format!("status={status}"),
        "--identity".into(),
        "author-one".into(),
        "--role".into(),
        "author".into(),
    ];
    if let Some(c) = condition {
        args.push("--section".into());
        args.push(format!("condition={c}"));
    }
    memstead().current_dir(ws).args(&args).assert().success();
}

fn check(ws: &Path, entity: &str, kind: &str, identity: &str) {
    memstead()
        .current_dir(ws)
        .args([
            "check",
            entity,
            "--verdict",
            "ok",
            "--kind",
            kind,
            "--method",
            "test",
            "--identity",
            identity,
            "--role",
            "checker",
            "--quiet",
        ])
        .assert()
        .success();
}

fn cli_axis(ws: &Path) -> serde_json::Value {
    let out = memstead()
        .current_dir(ws)
        .args(["health", "--include", "open_questions", "--json", "--quiet"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["open_questions"].clone()
}

fn mcp_axis(ws: &Path) -> serde_json::Value {
    let bin = assert_cmd::cargo::cargo_bin("memstead-mcp");
    let mut child = StdCommand::new(bin)
        .current_dir(ws)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("memstead-mcp spawns");
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut send = |line: &str| {
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    };
    let mut recv = |id: u64| -> serde_json::Value {
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 {
                panic!("mcp exited before answering id {id}");
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                && v["id"] == serde_json::json!(id)
            {
                return v;
            }
        }
    };
    send(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
    );
    recv(1);
    send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    send(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memstead_health","arguments":{"include":["open_questions"]}}}"#,
    );
    let reply = recv(2);
    let _ = child.kill();
    let _ = child.wait();
    reply["result"]["structuredContent"]["open_questions"].clone()
}

fn ids(axis: &serde_json::Value, kind: &str) -> Vec<String> {
    axis["qs"][kind]["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|i| i["id"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// B5 AC1: missing, unchecked, and checked read apart; the closed entity
/// is absent; CLI and MCP agree byte for byte.
#[test]
fn the_axis_reads_missing_unchecked_and_checked_apart() {
    let ws = workspace("verification");
    let root = ws.path();
    question(root, "No condition", "open", None);
    question(root, "Unchecked", "open", Some("when the benchmark lands"));
    question(root, "Checked", "open", Some("when the benchmark lands"));
    question(root, "Closed", "answered", None);
    check(root, "qs--checked", "verification", "checker-two");

    let cli = cli_axis(root);
    assert_eq!(
        ids(&cli, "resolution_missing"),
        vec!["qs--no-condition"],
        "{cli}"
    );
    assert_eq!(
        ids(&cli, "resolution_unchecked"),
        vec!["qs--unchecked"],
        "{cli}"
    );
    assert_eq!(
        cli["qs"]["resolution_unchecked"]["items"][0]["check_kind"],
        "verification"
    );
    assert_eq!(cli["qs"]["resolution_missing"]["count"], 1);
    assert!(!cli.to_string().contains("qs--closed"), "{cli}");
    assert!(!cli.to_string().contains("\"qs--checked\""), "{cli}");
    assert_eq!(cli["qs"]["total_open"], 2, "{cli}");

    let mcp = mcp_axis(root);
    assert_eq!(
        serde_json::to_string(&cli).unwrap(),
        serde_json::to_string(&mcp).unwrap()
    );
}

/// B5 AC1 refusal complement, second half: an `x-` check of a kind other
/// than the declared one does not count as checked; the declared `x-`
/// kind does.
#[test]
fn a_foreign_check_counts_only_under_the_declared_kind() {
    let ws = workspace("x-review");
    let root = ws.path();
    question(root, "Reviewed", "open", Some("when two people agree"));
    question(root, "Walked", "open", Some("when two people agree"));
    check(root, "qs--reviewed", "x-review", "checker-two");
    check(root, "qs--walked", "x-step-walk", "checker-two");
    let cli = cli_axis(root);
    assert_eq!(
        ids(&cli, "resolution_unchecked"),
        vec!["qs--walked"],
        "{cli}"
    );
    assert_eq!(
        cli["qs"]["resolution_unchecked"]["items"][0]["check_kind"],
        "x-review"
    );
}

/// B5 AC1 refusal complement, first half: a declaration naming a section
/// the type does not declare refuses at install, naming it.
#[test]
fn a_declaration_naming_an_undeclared_section_refuses_at_install() {
    let ws = TempDir::new().unwrap();
    memstead()
        .current_dir(ws.path())
        .args([
            "init",
            "--name",
            "qs",
            "--schema",
            "default@1.3.0",
            "--quiet",
        ])
        .assert()
        .success();
    let pkg = ws.path().join("bad-inquiry");
    write_package(&pkg, "nowhere", "verification");
    let out = memstead()
        .current_dir(ws.path())
        .args(["schema", "install", pkg.to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("resolution declaration is invalid"), "{text}");
    assert!(
        text.contains("`condition_section` names no section"),
        "{text}"
    );
    assert!(text.contains("nowhere"), "{text}");
    // `schema validate` says the same without a workspace.
    let out = memstead()
        .current_dir(ws.path())
        .args(["schema", "validate", pkg.to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}
