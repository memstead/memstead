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
                let rendered = v["readme"].as_str().expect("rendered README is a string");
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
        .stderr(predicates::str::contains("NO_WORKSPACE"));
}
