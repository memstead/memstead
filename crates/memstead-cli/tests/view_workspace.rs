//! A workspace that mounts mems out of another workspace's git
//! directory (a view) is a git-backed workspace with several mems.
//!
//! The fixture: a store with the mems `notes` and `facts`, and a view
//! made with `memstead mem mount --new-workspace` that mounts `facts`
//! read-only first and `notes` writable second. The view has no
//! `mem-repo/` of its own. Every command that works on a mem-repo
//! workspace's mounts works there, and a write into the read-only mount
//! says that the mount is read-only.

use std::fs;
use std::path::{Path, PathBuf};

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

fn refused(ws: &Path, args: &[&str], code: &str) -> String {
    let (succeeded, printed) = run(ws, args);
    assert!(
        !succeeded,
        "`memstead {}` answered:\n{printed}",
        args.join(" ")
    );
    assert!(
        printed.contains(code),
        "`memstead {}` should refuse {code}:\n{printed}",
        args.join(" ")
    );
    printed
}

fn spec<'a>(mem: &'a str, title: &'a str) -> Vec<&'a str> {
    vec![
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
    ]
}

/// The store and the view. The read-only mount comes first on the
/// view's roster, so that nothing passes by being the first mem.
fn fixture(root: &Path) -> (PathBuf, PathBuf) {
    let store = root.join("store");
    let view = root.join("view");
    fs::create_dir_all(&store).unwrap();
    fs::create_dir_all(&view).unwrap();
    ok(&store, &["mem-repo", "init", "."]);
    ok(&store, &["workspace", "allow-create", "--schema", "*", "*"]);
    for mem in ["notes", "facts"] {
        ok(&store, &["mem", "init", mem, "--schema", "default@1.3.0"]);
    }
    ok(&store, &spec("notes", "A Note"));
    ok(&store, &spec("facts", "A Fact"));
    let gitdir = store.join("mem-repo/.git").canonicalize().unwrap();
    let gitdir = gitdir.to_str().unwrap();
    ok(
        &view,
        &[
            "mem",
            "mount",
            "facts",
            "--branch",
            "facts",
            "--gitdir",
            gitdir,
            "--capability",
            "read-only",
            "--new-workspace",
            "--operator-mode",
        ],
    );
    ok(
        &view,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "notes",
            "--gitdir",
            gitdir,
            "--capability",
            "write",
            "--operator-mode",
        ],
    );
    assert!(!view.join("mem-repo").exists(), "a view has no mem-repo");
    (store, view)
}

#[test]
fn the_commands_of_a_mem_repo_workspace_work_in_a_view() {
    let root = TempDir::new().unwrap();
    let (store, view) = fixture(root.path());
    let store_policy = fs::read_to_string(store.join(".memstead/workspace.toml")).unwrap();

    let listed = ok(&view, &["mem", "list"]);
    assert!(listed.contains("`notes` (write)"), "{listed}");
    assert!(listed.contains("`facts` (read_only)"), "{listed}");
    ok(&view, &["status"]);
    ok(&view, &["health"]);
    ok(&view, &["workspace", "show"]);
    let dump = ok(&view, &["workspace", "dump"]);
    assert!(
        dump.contains("\"notes\"") && dump.contains("\"facts\""),
        "{dump}"
    );

    // The workspace policy commands write the view's own policy.
    ok(&view, &["workspace", "allow-create", "--schema", "*", "*"]);
    ok(&view, &["workspace", "grant-cross-link", "notes", "facts"]);
    let policy = fs::read_to_string(view.join(".memstead/workspace.toml")).unwrap();
    assert!(policy.contains("[cross_mem_links]"), "{policy}");
    assert_eq!(
        fs::read_to_string(store.join(".memstead/workspace.toml")).unwrap(),
        store_policy,
        "the store's policy is not the view's to write"
    );

    // A write into the writable mem, which is not the first mount, and
    // a link from it into the read-only one, which the grant allows.
    ok(&view, &spec("notes", "From The View"));
    let (linked, printed) = run(
        &view,
        &[
            "update",
            "notes--from-the-view",
            "--auto-hash",
            "--section",
            "purpose=see [[facts--a-fact]]",
        ],
    );
    assert!(linked, "{printed}");
    assert!(
        !printed.contains("CROSS_MEM_LINK_NOT_ALLOWED"),
        "the grant given by the CLI is in force: {printed}"
    );
    ok(
        &view,
        &[
            "update",
            "notes--a-note",
            "--auto-hash",
            "--section",
            "purpose=changed in the view",
        ],
    );
    ok(&view, &["delete", "notes--from-the-view"]);

    // The store sees what the view wrote.
    let seen = ok(&store, &["entity", "notes--a-note"]);
    assert!(seen.contains("changed in the view"), "{seen}");

    // A batch into the writable mem.
    let batch = root.path().join("batch.json");
    fs::write(
        &batch,
        r#"{"creates":[{"mem":"notes","title":"Batched","entity_type":"spec","metadata":{"level":"M0"},"sections":{"identity":"i","purpose":"p"}}]}"#,
    )
    .unwrap();
    ok(&view, &["batch-create", "--from", batch.to_str().unwrap()]);
    ok(&view, &["entity", "notes--batched"]);

    // The read-only mount says what it is, on every write.
    refused(&view, &spec("facts", "Not Here"), "READ_ONLY_MOUNT");
    refused(
        &view,
        &[
            "update",
            "facts--a-fact",
            "--auto-hash",
            "--section",
            "purpose=no",
        ],
        "READ_ONLY_MOUNT",
    );
    refused(&view, &["delete", "facts--a-fact"], "READ_ONLY_MOUNT");
    let (_, printed) = run(&view, &spec("facts", "Not Here"));
    assert!(!printed.contains("UNKNOWN_MEM"), "{printed}");
    // A name the view does not mount is unknown.
    refused(&view, &spec("nosuch", "Nowhere"), "UNKNOWN_MEM");

    // A mount comes off the roster, and comes back.
    ok(&view, &["mem", "unregister", "facts", "--operator-mode"]);
    let listed = ok(&view, &["mem", "list"]);
    assert!(!listed.contains("`facts`"), "{listed}");
}

fn store_branches(store: &Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(store.join("mem-repo/.git"))
        .args([
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(String::from)
        .collect()
}

/// The verbs that act on the store itself (a mem's branch, the git
/// directory, its remotes, its recovery) refuse in a view, as they do
/// in a folder workspace: a view serves what its mounts hold and is not
/// the store they live in. Nothing in the store or in the view changes.
/// The closed list is what a comparison of every command against the
/// released binary found opened when a view began to count as
/// git-backed.
#[test]
fn the_verbs_that_act_on_the_store_refuse_in_a_view() {
    let root = TempDir::new().unwrap();
    let (store, view) = fixture(root.path());
    ok(&view, &["workspace", "allow-create", "--schema", "*", "*"]);
    let branches = store_branches(&store);
    let roster = fs::read_to_string(view.join(".memstead/state/mounts.json")).unwrap();
    let tip = branches
        .iter()
        .find(|line| line.starts_with("refs/heads/notes "))
        .and_then(|line| line.split(' ').nth(1))
        .unwrap()
        .to_string();
    let calls: Vec<Vec<&str>> = vec![
        vec!["mem", "init", "beta", "--schema", "default@1.3.0"],
        vec![
            "mem",
            "init",
            "beta",
            "--schema",
            "default@1.3.0",
            "--storage",
            "folder",
            "--operator-mode",
        ],
        vec!["mem", "delete", "notes", "--operator-mode"],
        vec!["mem", "rename", "notes", "renamed", "--operator-mode"],
        vec!["branch-reset", "notes", &tip],
        vec![
            "mem-repo",
            "remote-add",
            "origin",
            "https://example.invalid/x.git",
        ],
        vec!["fetch", "notes"],
        vec!["pull", "notes"],
        vec!["pull", "--all"],
        vec!["push", "notes"],
        vec!["push", "--all"],
        vec!["recover"],
    ];
    for args in &calls {
        let (succeeded, printed) = run(&view, args);
        assert!(
            !succeeded,
            "`memstead {}` answered in a view:\n{printed}",
            args.join(" ")
        );
        assert!(
            !printed.contains("UNKNOWN_REMOTE") && !printed.contains("UNKNOWN_REF"),
            "`memstead {}` got past the gate: {printed}",
            args.join(" ")
        );
        // A view is no folder workspace: the refusal names what is
        // missing, the workspace's own mem-repo, and no shape.
        assert!(
            !printed.contains("filesystem"),
            "`memstead {}` calls a view a filesystem workspace: {printed}",
            args.join(" ")
        );
        assert_eq!(
            store_branches(&store),
            branches,
            "the store is as it was after `{}`",
            args.join(" ")
        );
        assert_eq!(
            fs::read_to_string(view.join(".memstead/state/mounts.json")).unwrap(),
            roster,
            "the view's roster is as it was after `{}`",
            args.join(" ")
        );
        assert!(!view.join("beta").exists(), "nothing was created");
    }
    let remotes = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(store.join("mem-repo/.git"))
        .arg("remote")
        .output()
        .unwrap();
    assert!(
        remotes.stdout.is_empty(),
        "no remote was added to the store"
    );
    // A schema installed in a view lands in the view's own schema
    // directory, never on the store's schema ref.
    ok(&view, &["schema", "new", "viewpkg"]);
    let printed = ok(&view, &["schema", "install", "viewpkg"]);
    assert!(printed.contains(".memstead/schemas/viewpkg@"), "{printed}");
    assert_eq!(store_branches(&store), branches, "the store's refs stay");

    // What a view could always do stays: take a mount off its roster.
    ok(&view, &["mem", "unregister", "notes", "--operator-mode"]);
    assert_eq!(store_branches(&store), branches, "the branch stays");
}

/// A folder workspace that also mounts a git branch keeps the refusals
/// of a folder workspace for those verbs. Its own mem is not deleted
/// (the delete would remove the workspace's directory), and the mount's
/// branch is not deleted in the other workspace's git directory.
#[test]
fn a_folder_workspace_with_a_branch_mount_keeps_its_own_mem() {
    let root = TempDir::new().unwrap();
    let (store, _view) = fixture(root.path());
    let ws = root.path().join("plain");
    fs::create_dir_all(&ws).unwrap();
    ok(
        &ws,
        &["init", "--name", "plain", "--schema", "default@1.3.0"],
    );
    fs::write(ws.join("keep.txt"), "kept").unwrap();
    let gitdir = store.join("mem-repo/.git").canonicalize().unwrap();
    ok(
        &ws,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "notes",
            "--gitdir",
            gitdir.to_str().unwrap(),
            "--capability",
            "write",
            "--operator-mode",
        ],
    );
    let branches = store_branches(&store);
    for args in [
        vec!["mem", "delete", "plain", "--operator-mode"],
        vec!["mem", "unregister", "plain", "--operator-mode"],
        vec!["mem", "rename", "plain", "renamed", "--operator-mode"],
        vec!["mem", "delete", "notes", "--operator-mode"],
        vec!["mem", "rename", "notes", "renamed", "--operator-mode"],
    ] {
        refused(&ws, &args, "UNSUPPORTED_WORKSPACE_SHAPE");
        assert_eq!(
            fs::read_to_string(ws.join("keep.txt")).unwrap(),
            "kept",
            "the workspace's directory is as it was"
        );
        assert_eq!(store_branches(&store), branches, "the store is as it was");
    }
    let listed = ok(&ws, &["mem", "list"]);
    assert!(
        listed.contains("`plain`") && listed.contains("`notes`"),
        "{listed}"
    );
    // The content of the mount is served: a create lands in it.
    ok(&ws, &spec("notes", "Into The Mount"));
}

/// A view whose last mount was taken off reads as the folder shape,
/// and its refusals say what is the case there: it mounts no mem, and a
/// mount is what brings it back. No sentence names "the one mem" of a
/// folder workspace with an empty name.
#[test]
fn a_view_with_no_mount_left_says_so() {
    let root = TempDir::new().unwrap();
    let (store, view) = fixture(root.path());
    ok(&view, &["mem", "unregister", "facts", "--operator-mode"]);
    ok(&view, &["mem", "unregister", "notes", "--operator-mode"]);
    let listed = ok(&view, &["mem", "list"]);
    assert!(listed.contains("# Mems (0)"), "{listed}");

    for args in [
        vec!["workspace", "dump"],
        vec!["workspace", "grant-cross-link", "notes", "facts"],
        vec!["mem", "unregister", "notes", "--operator-mode"],
    ] {
        let printed = refused(&view, &args, "UNSUPPORTED_WORKSPACE_SHAPE");
        assert!(printed.contains("mounts no mem"), "{printed}");
        assert!(printed.contains("mem mount <name>"), "{printed}");
        assert!(
            !printed.contains("to migrate") && !printed.contains("here instead"),
            "no advice for a folder workspace that has a mem: {printed}"
        );
    }
    let printed = refused(&view, &spec("notes", "Nowhere"), "UNKNOWN_MEM");
    assert!(
        printed.contains("This workspace mounts no mem"),
        "{printed}"
    );
    // A create that names no mem has none to fall back on, and says so
    // without an empty name.
    let printed = refused(
        &view,
        &[
            "create",
            "--title",
            "Nowhere",
            "--type",
            "spec",
            "--metadata",
            "level=M0",
            "--section",
            "identity=i",
            "--section",
            "purpose=p",
        ],
        "NO_WRITABLE_MEM",
    );
    assert!(!printed.contains("unknown mem"), "{printed}");
    assert!(!printed.contains("holds one mem"), "{printed}");

    let gitdir = store.join("mem-repo/.git").canonicalize().unwrap();
    ok(
        &view,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "notes",
            "--gitdir",
            gitdir.to_str().unwrap(),
            "--capability",
            "write",
            "--operator-mode",
        ],
    );
    ok(&view, &spec("notes", "Back Again"));
    ok(&view, &["workspace", "dump"]);

    // A mount may carry another name than its branch; the dump reads
    // the mount's branch.
    ok(
        &view,
        &[
            "mem",
            "mount",
            "other",
            "--branch",
            "facts",
            "--gitdir",
            gitdir.to_str().unwrap(),
            "--capability",
            "read-only",
            "--operator-mode",
        ],
    );
    let dump = ok(&view, &["workspace", "dump"]);
    assert!(dump.contains("\"other\""), "{dump}");
}

/// A workspace that holds one folder mem is still the filesystem shape:
/// the commands that need a git-backed workspace refuse there, and a
/// create that names another mem than its own is refused.
#[test]
fn a_folder_workspace_keeps_its_shape() {
    let root = TempDir::new().unwrap();
    let ws = root.path().join("plain");
    fs::create_dir_all(&ws).unwrap();
    ok(
        &ws,
        &["init", "--name", "plain", "--schema", "default@1.3.0"],
    );
    refused(
        &ws,
        &["workspace", "grant-cross-link", "a", "b"],
        "UNSUPPORTED_WORKSPACE_SHAPE",
    );
    refused(&ws, &["workspace", "dump"], "UNSUPPORTED_WORKSPACE_SHAPE");
    refused(&ws, &spec("nosuch", "Nowhere"), "UNKNOWN_MEM");
}
