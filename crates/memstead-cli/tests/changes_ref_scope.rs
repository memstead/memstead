//! `memstead changes` takes a cursor of the named mem's own history.
//!
//! One mem-repo holds the mems `alpha` and `gamma`; a second workspace
//! mounts `alpha` alone. A cursor that names `gamma`'s branch or a
//! commit of its history refuses exactly as a cursor that names nothing
//! does, and nothing of `gamma` is printed.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::Command;
use tempfile::TempDir;

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

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
        "`memstead {}` failed:\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Everything a refused command printed, on both streams.
fn refused(ws: &Path, args: &[&str], code: &str) -> String {
    let out = run(ws, args);
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !out.status.success(),
        "`memstead {}` should refuse",
        args.join(" ")
    );
    assert!(
        printed.contains(code),
        "`memstead {}` should refuse {code}: {printed}",
        args.join(" ")
    );
    printed
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {}", args.join(" "));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
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

#[test]
fn a_cursor_into_a_mem_the_workspace_does_not_mount_is_no_cursor() {
    let root = TempDir::new().unwrap();
    let store = root.path().join("store");
    let view = root.path().join("view");
    fs::create_dir_all(&store).unwrap();
    fs::create_dir_all(&view).unwrap();
    ok(&store, &["mem-repo", "init", "."]);
    ok(&store, &["workspace", "allow-create", "--schema", "*", "*"]);
    for mem in ["alpha", "gamma"] {
        ok(&store, &["mem", "init", mem, "--schema", "default@1.3.0"]);
    }
    spec(&store, "alpha", "Alpha One");
    spec(&store, "gamma", "Gamma One");
    let gitdir = store.join("mem-repo/.git").canonicalize().unwrap();
    ok(
        &view,
        &[
            "mem",
            "mount",
            "alpha",
            "--branch",
            "alpha",
            "--gitdir",
            gitdir.to_str().unwrap(),
            "--capability",
            "write",
            "--new-workspace",
            "--operator-mode",
        ],
    );
    let gamma_tip = git(&gitdir, &["rev-parse", "refs/heads/gamma"]);

    let blank = |printed: &str, cursor: &str| printed.replace(cursor, "<cursor>");
    let nothing = blank(
        &refused(
            &view,
            &["changes", "--mem", "alpha", "--since", "no-such-branch"],
            "INVALID_CURSOR",
        ),
        "no-such-branch",
    );
    assert_eq!(
        blank(
            &refused(
                &view,
                &[
                    "changes",
                    "--mem",
                    "alpha",
                    "--since",
                    "0123456789abcdef0123456789abcdef01234567",
                ],
                "INVALID_CURSOR",
            ),
            "0123456789abcdef0123456789abcdef01234567",
        ),
        nothing,
        "a hash that names nothing answers as a name that names nothing"
    );
    for cursor in [
        "gamma".to_string(),
        "refs/heads/gamma".to_string(),
        gamma_tip.clone(),
        format!("{gamma_tip}~1"),
        "refs/heads/__MEMSTEAD".to_string(),
        "main".to_string(),
    ] {
        let printed = refused(
            &view,
            &["changes", "--mem", "alpha", "--since", &cursor],
            "INVALID_CURSOR",
        );
        assert!(!printed.contains("Gamma One"), "{cursor}: {printed}");
        assert!(!printed.contains("gamma-one"), "{cursor}: {printed}");
        assert_eq!(
            blank(&printed, &cursor),
            nothing,
            "{cursor}: no different from a cursor that names nothing"
        );
    }

    // The mem's own history keeps answering.
    let all = ok(
        &view,
        &["changes", "--mem", "alpha", "--since", EMPTY_TREE, "--json"],
    );
    assert!(all.contains("alpha--alpha-one"), "{all}");
    let alpha_tip = git(&gitdir, &["rev-parse", "refs/heads/alpha"]);
    let none = ok(
        &view,
        &["changes", "--mem", "alpha", "--since", &alpha_tip, "--json"],
    );
    assert!(!none.contains("alpha--alpha-one"), "{none}");
}
