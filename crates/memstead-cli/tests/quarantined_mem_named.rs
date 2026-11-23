//! Every verb that names a quarantined mem, or an entity of one, says
//! that the mem is out of service: `MEM_QUARANTINED` with the reason and
//! the repair, never `UNKNOWN_MEM` and never `ENTITY_NOT_FOUND`.
//!
//! The fixture: a workspace with the mem `live` in service and the
//! folder mem `held`, whose schema pin is made to resolve nowhere, so a
//! boot quarantines it and its roster row stays.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// Everything a command printed, on both streams, and whether it
/// succeeded.
fn run(ws: &Path, args: &[&str]) -> (bool, String) {
    let out = memstead()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        ),
    )
}

fn ok(ws: &Path, args: &[&str]) {
    let (succeeded, printed) = run(ws, args);
    assert!(
        succeeded,
        "`memstead {}` failed:\n{printed}",
        args.join(" ")
    );
}

fn spec(ws: &Path, mem: &str, title: &str) {
    ok(
        ws,
        &[
            "create",
            "--mem",
            mem,
            "--title",
            title,
            "--type",
            "spec",
            "--metadata",
            "level=M0",
            "--section",
            "identity=i",
            "--section",
            "purpose=p",
        ],
    );
}

/// A workspace with `live` in service and the folder mem `held`, which
/// `quarantined` puts out of service by a schema pin that resolves
/// nowhere.
fn workspace(root: &Path, quarantined: bool) -> PathBuf {
    let ws = root.join("ws");
    fs::create_dir_all(&ws).unwrap();
    ok(&ws, &["mem-repo", "init", "."]);
    ok(&ws, &["workspace", "allow-create", "--schema", "*", "*"]);
    ok(&ws, &["mem", "init", "live", "--schema", "default@1.3.0"]);
    ok(
        &ws,
        &[
            "mem",
            "init",
            "held",
            "--storage",
            "folder",
            "--schema",
            "default@1.3.0",
        ],
    );
    spec(&ws, "live", "Alive");
    spec(&ws, "held", "Kept");
    ok(&ws, &["workspace", "grant-cross-link", "held", "live"]);
    ok(&ws, &["workspace", "grant-cross-link", "live", "held"]);
    if quarantined {
        let config = ws.join("held/.memstead/config.json");
        let pinned = fs::read_to_string(&config)
            .unwrap()
            .replace("default@1.3.0", "nosuch@9.9.9");
        fs::write(&config, pinned).unwrap();
    }
    ws
}

const NEW_SPEC: [&str; 11] = [
    "--title",
    "New",
    "--type",
    "spec",
    "--metadata",
    "level=M0",
    "--section",
    "identity=i",
    "--section",
    "purpose=p",
    "--json",
];

/// The calls that name the mem, with the mem's name left open.
fn mem_calls(mem: &str) -> Vec<Vec<String>> {
    let mut calls: Vec<Vec<&str>> = vec![
        vec!["search", "--mem", mem, "kept"],
        vec!["list", "--mem", mem],
        vec!["export", "--mem", mem, "--format", "llms-txt"],
        vec!["export", "--mem", mem],
        vec!["changes", "--mem", mem, "--since", ""],
        vec!["anchors", "--mem", mem],
        vec!["health", "--mem", mem],
        vec!["type", "list", "--mem", mem],
        vec!["overview", "--mem", mem],
        vec!["due", "--mem", mem],
        vec!["gates", "--mem", mem],
        vec!["uninstall", mem],
        vec!["mem", "rename", mem, "elsewhere", "--operator-mode"],
        vec!["mem", "delete", mem, "--operator-mode"],
    ];
    let mut create = vec!["create", "--mem", mem];
    create.extend(NEW_SPEC);
    calls.push(create);
    calls
        .into_iter()
        .map(|c| c.into_iter().map(str::to_string).collect())
        .collect()
}

/// The calls that name an entity of the quarantined mem and answer for
/// the mem.
fn entity_calls() -> Vec<Vec<&'static str>> {
    const E: &str = "held--kept";
    vec![
        vec!["entity", E],
        vec!["search", "--related-to", E],
        vec!["relations", E],
        vec!["context", E],
        vec!["anchors", E],
        vec![
            "update",
            E,
            "--expected-hash",
            "0000000000000000",
            "--section",
            "purpose=q",
        ],
        vec!["update", E, "--auto-hash", "--section", "purpose=q"],
        vec!["update", E, "--force", "--section", "purpose=q"],
        vec!["delete", E],
        vec!["rename", E, "Renamed", "--auto-hash"],
        vec!["rename", E, "Renamed", "--force"],
        vec!["retype", E, "--type", "spec", "--auto-hash"],
        vec!["retype", E, "--type", "spec", "--force"],
        vec!["relate", E, "DEPENDS_ON", "live--alive"],
        vec!["check", E, "--verdict", "ok", "--method", "m"],
    ]
}

/// The calls that read the whole workspace: they refuse `MEM_VIEW_STALE`
/// with the mem named as out of service, by design.
fn workspace_wide_calls() -> Vec<Vec<&'static str>> {
    vec![
        vec!["retype", "held--kept", "--type", "spec"],
        vec!["rename", "held--kept", "Renamed"],
        vec!["relate", "live--alive", "DEPENDS_ON", "held--kept"],
    ]
}

#[test]
fn every_verb_names_a_quarantined_mem_as_out_of_service() {
    let root = TempDir::new().unwrap();
    let ws = workspace(root.path(), true);
    let (_, listed) = run(&ws, &["mem", "list"]);
    assert!(listed.contains("SCHEMA_NOT_FOUND"), "{listed}");

    let by_mem = mem_calls("held");
    let by_entity: Vec<Vec<String>> = entity_calls()
        .into_iter()
        .map(|c| c.into_iter().map(str::to_string).collect())
        .collect();
    for call in by_mem.iter().chain(by_entity.iter()) {
        let args: Vec<&str> = call.iter().map(String::as_str).collect();
        let (succeeded, printed) = run(&ws, &args);
        let name = args.join(" ");
        assert!(!succeeded, "`{name}` answered: {printed}");
        assert!(printed.contains("MEM_QUARANTINED"), "`{name}`: {printed}");
        assert!(printed.contains("held"), "`{name}`: {printed}");
        assert!(printed.contains("SCHEMA_NOT_FOUND"), "`{name}`: {printed}");
        assert!(
            printed.contains("memstead mem unregister"),
            "`{name}` names the repair: {printed}"
        );
        assert!(!printed.contains("UNKNOWN_MEM"), "`{name}`: {printed}");
        assert!(!printed.contains("ENTITY_NOT_FOUND"), "`{name}`: {printed}");
    }

    for args in workspace_wide_calls() {
        let (succeeded, printed) = run(&ws, &args);
        let name = args.join(" ");
        assert!(!succeeded, "`{name}` answered: {printed}");
        assert!(printed.contains("MEM_VIEW_STALE"), "`{name}`: {printed}");
        assert!(printed.contains("out_of_service"), "`{name}`: {printed}");
        assert!(printed.contains("held"), "`{name}`: {printed}");
        assert!(!printed.contains("UNKNOWN_MEM"), "`{name}`: {printed}");
        assert!(!printed.contains("ENTITY_NOT_FOUND"), "`{name}`: {printed}");
    }

    // Nothing of the quarantined mem was touched, and it is still on
    // the roster for the repair to find.
    let (_, listed) = run(&ws, &["mem", "list"]);
    assert!(listed.contains("held"), "{listed}");
}

/// A name the roster does not carry keeps its old answer, and so does an
/// id of a mem in service that has no such entity. Every mem of this
/// workspace is in service: with one out of service, the verbs that read
/// the whole workspace would name that mem first.
#[test]
fn a_name_nobody_knows_is_still_unknown() {
    let root = TempDir::new().unwrap();
    let ws = workspace(root.path(), false);
    for call in mem_calls("nosuch") {
        let args: Vec<&str> = call.iter().map(String::as_str).collect();
        let (succeeded, printed) = run(&ws, &args);
        let name = args.join(" ");
        assert!(!succeeded, "`{name}` answered: {printed}");
        assert!(printed.contains("UNKNOWN_MEM"), "`{name}`: {printed}");
        assert!(!printed.contains("MEM_QUARANTINED"), "`{name}`: {printed}");
    }
    for args in [
        vec!["entity", "live--nosuch"],
        vec!["relations", "live--nosuch"],
        vec![
            "update",
            "live--nosuch",
            "--auto-hash",
            "--section",
            "purpose=q",
        ],
    ] {
        let (succeeded, printed) = run(&ws, &args);
        let name = args.join(" ");
        assert!(!succeeded, "`{name}` answered: {printed}");
        assert!(printed.contains("ENTITY_NOT_FOUND"), "`{name}`: {printed}");
        assert!(!printed.contains("MEM_QUARANTINED"), "`{name}`: {printed}");
    }
    // The mem in service answers.
    let (succeeded, printed) = run(&ws, &["entity", "live--alive"]);
    assert!(succeeded, "{printed}");
}
