//! Integration tests for `memstead mem mount`: a mem that already
//! exists as a branch is mounted with a chosen capability, from another
//! workspace's mem-repo or from a remote, and removed again with
//! `memstead mem unregister`.
//!
//! Two fixtures. A "holder" workspace with a mem-repo of its own, a
//! schema no built-in provides installed in it and a mem pinned to that
//! schema; a "view" directory that becomes a workspace of engine state
//! only by mounting the holder's mem. And two stores A and B sharing a
//! bare remote to which A pushes one mem alone.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
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
        "`memstead {}` failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
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

/// Every ref of a git directory with its target, and every object file
/// with its bytes: what must be identical when nothing was written.
fn refs_and_objects(gitdir: &Path) -> (String, Vec<(PathBuf, Vec<u8>)>) {
    let refs = git(
        gitdir,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    );
    let mut objects = Vec::new();
    let mut stack = vec![gitdir.join("objects")];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = fs::read(&path).unwrap();
                objects.push((path.strip_prefix(gitdir).unwrap().to_path_buf(), bytes));
            }
        }
    }
    objects.sort();
    (refs, objects)
}

fn roster(ws: &Path) -> String {
    fs::read_to_string(ws.join(".memstead/state/mounts.json")).unwrap_or_default()
}

fn create(ws: &Path, mem: &str, title: &str) -> std::process::Output {
    run(
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
    )
}

/// A workspace with its own mem-repo, the schema `widgets@0.1.0`
/// installed in it, and the mem `notes` pinned to that schema holding
/// one entity.
fn holder(root: &Path) -> PathBuf {
    let holder = root.join("holder");
    fs::create_dir_all(&holder).unwrap();
    ok(&holder, &["mem-repo", "init", "."]);
    ok(
        &holder,
        &["workspace", "allow-create", "--schema", "*", "*"],
    );
    ok(&holder, &["schema", "new", "widgets"]);
    ok(&holder, &["schema", "install", "widgets"]);
    ok(
        &holder,
        &["mem", "init", "notes", "--schema", "widgets@0.1.0"],
    );
    assert!(create(&holder, "notes", "Held").status.success());
    holder
}

fn holder_gitdir(holder: &Path) -> String {
    holder
        .join("mem-repo/.git")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

/// A workspace of engine state only mounts a mem that lives in another
/// workspace's mem-repo, read-only: it reads, it refuses writes, the
/// schema installed in that mem-repo resolves, and the other git
/// directory is byte-identical before the mount, after it, after reads
/// and a refused write, and after the unregister.
#[test]
fn a_mem_of_another_mem_repo_is_mounted_read_only_and_nothing_is_written_there() {
    let root = TempDir::new().unwrap();
    let holder = holder(root.path());
    let gitdir = holder_gitdir(&holder);
    let view = root.path().join("view");
    fs::create_dir_all(&view).unwrap();
    let before = refs_and_objects(Path::new(&gitdir));

    // No workspace yet: the verb says how to make one, and makes none.
    refused(
        &view,
        &[
            "mem", "mount", "notes", "--branch", "notes", "--gitdir", &gitdir,
        ],
        "WORKSPACE_NOT_INITIALISED",
    );
    assert!(!view.join(".memstead").exists());

    ok(
        &view,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "notes",
            "--gitdir",
            &gitdir,
            "--new-workspace",
            "--operator-mode",
        ],
    );
    assert!(!view.join("mem-repo").exists(), "engine state only");
    assert!(roster(&view).contains("\"read-only\""), "{}", roster(&view));

    // It reads, under the schema the holder's mem-repo has installed.
    let listed = ok(&view, &["mem", "list"]);
    assert!(
        listed.contains("`notes`") && listed.contains("widgets@0.1.0"),
        "{listed}"
    );
    let entity = ok(&view, &["entity", "notes--held"]);
    assert!(entity.contains("# Held"), "{entity}");
    let schema = ok(&view, &["schema", "widgets@0.1.0"]);
    assert!(schema.contains("widgets"), "{schema}");

    // It refuses a write.
    let out = create(&view, "notes", "Not Through Here");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("[READ_ONLY_MOUNT]"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        refs_and_objects(Path::new(&gitdir)),
        before,
        "the mount, the reads and the refused write wrote nothing there"
    );

    // The unregister removes the row and writes nothing there either.
    ok(&view, &["mem", "unregister", "notes", "--operator-mode"]);
    assert!(!roster(&view).contains("notes"), "{}", roster(&view));
    refused(&view, &["entity", "notes--held"], "ENTITY_NOT_FOUND");
    assert!(ok(&view, &["mem", "list"]).contains("no mems mounted"));
    assert_eq!(
        refs_and_objects(Path::new(&gitdir)),
        before,
        "no ref, no object, no tombstone"
    );
    // The holder serves its mem as before.
    assert!(ok(&holder, &["entity", "notes--held"]).contains("# Held"));
}

/// Mounted with capability write, a write through the mounting
/// workspace lands and the holding workspace reads it.
#[test]
fn a_write_mount_of_another_mem_repo_writes_there() {
    let root = TempDir::new().unwrap();
    let holder = holder(root.path());
    let gitdir = holder_gitdir(&holder);
    let view = root.path().join("view");
    fs::create_dir_all(&view).unwrap();
    ok(
        &view,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "refs/heads/notes",
            "--gitdir",
            &gitdir,
            "--capability",
            "write",
            "--new-workspace",
            "--operator-mode",
        ],
    );
    assert!(roster(&view).contains("\"write\""), "{}", roster(&view));
    let out = create(&view, "notes", "From The View");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(ok(&holder, &["entity", "notes--from-the-view"]).contains("# From The View"));
    assert!(ok(&view, &["entity", "notes--held"]).contains("# Held"));

    // Unregistering it here retires nothing there: the holder still
    // serves the mem, and its registry entry gets no tombstone.
    let written = refs_and_objects(Path::new(&gitdir));
    ok(&view, &["mem", "unregister", "notes", "--operator-mode"]);
    assert_eq!(refs_and_objects(Path::new(&gitdir)), written);
    assert!(ok(&holder, &["entity", "notes--from-the-view"]).contains("# From The View"));
}

/// A workspace with its own mem-repo that mounts a branch of another
/// git directory does not delete that branch: the holder's roster still
/// names the mem. The delete refuses before anything changes and names
/// the unregister, which then takes the mount off and leaves the other
/// git directory as it was.
#[test]
fn a_mem_delete_leaves_a_branch_of_another_git_directory_where_it_is() {
    let root = TempDir::new().unwrap();
    let holder = holder(root.path());
    let gitdir = holder_gitdir(&holder);
    let view = root.path().join("view");
    fs::create_dir_all(&view).unwrap();
    ok(&view, &["mem-repo", "init", "."]);
    ok(
        &view,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "refs/heads/notes",
            "--gitdir",
            &gitdir,
            "--capability",
            "write",
            "--operator-mode",
        ],
    );
    let before = refs_and_objects(Path::new(&gitdir));
    let roster_before = roster(&view);
    let stderr = refused(
        &view,
        &["mem", "delete", "notes", "--operator-mode"],
        "INVALID_INPUT",
    );
    assert!(stderr.contains("mem unregister notes"), "{stderr}");
    assert_eq!(refs_and_objects(Path::new(&gitdir)), before);
    assert_eq!(roster(&view), roster_before);

    ok(&view, &["mem", "unregister", "notes", "--operator-mode"]);
    assert_eq!(refs_and_objects(Path::new(&gitdir)), before);
    assert!(ok(&holder, &["entity", "notes--held"]).contains("# Held"));
}

/// The refusals of the first criterion leave the roster file as it was,
/// byte for byte: a name already mounted, a branch that does not exist,
/// a git directory that is no mem-repo. And the create rules admit the
/// name as they do for `mem init`.
#[test]
fn a_refused_mount_leaves_the_roster_as_it_was() {
    let root = TempDir::new().unwrap();
    let holder = holder(root.path());
    let gitdir = holder_gitdir(&holder);
    let view = root.path().join("view");
    fs::create_dir_all(&view).unwrap();
    let mount = |name: &str, branch: &str, gitdir: &str| -> Vec<String> {
        ["mem", "mount", name, "--branch", branch, "--gitdir", gitdir]
            .iter()
            .map(|s| s.to_string())
            .collect()
    };
    let args = |v: &[String], extra: &[&str]| -> Vec<String> {
        v.iter()
            .cloned()
            .chain(extra.iter().map(|s| s.to_string()))
            .collect()
    };
    let run_refused = |v: Vec<String>, code: &str| {
        let refs: Vec<&str> = v.iter().map(String::as_str).collect();
        refused(&view, &refs, code)
    };

    let first = args(
        &mount("notes", "notes", &gitdir),
        &["--new-workspace", "--operator-mode"],
    );
    let refs: Vec<&str> = first.iter().map(String::as_str).collect();
    ok(&view, &refs);
    let standing = roster(&view);

    // Already mounted, under either capability.
    run_refused(
        args(&mount("notes", "notes", &gitdir), &["--operator-mode"]),
        "MEM_NAME_COLLISION",
    );
    run_refused(
        args(
            &mount("notes", "notes", &gitdir),
            &["--operator-mode", "--capability", "write"],
        ),
        "MEM_NAME_COLLISION",
    );
    // A branch that does not exist.
    run_refused(
        args(
            &mount("other", "no-such-branch", &gitdir),
            &["--operator-mode"],
        ),
        "UNKNOWN_REF",
    );
    // A git directory that is no mem-repo, and a path that is none.
    let plain = root.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    git(&plain, &["init", "-q"]);
    let plain_gitdir = plain.join(".git");
    run_refused(
        args(
            &mount("other", "main", plain_gitdir.to_str().unwrap()),
            &["--operator-mode"],
        ),
        "INVALID_INPUT",
    );
    run_refused(
        args(
            &mount(
                "other",
                "notes",
                root.path().join("nowhere").to_str().unwrap(),
            ),
            &["--operator-mode"],
        ),
        "INVALID_INPUT",
    );
    // A schema that contradicts the mem's registry entry: never re-pinned.
    run_refused(
        args(
            &mount("again", "notes", &gitdir),
            &["--operator-mode", "--schema", "default@1.3.0"],
        ),
        "INVALID_INPUT",
    );
    // Without operator mode the workspace's create rules decide.
    run_refused(mount("again", "notes", &gitdir), "MEM_PATH_NOT_ALLOWED");
    assert_eq!(
        roster(&view),
        standing,
        "every refusal left the roster alone"
    );

    // A rule that admits the name admits the mount.
    ok(
        &view,
        &["workspace", "allow-create", "--schema", "*", "again"],
    );
    let admitted = mount("again", "notes", &gitdir);
    let refs: Vec<&str> = admitted.iter().map(String::as_str).collect();
    ok(&view, &refs);
    assert!(ok(&view, &["entity", "again--held"]).contains("# Held"));
}

/// Two stores and a bare remote. A holds the mem `notes` and pushes it
/// alone, so the remote carries the branch and no registry.
struct Remote {
    _root: TempDir,
    a: PathBuf,
    b: PathBuf,
}

fn pushed_alone() -> Remote {
    let root = TempDir::new().unwrap();
    let remote = root.path().join("remote.git");
    git(root.path(), &["init", "-q", "--bare", "remote.git"]);
    let a = root.path().join("a");
    let b = root.path().join("b");
    for (ws, with_mem) in [(&a, true), (&b, false)] {
        fs::create_dir_all(ws).unwrap();
        ok(ws, &["mem-repo", "init", "."]);
        ok(ws, &["workspace", "allow-create", "--schema", "*", "*"]);
        ok(
            ws,
            &["mem-repo", "remote-add", "origin", remote.to_str().unwrap()],
        );
        if with_mem {
            ok(ws, &["mem", "init", "notes", "--schema", "default@1.3.0"]);
        }
    }
    let out = run(
        &a,
        &[
            "create",
            "--mem",
            "notes",
            "--title",
            "From A",
            "--type",
            "spec",
            "--metadata",
            "level=M0",
            "--section",
            "identity=x",
            "--section",
            "purpose=y",
            "--identity",
            "writer-a",
            "--note",
            "the first write",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    ok(&a, &["push", "notes"]);
    Remote { _root: root, a, b }
}

/// A mem pushed alone from store A is mounted writable in store B with
/// the verb: the local branch tip in B equals the tip A pushed (no seed
/// commit), B reads A's entities and their history, and after A pushes
/// a further write a pull in B fast-forwards.
#[test]
fn a_mem_pushed_alone_from_another_store_is_adopted_with_its_history() {
    let f = pushed_alone();
    let pushed_tip = git(&f.a.join("mem-repo"), &["rev-parse", "refs/heads/notes"]);
    let json = ok(
        &f.b,
        &[
            "mem",
            "mount",
            "notes",
            "--remote",
            "origin",
            "--schema",
            "default@1.3.0",
            "--capability",
            "write",
            "--json",
        ],
    );
    let mounted: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(mounted["head"], pushed_tip.as_str(), "{mounted}");
    assert_eq!(mounted["fetched"], true);
    assert_eq!(mounted["config_written"], true);
    assert_eq!(
        git(&f.b.join("mem-repo"), &["rev-parse", "refs/heads/notes"]),
        pushed_tip,
        "no seed commit: B's branch is A's"
    );

    // A's entity and its history, as A wrote it.
    assert!(ok(&f.b, &["entity", "notes--from-a"]).contains("# From A"));
    let feed = ok(
        &f.b,
        &[
            "changes",
            "--mem",
            "notes",
            "--since",
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
            "--include-notes",
            "--json",
        ],
    );
    let feed: serde_json::Value = serde_json::from_str(&feed).unwrap();
    let notes = feed["notes"].as_array().unwrap();
    assert!(
        notes
            .iter()
            .any(|n| n["identity"] == "writer-a" && n["note"] == "the first write"),
        "{feed}"
    );

    // A further write of A's reaches B by a fast-forward pull.
    assert!(
        run(
            &f.a,
            &[
                "create",
                "--mem",
                "notes",
                "--title",
                "Later",
                "--type",
                "spec",
                "--metadata",
                "level=M0",
                "--section",
                "identity=x",
                "--section",
                "purpose=y",
            ],
        )
        .status
        .success()
    );
    ok(&f.a, &["push", "notes"]);
    ok(&f.b, &["pull", "notes"]);
    assert!(ok(&f.b, &["entity", "notes--later"]).contains("# Later"));
    assert_eq!(
        git(&f.b.join("mem-repo"), &["rev-parse", "refs/heads/notes"]),
        git(&f.a.join("mem-repo"), &["rev-parse", "refs/heads/notes"]),
    );

    // B writes in turn, and A pulls it: one history, two stores.
    assert!(
        run(
            &f.b,
            &[
                "create",
                "--mem",
                "notes",
                "--title",
                "From B",
                "--type",
                "spec",
                "--metadata",
                "level=M0",
                "--section",
                "identity=x",
                "--section",
                "purpose=y",
            ],
        )
        .status
        .success()
    );
    ok(&f.b, &["push", "notes"]);
    ok(&f.a, &["pull", "notes"]);
    assert!(ok(&f.a, &["entity", "notes--from-b"]).contains("# From B"));
}

/// Without `--schema`, and with no registry entry for the mem in any
/// place the verb reads, the verb refuses naming the missing pin and
/// mounts nothing: no roster row, no local branch, no registry entry.
/// A remote that is not configured refuses `UNKNOWN_REMOTE`, a mem the
/// remote does not hold `UNKNOWN_REF`.
#[test]
fn a_remote_mount_never_invents_a_pin() {
    let f = pushed_alone();
    let repo = f.b.join("mem-repo");
    let refs_before = git(&repo, &["for-each-ref", "refs/heads"]);
    let roster_before = roster(&f.b);

    let stderr = refused(
        &f.b,
        &[
            "mem",
            "mount",
            "notes",
            "--remote",
            "origin",
            "--capability",
            "write",
        ],
        "INVALID_INPUT",
    );
    assert!(stderr.contains("--schema"), "{stderr}");
    assert!(stderr.contains("schema pin"), "{stderr}");
    assert_eq!(roster(&f.b), roster_before);
    assert_eq!(
        git(&repo, &["for-each-ref", "refs/heads"]),
        refs_before,
        "no local branch and no registry entry were left behind"
    );

    refused(
        &f.b,
        &[
            "mem",
            "mount",
            "notes",
            "--remote",
            "nowhere",
            "--schema",
            "default@1.3.0",
        ],
        "UNKNOWN_REMOTE",
    );
    refused(
        &f.b,
        &[
            "mem",
            "mount",
            "absent",
            "--remote",
            "origin",
            "--schema",
            "default@1.3.0",
        ],
        "UNKNOWN_REF",
    );
    assert_eq!(roster(&f.b), roster_before);
    assert_eq!(git(&repo, &["for-each-ref", "refs/heads"]), refs_before);
}

/// Where the remote's registry lists the mem (the holder published it
/// with `push --all`), the mount takes the mem's entry from there:
/// no `--schema` is needed and the version travels.
#[test]
fn a_remote_mount_takes_the_entry_the_remotes_registry_holds() {
    let f = pushed_alone();
    ok(&f.a, &["mem", "set-version", "notes", "2.3.4"]);
    ok(&f.a, &["push", "--all"]);
    ok(
        &f.b,
        &[
            "mem",
            "mount",
            "notes",
            "--remote",
            "origin",
            "--capability",
            "write",
        ],
    );
    let listed = ok(&f.b, &["mem", "list"]);
    assert!(listed.contains("2.3.4"), "{listed}");
    assert!(listed.contains("default@1.3.0"), "{listed}");
    // Unregistered and mounted again, from the branch that is there now.
    ok(&f.b, &["mem", "unregister", "notes", "--operator-mode"]);
    ok(
        &f.b,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "notes",
            "--capability",
            "write",
        ],
    );
    assert!(ok(&f.b, &["entity", "notes--from-a"]).contains("# From A"));
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

/// The local branches of a store's mem-repo with their tips, the
/// registry ref among them.
fn heads(ws: &Path) -> String {
    git(
        &ws.join("mem-repo"),
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
        ],
    )
}

/// A read-only mount of a git directory in which no engine has written
/// yet (it holds no lock file for the branch), the reads through it, a
/// write it refuses and its unregister lay no file there at all. The
/// directory is not even writable for the whole sequence.
#[cfg(unix)]
#[test]
fn a_read_only_mount_lays_no_file_where_the_mem_lives() {
    use std::os::unix::fs::PermissionsExt;

    let f = pushed_alone();
    ok(&f.a, &["push", "--all"]);
    let remote = f._root.path().join("remote.git").canonicalize().unwrap();
    assert!(!remote.join("memstead-locks").exists());
    let view = f._root.path().join("view");
    fs::create_dir_all(&view).unwrap();
    let before = every_file(&remote);
    let writable = fs::metadata(&remote).unwrap().permissions();
    fs::set_permissions(&remote, fs::Permissions::from_mode(0o555)).unwrap();

    let run_all = || {
        ok(
            &view,
            &[
                "mem",
                "mount",
                "notes",
                "--branch",
                "notes",
                "--gitdir",
                remote.to_str().unwrap(),
                "--new-workspace",
                "--operator-mode",
            ],
        );
        assert!(ok(&view, &["entity", "notes--from-a"]).contains("# From A"));
        assert!(ok(&view, &["mem", "list"]).contains("`notes`"));
        refused(
            &view,
            &[
                "create",
                "--mem",
                "notes",
                "--title",
                "Not Through Here",
                "--type",
                "spec",
                "--metadata",
                "level=M0",
                "--section",
                "identity=x",
                "--section",
                "purpose=y",
            ],
            "READ_ONLY_MOUNT",
        );
        refused(
            &view,
            &[
                "update",
                "notes--from-a",
                "--section",
                "purpose=z",
                "--auto-hash",
            ],
            "READ_ONLY_MOUNT",
        );
        refused(&view, &["delete", "notes--from-a"], "READ_ONLY_MOUNT");
        refused(
            &view,
            &["rename", "notes--from-a", "Elsewhere", "--auto-hash"],
            "READ_ONLY_MOUNT",
        );
        refused(
            &view,
            &[
                "retype",
                "notes--from-a",
                "--type",
                "decision",
                "--auto-hash",
            ],
            "READ_ONLY_MOUNT",
        );
        refused(
            &view,
            &["mem", "set-schema", "notes", "default@1.3.0"],
            "READ_ONLY_MOUNT",
        );
        refused(
            &view,
            &["mem", "set-version", "notes", "9.9.9"],
            "READ_ONLY_MOUNT",
        );
        ok(&view, &["mem", "unregister", "notes", "--operator-mode"]);
    };
    run_all();
    let after = every_file(&remote);
    fs::set_permissions(&remote, writable).unwrap();
    assert!(
        after == before,
        "the mount, its reads, its refused writes and its unregister laid nothing there"
    );
    assert!(!roster(&view).contains("notes"), "{}", roster(&view));
}

/// A mount refused on its inputs takes nothing that stood before the
/// call: the registry entry the store already held stays, no branch is
/// left behind, the roster is as it was, and the mount that follows
/// still finds the entry.
#[test]
fn a_refused_mount_takes_nothing_that_stood_before() {
    let f = pushed_alone();
    ok(&f.a, &["mem", "set-version", "notes", "2.3.4"]);
    ok(&f.a, &["push", "--all"]);
    ok(&f.b, &["pull", "--all"]);
    let (heads_before, roster_before) = (heads(&f.b), roster(&f.b));
    assert!(
        !heads_before.contains("refs/heads/notes "),
        "{heads_before}"
    );

    // A pin that contradicts the entry.
    let stderr = refused(
        &f.b,
        &[
            "mem",
            "mount",
            "notes",
            "--remote",
            "origin",
            "--schema",
            "default@1.2.0",
            "--capability",
            "write",
        ],
        "INVALID_INPUT",
    );
    assert!(stderr.contains("never re-pins"), "{stderr}");
    assert_eq!(heads(&f.b), heads_before, "no branch, no registry commit");
    assert_eq!(roster(&f.b), roster_before);

    // A name the create rules do not admit, in a third store that has
    // the entry and no rule.
    let c = f._root.path().join("c");
    fs::create_dir_all(&c).unwrap();
    ok(&c, &["mem-repo", "init", "."]);
    let remote = f._root.path().join("remote.git");
    ok(
        &c,
        &["mem-repo", "remote-add", "origin", remote.to_str().unwrap()],
    );
    ok(&c, &["pull", "--all"]);
    let (c_heads, c_roster) = (heads(&c), roster(&c));
    refused(
        &c,
        &[
            "mem",
            "mount",
            "notes",
            "--remote",
            "origin",
            "--capability",
            "write",
        ],
        "MEM_PATH_NOT_ALLOWED",
    );
    assert_eq!(heads(&c), c_heads, "no branch, no registry commit");
    assert_eq!(roster(&c), c_roster);

    // The entry still stands: the mount needs no `--schema`.
    ok(
        &f.b,
        &[
            "mem",
            "mount",
            "notes",
            "--remote",
            "origin",
            "--capability",
            "write",
            "--operator-mode",
        ],
    );
    let listed = ok(&f.b, &["mem", "list"]);
    assert!(
        listed.contains("2.3.4") && listed.contains("default@1.3.0"),
        "{listed}"
    );
}

/// A pin this workspace cannot resolve refuses before anything is
/// written: no branch, no registry commit, no roster file.
#[test]
fn a_mount_under_a_schema_that_does_not_resolve_writes_nothing() {
    let f = pushed_alone();
    let heads_before = heads(&f.b);
    let stderr = refused(
        &f.b,
        &[
            "mem",
            "mount",
            "notes",
            "--remote",
            "origin",
            "--schema",
            "widgets@0.1.0",
            "--capability",
            "write",
        ],
        "INVALID_INPUT",
    );
    assert!(stderr.contains("SCHEMA_NOT_FOUND"), "{stderr}");
    assert_eq!(heads(&f.b), heads_before);
    assert_eq!(roster(&f.b), "", "no roster file was laid");
}

/// `--new-workspace` with a mount that is refused leaves no workspace
/// behind: the directory is as empty as before.
#[test]
fn a_refused_mount_leaves_no_new_workspace() {
    let root = TempDir::new().unwrap();
    let holder = holder(root.path());
    let gitdir = holder_gitdir(&holder);
    let view = root.path().join("view");
    fs::create_dir_all(&view).unwrap();
    refused(
        &view,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "nowhere",
            "--gitdir",
            &gitdir,
            "--new-workspace",
            "--operator-mode",
        ],
        "UNKNOWN_REF",
    );
    refused(
        &view,
        &[
            "mem",
            "mount",
            "notes",
            "--branch",
            "notes",
            "--gitdir",
            &gitdir,
            "--new-workspace",
        ],
        "MEM_PATH_NOT_ALLOWED",
    );
    assert_eq!(
        fs::read_dir(&view).unwrap().count(),
        0,
        "a refused mount made no workspace"
    );
}
