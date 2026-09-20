//! `memstead proposal brief`: the CLI face of `Engine::proposal_brief`.
//! One fork end to end (the markdown on stdout, the JSON envelope with
//! the four shas and the disposition skeleton under `--json`, the
//! disposition file `--out` writes), the two typed refusals, and the
//! help text naming the fork, the base rule and the vocabulary.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use serde_json::Value;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn write_mem_dir(root: &Path, name: &str) {
    let dir = root.join(name);
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{"schema": "default@1.0.0", "version": "0.2.0", "description": "the alpha mem"}"#,
    )
    .unwrap();
    fs::write(
        dir.join("alpha.md"),
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nThe alpha entity.\n\n## Purpose\n\nSeed.\n",
    )
    .unwrap();
    fs::write(
        dir.join("gamma.md"),
        format!(
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n\
             # Gamma\n\n## Identity\n\nBuilds on [[{name}--alpha]].\n\n## Purpose\n\nSeed.\n"
        ),
    )
    .unwrap();
}

/// A mem-repo workspace with one mem `alpha` and its fork
/// `alpha-fork`, where the fork added one entity.
fn seed() -> TempDir {
    let ws = TempDir::new().unwrap();
    write_mem_dir(ws.path(), "alpha");
    let dir = ws.path().join("alpha");
    init_real_mem_repo_from_disk(ws.path(), &[(dir.as_path(), "alpha")]);
    memstead()
        .current_dir(ws.path())
        .args([
            "mem",
            "fork",
            "alpha",
            "alpha-fork",
            "--operator-mode",
            "--quiet",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args([
            "create",
            "--mem",
            "alpha-fork",
            "--title",
            "Eta",
            "--type",
            "spec",
            "--section",
            "identity=Eta is the proposer's new card.",
            "--section",
            "purpose=A new card.",
            "--quiet",
        ])
        .assert()
        .success();
    ws
}

fn json_stdout(out: &std::process::Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(text.trim())
        .unwrap_or_else(|e| panic!("one JSON document on stdout; got:\n{text}\n({e})"))
}

/// The markdown on stdout, the JSON envelope under `--json` with the
/// four shas, one entry for the added entity and none for the
/// untouched ones, the skeleton slot, and the file `--out` writes,
/// which is the same JSON.
#[test]
fn proposal_brief_renders_markdown_json_and_the_disposition_file() {
    let ws = seed();
    let out = memstead()
        .current_dir(ws.path())
        .args(["proposal", "brief", "alpha-fork", "--quiet"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let md = String::from_utf8_lossy(&out.stdout);
    assert!(
        md.starts_with("# Proposal brief: `alpha-fork` against `alpha`"),
        "{md}"
    );
    assert!(md.contains("## `eta`: added (Eta)"), "{md}");
    assert!(md.contains("- Precheck (create in `alpha`): clean"), "{md}");
    assert!(
        !md.contains("`gamma`:"),
        "an untouched entity is absent:\n{md}"
    );

    let file = ws.path().join("brief.json");
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "proposal",
            "brief",
            "alpha-fork",
            "--out",
            file.to_str().unwrap(),
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let brief = json_stdout(&out);
    assert_eq!(brief["fork"], "alpha-fork");
    assert_eq!(brief["target"], "alpha");
    for key in ["ancestor", "base", "fork_tip", "target_tip"] {
        let sha = brief[key].as_str().unwrap_or_default();
        assert_eq!(sha.len(), 40, "{key}: {sha}");
    }
    assert_eq!(brief["base_is_ancestor"], false);
    assert_ne!(brief["base"], brief["ancestor"]);
    assert_eq!(brief["entries"].as_array().map(Vec::len), Some(1));
    assert_eq!(brief["entries"][0]["slug"], "eta");
    assert_eq!(brief["entries"][0]["status"], "added");
    assert_eq!(brief["entries"][0]["fork_id"], "alpha-fork--eta");
    assert_eq!(brief["entries"][0]["target_id"], "alpha--eta");
    assert_eq!(brief["entries"][0]["precheck"]["outcome"], "clean");
    assert_eq!(
        brief["dispositions"]["eta"],
        serde_json::json!({
            "disposition": "",
            "reason": "",
            "accepts": ["adopt", "adopt_with_changes", "reject"]
        })
    );
    let written: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written, brief, "the file is the JSON form");
}

/// A mem with no `forkedFrom` refuses `INVALID_INPUT` naming the
/// reason; an unknown mem refuses `UNKNOWN_MEM`; nothing is written
/// where `--out` pointed.
#[test]
fn proposal_brief_refuses_a_non_fork_and_an_unknown_mem() {
    let ws = seed();
    let file = ws.path().join("never.json");
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "proposal",
            "brief",
            "alpha",
            "--out",
            file.to_str().unwrap(),
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let envelope = json_stdout(&out);
    assert_eq!(envelope["code"], "INVALID_INPUT", "{envelope}");
    assert!(
        envelope["message"]
            .as_str()
            .unwrap_or_default()
            .contains("forkedFrom"),
        "{envelope}"
    );
    assert!(!file.exists(), "a refusal writes no file");

    let out = memstead()
        .current_dir(ws.path())
        .args(["proposal", "brief", "nobody", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(json_stdout(&out)["code"], "UNKNOWN_MEM");
}

/// The help text names the fork, the base rule and the disposition
/// vocabulary.
#[test]
fn proposal_brief_help_names_fork_base_rule_and_vocabulary() {
    let out = memstead()
        .args(["proposal", "brief", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for needle in [
        "<FORK>",
        "forkedFrom",
        "forkedFrom.base",
        "forkedFrom.sha",
        "adopt_with_changes",
        "reject",
        "--out",
        "INVALID_INPUT",
        "UNKNOWN_MEM",
    ] {
        assert!(help.contains(needle), "help lacks {needle:?}:\n{help}");
    }
}
