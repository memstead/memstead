//! Integration tests for `memstead mem unregister` on a mount that is
//! out of service: a quarantined mount of either capability leaves the
//! roster, whatever put it out of service, and the workspace is whole
//! again.
//!
//! The fixtures. A "holder" workspace with a mem-repo of its own, the
//! schema `widgets@0.1.0` installed in it and the mem `notes` pinned to
//! that schema. A "view" workspace with a mem-repo of its own, the same
//! schema installed and the mem `local`, which mounts the holder's mem.
//! The mount goes out of service when the holder is removed, or when the
//! schema its pin names is taken out of the registry that holds it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    let mut cmd = Command::cargo_bin("memstead").expect("memstead binary must be built by cargo");
    cmd.env_remove("MEMSTEAD_OPERATOR_MODE");
    cmd
}

fn run(ws: &Path, args: &[&str]) -> std::process::Output {
    memstead()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap()
}

fn ok(ws: &Path, args: &[&str]) -> String {
    let out = run(ws, args);
    assert!(
        out.status.success(),
        "`memstead {}` failed:\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run a command that must refuse, and hand back its stderr.
fn refused(ws: &Path, args: &[&str], code: &str) -> String {
    let out = run(ws, args);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !out.status.success(),
        "`memstead {}` should refuse",
        args.join(" ")
    );
    assert!(
        stderr.contains(&format!("[{code}]")),
        "`memstead {}` should refuse {code}: {stderr}",
        args.join(" ")
    );
    stderr
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Every path under a directory with its bytes (a directory as an empty
/// entry): what must be identical when nothing at all was laid there,
/// lock files included.
fn every_file(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.push((path.clone(), Vec::new()));
                stack.push(path);
            } else {
                found.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    found.sort();
    found
}

fn roster(ws: &Path) -> String {
    fs::read_to_string(ws.join(".memstead/state/mounts.json")).unwrap_or_default()
}

fn note(ws: &Path, mem: &str, title: &str) {
    ok(
        ws,
        &[
            "create",
            "--mem",
            mem,
            "--title",
            title,
            "--type",
            "note",
            "--section",
            "summary=Hello.",
        ],
    );
}

/// A workspace with its own mem-repo and the schema `widgets@0.1.0`
/// installed in it.
fn store(root: &Path, name: &str) -> PathBuf {
    let ws = root.join(name);
    fs::create_dir_all(&ws).unwrap();
    ok(&ws, &["mem-repo", "init", "."]);
    ok(&ws, &["workspace", "allow-create", "--schema", "*", "*"]);
    ok(&ws, &["schema", "new", "widgets"]);
    ok(&ws, &["schema", "install", "widgets"]);
    ws
}

/// The holder: a store with the mem `notes` holding one entity.
fn holder(root: &Path) -> PathBuf {
    let holder = store(root, "holder");
    ok(
        &holder,
        &["mem", "init", "notes", "--schema", "widgets@0.1.0"],
    );
    note(&holder, "notes", "Held");
    holder
}

fn gitdir_of(ws: &Path) -> String {
    ws.join("mem-repo/.git")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

/// The view: a store with the mem `local` holding two entities, which
/// mounts the holder's `notes` under `capability`.
fn view_mounting(root: &Path, holder: &Path, capability: &str) -> PathBuf {
    let view = store(root, "view");
    ok(
        &view,
        &["mem", "init", "local", "--schema", "widgets@0.1.0"],
    );
    note(&view, "local", "Keeper");
    note(&view, "local", "Goner");
    ok(
        &view,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "notes",
            "--gitdir",
            &gitdir_of(holder),
            "--capability",
            capability,
            "--operator-mode",
        ],
    );
    assert!(ok(&view, &["entity", "notes--held"]).contains("# Held"));
    view
}

/// Take the schema packages out of the registry of `ws`'s mem-repo, so
/// a pin that names one of them resolves nowhere. Storage stays as it
/// is otherwise: every branch and every mem's registry entry.
fn drop_installed_schemas(ws: &Path, scratch: &Path) {
    let gitdir = ws.join("mem-repo/.git");
    let index = scratch.join("drop-schemas.index");
    let plumb = |args: &[&str]| -> String {
        let out = StdCommand::new("git")
            .arg("-C")
            .arg(&gitdir)
            .env("GIT_INDEX_FILE", &index)
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.com",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    plumb(&["read-tree", "refs/heads/__MEMSTEAD"]);
    plumb(&["rm", "-r", "--cached", "-q", "schemas"]);
    let tree = plumb(&["write-tree"]);
    let commit = plumb(&[
        "commit-tree",
        &tree,
        "-p",
        "refs/heads/__MEMSTEAD",
        "-m",
        "fixture: the installed schema packages are gone",
    ]);
    plumb(&["update-ref", "refs/heads/__MEMSTEAD", &commit]);
    let _ = fs::remove_file(&index);
}

fn quarantined(ws: &Path, mem: &str) -> bool {
    let listed = ok(ws, &["mem", "list"]);
    listed.contains("Quarantined") && listed.contains(&format!("`{mem}`"))
}

/// What both capabilities share when the git directory a mount names is
/// removed: the mount is quarantined and bars every verb that reads the
/// whole workspace; the refusals name the unregister; the unregister
/// takes the row off the roster and lays nothing where the storage was;
/// and the workspace is whole again.
fn a_mount_whose_git_directory_is_gone_leaves_the_roster(capability: &str) {
    let root = TempDir::new().unwrap();
    let holder = holder(root.path());
    let view = view_mounting(root.path(), &holder, capability);
    fs::remove_dir_all(&holder).unwrap();

    assert!(quarantined(&view, "notes"));
    let barred = refused(&view, &["delete", "local--goner"], "MEM_VIEW_STALE");
    assert!(
        barred.contains("out_of_service") && barred.contains("`memstead mem unregister notes`"),
        "{barred}"
    );

    // Destroying it is refused, and so is removing it without leave.
    let before = roster(&view);
    let destroy = refused(
        &view,
        &["mem", "delete", "notes", "--operator-mode"],
        "MEM_QUARANTINED",
    );
    assert!(
        destroy.contains("`memstead mem unregister notes`"),
        "{destroy}"
    );
    refused(
        &view,
        &["mem", "unregister", "notes"],
        "MEM_PATH_NOT_ALLOWED",
    );
    assert_eq!(
        roster(&view),
        before,
        "a refusal leaves the roster as it was"
    );

    // The unregister.
    ok(&view, &["mem", "unregister", "notes", "--operator-mode"]);
    assert!(!roster(&view).contains("notes"), "{}", roster(&view));
    let listed = ok(&view, &["mem", "list"]);
    assert!(
        !listed.contains("notes") && !listed.contains("Quarantined"),
        "{listed}"
    );
    assert!(!ok(&view, &["status"]).contains("notes"));
    assert!(!holder.exists(), "nothing is created where the storage was");

    // The workspace is whole again, and the name is free.
    ok(&view, &["delete", "local--goner"]);
    refused(&view, &["entity", "local--goner"], "ENTITY_NOT_FOUND");
    refused(
        &view,
        &["mem", "unregister", "notes", "--operator-mode"],
        "UNKNOWN_MEM",
    );
    ok(
        &view,
        &["mem", "init", "notes", "--schema", "widgets@0.1.0"],
    );
    note(&view, "notes", "Fresh");
}

#[test]
fn a_write_mount_whose_git_directory_is_gone_leaves_the_roster() {
    a_mount_whose_git_directory_is_gone_leaves_the_roster("write");
}

#[test]
fn a_read_only_mount_whose_git_directory_is_gone_leaves_the_roster() {
    a_mount_whose_git_directory_is_gone_leaves_the_roster("read-only");
}

/// A write mount of the workspace's own mem-repo, out of service for a
/// cause that leaves its storage in place (its pin resolves nowhere):
/// the unregister takes the row off the roster and marks the mem's
/// registry entry as it does for a mount in service, so `mem init`
/// reattaches the branch without a recovery flag.
#[test]
fn a_quarantined_mount_of_the_own_mem_repo_is_marked_and_reattaches() {
    let root = TempDir::new().unwrap();
    let ws = store(root.path(), "ws");
    ok(&ws, &["mem", "init", "things", "--schema", "widgets@0.1.0"]);
    note(&ws, "things", "Held");
    ok(&ws, &["mem", "init", "local", "--schema", "default@1.3.0"]);
    let spec = |title: &str| {
        ok(
            &ws,
            &[
                "create",
                "--mem",
                "local",
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
    };
    spec("Keeper");
    spec("Goner");
    drop_installed_schemas(&ws, root.path());

    assert!(quarantined(&ws, "things"));
    assert!(ok(&ws, &["entity", "local--keeper"]).contains("# Keeper"));
    refused(&ws, &["delete", "local--goner"], "MEM_VIEW_STALE");
    let tip = git(&ws.join("mem-repo"), &["rev-parse", "refs/heads/things"]);
    refused(
        &ws,
        &["mem", "delete", "things", "--operator-mode"],
        "MEM_QUARANTINED",
    );
    assert!(roster(&ws).contains("things"));
    assert_eq!(
        git(&ws.join("mem-repo"), &["rev-parse", "refs/heads/things"]),
        tip,
        "the refused delete destroyed nothing"
    );

    ok(&ws, &["mem", "unregister", "things", "--operator-mode"]);
    assert!(!roster(&ws).contains("things"), "{}", roster(&ws));
    assert!(!ok(&ws, &["mem", "list"]).contains("things"));
    ok(&ws, &["delete", "local--goner"]);
    let entry = git(
        &ws.join("mem-repo"),
        &["show", "refs/heads/__MEMSTEAD:mems/things/config.json"],
    );
    assert!(entry.contains("unregistered"), "{entry}");
    assert_eq!(
        git(&ws.join("mem-repo"), &["rev-parse", "refs/heads/things"]),
        tip,
        "the branch stays"
    );

    // The schema is installed again, and the name reattaches the branch
    // with no recovery flag.
    ok(&ws, &["schema", "install", "widgets"]);
    ok(&ws, &["mem", "init", "things", "--schema", "widgets@0.1.0"]);
    assert!(ok(&ws, &["entity", "things--held"]).contains("# Held"));
}

/// A write mount of another git directory that still exists, out of
/// service for its pin: the unregister changes nothing there, byte for
/// byte. No ref, no object, no registry entry, no marker, no lock file.
#[test]
fn the_unregister_of_a_quarantined_mount_writes_nothing_into_another_git_directory() {
    let root = TempDir::new().unwrap();
    let holder = holder(root.path());
    // A view without the schema of its own: the pin resolves from the
    // holder's registry alone.
    let view = root.path().join("view");
    fs::create_dir_all(&view).unwrap();
    ok(&view, &["mem-repo", "init", "."]);
    ok(&view, &["workspace", "allow-create", "--schema", "*", "*"]);
    ok(
        &view,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "notes",
            "--gitdir",
            &gitdir_of(&holder),
            "--capability",
            "write",
            "--operator-mode",
        ],
    );
    note(&view, "notes", "Written Through The View");
    drop_installed_schemas(&holder, root.path());
    assert!(quarantined(&view, "notes"));

    let there = holder.join("mem-repo/.git");
    let before = every_file(&there);
    ok(&view, &["mem", "unregister", "notes", "--operator-mode"]);
    assert!(!roster(&view).contains("notes"), "{}", roster(&view));
    assert_eq!(every_file(&there), before);

    // The branch is still there, so the name mounts again once the pin
    // resolves.
    ok(&holder, &["schema", "install", "widgets"]);
    ok(
        &view,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "notes",
            "--gitdir",
            &gitdir_of(&holder),
            "--operator-mode",
        ],
    );
    assert!(
        ok(&view, &["entity", "notes--written-through-the-view"])
            .contains("# Written Through The View")
    );
}

/// Two mounts out of service at once: neither bars the removal of the
/// other.
#[test]
fn two_quarantined_mounts_are_removed_one_after_the_other() {
    let root = TempDir::new().unwrap();
    let holder = holder(root.path());
    ok(
        &holder,
        &["mem", "init", "more", "--schema", "widgets@0.1.0"],
    );
    note(&holder, "more", "Also Held");
    let view = view_mounting(root.path(), &holder, "write");
    ok(
        &view,
        &[
            "mem",
            "mount",
            "more",
            "--branch",
            "more",
            "--gitdir",
            &gitdir_of(&holder),
            "--operator-mode",
        ],
    );
    fs::remove_dir_all(&holder).unwrap();
    assert!(quarantined(&view, "notes") && quarantined(&view, "more"));

    // Destroying either is refused as an operation that names a
    // quarantined mem, whatever else is out of service.
    let before = roster(&view);
    for name in ["notes", "more"] {
        let destroy = refused(
            &view,
            &["mem", "delete", name, "--operator-mode"],
            "MEM_QUARANTINED",
        );
        assert!(
            destroy.contains(&format!("`memstead mem unregister {name}`")),
            "{destroy}"
        );
    }
    assert_eq!(roster(&view), before);

    ok(&view, &["mem", "unregister", "notes", "--operator-mode"]);
    assert!(quarantined(&view, "more"));
    refused(&view, &["delete", "local--goner"], "MEM_VIEW_STALE");
    ok(&view, &["mem", "unregister", "more", "--operator-mode"]);
    assert!(!roster(&view).contains("more"), "{}", roster(&view));
    ok(&view, &["delete", "local--goner"]);
}

/// Entities of another mem that point into the quarantined mem do not
/// bar its unregister: the response names them. The same mount in
/// service refuses on them, as before.
#[test]
fn the_unregister_of_a_quarantined_mount_reports_who_pointed_into_it() {
    let root = TempDir::new().unwrap();
    let holder = holder(root.path());
    let view = view_mounting(root.path(), &holder, "write");
    ok(&view, &["workspace", "grant-cross-link", "local", "notes"]);
    ok(
        &view,
        &["relate", "local--keeper", "RELATES_TO", "notes--held"],
    );

    // In service: refused.
    refused(
        &view,
        &["mem", "unregister", "notes", "--operator-mode"],
        "MEM_HAS_INCOMING_REFS",
    );
    assert!(roster(&view).contains("notes"));

    fs::remove_dir_all(&holder).unwrap();
    assert!(quarantined(&view, "notes"));
    let reply = ok(
        &view,
        &["mem", "unregister", "notes", "--operator-mode", "--json"],
    );
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("a JSON reply");
    let detached = reply["detached_referrers"]
        .as_array()
        .unwrap_or_else(|| panic!("detached_referrers: {reply}"));
    assert_eq!(detached.len(), 1, "{reply}");
    assert_eq!(detached[0]["from_id"], "local--keeper", "{reply}");
    assert_eq!(detached[0]["mem"], "local", "{reply}");
    assert_eq!(
        detached[0]["rel_types"],
        serde_json::json!(["RELATES_TO"]),
        "{reply}"
    );
    assert!(!roster(&view).contains("notes"), "{}", roster(&view));
}

/// With no edge into it, the list is empty.
#[test]
fn the_unregister_of_a_quarantined_mount_nobody_points_into_reports_no_referrer() {
    let root = TempDir::new().unwrap();
    let holder = holder(root.path());
    let view = view_mounting(root.path(), &holder, "read-only");
    fs::remove_dir_all(&holder).unwrap();
    let reply = ok(
        &view,
        &["mem", "unregister", "notes", "--operator-mode", "--json"],
    );
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("a JSON reply");
    assert!(
        reply["detached_referrers"]
            .as_array()
            .is_none_or(|list| list.is_empty()),
        "{reply}"
    );
}

/// Only an entity that holds an edge into the removed mem is listed:
/// one with several relationship types appears once with all of them,
/// and one whose edges stay inside its own mem does not appear.
#[test]
fn only_holders_of_an_edge_into_the_removed_mem_are_listed() {
    let root = TempDir::new().unwrap();
    let holder = holder(root.path());
    let view = view_mounting(root.path(), &holder, "write");
    ok(&view, &["workspace", "grant-cross-link", "local", "notes"]);
    ok(
        &view,
        &["relate", "local--keeper", "RELATES_TO", "notes--held"],
    );
    ok(
        &view,
        &["relate", "local--keeper", "PART_OF", "notes--held"],
    );
    // An edge that stays inside `local`.
    ok(
        &view,
        &["relate", "local--goner", "RELATES_TO", "local--keeper"],
    );

    fs::remove_dir_all(&holder).unwrap();
    assert!(quarantined(&view, "notes"));
    let reply = ok(
        &view,
        &["mem", "unregister", "notes", "--operator-mode", "--json"],
    );
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("a JSON reply");
    assert_eq!(
        reply["detached_referrers"],
        serde_json::json!([
            {"from_id": "local--keeper", "mem": "local", "rel_types": ["PART_OF", "RELATES_TO"]}
        ]),
        "{reply}"
    );
}

/// A folder mem in a mem-repo workspace, out of service for its pin
/// while its folder is in place: the unregister takes the row off the
/// roster and changes one file in the folder, the mem's config, which
/// gets the marker a mount in service gets. No entity file is touched.
#[test]
fn a_quarantined_folder_mount_leaves_the_roster_and_is_marked() {
    let root = TempDir::new().unwrap();
    let ws = store(root.path(), "ws");
    ok(
        &ws,
        &[
            "mem",
            "init",
            "papers",
            "--storage",
            "folder",
            "--schema",
            "widgets@0.1.0",
        ],
    );
    note(&ws, "papers", "Held");
    drop_installed_schemas(&ws, root.path());
    assert!(quarantined(&ws, "papers"));
    refused(
        &ws,
        &["mem", "delete", "papers", "--operator-mode"],
        "MEM_QUARANTINED",
    );

    let folder = ws.join("papers");
    let before = every_file(&folder);
    ok(&ws, &["mem", "unregister", "papers", "--operator-mode"]);
    assert!(!roster(&ws).contains("papers"), "{}", roster(&ws));
    let after = every_file(&folder);
    let changed: Vec<&PathBuf> = after
        .iter()
        .filter(|entry| !before.contains(entry))
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        changed,
        [&folder.join(".memstead/config.json")],
        "the marker is the one write"
    );
    assert_eq!(
        before.len(),
        after.len(),
        "no file is laid and none removed"
    );
    let config = fs::read_to_string(folder.join(".memstead/config.json")).unwrap();
    assert!(config.contains("unregistered"), "{config}");
}
