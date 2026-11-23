//! Where the automatic pack of a mem-repo must not act, and what it
//! must never cost a write.
//!
//! The engine starts `git gc --auto` after a write once enough loose
//! objects have gathered. These tests run `memstead` with a search path
//! of their own: with the real `git`, with no `git` at all, and with a
//! stand-in that records how it was called and fails. Two loose objects
//! whose hash begins with `17` are laid into the git directory to bring
//! it over a lowered threshold, since git judges by that one bucket.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use tempfile::TempDir;

/// Blob contents whose git object id begins with `17`.
const SEEDS: [&str; 2] = ["pack-seed-24\n", "pack-seed-178\n"];

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// `memstead` in `ws`, with `path` as its search path when given.
fn run(ws: &Path, path: Option<&Path>, args: &[&str]) -> (bool, String) {
    let mut command = memstead();
    command.current_dir(ws).args(args).arg("--quiet");
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let out = command.output().unwrap();
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
    let (succeeded, printed) = run(ws, None, args);
    assert!(
        succeeded,
        "`memstead {}` failed:\n{printed}",
        args.join(" ")
    );
}

fn spec_args<'a>(mem: &'a str, title: &'a str) -> Vec<&'a str> {
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

fn git(gitdir: &Path, args: &[&str], stdin: Option<&str>) {
    use std::io::Write;
    let mut child = StdCommand::new("git")
        .arg("--git-dir")
        .arg(gitdir)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("git runs");
    if let Some(text) = stdin {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A workspace with the git-branch mem `notes` and the folder mem
/// `papers`. The git directory's automatic threshold is lowered to one
/// object in the bucket git judges by, and git is told to pack in the
/// foreground so a test can look right after a write.
fn workspace(root: &Path) -> (PathBuf, PathBuf) {
    let ws = root.join("ws");
    fs::create_dir_all(&ws).unwrap();
    ok(&ws, &["mem-repo", "init", "."]);
    ok(&ws, &["workspace", "allow-create", "--schema", "*", "*"]);
    ok(&ws, &["mem", "init", "notes", "--schema", "default@1.3.0"]);
    ok(
        &ws,
        &[
            "mem",
            "init",
            "papers",
            "--storage",
            "folder",
            "--schema",
            "default@1.3.0",
        ],
    );
    let gitdir = ws.join("mem-repo").join(".git");
    git(&gitdir, &["config", "gc.auto", "256"], None);
    git(&gitdir, &["config", "gc.autoDetach", "false"], None);
    (ws, gitdir)
}

/// Bring the git directory over the lowered threshold.
fn seed_the_bucket(gitdir: &Path) {
    for seed in SEEDS {
        git(gitdir, &["hash-object", "-w", "--stdin"], Some(seed));
    }
    let bucket = fs::read_dir(gitdir.join("objects").join("17"))
        .expect("the bucket exists")
        .count();
    assert!(bucket >= 2, "the seeds landed in the bucket: {bucket}");
}

fn pack_files(gitdir: &Path) -> usize {
    fs::read_dir(gitdir.join("objects").join("pack"))
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".pack"))
                .count()
        })
        .unwrap_or(0)
}

/// A directory to use as the whole search path, holding a `git` that
/// appends its arguments to a log and exits with `status`.
fn stand_in(root: &Path, status: i32) -> (PathBuf, PathBuf) {
    let dir = root.join("bin");
    fs::create_dir_all(&dir).unwrap();
    let log = root.join("git-calls.log");
    let script = format!(
        "#!/bin/sh\necho \"$@\" >> \"{}\"\nexit {status}\n",
        log.display()
    );
    let path = dir.join("git");
    fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    (dir, log)
}

/// The control for the others: with the real `git` on the path, a write
/// into a git directory that is over its threshold packs it.
#[cfg(unix)]
#[test]
fn a_write_over_the_threshold_packs_the_git_directory() {
    let root = TempDir::new().unwrap();
    let (ws, gitdir) = workspace(root.path());
    seed_the_bucket(&gitdir);
    assert_eq!(pack_files(&gitdir), 0);
    let (succeeded, printed) = run(&ws, None, &spec_args("notes", "First"));
    assert!(succeeded, "{printed}");
    assert!(pack_files(&gitdir) >= 1, "the write started the pack");
    let (succeeded, printed) = run(&ws, None, &["entity", "notes--first"]);
    assert!(succeeded, "{printed}");
}

/// With no `git` on the path a write over the threshold succeeds, says
/// nothing about packing, and leaves every object loose.
#[cfg(unix)]
#[test]
fn without_git_on_the_path_a_write_succeeds_and_nothing_is_packed() {
    let root = TempDir::new().unwrap();
    let (ws, gitdir) = workspace(root.path());
    seed_the_bucket(&gitdir);
    let empty = root.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    for title in ["First", "Second"] {
        let (succeeded, printed) = run(&ws, Some(&empty), &spec_args("notes", title));
        assert!(succeeded, "{printed}");
        let lowered = printed.to_lowercase();
        for word in ["warning", "error", "pack", " gc"] {
            assert!(!lowered.contains(word), "`{word}` in: {printed}");
        }
    }
    assert_eq!(pack_files(&gitdir), 0, "nothing was packed");
    let (succeeded, printed) = run(&ws, Some(&empty), &["entity", "notes--second"]);
    assert!(succeeded, "{printed}");
}

/// With a `git` that fails, a write over the threshold still succeeds
/// and reports success: the pack was asked for, and its failure is not
/// the write's.
#[cfg(unix)]
#[test]
fn a_git_that_fails_does_not_fail_the_write() {
    let root = TempDir::new().unwrap();
    let (ws, gitdir) = workspace(root.path());
    seed_the_bucket(&gitdir);
    let (bin, log) = stand_in(root.path(), 1);
    let (succeeded, printed) = run(&ws, Some(&bin), &spec_args("notes", "First"));
    assert!(succeeded, "{printed}");
    let lowered = printed.to_lowercase();
    for word in ["warning", "error"] {
        assert!(!lowered.contains(word), "`{word}` in: {printed}");
    }
    let calls = fs::read_to_string(&log).expect("git was asked to pack");
    assert!(calls.contains("gc --auto"), "{calls}");
    assert!(
        !calls.contains("prune"),
        "no expiry of the engine's own: {calls}"
    );
    assert_eq!(pack_files(&gitdir), 0);
    let (succeeded, printed) = run(&ws, Some(&bin), &["entity", "notes--first"]);
    assert!(succeeded, "{printed}");
}

/// Below the threshold a write starts no `git` process, and neither
/// does a write into a folder mem, over the threshold or not.
#[cfg(unix)]
#[test]
fn below_the_threshold_and_in_a_folder_mem_no_git_process_is_started() {
    let root = TempDir::new().unwrap();
    let (ws, gitdir) = workspace(root.path());
    let (bin, log) = stand_in(root.path(), 0);
    for title in ["First", "Second", "Third"] {
        let (succeeded, printed) = run(&ws, Some(&bin), &spec_args("notes", title));
        assert!(succeeded, "{printed}");
    }
    assert!(
        !log.exists(),
        "below the threshold: {}",
        fs::read_to_string(&log).unwrap_or_default()
    );

    seed_the_bucket(&gitdir);
    let (succeeded, printed) = run(&ws, Some(&bin), &spec_args("papers", "On Paper"));
    assert!(succeeded, "{printed}");
    assert!(
        !log.exists(),
        "a folder mem's write: {}",
        fs::read_to_string(&log).unwrap_or_default()
    );

    // The same git directory, written into: now git is asked.
    let (succeeded, printed) = run(&ws, Some(&bin), &spec_args("notes", "Fourth"));
    assert!(succeeded, "{printed}");
    assert!(
        fs::read_to_string(&log)
            .unwrap_or_default()
            .contains("gc --auto"),
        "over the threshold a write into the git directory asks git"
    );
}
