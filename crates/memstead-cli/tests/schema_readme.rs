//! `memstead schema <ref>` renders a built-in package's README for the
//! package it ships in.
//!
//! Sibling generations of a built-in carry the README of their first
//! generation verbatim: the bytes are sealed by the retention guard
//! (`memstead-schema/tests/builtin_retention.rs`), so 17 of 25 packages
//! stated a version that was not theirs and no in-place edit may fix
//! it. The render substitutes every `<name>@<x.y.z>` reference to the
//! package's OWN name with the resolved pin and leaves everything else
//! alone. Exercised for every built-in the binary carries, so a new
//! generation is covered the day it ships.

use assert_cmd::Command;
use memstead_schema::builtins::{PACKAGE_README_FILE, builtin_packages};

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// Length of a leading `MAJOR.MINOR.PATCH` in `s` (0 when absent).
fn pin_len(s: &str) -> usize {
    let mut len = 0;
    for part in 0..3 {
        let digits = s[len..].chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return 0;
        }
        len += digits;
        if part < 2 {
            if !s[len..].starts_with('.') {
                return 0;
            }
            len += 1;
        }
    }
    len
}

#[test]
fn every_builtin_renders_its_readme_with_its_own_pin() {
    let packages = builtin_packages();
    assert!(packages.len() > 1, "the binary carries built-ins");
    let mut with_readme = 0;
    for pkg in &packages {
        let pin = format!("{}@{}", pkg.name, pkg.version);
        let out = memstead()
            .current_dir(std::env::temp_dir())
            .args(["--json", "schema", &pin])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v: serde_json::Value = serde_json::from_slice(&out).expect("JSON envelope");
        assert_eq!(v["schema"], pin);
        assert_eq!(v["name"], pkg.name);
        assert_eq!(v["version"], pkg.version);
        assert_eq!(v["origin"], "builtin");
        let shipped = pkg
            .files
            .iter()
            .find(|(p, _)| p == PACKAGE_README_FILE)
            .map(|(_, b)| std::str::from_utf8(b).expect("UTF-8 README"));
        match shipped {
            None => assert!(
                v["readme"].is_null(),
                "{pin}: no README shipped, `readme` is null"
            ),
            Some(shipped) => {
                with_readme += 1;
                let served = v["readme"].as_str().expect("rendered README is a string");
                // The copy steps follow the sealed README; the rest is the
                // README itself.
                let steps = memstead_schema::builtins::fork_steps(&pkg.name, &pkg.version);
                let rendered = served
                    .strip_suffix(steps.as_str())
                    .unwrap_or_else(|| panic!("{pin}: the copy steps follow the README"));
                let needle = format!("{}@", pkg.name);
                // Every own-name pin in the rendered text is the package's
                // own version; the shipped bytes are the proof the render
                // did something only where the stale pin was present.
                let mut own_pins = 0;
                for (i, _) in rendered.match_indices(&needle) {
                    let tail = &rendered[i + needle.len()..];
                    let bounded = rendered[..i]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'));
                    if bounded && pin_len(tail) > 0 {
                        own_pins += 1;
                        assert!(
                            tail.starts_with(&pkg.version),
                            "{pin}: rendered README states {}",
                            &rendered[i..i + needle.len() + pin_len(tail)]
                        );
                    }
                }
                if shipped.contains(&needle) {
                    assert!(own_pins > 0, "{pin}: the shipped README pins its own name");
                }
                // Nothing but own-name pins changes: the rendered text with
                // its own pins blanked equals the shipped text with every
                // own-name pin blanked.
                let blank = |text: &str| {
                    let mut out = String::new();
                    let mut rest = text;
                    while let Some(at) = rest.find(&needle) {
                        let (before, tail) = rest.split_at(at);
                        let after = &tail[needle.len()..];
                        out.push_str(before);
                        out.push_str(&needle);
                        let n = pin_len(after);
                        rest = &after[n..];
                    }
                    out.push_str(rest);
                    out
                };
                assert_eq!(
                    blank(rendered),
                    blank(shipped),
                    "{pin}: only own-name pins change"
                );
            }
        }
    }
    assert!(with_readme > 0, "at least one built-in ships a README");
}

#[test]
fn bare_name_reads_newest_generation_and_markdown_carries_the_pin() {
    let packages = builtin_packages();
    let mut newest: Option<&str> = None;
    for pkg in packages.iter().filter(|p| p.name == "planning") {
        let v = semver::Version::parse(&pkg.version).expect("semver");
        let cur = newest.map(|s| semver::Version::parse(s).expect("semver"));
        if cur.is_none_or(|c| v > c) {
            newest = Some(&pkg.version);
        }
    }
    let newest = newest.expect("planning is a built-in");
    let out = memstead()
        .current_dir(std::env::temp_dir())
        .args(["schema", "planning"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.starts_with(&format!("<!-- planning@{newest}: built-in package README")),
        "markdown names the resolved pin first: {}",
        text.lines().next().unwrap_or("")
    );
    assert!(text.contains(&format!("planning@{newest}")));
}

#[test]
fn unknown_reference_refuses_typed_and_subcommands_still_route() {
    memstead()
        .current_dir(std::env::temp_dir())
        .args(["schema", "not-a-builtin"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("SCHEMA_NOT_FOUND"));
    memstead()
        .current_dir(std::env::temp_dir())
        .args(["schema"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("INVALID_INPUT"));
    // The subcommands keep their names: `install` with no workspace
    // refuses on the workspace, not on argument parsing.
    let dir = tempfile::tempdir().unwrap();
    memstead()
        .current_dir(dir.path())
        .args(["schema", "install", "planning@0.1.0"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("WORKSPACE_NOT_INITIALISED"));
}

fn run_ok(dir: &std::path::Path, args: &[&str]) -> String {
    let out = memstead()
        .current_dir(dir)
        .args(args)
        .arg("--quiet")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).unwrap()
}

/// What the read of a scaffolded, installed package must name: the
/// scaffold's entity type and its relationship vocabulary.
fn assert_summarises_the_scaffold(text: &str, pin: &str) {
    assert!(text.contains(pin), "names the pin: {text}");
    assert!(
        text.contains("## Entity types") && text.contains("`note`"),
        "{text}"
    );
    assert!(
        text.contains("## Relationships") && text.contains("`REFERENCES`"),
        "{text}"
    );
}

/// A schema that was scaffolded and installed is readable with the verb
/// that reads a built-in, although the scaffold writes no README: in a
/// mem-repo workspace, in a folder workspace, and in a workspace that
/// reaches the package only through the git directory of a git-branch
/// mount.
#[test]
fn an_installed_schema_without_a_readme_is_shown_from_what_it_declares() {
    let tmp = tempfile::TempDir::new().unwrap();

    // A mem-repo workspace: the package is sealed on the registry ref.
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_ok(&repo, &["mem-repo", "init", ".", "--no-gitignore"]);
    run_ok(&repo, &["schema", "new", "fieldnote"]);
    assert!(!repo.join("fieldnote").join("README.md").exists());
    run_ok(&repo, &["schema", "install", "fieldnote"]);
    let text = run_ok(&repo, &["schema", "fieldnote@0.1.0"]);
    assert_summarises_the_scaffold(&text, "fieldnote@0.1.0");
    let json: serde_json::Value =
        serde_json::from_str(&run_ok(&repo, &["--json", "schema", "fieldnote@0.1.0"])).unwrap();
    assert_eq!(json["origin"], "workspace");
    assert!(json["readme"].is_null());
    assert_summarises_the_scaffold(json["summary"].as_str().unwrap(), "fieldnote@0.1.0");

    // A folder workspace: the package sits in `.memstead/schemas/`.
    let folder = tmp.path().join("folder");
    std::fs::create_dir_all(&folder).unwrap();
    run_ok(
        &folder,
        &["init", "--name", "home", "--schema", "default@1.3.0"],
    );
    run_ok(&folder, &["schema", "new", "fieldnote"]);
    run_ok(&folder, &["schema", "install", "fieldnote"]);
    let text = run_ok(&folder, &["schema", "fieldnote@0.1.0"]);
    assert_summarises_the_scaffold(&text, "fieldnote@0.1.0");

    // A workspace of engine state only, which reaches the package
    // through the git directory its one git-branch mount names.
    let view = tmp.path().join("view");
    std::fs::create_dir_all(view.join(".memstead").join("state")).unwrap();
    std::fs::write(
        view.join(".memstead").join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    let gitdir = std::fs::canonicalize(repo.join("mem-repo").join(".git")).unwrap();
    std::fs::write(
        view.join(".memstead").join("state").join("mounts.json"),
        serde_json::json!({
            "format": "memstead-mounts-3",
            "mounts": [{
                "mem": "notes",
                "schema": "fieldnote@0.1.0",
                "storage": { "type": "git-branch", "gitdir": gitdir, "branch": "refs/heads/notes" },
                "capability": "read-only",
                "lifecycle": "eager",
                "cross_linkable": true
            }]
        })
        .to_string(),
    )
    .unwrap();
    let text = run_ok(&view, &["schema", "fieldnote@0.1.0"]);
    assert_summarises_the_scaffold(&text, "fieldnote@0.1.0");
}

/// A name and version installed nowhere still refuses, and a package
/// that carries a README is shown with its README as before.
#[test]
fn an_uninstalled_schema_still_refuses_and_a_readme_is_still_shown() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_ok(&repo, &["mem-repo", "init", ".", "--no-gitignore"]);

    let out = memstead()
        .current_dir(&repo)
        .args(["--json", "schema", "fieldnote@0.1.0", "--quiet"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("SCHEMA_NOT_FOUND"), "got: {text}");

    // Installed, but another version asked for: still not found.
    run_ok(&repo, &["schema", "new", "fieldnote"]);
    std::fs::write(
        repo.join("fieldnote").join("README.md"),
        "# fieldnote\n\nA note kept in the field, written by its author.\n",
    )
    .unwrap();
    run_ok(&repo, &["schema", "install", "fieldnote"]);
    memstead()
        .current_dir(&repo)
        .args(["schema", "fieldnote@0.2.0", "--quiet"])
        .assert()
        .failure();

    // With a README in the package, the README is what is shown.
    let text = run_ok(&repo, &["schema", "fieldnote@0.1.0"]);
    assert!(
        text.contains("A note kept in the field, written by its author."),
        "{text}"
    );
    assert!(
        !text.contains("## Entity types"),
        "the summary is not added: {text}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&run_ok(&repo, &["--json", "schema", "fieldnote@0.1.0"])).unwrap();
    assert!(json["readme"].as_str().is_some());
    assert!(json["summary"].is_null());
}
