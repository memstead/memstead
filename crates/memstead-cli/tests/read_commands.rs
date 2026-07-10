#![cfg(feature = "mem-repo")]
//! Integration tests for `memstead` read subcommands.
//!
//! Each test sets up a fresh temp mem with one or two entities and runs the
//! binary as a subprocess. Tests cover: default markdown output, `--json`
//! output, and typed exit codes.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use predicates::prelude::*;
use predicates::str::contains;
use tempfile::TempDir;

/// Seed a canonical `cli-test/` mem dir under `root`. Returns the
/// mem's absolute path. The dir basename equals the declared
/// `name: "cli-test"` so the engine's basename-invariant holds.
///
/// Also lays down `<root>/mem-repo/.git/` so the engine's
/// `mem-repo/.git/` fail-fast accepts the workspace and so
/// `find_workspace_root` (the CLI's walk-up) resolves `<root>` as the
/// workspace.
fn seed_cli_test_mem(root: &Path) -> std::path::PathBuf {
    let dir = root.join("cli-test");
    fs::create_dir_all(&dir).unwrap();
    make_test_mem(&dir);
    init_real_mem_repo_from_disk(root, &[(&dir, "cli-test")]);
    dir
}

/// Write a minimal single-type mem with one basic entity into `dir`.
fn make_test_mem(dir: &Path) {
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();

    fs::write(
        dir.join("alpha.md"),
        r#"---
type: spec
created_date: 2026-01-01
last_modified: 2026-01-01
level: M0
---
# Alpha

## Identity

The alpha entity used to exercise CLI read commands.

## Purpose

Verifies memstead CLI integration end-to-end.

## Relationships

- **USES**: [[beta]]
"#,
    )
    .unwrap();

    fs::write(
        dir.join("beta.md"),
        r#"---
type: spec
created_date: 2026-01-02
last_modified: 2026-01-02
level: M0
---
# Beta

## Identity

The beta entity, used by alpha via USES.

## Purpose

Provides a second entity so relations and path commands have something to trace.
"#,
    )
    .unwrap();
}

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

#[test]
fn status_markdown() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .arg("status")
        .assert()
        .success()
        .stdout(contains("# Graph status"))
        .stdout(contains("Nodes: 2"));
}

/// Smoke-test Bug 2 closure for `memstead status` on a filesystem-mem
/// workspace. Pre-CLI-parity, this command would error out with the
/// "No mems found. Run `memstead mem-repo init`" message; post the
/// `CliEngine` foundation the command dispatches into the unified
/// `memstead_base::Engine` (lean path) and emits the same shape the
/// mem-repo path produces.
#[test]
fn status_works_on_filesystem_mem_workspace() {
    let tmp = TempDir::new().unwrap();
    // `memstead init --name demo --schema default@1.0.0` lays down
    // `.memstead/config.json` plus the empty cache / memstead-io subdirs.
    memstead()
        .current_dir(tmp.path())
        .args(["init", "--name", "demo", "--schema", "default@1.0.0"])
        .assert()
        .success();

    // Empty filesystem-mem has zero entities — the command must
    // still produce the canonical markdown layout, not bail.
    memstead()
        .current_dir(tmp.path())
        .arg("status")
        .assert()
        .success()
        .stdout(contains("# Graph status"))
        .stdout(contains("Nodes: 0"));
}

#[test]
fn status_json() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    let output = memstead()
        .current_dir(tmp.path())
        .args(["--json", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(parsed["total_nodes"], 2);
    assert_eq!(parsed["real_nodes"], 2);
}

#[test]
fn entity_markdown() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args(["entity", "cli-test--alpha"])
        .assert()
        .success()
        .stdout(contains("# Alpha"))
        .stdout(contains("## Identity"))
        .stdout(contains("_hash:"));
}

/// Helper: lay down a filesystem-mem workspace at `tmp` with one
/// entity hand-shaped as `demo--alpha`. Returns the path to the
/// workspace root. Used by the suite of filesystem-mem dispatch
/// tests for read-side subcommands.
fn seed_filesystem_mem(tmp: &TempDir) {
    memstead()
        .current_dir(tmp.path())
        .args(["init", "--name", "demo", "--schema", "default@1.0.0"])
        .assert()
        .success();
    fs::write(
        tmp.path().join("alpha.md"),
        r#"---
type: spec
created_date: 2026-01-01
last_modified: 2026-01-01
level: M0
---
# Alpha

## Identity

A filesystem-mem entity exercising CLI parity.

## Purpose

Lets the read-side CLI commands round-trip without the mem-repo path.
"#,
    )
    .unwrap();
}

#[test]
fn list_works_on_filesystem_mem_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_filesystem_mem(&tmp);

    memstead()
        .current_dir(tmp.path())
        .arg("list")
        .assert()
        .success()
        .stdout(contains("demo--alpha"));
}

#[test]
fn search_works_on_filesystem_mem_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_filesystem_mem(&tmp);

    memstead()
        .current_dir(tmp.path())
        .args(["search", "Alpha"])
        .assert()
        .success()
        .stdout(contains("Alpha"));
}

#[test]
fn relations_works_on_filesystem_mem_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_filesystem_mem(&tmp);

    memstead()
        .current_dir(tmp.path())
        .args(["relations", "demo--alpha"])
        .assert()
        .success()
        .stdout(contains("demo--alpha"));
}

#[test]
fn overview_works_on_filesystem_mem_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_filesystem_mem(&tmp);

    memstead()
        .current_dir(tmp.path())
        .arg("overview")
        .assert()
        .success();
}

/// `overview --json`
/// promotes `overview_mode`, `total_chunks`, and `hints` to structured
/// envelope siblings so a consumer branches on the mode / fetches the
/// next chunk without parsing the `markdown` string. The `markdown`
/// field stays present (promotion is additive).
#[test]
fn overview_json_promotes_mode_chunks_and_hints_as_siblings() {
    let tmp = TempDir::new().unwrap();
    seed_filesystem_mem(&tmp);

    let output = memstead()
        .current_dir(tmp.path())
        .args(["--json", "overview"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");

    assert!(
        parsed.get("markdown").and_then(|v| v.as_str()).is_some(),
        "markdown field must remain for the human-rendered view: {parsed}"
    );
    let mode = parsed
        .get("overview_mode")
        .and_then(|v| v.as_str())
        .expect("overview_mode promoted as a sibling");
    assert!(
        matches!(mode, "complete" | "reduced" | "overbudget"),
        "overview_mode must be a known value, got: {mode}"
    );
    assert!(
        parsed
            .get("total_chunks")
            .and_then(|v| v.as_u64())
            .is_some(),
        "total_chunks must be a numeric sibling: {parsed}"
    );
    assert!(
        parsed.get("hints").map(|v| v.is_array()).unwrap_or(false),
        "hints must be an array sibling: {parsed}"
    );
}

#[test]
fn context_works_on_filesystem_mem_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_filesystem_mem(&tmp);

    memstead()
        .current_dir(tmp.path())
        .args(["context", "demo--alpha"])
        .assert()
        .success();
}

#[test]
fn health_works_on_filesystem_mem_workspace() {
    let tmp = TempDir::new().unwrap();
    seed_filesystem_mem(&tmp);

    memstead()
        .current_dir(tmp.path())
        .arg("health")
        .assert()
        .success()
        .stdout(contains("# Graph health"));
}

/// `memstead entity <id>` on a filesystem-mem workspace dispatches via
/// the `CliEngine::Filesystem` arm and reads the entity from the
/// directory walk. Pre-CLI-parity this errored with the
/// "No mems found" bail; post the foundation it round-trips.
#[test]
fn entity_works_on_filesystem_mem_workspace() {
    let tmp = TempDir::new().unwrap();
    memstead()
        .current_dir(tmp.path())
        .args(["init", "--name", "demo", "--schema", "default@1.0.0"])
        .assert()
        .success();

    // Drop a hand-shaped entity .md so the engine's directory walk
    // picks it up on init. Avoids a `memstead create` round-trip until
    // that command also dispatches through `CliEngine`.
    fs::write(
        tmp.path().join("alpha.md"),
        r#"---
type: spec
created_date: 2026-01-01
last_modified: 2026-01-01
level: M0
---
# Alpha

## Identity

A filesystem-mem entity exercising CLI parity.

## Purpose

Lets `memstead entity` round-trip without the mem-repo path.
"#,
    )
    .unwrap();

    memstead()
        .current_dir(tmp.path())
        .args(["entity", "demo--alpha"])
        .assert()
        .success()
        .stdout(contains("# Alpha"))
        .stdout(contains("## Identity"))
        .stdout(contains("_hash:"));
}

#[test]
fn entity_not_found_exit_code_3() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args(["entity", "cli-test--does-not-exist"])
        .assert()
        .failure()
        .code(3)
        .stderr(contains("Entity not found"));
}

/// A missing/unmatched `--mem` is a not-found condition — exit 3 on
/// every command, the same bucket as the `entity <missing>` precedent
/// above. Locks the uniform `UNKNOWN_MEM` → `NotFound` mapping across
/// the read-scoped read path (`search`/`list`), `changes`, and the
/// engine-error path (`reload`). Measured standalone, not through a
/// pipe — a pipe would mask the exit through the last process.
#[test]
fn unknown_mem_exit_code_3() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    for args in [
        vec!["search", "x", "--mem", "nope"],
        vec!["list", "--mem", "nope"],
        vec!["reload", "--mem", "nope"],
        vec!["changes", "--since", "HEAD", "--mem", "nope"],
    ] {
        memstead()
            .current_dir(tmp.path())
            .args(&args)
            .assert()
            .failure()
            .code(3);
    }
}

#[test]
fn entity_not_found_json_envelope() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    let assert = memstead()
        .current_dir(tmp.path())
        .args(["--json", "entity", "cli-test--does-not-exist"])
        .assert()
        .failure()
        .code(3);
    // Under `--json` the error envelope rides stdout
    // so `… --json | jq -r .code` works on the error path.
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let envelope: serde_json::Value = serde_json::from_str(stdout.trim()).expect("JSON envelope");
    // Wire shape: `{code, message, details}` matching MCP. Process exit
    // stays at the NotFound exit-kind (numeric 3) but it rides on the
    // process-status channel rather than inside the JSON body.
    assert_eq!(envelope["code"], "ENTITY_NOT_FOUND");
    assert!(
        envelope["message"]
            .as_str()
            .unwrap()
            .contains("Entity not found")
    );
}

#[test]
fn relations_markdown() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args(["relations", "cli-test--alpha"])
        .assert()
        .success()
        .stdout(contains("## Outgoing"))
        .stdout(contains("USES"));
}

#[test]
fn search_finds_entity() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args(["search", "alpha"])
        .assert()
        .success()
        .stdout(contains("Alpha"));
}

#[test]
fn list_all() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .arg("list")
        .assert()
        .success()
        .stdout(contains("Alpha"))
        .stdout(contains("Beta"));
}

#[test]
fn overview_runs() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .arg("overview")
        .assert()
        .success();
}

/// Full CLI's overview command renders the rich content (community
/// bridges, mem distribution, dangling links) via the shared
/// `memstead-engine::overview::compose_overview` composer. The full CLI
/// renders the content directly: when `--include` is passed the
/// `OVERVIEW_RICH_CONTENT_PRO_ONLY` (formerly `mcp_only_notice`)
/// warning string must not appear in the response.
#[test]
fn overview_with_include_renders_rich_content_without_pro_only_warning() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args([
            "overview",
            "--include",
            "mem_distribution,community_bridges,dangling_links",
        ])
        .assert()
        .success()
        .stdout(contains("## Schemas"))
        .stdout(contains("## Mems"))
        // The lean CLI's pre-lift output would have included this
        // warning code; the full CLI's shared-composer path does NOT.
        .stdout(predicates::str::contains("OVERVIEW_RICH_CONTENT_PRO_ONLY").not())
        // Full CLI uses `memstead type <name>` for the schema-lookup hint,
        // not the MCP-flavour `memstead_schema(name=...)`.
        .stdout(contains("`memstead type <name>`"));
}

#[test]
fn type_named() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args(["type", "spec"])
        .assert()
        .success()
        .stdout(contains("# Type: spec"))
        .stdout(contains("## Sections"));
}

#[test]
fn health_summary() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .arg("health")
        .assert()
        .success()
        .stdout(contains("# Graph health"))
        .stdout(contains("Entities: 2"));
}

/// Seed a workspace whose mem uses a custom schema with one
/// `required_outgoing` block (decision needs CHOSEN). When
/// `with_violation` is true, a single decision entity is authored
/// without any CHOSEN edge so `memstead_health
/// include=missing_required_outgoing` reports one violator;
/// otherwise the mem has no entities and the report is empty.
fn seed_strict_health_workspace(root: &Path, with_violation: bool) {
    // Authored schema at the fixed folder-backend location
    // (`<workspace>/.memstead/schemas/`); the `schemas_dir` key is retired.
    let schema_dir = root
        .join(".memstead")
        .join("schemas")
        .join("strictdecision");
    fs::create_dir_all(schema_dir.join("types")).unwrap();
    fs::write(
        schema_dir.join("schema.yaml"),
        r#"name: strictdecision
version: 0.1.0
description: Minimal schema pinning required_outgoing for the CLI --strict test.
when_to_use: Used only by memstead-cli health-strict integration tests.
types:
  - decision
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hierarchy
      default_weight: 3.0
      acyclic: true
    - name: REFERENCES
      description: inline link
      default_weight: 0.5
    - name: CHOSEN
      description: decision picked option
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
    )
    .unwrap();
    fs::write(
        schema_dir.join("types").join("decision.yaml"),
        r#"name: decision
description: A choice with required CHOSEN edge.
when_to_use: tests
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
hierarchy_relationship: PART_OF
propagating_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules: []
required_outgoing:
  - relationships: [CHOSEN]
    cardinality: at_least_one
"#,
    )
    .unwrap();

    let mem_dir = root.join("strictmem");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{ "schema": "strictdecision@0.1.0" }"#,
    )
    .unwrap();

    if with_violation {
        fs::write(
            mem_dir.join("violator.md"),
            r#"---
type: decision
created_date: 2026-01-01
last_modified: 2026-01-01
---
# Violator

## Body

A decision entity authored without any CHOSEN edge — exercises the
`MISSING_REQUIRED_OUTGOING` health surface.
"#,
        )
        .unwrap();
    }

    init_real_mem_repo_from_disk(root, &[(&mem_dir, "strictmem")]);
}

#[test]
fn health_strict_exits_zero_when_no_violations() {
    let tmp = TempDir::new().unwrap();
    seed_strict_health_workspace(tmp.path(), false);

    memstead()
        .current_dir(tmp.path())
        .args([
            "health",
            "--include",
            "missing_required_outgoing",
            "--strict",
        ])
        .assert()
        .success();
}

#[test]
fn health_strict_exits_one_when_violations_present() {
    let tmp = TempDir::new().unwrap();
    seed_strict_health_workspace(tmp.path(), true);

    let assert = memstead()
        .current_dir(tmp.path())
        .args([
            "health",
            "--include",
            "missing_required_outgoing",
            "--strict",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(contains("strict mode"))
        .stderr(contains("missing_required_outgoing: 1"));
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("Missing required outgoing"),
        "violation report still rendered to stdout before non-zero exit; got:\n{stdout}"
    );
}
