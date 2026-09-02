#![cfg(feature = "mem-repo")]
//! The health mem filter applies to every section and warning
//! (backlog-decisions plan B9): on two mounted mems that each carry one
//! anchor row and one warning of their own, `health --mem alpha` lists
//! only alpha's anchor row, only alpha's config entry and only alpha's
//! warning, and no section names beta except the mem rosters; CLI JSON and
//! MCP `structured_content` agree byte for byte; without a filter every
//! section carries both mems as before.

use std::fs;
use std::io::{BufRead, BufReader, Write as _};
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use tempfile::TempDir;

const INCLUDES: &str =
    "anchors,stale,integrity,dangling_links,checks,ledger,config,vital_signs,open_questions";

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// A folder mem pinned to the oldest `default` generation, so the engine
/// warns `SCHEMA_GENERATIONS_BEHIND` for it: one warning per mem.
fn write_old_default_mem(dir: &Path, slug: &str, title: &str) {
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    fs::write(
        dir.join(format!("{slug}.md")),
        format!(
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# {title}\n\n## Identity\n\nThe {title} surface.\n\n## Purpose\n\nExists.\n"
        ),
    )
    .unwrap();
}

/// Two mounted mems, each with one spec carrying one resolving file
/// anchor and each pinned to a generations-behind schema.
fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    for mem in ["alpha", "beta"] {
        let dir = tmp.path().join(format!("{mem}-mem"));
        fs::create_dir_all(&dir).unwrap();
        write_old_default_mem(&dir, "surface", mem);
    }
    init_real_mem_repo_from_disk(
        tmp.path(),
        &[
            (&tmp.path().join("alpha-mem"), "alpha"),
            (&tmp.path().join("beta-mem"), "beta"),
        ],
    );
    for mem in ["alpha", "beta"] {
        fs::write(tmp.path().join(format!("{mem}.txt")), format!("{mem}\n")).unwrap();
        memstead()
            .current_dir(tmp.path())
            .args([
                "update",
                &format!("{mem}--surface"),
                "--quiet",
                "--anchor",
                &format!(
                    r#"{{"artifact":"{mem}.txt","grain":"file","class":"anchored","hash_stability":"stable"}}"#
                ),
            ])
            .assert()
            .success();
        memstead()
            .current_dir(tmp.path())
            .args(["verify-anchors", "--mem", mem, "--quiet"])
            .assert()
            .success();
    }
    tmp
}

fn cli_health(root: &Path, mem: Option<&str>) -> serde_json::Value {
    let mut cmd = memstead();
    cmd.current_dir(root)
        .args(["health", "--include", INCLUDES, "--json", "--quiet"]);
    if let Some(m) = mem {
        cmd.args(["--mem", m]);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn mcp_health(root: &Path, mem: &str) -> serde_json::Value {
    let bin = assert_cmd::cargo::cargo_bin("memstead-mcp");
    let mut child = StdCommand::new(bin)
        .current_dir(root)
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
    let includes: Vec<String> = INCLUDES.split(',').map(|s| format!("\"{s}\"")).collect();
    send(&format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"memstead_health","arguments":{{"include":[{}],"mem":"{mem}"}}}}}}"#,
        includes.join(",")
    ));
    let reply = recv(2);
    let _ = child.kill();
    let _ = child.wait();
    reply["result"]["structuredContent"].clone()
}

fn warning_mems(v: &serde_json::Value) -> Vec<String> {
    v["warnings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|w| w["details"]["mem"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn keys(v: &serde_json::Value) -> Vec<String> {
    v.as_object()
        .map(|o| o.keys().filter(|k| !k.starts_with('_')).cloned().collect())
        .unwrap_or_default()
}

/// B9 AC1: every section and every warning scoped to alpha; CLI and MCP
/// agree byte for byte.
#[test]
fn a_mem_filtered_health_read_carries_only_that_mem() {
    let tmp = workspace();
    let cli = cli_health(tmp.path(), Some("alpha"));
    assert_eq!(keys(&cli["anchors"]), vec!["alpha"], "{}", cli["anchors"]);
    assert_eq!(cli["anchors"]["alpha"]["resolves"], 1);
    assert_eq!(keys(&cli["checks"]), vec!["alpha"]);
    assert_eq!(keys(&cli["open_questions"]), vec!["alpha"]);
    assert_eq!(keys(&cli["vital_signs"]), vec!["alpha"]);
    let config_mems: Vec<&str> = cli["mems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(config_mems, vec!["alpha"]);
    let mems = warning_mems(&cli);
    assert!(
        !mems.is_empty(),
        "alpha carries a warning of its own: {cli}"
    );
    assert!(mems.iter().all(|m| m == "alpha"), "{cli}");
    // No section names beta; only the mem rosters do.
    let mut scoped = cli.clone();
    let obj = scoped.as_object_mut().unwrap();
    obj.remove("writable_mems");
    obj.remove("read_mems");
    assert!(
        !scoped.to_string().contains("beta"),
        "beta rides along: {scoped}"
    );

    let mcp = mcp_health(tmp.path(), "alpha");
    assert_eq!(
        serde_json::to_string(&cli).unwrap(),
        serde_json::to_string(&mcp).unwrap()
    );
}

/// B9 AC1 refusal complement (the in-test half): without a filter every
/// section still carries both mems in the historical shape.
#[test]
fn an_unfiltered_read_carries_every_mem() {
    let tmp = workspace();
    let cli = cli_health(tmp.path(), None);
    assert_eq!(keys(&cli["anchors"]), vec!["alpha", "beta"]);
    let config_mems: Vec<&str> = cli["mems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(config_mems, vec!["alpha", "beta"]);
    let mut mems = warning_mems(&cli);
    mems.sort();
    mems.dedup();
    assert_eq!(mems, vec!["alpha", "beta"], "{cli}");
    assert!(cli.get("_mem_schema").is_none());
}
