//! `memstead proposal merge` and `memstead proposal list`: the CLI face
//! of `Engine::proposal_merge` and `Engine::proposal_list` (plan-proposal
//! 03). One fork end to end: the brief written to a file, the file
//! filled, the merge under two identities, the record listed, the
//! provenance read naming the proposal; the refusals a caller meets
//! first (no identity, a stale file, no file); the help texts.

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
}

/// A mem-repo workspace with one mem `alpha` and its fork `alpha-fork`,
/// where the proposer `proposer-p1` added one entity and reworded
/// another.
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
            "--identity",
            "proposer-p1",
            "--quiet",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args([
            "update",
            "alpha-fork--alpha",
            "--section",
            "identity=The alpha entity, as the proposer has it.",
            "--force",
            "--identity",
            "proposer-p1",
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

/// The brief written to a file and filled: eta adopted, alpha rejected.
fn filled_brief(ws: &Path) -> std::path::PathBuf {
    let file = ws.join("brief.json");
    memstead()
        .current_dir(ws)
        .args([
            "proposal",
            "brief",
            "alpha-fork",
            "--out",
            file.to_str().unwrap(),
            "--quiet",
        ])
        .assert()
        .success();
    let mut brief: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    brief["dispositions"]["eta"]["disposition"] = Value::String("adopt".into());
    brief["dispositions"]["alpha"]["disposition"] = Value::String("reject".into());
    brief["dispositions"]["alpha"]["reason"] = Value::String("the seed wording stands".into());
    fs::write(&file, serde_json::to_string_pretty(&brief).unwrap()).unwrap();
    file
}

/// The merge lands under the proposer's identity with the merger
/// beside; the JSON names the commits, the entities and the validation;
/// `proposal list` renders the record; `entity --provenance` names the
/// proposal and the disposition; a second merge of the same file is
/// stale.
#[test]
fn proposal_merge_lands_lists_and_shows_in_provenance() {
    let ws = seed();
    let file = filled_brief(ws.path());

    let out = memstead()
        .current_dir(ws.path())
        .args([
            "proposal",
            "merge",
            "alpha-fork",
            "--dispositions",
            file.to_str().unwrap(),
            "--identity",
            "owner-o1",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let outcome = json_stdout(&out);
    assert_eq!(outcome["fork"], "alpha-fork");
    assert_eq!(outcome["target"], "alpha");
    assert_eq!(outcome["merged_by"], "owner-o1");
    assert_eq!(outcome["merge_commits"].as_array().map(Vec::len), Some(1));
    assert_eq!(outcome["merge_commits"][0]["identity"], "proposer-p1");
    assert_eq!(
        outcome["merge_commits"][0]["entities"],
        serde_json::json!(["alpha--eta"])
    );
    assert!(outcome.get("amend_commit").is_none());
    let entities = outcome["entities"].as_array().unwrap();
    let eta = entities.iter().find(|e| e["slug"] == "eta").unwrap();
    assert_eq!(eta["disposition"], "adopt");
    assert_eq!(eta["action"], "created");
    assert_eq!(eta["proposer"], "proposer-p1");
    assert_eq!(eta["check_recorded"], true);
    let alpha = entities.iter().find(|e| e["slug"] == "alpha").unwrap();
    assert_eq!(alpha["disposition"], "reject");
    assert_eq!(alpha["action"], "none");
    assert_eq!(outcome["record_path"], ".memstead/proposals.json");
    assert_eq!(outcome["validation"]["all_entities_parse"], true);
    assert!(outcome["validation"].get("new_findings").is_none());
    let proposal_id = outcome["proposal_id"].as_str().unwrap().to_string();
    assert!(proposal_id.starts_with("alpha-fork@"), "{proposal_id}");

    // The list, as JSON and as markdown.
    let out = memstead()
        .current_dir(ws.path())
        .args(["proposal", "list", "alpha", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let record = json_stdout(&out);
    assert_eq!(record["version"], 1);
    assert_eq!(record["proposals"][0]["id"], proposal_id);
    assert_eq!(record["proposals"][0]["proposer"], "proposer-p1");
    assert_eq!(record["proposals"][0]["merged_by"], "owner-o1");
    assert_eq!(
        record["proposals"][0]["entities"]["alpha"]["disposition"],
        "reject"
    );
    assert_eq!(
        record["proposals"][0]["entities"]["alpha"]["reason"],
        "the seed wording stands"
    );
    let out = memstead()
        .current_dir(ws.path())
        .args(["proposal", "list", "alpha", "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let md = String::from_utf8_lossy(&out.stdout);
    assert!(md.contains("# Proposals merged into `alpha`"), "{md}");
    assert!(md.contains(&format!("## `{proposal_id}`")), "{md}");
    assert!(
        md.contains("- `alpha`: reject (the seed wording stands)"),
        "{md}"
    );
    assert!(md.contains("- `eta`: adopt"), "{md}");

    // The provenance read names the proposal and the disposition.
    let out = memstead()
        .current_dir(ws.path())
        .args(["entity", "alpha--eta", "--provenance", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let envelope = json_stdout(&out);
    let created = &envelope["mutation_provenance"]["created_by"];
    assert_eq!(created["identity"], "proposer-p1");
    assert_eq!(created["merged_by"], "owner-o1");
    assert_eq!(created["proposal"], proposal_id);
    assert_eq!(created["disposition"], "adopt");
    assert_eq!(envelope["mutation_provenance"]["check_state"], "checked_ok");
    let out = memstead()
        .current_dir(ws.path())
        .args(["entity", "alpha--eta", "--provenance", "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let md = String::from_utf8_lossy(&out.stdout);
    assert!(
        md.contains(&format!(
            "identity proposer-p1, merged by owner-o1 under proposal {proposal_id} (adopt)"
        )),
        "{md}"
    );

    // The same file again: stale, naming both shas.
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "proposal",
            "merge",
            "alpha-fork",
            "--dispositions",
            file.to_str().unwrap(),
            "--identity",
            "owner-o1",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let envelope = json_stdout(&out);
    assert_eq!(envelope["code"], "PROPOSAL_STALE", "{envelope}");
    assert_eq!(envelope["details"]["side"], "target");
    assert_eq!(envelope["details"]["current"], outcome["target_tip_after"]);
}

/// Without `--identity` the merge refuses `INVALID_INPUT` naming the
/// flag; a missing or unreadable file refuses `INVALID_INPUT`; nothing
/// lands either way.
#[test]
fn proposal_merge_refuses_without_identity_and_without_a_file() {
    let ws = seed();
    let file = filled_brief(ws.path());
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "proposal",
            "merge",
            "alpha-fork",
            "--dispositions",
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
            .contains("--identity"),
        "{envelope}"
    );

    let out = memstead()
        .current_dir(ws.path())
        .args([
            "proposal",
            "merge",
            "alpha-fork",
            "--dispositions",
            "never.json",
            "--identity",
            "owner-o1",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(json_stdout(&out)["code"], "INVALID_INPUT");

    let out = memstead()
        .current_dir(ws.path())
        .args(["proposal", "list", "alpha", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        json_stdout(&out)["proposals"].as_array().map(Vec::len),
        Some(0),
        "nothing landed"
    );
    let out = memstead()
        .current_dir(ws.path())
        .args(["proposal", "list", "nobody", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(json_stdout(&out)["code"], "UNKNOWN_MEM");
}

/// The help texts name the file, the identity, the refusal codes and
/// the record.
#[test]
fn proposal_merge_and_list_help_name_the_contract() {
    let out = memstead()
        .args(["proposal", "merge", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for needle in [
        "<FORK>",
        "--dispositions",
        "--identity",
        "PROPOSAL_STALE",
        "PROPOSAL_CONFLICT",
        "PROPOSAL_UNATTRIBUTED",
        "PROPOSAL_DISPOSITIONS_INCOMPLETE",
        "Merged-By",
        ".memstead/proposals.json",
    ] {
        assert!(help.contains(needle), "help lacks {needle:?}:\n{help}");
    }
    let out = memstead()
        .args(["proposal", "list", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for needle in ["<TARGET>", ".memstead/proposals.json", "UNKNOWN_MEM"] {
        assert!(help.contains(needle), "help lacks {needle:?}:\n{help}");
    }
}
