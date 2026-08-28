//! The markdown form of `memstead health` must serve what the include
//! gathered. Found 2026-08-27 (consistency-sweep 04/02's closing grade):
//! on a folder workspace, `--json health --include conformance` returned
//! a populated `findings` array while the default markdown form printed
//! only the summary — the flag was accepted, documented, and had no
//! effect on the rendering, so an operator diagnosing a mem by eye was
//! told nothing about content the engine was holding and reporting.
//! Runs the real binary against a quickstart (folder) workspace, the
//! shape the defect was found on.

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn stdout_of(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).expect("stdout is UTF-8")
}

/// An entity carrying one conformance violation (enum value outside the
/// declared set) and one body observation (a heading the type does not
/// declare), written raw to disk — the SIMULATION of out-of-band damage;
/// health is the read under test.
const DEFECTIVE: &str = "---\ntype: spec\ncreated_date: 2026-01-01\n\
last_modified: 2026-01-01\nlevel: M9\n---\n# Torn\n\n## Identity\n\n\
An entity with a bad enum value.\n\n## Purpose\n\nA fixture.\n\n\
## Undeclared Heading\n\nContent the type does not declare.\n";

#[test]
fn markdown_health_serves_conformance_findings() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("ws");
    memstead()
        .args(["quickstart", "--agent", "claude-code"])
        .arg(&root)
        .assert()
        .success();
    std::fs::write(root.join("torn.md"), DEFECTIVE).unwrap();

    // The JSON form carries the finding — the baseline that was already
    // true when the markdown form said nothing.
    let json_out = stdout_of(
        memstead()
            .current_dir(&root)
            .args(["--json", "health", "--include", "conformance"])
            .assert()
            .success(),
    );
    let report: serde_json::Value = serde_json::from_str(&json_out).unwrap();
    let findings = report["findings"].as_array().expect("findings array");
    assert!(
        findings
            .iter()
            .any(|f| f["code"] == "INVALID_ENUM_VALUE" && f["id"].as_str() == Some("ws--torn")),
        "JSON baseline lost the finding: {report}"
    );

    // The markdown form must serve the same data, not just the summary.
    let md = stdout_of(
        memstead()
            .current_dir(&root)
            .args(["health", "--include", "conformance"])
            .assert()
            .success(),
    );
    assert!(
        md.contains("## Conformance findings (") && md.contains("INVALID_ENUM_VALUE"),
        "markdown health dropped the conformance findings:\n{md}"
    );
    assert!(
        md.contains("## Body observations ("),
        "markdown health dropped the body observations:\n{md}"
    );
}

/// Requested-and-clean renders an explicit zero, so "no section" can
/// never be misread as "not served".
#[test]
fn markdown_health_renders_explicit_zero_when_clean() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("ws");
    memstead()
        .args(["quickstart", "--agent", "claude-code"])
        .arg(&root)
        .assert()
        .success();

    let md = stdout_of(
        memstead()
            .current_dir(&root)
            .args(["health", "--include", "conformance"])
            .assert()
            .success(),
    );
    assert!(
        md.contains("## Conformance findings (0)"),
        "a requested, clean conformance include must state its zero:\n{md}"
    );
}
