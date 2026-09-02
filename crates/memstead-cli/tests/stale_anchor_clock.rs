//! The stale axis defers to anchor state (backlog-decisions plan B7): with
//! the clock pinned 120 days ahead over a 90-day threshold, an entity whose
//! anchor resolves is absent from the stale list and named under
//! `anchor_fresh` with the anchor clock; an entity whose anchor drifted is
//! listed as `drifted` under the anchor clock, not as stale by age; an
//! entity with no anchor is listed by the day threshold exactly as before
//! (the three historical keys and nothing else); CLI JSON and MCP
//! `structured_content` agree byte for byte; and an anchor-less workspace
//! renders no anchor key at all.

use std::fs;
use std::io::{BufRead, BufReader, Write as _};
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};

use assert_cmd::Command;
use tempfile::TempDir;

/// A day 120 days past any entity written today (the fixture is created
/// at test time, so any pin far enough ahead crosses the 90-day threshold
/// of the default schema's `concept`).
const PINNED_TODAY: &str = "2030-01-01";

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn concept(ws: &Path, title: &str) {
    memstead()
        .current_dir(ws)
        .args([
            "create",
            "--quiet",
            "--type",
            "concept",
            "--title",
            title,
            "--section",
            "definition=x",
            "--section",
            "explanation=y",
            "--identity",
            "author-one",
            "--role",
            "author",
        ])
        .assert()
        .success();
}

fn anchor(ws: &Path, id: &str, artifact: &str) {
    memstead()
        .current_dir(ws)
        .args([
            "update",
            id,
            "--quiet",
            "--anchor",
            &format!(
                r#"{{"artifact":"{artifact}","grain":"file","class":"anchored","hash_stability":"stable"}}"#
            ),
        ])
        .assert()
        .success();
}

/// A folder workspace with three concepts: one whose file anchor resolves,
/// one whose file anchor drifted after the first verify backfilled its
/// hash, and one with no anchor.
fn workspace() -> TempDir {
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
    for t in ["Resolving", "Drifting", "Plain"] {
        concept(ws.path(), t);
    }
    fs::write(ws.path().join("r.txt"), "hello\n").unwrap();
    fs::write(ws.path().join("d.txt"), "hello\n").unwrap();
    anchor(ws.path(), "notes--resolving", "r.txt");
    anchor(ws.path(), "notes--drifting", "d.txt");
    memstead()
        .current_dir(ws.path())
        .args(["verify-anchors", "--mem", "notes", "--quiet"])
        .assert()
        .success();
    fs::write(ws.path().join("d.txt"), "changed\n").unwrap();
    ws
}

fn cli_health(ws: &Path, pin: Option<&str>) -> serde_json::Value {
    let mut cmd = memstead();
    cmd.current_dir(ws)
        .args(["health", "--include", "stale", "--json", "--quiet"]);
    if let Some(p) = pin {
        cmd.env("MEMSTEAD_TODAY", p);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn mcp_health(ws: &Path, pin: &str) -> serde_json::Value {
    let bin = assert_cmd::cargo::cargo_bin("memstead-mcp");
    let mut child = StdCommand::new(bin)
        .current_dir(ws)
        .env("MEMSTEAD_TODAY", pin)
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
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memstead_health","arguments":{"include":["stale"]}}}"#,
    );
    let reply = recv(2);
    let _ = child.kill();
    let _ = child.wait();
    reply["result"]["structuredContent"].clone()
}

fn ids(rows: &serde_json::Value) -> Vec<String> {
    rows.as_array()
        .map(|a| {
            a.iter()
                .map(|r| r["id"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// B7 AC1: resolving absent and fresh by the anchor clock, drifted listed
/// under the anchor clock, the anchor-less entity by the day threshold as
/// before; CLI and MCP agree byte for byte.
#[test]
fn anchored_entities_read_by_their_anchors() {
    let ws = workspace();
    let cli = cli_health(ws.path(), Some(PINNED_TODAY));
    let stale = &cli["stale"];
    assert_eq!(ids(stale), vec!["notes--drifting", "notes--plain"], "{cli}");
    let drifting = &stale[0];
    assert_eq!(drifting["clock"], "anchors");
    assert_eq!(drifting["anchor_state"], "drifted");
    let plain = &stale[1];
    assert!(
        plain.get("clock").is_none(),
        "day-threshold row names no clock key: {plain}"
    );
    assert!(plain.get("anchor_state").is_none());
    assert!(plain["days_since_modified"].as_u64().unwrap() > 90);
    let fresh = &cli["anchor_fresh"];
    assert_eq!(ids(fresh), vec!["notes--resolving"], "{cli}");
    assert_eq!(fresh[0]["clock"], "anchors");
    assert_eq!(fresh[0]["anchor_state"], "resolves");
    assert_eq!(cli["summary"]["total_stale"], 2);

    let mcp = mcp_health(ws.path(), PINNED_TODAY);
    for key in ["stale", "anchor_fresh"] {
        assert_eq!(
            serde_json::to_string(&cli[key]).unwrap(),
            serde_json::to_string(&mcp[key]).unwrap(),
            "{key}"
        );
    }
    assert_eq!(cli["summary"]["total_stale"], mcp["summary"]["total_stale"]);
}

/// A drifted anchor lists the entity as its own condition whatever its
/// age: with no pin (entities written today) the drifted one is still
/// listed, the resolving and the plain ones are not, and nothing is fresh
/// by anchor because the threshold never spoke.
#[test]
fn a_drifted_anchor_lists_the_entity_regardless_of_age() {
    let ws = workspace();
    let cli = cli_health(ws.path(), None);
    assert_eq!(ids(&cli["stale"]), vec!["notes--drifting"], "{cli}");
    assert_eq!(cli["stale"][0]["anchor_state"], "drifted");
    assert!(cli.get("anchor_fresh").is_none(), "{cli}");
}

/// B7 AC1 refusal complement: an anchor-less workspace renders the
/// historical shape and nothing else, pinned or not.
#[test]
fn an_anchorless_workspace_renders_as_before() {
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
    concept(ws.path(), "Plain");
    let unpinned = cli_health(ws.path(), None);
    assert_eq!(unpinned["stale"], serde_json::json!([]));
    assert!(unpinned.get("anchor_fresh").is_none());
    let pinned = cli_health(ws.path(), Some(PINNED_TODAY));
    let rows = pinned["stale"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let keys: Vec<&String> = rows[0].as_object().unwrap().keys().collect();
    assert_eq!(keys, vec!["days_since_modified", "id", "title"], "{pinned}");
    assert!(pinned.get("anchor_fresh").is_none());
    let text = String::from_utf8(
        memstead()
            .current_dir(ws.path())
            .env("MEMSTEAD_TODAY", PINNED_TODAY)
            .args(["health", "--include", "stale", "--quiet"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(!text.contains("anchor clock"), "{text}");
}
