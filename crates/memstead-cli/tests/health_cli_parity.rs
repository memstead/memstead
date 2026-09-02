#![cfg(feature = "mem-repo")]
// `memstead mem set-schema` ships only in the full build.

//! `memstead health` composes its report through the engine's
//! `compose_health` (backlog-engine plan A7): the JSON output is
//! byte-identical to the MCP `memstead_health` `structuredContent` for every
//! include key and under a `--mem` filter, and the CLI's markdown rendering
//! is unchanged against the fixtures recorded before the composer took
//! over (`tests/fixtures/health-markdown/`).
//!
//! Re-record the markdown fixtures deliberately, never to make a red test
//! green: `MEMSTEAD_RECORD_HEALTH_MARKDOWN=1 cargo test -p memstead-cli
//! --test health_cli_parity`.

use std::fs;
use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command as StdCommand, Stdio};

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

const MANIFEST: &str = r#"name: parity
version: 0.1.0
description: health parity fixture schema
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

fn type_yaml(name: &str) -> String {
    format!(
        "name: {name}\ndescription: t\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\n  - key: details\n    heading: Details\n    required: false\n    search_weight: 5.0\n    catch_all: false\n    write_rules: []\nmetadata_fields: []\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: _default\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\n  - details\nhealth_required_fields:\n  - body\n  - details\nstaleness_threshold_days: 90\nwrite_rules: []\n{}",
        if name == "concept" {
            "last_resort: true\n"
        } else {
            ""
        }
    )
}

fn create(ws: &Path, mem: &str, ty: &str, title: &str, body: &str, relations: &[&str]) {
    let section = format!("body={body}");
    let mut args = vec![
        "create",
        "--quiet",
        "--mem",
        mem,
        "--type",
        ty,
        "--title",
        title,
        "--section",
        &section,
    ];
    for r in relations {
        args.push("--relation");
        args.push(r);
    }
    memstead().current_dir(ws).args(&args).assert().success();
}

/// A mem-repo workspace with two git-branch mems on one schema: `notes`
/// carries a hub, two dependants, a stub target and
/// an orphan; `side` carries one entity so a `--mem` filter has something
/// to exclude.
fn seed() -> TempDir {
    let ws = TempDir::new().unwrap();
    let root = ws.path();
    memstead()
        .current_dir(root)
        .args(["mem-repo", "init", "--quiet", "--no-gitignore"])
        .assert()
        .success();
    let dir = root.join(".memstead").join("schemas").join("parity@0.1.0");
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(dir.join("schema.yaml"), MANIFEST).unwrap();
    fs::write(dir.join("types").join("spec.yaml"), type_yaml("spec")).unwrap();
    fs::write(dir.join("types").join("concept.yaml"), type_yaml("concept")).unwrap();
    for mem in ["notes", "side"] {
        memstead()
            .current_dir(root)
            .args([
                "workspace",
                "allow-create",
                mem,
                "--schema",
                "parity@0.1.0",
                "--quiet",
            ])
            .assert()
            .success();
        memstead()
            .current_dir(root)
            .args(["mem", "init", mem, "--schema", "parity@0.1.0", "--quiet"])
            .assert()
            .success();
    }
    create(root, "notes", "concept", "Hub", "the hub", &[]);
    create(
        root,
        "notes",
        "spec",
        "Alpha",
        "a",
        &["DEPENDS_ON:notes--hub"],
    );
    create(
        root,
        "notes",
        "spec",
        "Beta",
        "b",
        &["DEPENDS_ON:notes--hub", "DEPENDS_ON:notes--ghost"],
    );
    create(
        root,
        "notes",
        "spec",
        "Gamma",
        "g",
        &["DEPENDS_ON:notes--alpha"],
    );
    create(root, "notes", "concept", "Lonely", "alone", &[]);
    create(root, "side", "spec", "Aside", "s", &[]);
    ws
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("health-markdown")
}

/// The temp root appears in `config`'s projection; fold both its raw and
/// its canonical spelling to one token.
fn normalise(text: &str, root: &Path) -> String {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    text.replace(&canonical.display().to_string(), "<ROOT>")
        .replace(&root.display().to_string(), "<ROOT>")
}

fn cli(ws: &Path, args: &[&str]) -> std::process::Output {
    let mut all = vec!["health", "--quiet"];
    all.extend_from_slice(args);
    memstead().current_dir(ws).args(&all).output().unwrap()
}

fn cli_markdown(ws: &Path, args: &[&str]) -> String {
    let out = cli(ws, args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    normalise(&String::from_utf8_lossy(&out.stdout), ws)
}

fn cli_json(ws: &Path, args: &[&str]) -> serde_json::Value {
    let mut all = vec!["--json"];
    all.extend_from_slice(args);
    let out = cli(ws, &all);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn spawn(ws: &Path) -> Self {
        let bin = assert_cmd::cargo::cargo_bin("memstead-mcp");
        let mut child = StdCommand::new(bin)
            .current_dir(ws)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("memstead-mcp spawns");
        let stdin = child.stdin.take().unwrap();
        let reader = BufReader::new(child.stdout.take().unwrap());
        let mut m = Self {
            child,
            stdin,
            reader,
            next_id: 1,
        };
        m.send(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        );
        m.recv(1);
        m.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        m.next_id = 2;
        m
    }

    fn send(&mut self, line: &str) {
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    fn recv(&mut self, id: u64) -> serde_json::Value {
        loop {
            let mut line = String::new();
            if self.reader.read_line(&mut line).unwrap() == 0 {
                panic!("mcp exited before answering id {id}");
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                && v["id"] == serde_json::json!(id)
            {
                return v;
            }
        }
    }

    fn health(&mut self, arguments: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        let req = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": "memstead_health", "arguments": arguments }
        });
        self.send(&req.to_string());
        let reply = self.recv(id);
        reply["result"]["structuredContent"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn include_keys() -> Vec<&'static str> {
    memstead_base::ops::health::HEALTH_INCLUDE_KEYS.to_vec()
}

/// A7 AC1 (JSON half): for no include, every include key on its own, all
/// keys at once, and a `--mem` filter, the CLI's `--json` bytes equal the
/// MCP tool's `structuredContent` bytes.
#[test]
fn cli_json_is_byte_identical_to_the_mcp_structured_content() {
    let ws = seed();
    let root = ws.path();
    let mut mcp = Mcp::spawn(root);
    let mut cases: Vec<(Vec<&str>, Option<&str>)> = vec![(vec![], None)];
    for k in include_keys() {
        cases.push((vec![k], None));
    }
    cases.push((include_keys(), None));
    cases.push((vec!["orphans", "integrity", "anchors"], Some("notes")));
    cases.push((vec!["most_connected"], Some("side")));
    for (include, mem) in cases {
        let mut args: Vec<&str> = Vec::new();
        let joined = include.join(",");
        if !include.is_empty() {
            args.push("--include");
            args.push(&joined);
        }
        if let Some(m) = mem {
            args.push("--mem");
            args.push(m);
        }
        let cli_v = cli_json(root, &args);
        let mut arguments = serde_json::json!({ "include": include });
        if let Some(m) = mem {
            arguments["mem"] = serde_json::json!(m);
        }
        let mcp_v = mcp.health(arguments);
        assert_eq!(
            serde_json::to_string(&cli_v).unwrap(),
            serde_json::to_string(&mcp_v).unwrap(),
            "include={include:?} mem={mem:?}"
        );
    }
}

/// A7 AC1 (markdown half): the CLI's markdown for every include key is
/// unchanged against the fixtures recorded before the refactor.
#[test]
fn cli_markdown_matches_the_recorded_fixtures() {
    let ws = seed();
    let root = ws.path();
    let record = std::env::var_os("MEMSTEAD_RECORD_HEALTH_MARKDOWN").is_some();
    let dir = fixture_dir();
    if record {
        fs::create_dir_all(&dir).unwrap();
    }
    let mut cases: Vec<(String, Vec<&str>)> = vec![("none".to_string(), vec![])];
    for k in include_keys() {
        cases.push((k.to_string(), vec![k]));
    }
    let all = include_keys();
    cases.push(("all".to_string(), all));
    for (name, include) in cases {
        let joined = include.join(",");
        let args: Vec<&str> = if include.is_empty() {
            vec![]
        } else {
            vec!["--include", &joined]
        };
        let got = cli_markdown(root, &args);
        let path = dir.join(format!("{name}.md"));
        if record {
            fs::write(&path, &got).unwrap();
            continue;
        }
        let want = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {} unreadable: {e}", path.display()));
        assert_eq!(got, want, "markdown drifted for include `{name}`");
    }
}

/// A7 AC1 refusal complement: `--mem` naming no mounted mem refuses with
/// `UNKNOWN_MEM` and names the writable roster.
#[test]
fn unknown_mem_refuses_and_names_the_roster() {
    let ws = seed();
    let out = memstead()
        .current_dir(ws.path())
        .args(["health", "--mem", "does-not-exist", "--json", "--quiet"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let env: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(env["code"], "UNKNOWN_MEM", "{env}");
    let text = env.to_string();
    assert!(text.contains("does-not-exist"), "{env}");
    assert!(text.contains("notes") && text.contains("side"), "{env}");
}

/// A7 AC1 source scan: the CLI crate composes no health axis itself.
#[test]
fn cli_health_command_calls_no_per_axis_composer() {
    let src = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("commands")
            .join("health.rs"),
    )
    .unwrap();
    for banned in [
        "health_anchors_axis",
        "health_open_questions_axis",
        "health_vital_signs_axis",
        "health_signals_axis",
        "health_labelling_axis",
        "health_checks_axis",
        "health_stale_derivations_axis",
        "ledger_reconciliation",
        "collect_tag_distribution",
        "conformance_findings",
        "consistency_findings",
        "body_observations(",
        "missing_required_outgoing(",
        "constraint_findings(",
        "schema_format_defects(",
        "config_projection(",
        "most_connected(",
        "dangling_links(",
    ] {
        assert!(
            !src.contains(banned),
            "commands/health.rs must not call `{banned}`: the engine composer owns the axes"
        );
    }
    assert!(
        src.contains("compose_health("),
        "commands/health.rs must build its report through compose_health"
    );
}
