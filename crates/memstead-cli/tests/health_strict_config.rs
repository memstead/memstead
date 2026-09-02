#![cfg(feature = "mem-repo")]
//! `memstead health --strict` on the configuration axes widened on
//! 2026-08-23: a strict run exited 0 on a dogfood workspace with three
//! schema-pin mismatches, two rotted schema packages, two mounts whose
//! branches did not exist, seven stubs and ten dangling links, because
//! none of those participated. Each class gets its fixture here, the
//! refusal and the complement: stale entities, drifted anchors and a
//! generations-behind pin stay advisory, and a clean workspace exits 0.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use memstead_base::{
    FileWorkspaceStore, Mount, MountCapability, MountLifecycle, MountStorage, Workspace,
    WorkspaceStoreAdapter,
};
use memstead_git_branch::test_support::{init_real_mem_repo, init_real_mem_repo_from_disk};
use predicates::str::contains;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// A folder mem with one entity under the built-in `default@1.0.0`.
fn write_default_mem(dir: &Path, entity_body: &str) {
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    fs::write(
        dir.join("alpha.md"),
        format!(
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\n{entity_body}\n\n## Purpose\n\nA fixture.\n"
        ),
    )
    .unwrap();
}

/// A real mem-repo workspace with one healthy mem `hold`, returning its
/// gitdir so tests can append fabricated mounts.
fn healthy_workspace(root: &Path) -> std::path::PathBuf {
    let dir = root.join("hold");
    fs::create_dir_all(&dir).unwrap();
    write_default_mem(&dir, "The anchor entity of the healthy mem.");
    init_real_mem_repo_from_disk(root, &[(&dir, "hold")]);
    root.join("mem-repo").join(".git")
}

fn load_workspace(root: &Path) -> Workspace {
    FileWorkspaceStore::new().load(root).unwrap()
}

fn save_workspace(root: &Path, ws: &Workspace) {
    FileWorkspaceStore::new().save_state(root, ws).unwrap();
}

fn mount(mem: &str, storage: MountStorage, schema: &str) -> Mount {
    Mount {
        mem: mem.to_string(),
        schema: Some(schema.parse().unwrap()),
        storage,
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    }
}

/// Run `health --json --strict` with the includes; returns (exit code,
/// the report JSON, the trailing strict envelope text if any).
fn strict_health(root: &Path, includes: &[&str]) -> (i32, serde_json::Value, String) {
    let mut cmd = memstead();
    cmd.current_dir(root).args(["--json", "health", "--strict"]);
    for inc in includes {
        cmd.args(["--include", inc]);
    }
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    // The report is the first JSON document; the strict envelope, when
    // the run refuses, follows on its own line.
    let split = stdout.find("\n{\"code\":\"HEALTH_STRICT_VIOLATIONS\"");
    let (report, envelope) = match split {
        Some(i) => (&stdout[..i], stdout[i..].trim().to_string()),
        None => (stdout.as_str(), String::new()),
    };
    let json: serde_json::Value = serde_json::from_str(report)
        .unwrap_or_else(|e| panic!("health --json must print a JSON report: {e}\n{stdout}"));
    (out.status.code().unwrap_or(-1), json, envelope)
}

fn warning_codes(json: &serde_json::Value) -> Vec<String> {
    json["warnings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|w| w["code"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Three unbacked mounts beside a healthy one: a git-branch mount whose
/// branch was never created (`missing_ref`), a folder mount whose path
/// is gone (`missing_path`), a folder mount that exists and holds no
/// entity (`empty`). Each surfaces `MOUNT_UNBACKED` with its reason,
/// the healthy mount stays silent, and `--strict` refuses naming the
/// count, with no include.
#[test]
fn strict_refuses_unbacked_mounts_with_the_right_reason_each() {
    let tmp = TempDir::new().unwrap();
    let gitdir = healthy_workspace(tmp.path());
    let hollow = tmp.path().join("hollow");
    fs::create_dir_all(hollow.join(".memstead")).unwrap();
    fs::write(
        hollow.join(".memstead").join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();

    let mut ws = load_workspace(tmp.path());
    ws.mounts.push(mount(
        "ghost",
        MountStorage::GitBranch {
            gitdir: gitdir.clone(),
            branch: "refs/heads/ghost".into(),
        },
        "default@1.0.0",
    ));
    ws.mounts.push(mount(
        "gone",
        MountStorage::Folder {
            path: tmp.path().join("gone"),
        },
        "default@1.0.0",
    ));
    ws.mounts.push(mount(
        "hollow",
        MountStorage::Folder { path: hollow },
        "default@1.0.0",
    ));
    save_workspace(tmp.path(), &ws);

    let (code, json, envelope) = strict_health(tmp.path(), &[]);
    let unbacked: Vec<(&str, &str)> = json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == "MOUNT_UNBACKED")
        .map(|w| {
            (
                w["details"]["mem"].as_str().unwrap(),
                w["details"]["reason"].as_str().unwrap(),
            )
        })
        .collect();
    // Storage that is GONE now quarantines rather than serving empty
    // (04/05): the mount is configured and cannot serve, which is what
    // quarantine means, and all three backends reach the same outcome for the
    // same condition (criterion 9). It is reported there, not as an unbacked
    // warning, and the quarantine roster is rendered wherever mems are listed
    // so the mount does not simply vanish (criterion 7).
    let quarantined: Vec<(&str, &str)> = json["quarantined"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|q| {
                    (
                        q["mem"].as_str().unwrap_or(""),
                        q["reason_code"].as_str().unwrap_or(""),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    // `ghost` is git-branch backed and stays a WARNING, not a quarantine: a
    // ref that does not exist is also the normal state of a mem never pushed
    // or never cloned, and quarantining it strands push, fetch and pull. This
    // departs from criterion 9's literal parity; the session log records what
    // the attempt at parity found.
    assert!(
        unbacked.contains(&("ghost", "missing_ref")),
        "a never-created branch still warns: {unbacked:?}"
    );
    assert!(
        quarantined.contains(&("gone", "MOUNT_UNBACKED")),
        "a folder that is gone quarantines: {quarantined:?}"
    );
    // And neither is reported as merely unbacked any more.
    assert!(
        !unbacked.iter().any(|(m, _)| *m == "gone"),
        "a gone folder is quarantined, not warned: {unbacked:?}"
    );
    assert!(
        unbacked.contains(&("hollow", "empty")),
        "a folder with no entity: {unbacked:?}"
    );
    assert!(
        !unbacked.iter().any(|(m, _)| *m == "hold"),
        "the healthy mount is silent: {unbacked:?}"
    );
    // One, not three: the two gone-storage mounts moved to quarantine, and
    // `hollow` (present but holding nothing) is the only genuine unbacked
    // case left. A legitimately empty mem is never quarantined (criterion 5).
    // The empty-but-present folder and the never-created branch. A
    // legitimately empty mem is never quarantined (criterion 5).
    assert_eq!(unbacked.len(), 2);
    assert_eq!(code, 1, "strict refuses with no include needed\n{envelope}");
    assert!(
        envelope.contains("mount_unbacked: 2"),
        "the envelope names the class and count: {envelope}"
    );

    // The cold-start surface says so too: an unbacked mem's roster entry
    // names the reason and the warning rides the overview, while the
    // healthy mem's entry carries nothing of the kind.
    let overview = String::from_utf8(
        memstead()
            .current_dir(tmp.path())
            .args(["overview"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    // Gone storage is on the quarantine roster, which overview renders as
    // part of the one dashboard. The binding constraint of 04/05 is that
    // quarantine must not become a second way to disappear, so a mount that
    // left the mem roster has to be findable here, with its reason.
    assert!(
        overview.contains("## Quarantined Mems"),
        "the quarantine roster is rendered:\n{overview}"
    );
    assert!(
        overview.contains("### gone"),
        "the gone mount is listed rather than vanished:\n{overview}"
    );
    assert!(
        overview.contains("MOUNT_UNBACKED"),
        "and names why:\n{overview}"
    );
    // The present-but-empty mount stays on the ordinary roster, warned.
    assert!(overview.contains("- **Unbacked:** empty ("), "{overview}");
    let hold_entry = overview
        .split("### hold")
        .nth(1)
        .and_then(|rest| rest.split("\n### ").next())
        .expect("the healthy mem has a roster entry");
    assert!(
        !hold_entry.contains("Unbacked"),
        "a resolving mount carries no unbacked line:\n{hold_entry}"
    );
}

/// A mount whose workspace expectation disagrees with the mem's own
/// config pin: `SCHEMA_PIN_MISMATCH` already fired; now it refuses.
#[test]
fn strict_refuses_a_schema_pin_mismatch() {
    let tmp = TempDir::new().unwrap();
    healthy_workspace(tmp.path());
    let mut ws = load_workspace(tmp.path());
    let m = ws.mounts.iter_mut().find(|m| m.mem == "hold").unwrap();
    // The mem's config says default@1.0.0; the mount expects a later one.
    m.schema = Some("default@1.3.0".parse().unwrap());
    save_workspace(tmp.path(), &ws);

    let (code, json, envelope) = strict_health(tmp.path(), &[]);
    assert!(
        warning_codes(&json).contains(&"SCHEMA_PIN_MISMATCH".to_string()),
        "{json}"
    );
    assert_eq!(code, 1, "{envelope}");
    assert!(envelope.contains("schema_pin_mismatch: 1"), "{envelope}");
}

/// A pinned schema sealed under an older engine whose package no
/// longer passes authoring validation (`SCHEMA_UNSTAMPED_SOURCE_ROT`)
/// refuses; the mem itself keeps running on the tolerant seal.
#[test]
fn strict_refuses_an_unstamped_rotted_schema() {
    const MANIFEST: &str = r#"name: rotted
version: 0.1.0
description: sealed under an older engine
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    const DOC_TYPE_ROTTED: &str = r#"name: doc
description: t
when_to_use: Here
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: _default
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
propagating_relationships: []
"#;
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("hold", "rotted@0.1.0")]);
    let gitdir = tmp.path().join("mem-repo").join(".git");
    memstead_git_branch::storage_memstead::write_schema_to_memstead_ref(
        &gitdir,
        "rotted",
        "0.1.0",
        &[
            ("schema.yaml".to_string(), MANIFEST.as_bytes().to_vec()),
            (
                "types/doc.yaml".to_string(),
                DOC_TYPE_ROTTED.as_bytes().to_vec(),
            ),
        ],
    )
    .expect("seal writes");

    let (code, json, envelope) = strict_health(tmp.path(), &[]);
    assert!(
        warning_codes(&json).contains(&"SCHEMA_UNSTAMPED_SOURCE_ROT".to_string()),
        "{json}"
    );
    assert_eq!(code, 1, "{envelope}");
    assert!(
        envelope.contains("schema_unstamped_source_rot: 1"),
        "{envelope}"
    );
}

/// 04/07, criteria 3, 5 and 6: cross-mem links are gated on write and
/// default-deny, so an edge whose grant was revoked is a state the engine
/// would refuse to create today. Before this plan it stayed where it was,
/// loaded without comment and exited zero under `--strict`; the workspace's
/// own policy file had stopped describing its graph and no surface noticed.
///
/// The revocation names the edges it orphans at the moment it happens
/// (criterion 5) and is never refused (criterion 6), and a strict run
/// afterwards does not exit clean (criterion 3).
#[test]
fn revoking_a_grant_names_the_edges_it_orphans_and_strict_then_refuses() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("alpha-mem");
    let b = tmp.path().join("beta-mem");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    write_default_mem(&a, "The anchor of alpha.");
    write_default_mem(&b, "The anchor of beta.");
    init_real_mem_repo_from_disk(tmp.path(), &[(&a, "alpha"), (&b, "beta")]);

    // Grant, then write the cross-mem edge under it.
    memstead()
        .current_dir(tmp.path())
        .args(["workspace", "grant-cross-link", "alpha", "beta"])
        .assert()
        .success();
    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--mem",
            "beta",
            "--title",
            "Target",
            "--type",
            "spec",
            "--section",
            "identity=A real target.",
            "--section",
            "purpose=Exists.",
            "--metadata",
            "level=M0",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--mem",
            "alpha",
            "--title",
            "Source",
            "--type",
            "spec",
            "--section",
            "identity=Depends on beta.",
            "--section",
            "purpose=The referrer.",
            "--metadata",
            "level=M0",
            "--relation",
            "DEPENDS_ON:beta--target",
        ])
        .assert()
        .success();

    // With the grant standing, strict is clean on this axis.
    let (code, _, envelope) = strict_health(tmp.path(), &["integrity"]);
    assert_eq!(code, 0, "a permitted edge must not fail strict: {envelope}");

    // Criterion 6: the revocation is not refused and needs no force flag.
    let out = memstead()
        .current_dir(tmp.path())
        .args(["workspace", "revoke-cross-link", "alpha", "beta"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "revocation is never refused: {text}");

    // Criterion 5: it NAMES the orphaned edge, not just a count.
    assert!(
        text.contains("alpha--source") && text.contains("beta--target"),
        "the revocation must name the edges it orphaned: {text}"
    );

    // Criterion 3: strict now refuses.
    let (code, json, envelope) = strict_health(tmp.path(), &["integrity"]);
    assert_eq!(code, 1, "{envelope}");
    assert!(
        envelope.contains("ungranted_cross_mem_edges: 1"),
        "{envelope}"
    );
    let codes: Vec<&str> = json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["code"].as_str())
        .collect();
    assert!(codes.contains(&"CROSS_MEM_EDGE_UNGRANTED"), "{codes:?}");

    // Criterion 5, the precision half: a SECOND, unrelated revocation must not
    // re-report the edge the first one orphaned. Reporting the whole standing
    // set would blame every later edit for what an earlier one left behind.
    memstead()
        .current_dir(tmp.path())
        .args(["workspace", "grant-cross-link", "beta", "alpha"])
        .assert()
        .success();
    let out = memstead()
        .current_dir(tmp.path())
        .args(["workspace", "revoke-cross-link", "beta", "alpha"])
        .output()
        .unwrap();
    let second = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{second}");
    assert!(
        !second.contains("alpha--source"),
        "the alpha→beta edge was orphaned by the FIRST revocation; this one \
         orphaned nothing and must say so: {second}"
    );

    // Criterion 4, at the CLI: the mem still serves. Reading the referrer is
    // not blocked by the condition its own edge raises.
    memstead()
        .current_dir(tmp.path())
        .args(["entity", "alpha--source"])
        .assert()
        .success();

    // Criterion 9, at the CLI: the remedy needs no grant.
    memstead()
        .current_dir(tmp.path())
        .args([
            "relate",
            "alpha--source",
            "DEPENDS_ON",
            "beta--target",
            "--remove",
        ])
        .assert()
        .success();
    let (code, _, envelope) = strict_health(tmp.path(), &["integrity"]);
    assert_eq!(code, 0, "removing the edge resolves it: {envelope}");
}

/// 04/06, criteria 2 and 4: the CLI's own rendered surfaces name the
/// condition. The markdown block used to print `section: (none)` and
/// leave the reader to work out which of three problems they had, and
/// the open-questions worklist carried a parallel `dangling_link`
/// vocabulary of its own. Both now read the same three codes the
/// findings list does.
#[test]
fn cli_markdown_and_open_questions_name_the_dangling_condition() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("hold");
    fs::create_dir_all(&dir).unwrap();
    write_default_mem(&dir, "Points at [[nowhere]] on purpose.");
    init_real_mem_repo_from_disk(tmp.path(), &[(&dir, "hold")]);

    // The human-facing markdown block.
    let md = memstead()
        .current_dir(tmp.path())
        .args(["health", "--include", "dangling_links"])
        .output()
        .unwrap();
    let md = String::from_utf8(md.stdout).unwrap();
    assert!(
        md.contains("[DANGLING_LINK_TARGET_MISSING]"),
        "the rendered block must name the condition: {md}"
    );

    // The open-questions worklist, on the same condition.
    let oq = memstead()
        .current_dir(tmp.path())
        .args(["--json", "health", "--include", "open_questions"])
        .output()
        .unwrap();
    let oq = String::from_utf8(oq.stdout).unwrap();
    assert!(
        oq.contains("DANGLING_LINK_TARGET_MISSING"),
        "the worklist must carry the same code, not a vocabulary of its \
         own: {oq}"
    );
    assert!(
        !oq.contains("\"dangling_link\""),
        "the parallel kind vocabulary is retired: {oq}"
    );
}

/// A body link to a target that does not exist creates a stub; with
/// `integrity` included the run refuses on
/// `DANGLING_LINK_TARGET_MISSING` and `UNRESOLVED_STUB`, and without the
/// include it does not (the axis is opt-in, like the other
/// include-gated ones). The other two dangling conditions have their own
/// coverage in `memstead-base`; this test's subject is the strict gate,
/// and it pins which condition its fixture actually raises.
#[test]
fn strict_with_integrity_refuses_dangling_links_and_unresolved_stubs() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("hold");
    fs::create_dir_all(&dir).unwrap();
    // A hand-authored body link that resolves to nothing: a dangling
    // link on load (no stub is materialised for a loaded file).
    write_default_mem(&dir, "Points at [[nowhere]] on purpose.");
    init_real_mem_repo_from_disk(tmp.path(), &[(&dir, "hold")]);
    // An engine write with a body link to a missing target: the engine
    // auto-creates the stub, which then has exactly one referrer and
    // no body of its own (`UNRESOLVED_STUB`).
    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--mem",
            "hold",
            "--title",
            "Linker",
            "--type",
            "spec",
            "--section",
            "identity=Links to [[ghost-target]] which nobody wrote.",
            "--section",
            "purpose=Stub fixture.",
            "--metadata",
            "level=M0",
        ])
        .assert()
        .success();

    let (code, json, envelope) = strict_health(tmp.path(), &["integrity"]);
    let codes: Vec<&str> = json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["code"].as_str())
        .collect();
    // Both of this fixture's dangling links are the *target missing*
    // condition: `[[nowhere]]` resolves to nothing at all, and
    // `[[ghost-target]]` resolves to the auto-created stub, which the
    // body scan counts as missing. Neither is a link to a written
    // entity that lacks a relationship, and neither is a relationship
    // row pointing at an absent target. The old assertion pinned the
    // fused `DANGLING_LINK`, which said none of that (04/06).
    assert_eq!(
        codes
            .iter()
            .filter(|c| **c == "DANGLING_LINK_TARGET_MISSING")
            .count(),
        2,
        "{codes:?}"
    );
    assert!(
        !codes.contains(&"DANGLING_LINK_NOT_RELATED")
            && !codes.contains(&"DANGLING_RELATION_TARGET_MISSING"),
        "this fixture produces neither of the other two conditions; {codes:?}"
    );
    assert!(codes.contains(&"UNRESOLVED_STUB"), "{codes:?}");
    assert_eq!(code, 1, "{envelope}");
    assert!(envelope.contains("dangling_links:"), "{envelope}");
    assert!(envelope.contains("unresolved_stubs: 1"), "{envelope}");

    // Without `integrity` the same workspace is not refused.
    let (code, _, envelope) = strict_health(tmp.path(), &["stubs"]);
    assert_eq!(code, 0, "the consistency axis is include-gated\n{envelope}");
}

/// The complement: a workspace whose only findings are a stale entity
/// (old `last_modified`), a generations-behind pin (`default@1.0.0`
/// while the catalogue carries newer generations) and drifted anchors
/// exits 0 under `--strict` with every include that would surface them.
#[test]
fn strict_passes_when_only_advisory_findings_exist() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("hold");
    fs::create_dir_all(&dir).unwrap();
    // `last_modified` far in the past: stale under the 90-day threshold.
    write_default_mem(&dir, "An old entity that nobody touched.");
    init_real_mem_repo_from_disk(tmp.path(), &[(&dir, "hold")]);

    // An anchor whose artifact then changes: drifted.
    let artifact = tmp.path().join("artifact.txt");
    fs::write(&artifact, "v1\n").unwrap();
    let anchor = serde_json::json!({
        "artifact": artifact.display().to_string(),
        "grain": "file",
        "class": "anchored",
        "hash": "0000000000000000000000000000000000000000000000000000000000000000",
        "hash_stability": "stable",
    });
    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--mem",
            "hold",
            "--title",
            "Anchored",
            "--type",
            "spec",
            "--section",
            "identity=Anchored to a file that will change.",
            "--section",
            "purpose=Drift fixture.",
            "--metadata",
            "level=M0",
            "--anchor",
            &anchor.to_string(),
        ])
        .assert()
        .success();

    let (code, json, envelope) = strict_health(tmp.path(), &["stale", "anchors", "integrity"]);
    let codes = warning_codes(&json);
    assert!(
        codes.contains(&"SCHEMA_GENERATIONS_BEHIND".to_string()),
        "default@1.0.0 is behind the catalogue: {codes:?}"
    );
    assert!(
        json["summary"]["total_stale"].as_u64().unwrap() >= 1,
        "the 2026-01-01 entity is stale: {}",
        json["summary"]
    );
    let drifted: u64 = json["anchors"]
        .as_object()
        .map(|m| m.values().filter_map(|v| v["drifted"].as_u64()).sum())
        .unwrap_or(0);
    assert!(drifted >= 1, "the anchor drifted: {}", json["anchors"]);
    assert_eq!(
        code, 0,
        "stale, drifted and generations-behind are advisory\n{envelope}"
    );
    assert!(envelope.is_empty(), "{envelope}");
}

/// A clean workspace with every include the runner passes exits 0 and
/// prints no strict envelope.
#[test]
fn strict_passes_on_a_clean_workspace_with_every_include() {
    let tmp = TempDir::new().unwrap();
    healthy_workspace(tmp.path());
    memstead()
        .current_dir(tmp.path())
        .args([
            "health",
            "--strict",
            "--include",
            "integrity,missing_required_outgoing,constraints,signals,stale,anchors",
        ])
        .assert()
        .success()
        .stdout(contains("# Graph health"));
}
