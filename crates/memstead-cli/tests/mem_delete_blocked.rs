//! A mem delete whose storage step meets a locked ref.
//!
//! The delete takes the mem off the roster and then removes its branch
//! and its registry entry in one ref transaction. A lock that another
//! process holds on the branch's ref for a moment (git does when it
//! packs refs) used to end that step, leaving the branch behind. The
//! step is now done again. When the lock outlasts the repetition, the
//! reply says so and names a way out that works when followed as
//! written.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::time::Duration;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

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

fn ok(ws: &Path, args: &[&str]) -> String {
    let (succeeded, printed) = run(ws, args);
    assert!(
        succeeded,
        "`memstead {}` failed:\n{printed}",
        args.join(" ")
    );
    printed
}

/// A workspace with one git-branch mem `name` holding one entity.
fn workspace(root: &Path, name: &str) -> PathBuf {
    let ws = root.join("ws");
    fs::create_dir_all(&ws).unwrap();
    ok(&ws, &["mem-repo", "init", "."]);
    ok(&ws, &["workspace", "allow-create", "--schema", "*", "*"]);
    ok(&ws, &["mem", "init", name, "--schema", "default@1.3.0"]);
    ok(
        &ws,
        &[
            "create",
            "--mem",
            name,
            "--title",
            "One",
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
    ws
}

fn branch_exists(ws: &Path, name: &str) -> bool {
    let out = StdCommand::new("git")
        .arg("--git-dir")
        .arg(ws.join("mem-repo/.git"))
        .args(["for-each-ref", "--format=%(refname)"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line == format!("refs/heads/{name}"))
}

fn lock_file(ws: &Path, name: &str) -> PathBuf {
    ws.join("mem-repo/.git/refs/heads")
        .join(format!("{name}.lock"))
}

/// A lock on the branch's ref that another process holds for a moment
/// does not stop the delete: the storage step is done again, and the
/// mem is gone whole.
#[test]
fn a_ref_locked_for_a_moment_does_not_leave_the_branch_behind() {
    let root = TempDir::new().unwrap();
    let ws = workspace(root.path(), "notes");
    let lock = lock_file(&ws, "notes");
    fs::write(&lock, "").unwrap();
    let held = lock.clone();
    let holder = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        fs::remove_file(held).unwrap();
    });
    let printed = ok(&ws, &["mem", "delete", "notes", "--operator-mode"]);
    holder.join().unwrap();
    assert!(printed.contains("Files deleted: true"), "{printed}");
    assert!(!printed.contains("MEM_FILES_NOT_DELETED"), "{printed}");
    assert!(!branch_exists(&ws, "notes"), "the branch is gone");
    // Nothing is left for a later create to trip over.
    ok(&ws, &["mem", "init", "notes", "--schema", "default@1.3.0"]);
}

/// A lock that outlasts the repetition: the delete answers as it did
/// (off the roster, storage kept, a warning), and every sentence of the
/// warning and of the later refusals names something that works when it
/// is run as written.
#[test]
fn a_ref_locked_for_long_leaves_a_branch_and_names_the_way_out() {
    let root = TempDir::new().unwrap();
    let ws = workspace(root.path(), "notes");
    let lock = lock_file(&ws, "notes");
    fs::write(&lock, "").unwrap();
    let printed = ok(&ws, &["mem", "delete", "notes", "--operator-mode"]);
    fs::remove_file(&lock).unwrap();
    assert!(printed.contains("Removed from router: true"), "{printed}");
    assert!(printed.contains("Files deleted: false"), "{printed}");
    assert!(printed.contains("MEM_FILES_NOT_DELETED"), "{printed}");
    assert!(
        printed.contains("memstead mem init notes --reattach"),
        "the warning names the way out: {printed}"
    );
    assert!(
        printed.contains("or operator mode"),
        "and what both calls need: {printed}"
    );
    assert!(
        !printed.contains("rerun delete"),
        "and no longer a rerun that refuses: {printed}"
    );
    assert!(branch_exists(&ws, "notes"));

    // The rerun the old text advised answers that the mem is unknown,
    // and destroys nothing.
    let (succeeded, rerun) = run(&ws, &["mem", "delete", "notes", "--operator-mode"]);
    assert!(!succeeded && rerun.contains("UNKNOWN_MEM"), "{rerun}");
    assert!(branch_exists(&ws, "notes"));

    // A plain init names what is left, with the flags the CLI takes.
    let (succeeded, residue) = run(&ws, &["mem", "init", "notes", "--schema", "default@1.3.0"]);
    assert!(!succeeded, "{residue}");
    assert!(
        residue.contains("MEM_STORAGE_RESIDUE_DETECTED"),
        "{residue}"
    );
    assert!(residue.contains("--reattach"), "{residue}");
    assert!(residue.contains("--force-overwrite"), "{residue}");
    assert!(
        !residue.contains("mem delete"),
        "the refusal names no delete, which would refuse: {residue}"
    );

    // The warning's way out, as written.
    ok(&ws, &["mem", "init", "notes", "--reattach"]);
    ok(&ws, &["entity", "notes--one"]);
    let printed = ok(&ws, &["mem", "delete", "notes", "--operator-mode"]);
    assert!(printed.contains("Files deleted: true"), "{printed}");
    assert!(!branch_exists(&ws, "notes"));
}

/// The way out works for a mem of another schema than the CLI's
/// default, under an allowlist that names only that schema and with no
/// operator mode: a reattach adopts the leftover's own pin, so the
/// rule judges the schema the mem carries and the mount records it.
#[test]
fn the_way_out_keeps_the_leftovers_own_schema() {
    let root = TempDir::new().unwrap();
    let ws = root.path().join("ws");
    fs::create_dir_all(&ws).unwrap();
    ok(&ws, &["mem-repo", "init", "."]);
    ok(
        &ws,
        &[
            "workspace",
            "allow-create",
            "--schema",
            "planning@0.4.0",
            "plans",
        ],
    );
    ok(&ws, &["workspace", "allow-delete", "plans"]);
    ok(&ws, &["mem", "init", "plans", "--schema", "planning@0.4.0"]);
    let lock = lock_file(&ws, "plans");
    fs::write(&lock, "").unwrap();
    let printed = ok(&ws, &["mem", "delete", "plans"]);
    fs::remove_file(&lock).unwrap();
    assert!(printed.contains("Files deleted: false"), "{printed}");

    // The warning's command, as written.
    let printed = ok(&ws, &["mem", "init", "plans", "--reattach"]);
    assert!(
        printed.contains("Schema: `planning@0.4.0`"),
        "the reply names the adopted pin: {printed}"
    );
    let listed = ok(&ws, &["mem", "list"]);
    assert!(listed.contains("schema `planning@0.4.0`"), "{listed}");
    let (_, health) = run(&ws, &["health", "--strict"]);
    assert!(!health.contains("SCHEMA_PIN_MISMATCH"), "{health}");

    let printed = ok(&ws, &["mem", "delete", "plans"]);
    assert!(printed.contains("Files deleted: true"), "{printed}");
    assert!(!branch_exists(&ws, "plans"));
}

/// A reattach after `mem unregister` keeps the pin too, also when the
/// call names another schema; an overwrite takes the named one.
#[test]
fn a_reattach_never_changes_a_pin() {
    let root = TempDir::new().unwrap();
    let ws = root.path().join("ws");
    fs::create_dir_all(&ws).unwrap();
    ok(&ws, &["mem-repo", "init", "."]);
    ok(&ws, &["workspace", "allow-create", "--schema", "*", "*"]);
    ok(&ws, &["mem", "init", "plans", "--schema", "planning@0.4.0"]);
    for args in [
        vec!["mem", "init", "plans"],
        vec![
            "mem",
            "init",
            "plans",
            "--schema",
            "default@1.3.0",
            "--reattach",
        ],
    ] {
        ok(&ws, &["mem", "unregister", "plans", "--operator-mode"]);
        let printed = ok(&ws, &args);
        assert!(printed.contains("Schema: `planning@0.4.0`"), "{printed}");
        let (_, health) = run(&ws, &["health", "--strict"]);
        assert!(!health.contains("SCHEMA_PIN_MISMATCH"), "{health}");
    }
    ok(&ws, &["mem", "unregister", "plans", "--operator-mode"]);
    let printed = ok(
        &ws,
        &[
            "mem",
            "init",
            "plans",
            "--schema",
            "default@1.3.0",
            "--force-overwrite",
        ],
    );
    assert!(printed.contains("Schema: `default@1.3.0`"), "{printed}");
}

/// A branch kept on purpose by `mem unregister` is removed by no
/// delete: the name is off the roster, and the delete says so.
#[test]
fn a_delete_does_not_remove_a_branch_an_unregister_kept() {
    let root = TempDir::new().unwrap();
    let ws = workspace(root.path(), "notes");
    ok(&ws, &["mem", "unregister", "notes", "--operator-mode"]);
    assert!(branch_exists(&ws, "notes"));
    let (succeeded, printed) = run(&ws, &["mem", "delete", "notes", "--operator-mode"]);
    assert!(!succeeded && printed.contains("UNKNOWN_MEM"), "{printed}");
    assert!(branch_exists(&ws, "notes"), "the branch is kept");
}
