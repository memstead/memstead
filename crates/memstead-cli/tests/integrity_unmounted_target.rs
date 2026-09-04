//! The integrity axis blames no grant after an unmount (backlog-decisions
//! plan B8): a cross-mem edge whose target mem is no longer mounted is
//! reported exactly once, as the dangling finding, and never as
//! `CROSS_MEM_EDGE_UNGRANTED` while the grant table still names the pair;
//! CLI JSON and MCP `structured_content` agree byte for byte. The refusal
//! complement: with the target mounted and the grant revoked, the edge
//! yields `CROSS_MEM_EDGE_UNGRANTED` with its cause and repair hint exactly
//! as before.

use std::fs;
use std::io::{BufRead, BufReader, Write as _};
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};

use assert_cmd::Command;
use memstead_base::{FileWorkspaceStore, WorkspaceStoreAdapter};
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// A folder mem with one entity under the built-in `default@1.0.0`.
fn write_default_mem(dir: &Path, slug: &str, title: &str) {
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
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# {title}\n\n## Identity\n\nThe anchor of {title}.\n\n## Purpose\n\nExists.\n"
        ),
    )
    .unwrap();
}

/// Two mounted mems, a declared grant `alpha` → `beta`, and one cross-mem
/// edge `alpha--source` DEPENDS_ON `beta--target` written under it.
fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("alpha-mem");
    let b = tmp.path().join("beta-mem");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    write_default_mem(&a, "seed", "Alpha");
    write_default_mem(&b, "target", "Target");
    init_real_mem_repo_from_disk(tmp.path(), &[(&a, "alpha"), (&b, "beta")]);
    memstead()
        .current_dir(tmp.path())
        .args(["workspace", "grant-cross-link", "alpha", "beta"])
        .assert()
        .success();
    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--mem",
            "alpha",
            "--title",
            "Source",
            "--type",
            "spec",
            "--section",
            "identity=Depends on beta.",
            "--section",
            "purpose=The referrer.",
            "--metadata",
            "level=M0",
            "--relation",
            "DEPENDS_ON:beta--target",
        ])
        .assert()
        .success();
    tmp
}

/// Drop `beta` from the mount store: the shape an out-of-band unmount
/// leaves behind (a mount whose backing vanished and was pruned), which
/// no lifecycle verb can produce because they refuse while an edge or a
/// grant still names the target.
fn unmount_beta(root: &Path) {
    let store = FileWorkspaceStore::new();
    let mut ws = store.load(root).unwrap();
    ws.mounts.retain(|m| m.mem != "beta");
    store.save_state(root, &ws).unwrap();
}

fn cli_integrity(root: &Path) -> serde_json::Value {
    let out = memstead()
        .current_dir(root)
        .args([
            "health",
            "--include",
            "integrity",
            "--mem",
            "alpha",
            "--json",
            "--quiet",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn mcp_integrity(root: &Path) -> serde_json::Value {
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
    send(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memstead_health","arguments":{"include":["integrity"],"mem":"alpha"}}}"#,
    );
    let reply = recv(2);
    let _ = child.kill();
    let _ = child.wait();
    reply["result"]["structuredContent"].clone()
}

fn codes(v: &serde_json::Value) -> Vec<String> {
    v["findings"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| f["code"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// B8 AC1: after the unmount the edge is reported exactly once, as the
/// dangling finding, with no grant finding beside it; CLI and MCP agree.
#[test]
fn an_unmounted_target_yields_one_dangling_finding_and_no_grant_finding() {
    let tmp = workspace();
    // With both mounted and the grant standing: clean on this axis.
    assert_eq!(codes(&cli_integrity(tmp.path())), Vec::<String>::new());

    unmount_beta(tmp.path());
    let cli = cli_integrity(tmp.path());
    assert_eq!(
        codes(&cli),
        vec!["DANGLING_RELATION_TARGET_MISSING"],
        "{cli}"
    );
    let row = &cli["findings"][0];
    assert_eq!(row["id"], "alpha--source");
    assert_eq!(row["detail"]["target_id"], "beta--target");
    assert_eq!(
        row["detail"]["repair"],
        "remove the relationship row, or create the target entity"
    );
    assert!(
        !cli.to_string().contains("CROSS_MEM_EDGE_UNGRANTED"),
        "{cli}"
    );

    let mcp = mcp_integrity(tmp.path());
    assert_eq!(
        serde_json::to_string(&cli["findings"]).unwrap(),
        serde_json::to_string(&mcp["findings"]).unwrap()
    );
}

/// B8 AC1 refusal complement: with the target mounted and the grant
/// revoked, the edge yields `CROSS_MEM_EDGE_UNGRANTED` with its cause and
/// repair hint exactly as before the plan.
#[test]
fn a_revoked_grant_on_a_mounted_target_still_yields_the_grant_finding() {
    let tmp = workspace();
    memstead()
        .current_dir(tmp.path())
        .args(["workspace", "revoke-cross-link", "alpha", "beta"])
        .assert()
        .success();
    let cli = cli_integrity(tmp.path());
    assert_eq!(codes(&cli), vec!["CROSS_MEM_EDGE_UNGRANTED"], "{cli}");
    let row = &cli["findings"][0];
    assert_eq!(row["id"], "alpha--source");
    assert_eq!(row["detail"]["from_mem"], "alpha");
    assert_eq!(row["detail"]["to_mem"], "beta");
    assert_eq!(
        row["detail"]["cause"],
        "no cross-mem grant permits this pair"
    );
    assert!(
        row["detail"]["repair"]
            .as_str()
            .unwrap()
            .starts_with("grant the pair with `memstead workspace grant-cross-link`"),
        "{row}"
    );
}
