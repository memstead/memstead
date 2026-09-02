#![cfg(feature = "mem-repo")]
// `memstead projection init` and the MCP counterpart ship only in the
// full build.

//! The `vital_signs` health axis (A6): five model-truth signals per mem,
//! each a count plus a capped list with a `more` remainder, never a
//! verdict; byte-identical between the CLI JSON and the MCP
//! `structured_content`; the last-resort type read from the schema's
//! declaration and reported `not_declared` where absent; a schema with
//! two last-resort types refused at load.

use std::fs;
use std::io::{BufRead, BufReader, Write as _};
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

const MANIFEST: &str = r#"name: vitals
version: 0.1.0
description: vital-signs fixture schema
when_to_use: tests
types:
  - spec
  - concept
relationships:
  mode: strict
  definitions:
    - name: DEPENDS_ON
      description: d
      default_weight: 2.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 0.2
  seed: 42
"#;

fn type_yaml(name: &str, last_resort: bool) -> String {
    format!(
        "name: {name}\ndescription: t\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\n  - key: details\n    heading: Details\n    required: false\n    search_weight: 5.0\n    catch_all: false\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: _default\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\n  - details\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n{}",
        if last_resort {
            "last_resort: true\n"
        } else {
            ""
        }
    )
}

fn write_schema(dir: &Path, spec_last_resort: bool, concept_last_resort: bool) {
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(dir.join("schema.yaml"), MANIFEST).unwrap();
    fs::write(
        dir.join("types").join("spec.yaml"),
        type_yaml("spec", spec_last_resort),
    )
    .unwrap();
    fs::write(
        dir.join("types").join("concept.yaml"),
        type_yaml("concept", concept_last_resort),
    )
    .unwrap();
}

/// A folder mem `notes` on the fixture schema (spec last-resort), with a
/// git source tree under `src/` and a codebase binding over it.
fn seed(declare_last_resort: bool) -> TempDir {
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
    write_schema(
        &ws.path()
            .join(".memstead")
            .join("schemas")
            .join("vitals@0.1.0"),
        declare_last_resort,
        false,
    );
    memstead()
        .current_dir(ws.path())
        .args(["mem", "set-schema", "notes", "vitals@0.1.0", "--quiet"])
        .assert()
        .success();
    let st = StdCommand::new("git")
        .args(["init", "-q"])
        .current_dir(ws.path())
        .status()
        .unwrap();
    assert!(st.success());
    fs::create_dir_all(ws.path().join("src")).unwrap();
    ws
}

fn create(ws: &Path, ty: &str, slug_title: &str, relations: &[&str]) {
    let mut args = vec![
        "create",
        "--quiet",
        "--type",
        ty,
        "--title",
        slug_title,
        "--section",
        "body=x",
    ];
    for r in relations {
        args.push("--relation");
        args.push(r);
    }
    memstead().current_dir(ws).args(&args).assert().success();
}

fn anchor(ws: &Path, id: &str, artifact: &str, class: &str) {
    let a = format!(
        r#"{{"artifact":"{artifact}","grain":"file","class":"{class}","hash_stability":"stable"}}"#
    );
    memstead()
        .current_dir(ws)
        .args(["update", id, "--quiet", "--anchor", &a])
        .assert()
        .success();
}

fn cli_axis(ws: &Path) -> serde_json::Value {
    let out = memstead()
        .current_dir(ws)
        .args(["health", "--include", "vital_signs", "--json", "--quiet"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["vital_signs"].clone()
}

/// The MCP counterpart over stdio: `memstead_health` with
/// `include: ["vital_signs"]`, the `structured_content` payload's axis.
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
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
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
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memstead_health","arguments":{"include":["vital_signs"]}}}"#,
    );
    let reply = recv(2);
    let _ = child.kill();
    let _ = child.wait();
    reply["result"]["structuredContent"]["vital_signs"].clone()
}

/// A6 AC1: each signal once on a fixture; CLI JSON and MCP
/// `structured_content` byte-identical.
#[test]
fn axis_reports_the_five_signals_from_a_fixture() {
    let ws = seed(true);
    let root = ws.path();
    // A community of ten: nine spec around one concept hub (a star plus
    // a chain, dense enough for the partition to keep it whole).
    create(root, "concept", "Beta", &[]);
    let names: Vec<String> = (1..=9).map(|i| format!("Alpha {i}")).collect();
    create(root, "spec", &names[0], &["DEPENDS_ON:notes--beta"]);
    for (i, name) in names.iter().enumerate().skip(1) {
        let prev = format!("DEPENDS_ON:notes--alpha-{i}");
        create(root, "spec", name, &["DEPENDS_ON:notes--beta", &prev]);
    }
    // The hub links back so it is not itself zero-outgoing.
    memstead()
        .current_dir(root)
        .args([
            "relate",
            "notes--beta",
            "DEPENDS_ON",
            "notes--alpha-1",
            "--quiet",
        ])
        .assert()
        .success();
    // A subject with three zero-outgoing entities under it.
    create(root, "spec", "Zed One", &[]);
    create(root, "spec", "Zed Two", &[]);
    create(root, "spec", "Zed Three", &[]);
    create(
        root,
        "concept",
        "Subject",
        &[
            "DEPENDS_ON:notes--zed-one",
            "DEPENDS_ON:notes--zed-two",
            "DEPENDS_ON:notes--zed-three",
        ],
    );
    // Sources: one large unowned file, one file claimed by two entities
    // and owned by none, one owned file.
    fs::write(root.join("src").join("big.rs"), "x".repeat(9_000)).unwrap();
    fs::write(root.join("src").join("shared.rs"), "fn s() {}\n").unwrap();
    fs::write(root.join("src").join("owned.rs"), "fn o() {}\n").unwrap();
    memstead()
        .current_dir(root)
        .args([
            "projection",
            "init",
            "--mem",
            "notes",
            "--source",
            "src",
            "--medium-type",
            "codebase",
            "--name",
            "src",
            "--quiet",
        ])
        .assert()
        .success();
    anchor(root, "notes--alpha-1", "src/shared.rs", "informed-by");
    anchor(root, "notes--alpha-2", "src/shared.rs", "informed-by");
    anchor(root, "notes--alpha-3", "src/owned.rs", "anchored");
    // One declared section carried empty.
    memstead()
        .current_dir(root)
        .args([
            "update",
            "notes--beta",
            "--quiet",
            "--auto-hash",
            "--section",
            "details= ",
        ])
        .assert()
        .success();

    let cli = cli_axis(root);
    let sig = &cli["notes"];
    // 1. type share: the ten-entity community at 9/10.
    assert_eq!(
        sig["type_share_by_community"]["status"], "declared",
        "{sig}"
    );
    assert_eq!(sig["type_share_by_community"]["last_resort_type"], "spec");
    let rows = sig["type_share_by_community"]["items"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|r| r["entities"] == 10 && r["on_last_resort_type"] == 9),
        "{rows:?}"
    );
    // 2. the large unowned file, largest first with its size.
    assert_eq!(sig["unclaimed_source_files"]["status"], "enumerated");
    let unclaimed = sig["unclaimed_source_files"]["items"].as_array().unwrap();
    assert_eq!(unclaimed[0]["artifact"], "src/big.rs", "{unclaimed:?}");
    assert_eq!(unclaimed[0]["bytes"], 9_000);
    assert!(
        unclaimed
            .iter()
            .all(|u| u["artifact"] != "src/shared.rs" && u["artifact"] != "src/owned.rs")
    );
    // 3. the contested unowned file.
    assert_eq!(sig["contested_unowned_files"]["count"], 1, "{sig}");
    assert_eq!(
        sig["contested_unowned_files"]["items"][0]["artifact"],
        "src/shared.rs"
    );
    // 4. three zero-outgoing entities folded into the subject's community.
    assert_eq!(sig["zero_outgoing_entities"]["entities"], 3, "{sig}");
    let groups = sig["zero_outgoing_entities"]["items"].as_array().unwrap();
    assert_eq!(
        groups.len(),
        1,
        "one community, not three singletons: {groups:?}"
    );
    assert_ne!(groups[0]["community"], "unplaced");
    assert_eq!(groups[0]["count"], 3);
    // 5. the empty declared section.
    assert_eq!(sig["empty_declared_sections"]["count"], 1, "{sig}");
    assert_eq!(
        sig["empty_declared_sections"]["items"][0]["id"],
        "notes--beta"
    );
    assert_eq!(
        sig["empty_declared_sections"]["items"][0]["section"],
        "details"
    );
    // No verdict, no threshold, no recommendation anywhere in the payload.
    let text = cli.to_string();
    for word in ["verdict", "threshold", "recommend"] {
        assert!(!text.contains(word), "{word} in payload: {text}");
    }
    assert_eq!(cli["_item_cap"], 20);

    // The MCP counterpart, byte for byte.
    let mcp = mcp_axis(root);
    assert_eq!(
        serde_json::to_string(&cli).unwrap(),
        serde_json::to_string(&mcp).unwrap()
    );
}

/// A6 AC1, refusal complement: a fixture with none of the signals reports
/// every count as zero and no list items.
#[test]
fn quiet_fixture_reports_zero_counts_and_no_lists() {
    let ws = seed(true);
    let root = ws.path();
    create(root, "concept", "One", &[]);
    create(root, "concept", "Two", &["DEPENDS_ON:notes--one"]);
    create(root, "concept", "Three", &["DEPENDS_ON:notes--two"]);
    create(root, "concept", "Four", &["DEPENDS_ON:notes--three"]);
    // One's outgoing edge to close the loop keeps every entity linked.
    memstead()
        .current_dir(root)
        .args([
            "relate",
            "notes--one",
            "DEPENDS_ON",
            "notes--four",
            "--quiet",
        ])
        .assert()
        .success();
    let sig = cli_axis(root)["notes"].clone();
    assert_eq!(sig["type_share_by_community"]["status"], "declared");
    for row in sig["type_share_by_community"]["items"].as_array().unwrap() {
        assert_eq!(row["on_last_resort_type"], 0, "{sig}");
    }
    assert_eq!(sig["unclaimed_source_files"]["status"], "no_bound_source");
    assert_eq!(sig["contested_unowned_files"]["count"], 0);
    assert!(
        sig["contested_unowned_files"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(sig["zero_outgoing_entities"]["entities"], 0);
    assert!(
        sig["zero_outgoing_entities"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(sig["empty_declared_sections"]["count"], 0);
    assert!(
        sig["empty_declared_sections"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// A6 AC2: no declaration, no guess — `not_declared`; and two
/// declarations refuse at load with the shape named.
#[test]
fn last_resort_is_read_from_the_declaration_and_two_are_refused() {
    let ws = seed(false);
    let root = ws.path();
    create(root, "spec", "One", &[]);
    let sig = cli_axis(root)["notes"].clone();
    assert_eq!(
        sig["type_share_by_community"]["status"], "not_declared",
        "{sig}"
    );
    assert!(sig["type_share_by_community"].get("items").is_none());

    let two = root.join("two-last-resorts");
    write_schema(&two, true, true);
    memstead()
        .current_dir(root)
        .args(["schema", "validate", two.to_str().unwrap(), "--quiet"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("more than one last-resort type"))
        .stderr(predicates::str::contains("concept, spec"));
}
