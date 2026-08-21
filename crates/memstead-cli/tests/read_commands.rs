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
/// `OVERVIEW_RICH_CONTENT_FULL_ONLY` (formerly `mcp_only_notice`)
/// warning string must not appear in the response.
#[test]
fn overview_with_include_renders_rich_content_without_full_only_warning() {
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
        .stdout(predicates::str::contains("OVERVIEW_RICH_CONTENT_FULL_ONLY").not())
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
no_self_loop_relationships: []
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

/// Seed a workspace pinned to a heading-round-trip-violating schema
/// (sealed posture: it loads; new installs would be refused) with one
/// entity whose content sits under the non-deriving heading and one
/// whose required section is genuinely absent — the two conditions the
/// `missing_fields` issue codes distinguish.
fn seed_violator_workspace(root: &Path) {
    let schema_dir = root.join(".memstead").join("schemas").join("debate");
    fs::create_dir_all(schema_dir.join("types")).unwrap();
    fs::write(
        schema_dir.join("schema.yaml"),
        r#"name: debate
version: 0.1.0
description: sealed-violator fixture for the CLI health projection tests.
when_to_use: Used only by memstead-cli integration tests.
types:
  - question
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
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
        schema_dir.join("types").join("question.yaml"),
        r#"name: question
description: t
when_to_use: tests
sections:
  - key: answers
    heading: Answers argued
    required: true
    search_weight: 10.0
    write_rules: []
  - key: notes
    heading: Notes
    required: false
    search_weight: 3.0
    catch_all: true
    write_rules: []
metadata_fields: []
title_weight: 100.0
text_fields:
  - answers
  - notes
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - answers
health_required_fields:
  - answers
staleness_threshold_days: 90
write_rules: []
"#,
    )
    .unwrap();

    let mem_dir = root.join("debatemem");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{ "schema": "debate@0.1.0" }"#,
    )
    .unwrap();
    fs::write(
        mem_dir.join("mismatch.md"),
        "---\ntype: question\n---\n# Mismatch\n\n## Answers argued\n\nPresent.\n",
    )
    .unwrap();
    fs::write(
        mem_dir.join("absent.md"),
        "---\ntype: question\n---\n# Absent\n",
    )
    .unwrap();

    init_real_mem_repo_from_disk(root, &[(&mem_dir, "debatemem")]);
}

/// The CLI `missing_fields` projection carries per-issue codes beside
/// the legacy `missing` array: a genuinely absent section reports
/// `MISSING`, content under a non-deriving heading reports
/// `SECTION_HEADING_MISMATCH` — never "missing"-only. The legacy array
/// stays bare field names for both.
#[test]
fn health_missing_fields_carries_issue_codes() {
    let tmp = TempDir::new().unwrap();
    seed_violator_workspace(tmp.path());

    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "health", "--include", "missing_fields"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).expect("health --json is JSON");
    let entries = json["missing_fields"].as_array().expect("include renders");
    let entry_for = |id: &str| {
        entries
            .iter()
            .find(|e| e["id"] == format!("debatemem--{id}"))
            .unwrap_or_else(|| panic!("entry for {id}: {entries:?}"))
    };

    let mismatch = entry_for("mismatch");
    assert_eq!(mismatch["missing"], serde_json::json!(["answers"]));
    assert_eq!(mismatch["issues"][0]["code"], "SECTION_HEADING_MISMATCH");
    assert!(
        mismatch["issues"][0]["message"]
            .as_str()
            .unwrap()
            .contains("is not missing"),
        "message rides beside the code: {mismatch}"
    );

    let absent = entry_for("absent");
    assert_eq!(absent["missing"], serde_json::json!(["answers"]));
    assert_eq!(absent["issues"][0]["code"], "MISSING");
    assert!(
        absent["issues"][0]["message"]
            .as_str()
            .unwrap()
            .contains("is empty"),
        "message rides beside the code: {absent}"
    );
}

/// `memstead health --include config` renders the same projection MCP's
/// `include_config: true` serves (`mems` / `mutations` / `plugin`);
/// without the token the response carries no config block.
#[test]
fn health_include_config_renders_workspace_config_projection() {
    let tmp = TempDir::new().unwrap();
    seed_strict_health_workspace(tmp.path(), false);

    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "health", "--include", "config"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).expect("health --json is JSON");
    for key in ["mems", "mutations", "plugin"] {
        assert!(
            json.get(key).is_some(),
            "--include config must render `{key}`: {json}"
        );
    }
    let mems = json["mems"].as_array().expect("mems detail array");
    assert!(
        mems.iter().any(|m| m["name"] == "strictmem"),
        "per-mem detail names the writable mem: {mems:?}"
    );
    assert!(
        json["mutations"].get("require_notes").is_some(),
        "mutations posture rides the projection: {json}"
    );

    // Refusal complement: no config block without the token.
    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "health"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).expect("health --json is JSON");
    for key in ["mems", "mutations", "plugin"] {
        assert!(
            json.get(key).is_none(),
            "no config block without the opt-in: {json}"
        );
    }
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

// ---------------------------------------------------------------------------
// Range filters (`--range-filter`) — MCP key grammar, same outcome codes
// ---------------------------------------------------------------------------

/// A range-filterable field narrows results from the CLI with the MCP
/// key grammar, and each of the four typed outcome codes is reachable
/// with the same meaning as over MCP (same engine path — the CLI only
/// splits KEY=VALUE).
#[test]
fn search_range_filter_narrows_and_surfaces_the_typed_codes() {
    let tmp = TempDir::new().unwrap();
    // Two mems: the default-schema `cli-test` (base-metadata dates are
    // range-filterable on every type) and a planning-schema `cli-plan`
    // (whose `decision.decided_on` is a type-SPECIFIC range field —
    // needed to reach RANGE_FILTER_TYPE_SCOPED).
    let dir = tmp.path().join("cli-test");
    fs::create_dir_all(&dir).unwrap();
    make_test_mem(&dir);
    let plan = tmp.path().join("cli-plan");
    fs::create_dir_all(plan.join(".memstead")).unwrap();
    fs::write(
        plan.join(".memstead").join("config.json"),
        r#"{ "schema": "planning@0.1.0" }"#,
    )
    .unwrap();
    init_real_mem_repo_from_disk(tmp.path(), &[(&dir, "cli-test"), (&plan, "cli-plan")]);

    // Supported key form narrows: alpha's created_date is 2026-01-01.
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "search",
            "--type",
            "spec",
            "--range-filter",
            "created_date_after=2025-01-01",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(
        json["hits"].as_array().is_some_and(|r| !r.is_empty()),
        "in-range date filter keeps the hit: {json}"
    );

    // …and excludes when out of range.
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "search",
            "--type",
            "spec",
            "--range-filter",
            "created_date_before=2020-01-01",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(
        json["hits"].as_array().is_some_and(|r| r.is_empty()),
        "out-of-range date filter drops the hit: {json}"
    );

    // Composable with --filter (equality) and the named shortcuts.
    memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "search",
            "--type",
            "spec",
            "--filter",
            "level=M0",
            "--range-filter",
            "created_date_after=2025-01-01",
        ])
        .assert()
        .success()
        .stdout(contains("alpha"));

    // The four typed outcome codes, same meaning as over MCP:
    // 1. malformed key → RANGE_FILTER_KEY_MALFORMED (not ignored).
    memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "search",
            "--type",
            "spec",
            "--range-filter",
            "bogus=1",
        ])
        .assert()
        .success()
        .stdout(contains("RANGE_FILTER_KEY_MALFORMED"));

    // 2. field declared on OTHER types in scope but not the queried
    //    one → RANGE_FILTER_TYPE_SCOPED (applied with type-narrowing):
    //    `decided_on` is range-filterable on planning's `decision`,
    //    not on `step`.
    memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "search",
            "--mem",
            "cli-plan",
            "--type",
            "step",
            "--range-filter",
            "decided_on_after=2020-01-01",
        ])
        .assert()
        .success()
        .stdout(contains("RANGE_FILTER_TYPE_SCOPED"));

    // 3. unknown field → UNKNOWN_RANGE_FILTER_FIELD, results UNFILTERED
    //    (not empty) — the filter is dropped with a warning.
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "search",
            "--type",
            "spec",
            "--range-filter",
            "min_nonexistent=1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(
        out.windows(b"UNKNOWN_RANGE_FILTER_FIELD".len())
            .any(|w| w == b"UNKNOWN_RANGE_FILTER_FIELD"),
        "unknown field surfaces its code: {json}"
    );
    assert!(
        json["hits"].as_array().is_some_and(|r| !r.is_empty()),
        "unknown range field leaves results unfiltered, not empty: {json}"
    );

    // 4. declared-but-not-range-filterable field → FIELD_NOT_RANGE_FILTERABLE.
    memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "search",
            "--type",
            "spec",
            "--range-filter",
            "min_level=1",
        ])
        .assert()
        .success()
        .stdout(contains("FIELD_NOT_RANGE_FILTERABLE"));
}

// ---------------------------------------------------------------------------
// Workspace override (global --workspace / MEMSTEAD_WORKSPACE)
// ---------------------------------------------------------------------------

/// The global override lets the CLI operate on a named workspace from
/// any working directory: flag alone, env alone, flag over env. An
/// override without the marker refuses naming the tried path and never
/// falls back to the walk — even when the walk WOULD succeed from cwd.
#[test]
fn workspace_override_flag_env_precedence_and_refusal() {
    let ws = TempDir::new().unwrap();
    seed_cli_test_mem(ws.path());
    let elsewhere = TempDir::new().unwrap();

    // Flag alone, from an unrelated cwd.
    memstead()
        .current_dir(elsewhere.path())
        .env_remove("MEMSTEAD_WORKSPACE")
        .args([
            "--workspace",
            ws.path().to_str().unwrap(),
            "entity",
            "cli-test--alpha",
        ])
        .assert()
        .success()
        .stdout(contains("Alpha"));

    // Env alone.
    memstead()
        .current_dir(elsewhere.path())
        .env("MEMSTEAD_WORKSPACE", ws.path())
        .args(["entity", "cli-test--alpha"])
        .assert()
        .success()
        .stdout(contains("Alpha"));

    // Both: the flag wins (env points at a non-workspace; the flag's
    // valid path must be used, or this would refuse).
    memstead()
        .current_dir(elsewhere.path())
        .env("MEMSTEAD_WORKSPACE", elsewhere.path())
        .args([
            "--workspace",
            ws.path().to_str().unwrap(),
            "entity",
            "cli-test--alpha",
        ])
        .assert()
        .success()
        .stdout(contains("Alpha"));

    // Refusal: a marker-less override refuses, names the tried path,
    // and does NOT fall back to the walk — run from INSIDE the valid
    // workspace so a fallback would have succeeded.
    memstead()
        .current_dir(ws.path())
        .env_remove("MEMSTEAD_WORKSPACE")
        .args([
            "--workspace",
            elsewhere.path().to_str().unwrap(),
            "entity",
            "cli-test--alpha",
        ])
        .assert()
        .failure()
        .stderr(
            contains("WORKSPACE_NOT_INITIALISED").and(contains(elsewhere.path().to_str().unwrap())),
        );

    // Without either, the walk behaves exactly as today.
    memstead()
        .current_dir(ws.path())
        .env_remove("MEMSTEAD_WORKSPACE")
        .args(["entity", "cli-test--alpha"])
        .assert()
        .success()
        .stdout(contains("Alpha"));
}

// ---------------------------------------------------------------------------
// Directional traversal (--direction) + CLI expansion parity
// ---------------------------------------------------------------------------

/// The CLI gains the expansion pair and the direction selector: an
/// `out` expansion from alpha reaches beta (alpha --USES--> beta) and
/// reports the traversal direction beside the edge label; `in` from
/// alpha reaches nothing; an unrecognised selector refuses naming the
/// accepted values instead of silently falling back to `both`.
#[test]
fn search_direction_and_expand_via_flags() {
    let tmp = TempDir::new().unwrap();
    seed_cli_test_mem(tmp.path());

    // out: beta is reached and the direction rides beside the label.
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "search",
            "exercise",
            "--expand-via",
            "USES",
            "--direction",
            "out",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let beta = json["hits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["id"] == "cli-test--beta")
        .unwrap_or_else(|| panic!("out-expansion reaches beta: {json}"));
    assert_eq!(beta["expansion"]["via_edge"], "USES");
    assert_eq!(
        beta["expansion"]["via_direction"], "out",
        "the reached entity reports its traversal direction: {beta}"
    );

    // in: alpha has no incoming USES — no expanded hit.
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "search",
            "exercise",
            "--expand-via",
            "USES",
            "--direction",
            "in",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(
        !json["hits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["id"] == "cli-test--beta"),
        "in-expansion must not reach a descendant: {json}"
    );

    // Unrecognised selector: refuses naming the accepted values.
    memstead()
        .current_dir(tmp.path())
        .args(["search", "alpha", "--direction", "sideways"])
        .assert()
        .failure()
        .stderr(
            contains("sideways")
                .and(contains("out"))
                .and(contains("in"))
                .and(contains("both")),
        );
}

/// Plan 08 duplicate check (CLI leg): an identifier-shaped metadata
/// value is findable by plain free-text search — the silent-zero
/// failure that produced duplicate entities is gone.
#[test]
fn search_finds_identifier_shaped_metadata_value() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("cli-test");
    fs::create_dir_all(&dir).unwrap();
    make_test_mem(&dir);
    // A file carrying the identifier in a metadata field the schema
    // never declared — tolerated on load, and now findable.
    fs::write(
        dir.join("akte.md"),
        r#"---
type: spec
aktenzeichen: 20/54/033
---
# Akte

## Identity

Die Akte selbst.

## Purpose

Nachweis für Suche in Metadaten.
"#,
    )
    .unwrap();
    init_real_mem_repo_from_disk(tmp.path(), &[(&dir, "cli-test")]);

    memstead()
        .current_dir(tmp.path())
        .args(["--json", "search", "20/54/033"])
        .assert()
        .success()
        .stdout(contains("cli-test--akte").and(contains("\"metadata\"")));
}

/// `memstead due` (first-author-path plan 08): the CLI wiring — a
/// workspace whose schema declares no due axis renders the honest
/// empty brief; a bad window refuses typed naming the accepted forms;
/// the default window is stated in `--help`; `--today` makes the
/// brief deterministic.
#[test]
fn due_brief_cli_wiring() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args(["due", "--today", "2026-08-10"])
        .assert()
        .success()
        .stdout(contains("# Due brief — 2026-08-10, window 90d"))
        .stdout(contains("No mounted mem's schema declares a due axis"));

    memstead()
        .current_dir(tmp.path())
        .args(["due", "--within", "90w"])
        .assert()
        .failure()
        .stderr(contains("INVALID_INPUT"))
        .stderr(contains("<N>d"));

    memstead()
        .current_dir(tmp.path())
        .args(["due", "--help"])
        .assert()
        .success()
        .stdout(contains("90d"));
}

/// `memstead export --format llms-txt` — the whole mem as one Markdown
/// document, on the **mem-repo** backend. Pins the shape properties the
/// deployed `/llms-full.txt` promises, so the exported and served documents
/// cannot drift apart in future changes: header block, one occurrence per
/// non-stub entity, the visible type line, empty sections kept, and a
/// separator between entities.
#[test]
fn llms_txt_export_pins_the_document_shape() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args(["export", "--help"])
        .assert()
        .success()
        .stdout(contains("llms-txt"));

    let out = memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "llms-txt"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let doc = String::from_utf8(out).unwrap();

    // Header block: the document says what it is before it says anything.
    assert!(doc.starts_with("# cli-test"), "header names the mem: {doc}");
    for field in [
        "Mem: ",
        "Subject: ",
        "Schema: ",
        "Entities: ",
        "Provenance: ",
    ] {
        assert!(doc.contains(field), "header carries `{field}`: {doc}");
    }
    assert!(
        doc.contains("Every non-stub entity of this Memstead graph follows, once"),
        "the header states its own contract: {doc}"
    );
    // No deployment is vouching for a file exported from a workspace, and the
    // header must not claim one is.
    assert!(
        !doc.contains("this deployment vouches"),
        "a CLI export never claims a deployment's provenance: {doc}"
    );

    // Stub filtering, asserted rather than assumed. A relationship to an
    // absent target auto-creates a stub; the document must exclude it AND its
    // `Entities:` count must agree, or the header's own contract is false.
    memstead()
        .current_dir(tmp.path())
        .args(["relate", "cli-test--alpha", "USES", "cli-test--phantom"])
        .assert()
        .success();
    let with_stub = String::from_utf8(
        memstead()
            .current_dir(tmp.path())
            .args(["export", "--format", "llms-txt"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    // "No body" is the claim, not "no mention": after the plain-text
    // degradation the stub's id legitimately appears as a named reference in
    // the Relationships block. What must not appear is a rendered entity —
    // a title heading of its own.
    assert!(
        !with_stub.contains("# Phantom"),
        "the stub entity has no rendered body in the document: {with_stub}"
    );
    assert!(
        with_stub.contains("Entities: 2"),
        "the count excludes the stub, matching the header's own promise: {with_stub}"
    );

    // Ordering is lexical by id, asserted as a full sequence over EIGHT
    // entities. The store iterates a HashMap, so a small sequence is a dice
    // roll, not an assertion: over three entities a dropped `ids.sort()`
    // still passed one run in six under fault injection. Eight (1/8! ≈
    // 2.5e-5) makes an unsorted order fail every run that will ever matter.
    for (title, blurb) in [
        ("Amber", "Sorts between alpha and beta."),
        ("Delta", "Ordering fixture."),
        ("Echo", "Ordering fixture."),
        ("Gamma", "Ordering fixture."),
        ("Iota", "Ordering fixture."),
        ("Zeta", "Ordering fixture."),
    ] {
        memstead()
            .current_dir(tmp.path())
            .args([
                "create",
                "--mem",
                "cli-test",
                "--title",
                title,
                "--type",
                "spec",
                "--section",
                &format!("identity={blurb}"),
                "--section",
                "purpose=Ordering fixture.",
            ])
            .assert()
            .success();
    }
    let ordered = String::from_utf8(
        memstead()
            .current_dir(tmp.path())
            .args(["export", "--format", "llms-txt"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    let positions: Vec<usize> = [
        "# Alpha", "# Amber", "# Beta", "# Delta", "# Echo", "# Gamma", "# Iota", "# Zeta",
    ]
    .iter()
    .map(|h| {
        ordered
            .find(h)
            .unwrap_or_else(|| panic!("{h} present in: {ordered}"))
    })
    .collect();
    let mut sorted = positions.clone();
    sorted.sort_unstable();
    assert_eq!(
        positions, sorted,
        "entities render in lexical id order over all eight: {ordered}"
    );

    // The type line, and each entity exactly once.
    assert!(doc.contains("_Type: spec_"), "type line rendered: {doc}");
    assert_eq!(
        doc.matches("# Alpha\n").count(),
        1,
        "each entity appears exactly once: {doc}"
    );
    assert!(doc.contains("\n---\n"), "entities are separated: {doc}");
    assert!(
        !doc.contains("[["),
        "no raw wiki-link syntax survives: {doc}"
    );
    // Checked on the POST-stub document too. The assertion above runs against
    // `doc`, captured before the stub existed — so a reference that stops
    // resolving *because* of stub exclusion would be invisible to it. The
    // auto-generated `## Relationships` block emits full-id wiki-links to stub
    // targets, which is exactly that case.
    assert!(
        !with_stub.contains("[["),
        "an unresolvable reference is named in plain text, not left as \
         wiki-link syntax: {with_stub}"
    );
    assert!(
        with_stub.contains("cli-test--phantom"),
        "and it is still named, so the reference is not silently dropped: \
         {with_stub}"
    );

    // An unmounted mem refuses rather than emitting an empty document — a
    // document reporting zero entities about a mem this workspace never
    // mounted is a confident wrong answer.
    memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "llms-txt", "--mem", "nope"])
        .assert()
        .failure()
        .stderr(contains("UNKNOWN_MEM"));
}

/// The two pinned link forms, and only those two: `--base-url` renders
/// absolute links exactly as the served document does; without it the same
/// text targets the document-relative `entity/<id>`.
#[test]
fn llms_txt_export_emits_only_the_two_pinned_link_forms() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("cli-test");
    fs::create_dir_all(&dir).unwrap();
    make_test_mem(&dir);
    // A second entity whose body wiki-links the first by bare slug — the
    // local-mem resolution pass.
    fs::write(
        dir.join("beta.md"),
        r#"---
type: spec
created_date: 2026-01-01
last_modified: 2026-01-01
level: M0
---
# Beta

## Identity

Beta references [[alpha]] by bare slug.

## Purpose

Exercises link resolution.
"#,
    )
    .unwrap();
    init_real_mem_repo_from_disk(tmp.path(), &[(&dir, "cli-test")]);

    let relative = String::from_utf8(
        memstead()
            .current_dir(tmp.path())
            .args(["export", "--format", "llms-txt"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        relative.contains("(entity/cli-test--alpha)"),
        "without --base-url the link is document-relative: {relative}"
    );
    assert!(
        !relative.contains("(/entity/"),
        "and never root-relative — that is a third form: {relative}"
    );

    let absolute = String::from_utf8(
        memstead()
            .current_dir(tmp.path())
            .args([
                "export",
                "--format",
                "llms-txt",
                "--base-url",
                "https://example.com",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        absolute.contains("(https://example.com/entity/cli-test--alpha)"),
        "with --base-url the link is absolute: {absolute}"
    );
    assert!(
        !absolute.contains("[["),
        "the bare slug resolved rather than surviving raw: {absolute}"
    );
}

/// Criterion 2's substance, end to end on a real workspace: the foreign-slug
/// passes must actually run.
///
/// They were unreachable once, and invisibly: engine-authored bare wiki-links
/// materialise a LOCAL stub, so with stubs in the link map every bare slug
/// resolved locally — to a stub the document itself excludes. A unit test over
/// a hand-built map could not see it, because that map was a shape the engine
/// cannot produce.
#[test]
fn llms_txt_foreign_slug_passes_resolve_and_never_guess() {
    let tmp = TempDir::new().unwrap();
    let mk = |name: &str, entities: &[(&str, &str)]| {
        let dir = tmp.path().join(name);
        fs::create_dir_all(dir.join(".memstead")).unwrap();
        fs::write(
            dir.join(".memstead/config.json"),
            r#"{ "schema": "default@1.0.0" }"#,
        )
        .unwrap();
        for (slug, body) in entities {
            fs::write(
                dir.join(format!("{slug}.md")),
                format!(
                    "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n\
# {slug}\n\n## Identity\n\n{body}\n\n## Purpose\n\nFixture.\n"
                ),
            )
            .unwrap();
        }
        dir
    };

    // `shared` exists in BOTH foreign mems; `only-alpha` in one; `ghost` in none.
    let home = mk("home", &[("seed", "Placeholder.")]);
    let alpha = mk("alpha", &[("shared", "A."), ("only-alpha", "B.")]);
    let beta = mk("beta", &[("shared", "C.")]);
    init_real_mem_repo_from_disk(
        tmp.path(),
        &[(&home, "home"), (&alpha, "alpha"), (&beta, "beta")],
    );

    // The referencing entity is written through the MUTATION surface, not as a
    // raw file — that is what auto-stubs each bare wiki-link into a local
    // stub, which is the state that made the foreign passes unreachable. A
    // raw-file fixture never creates those stubs and so cannot see the bug.
    memstead()
        .current_dir(tmp.path())
        .args([
            "create",
            "--mem",
            "home",
            "--title",
            "Hub",
            "--type",
            "spec",
            "--section",
            "identity=Refs [[shared]] and [[only-alpha]] and [[ghost]].",
            "--section",
            "purpose=Fixture.",
        ])
        .assert()
        .success();

    let doc = String::from_utf8(
        memstead()
            .current_dir(tmp.path())
            .args(["export", "--format", "llms-txt", "--mem", "home"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();

    assert!(
        doc.contains("(entity/alpha--only-alpha)"),
        "a slug unique to one foreign mem resolves there: {doc}"
    );
    assert!(
        !doc.contains("[[") && doc.contains("shared"),
        "a slug two foreign mems both own is named in plain text, never \
         guessed and never left as syntax: {doc}"
    );
    assert!(
        doc.contains("ghost"),
        "a reference to nothing is still named, just not linked: {doc}"
    );
    assert!(
        !doc.contains("entity/home--shared") && !doc.contains("entity/home--ghost"),
        "no link points at a stub the document excludes: {doc}"
    );
}

/// `memstead export --format html` (first-author-path plan 11): the
/// CLI wiring — the format appears in help, a fixture workspace
/// exports one self-contained file, and an unknown mem refuses with
/// the same typed code the other formats use.
#[test]
fn html_export_cli_wiring() {
    let tmp = TempDir::new().unwrap();
    let _mem = seed_cli_test_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args(["export", "--help"])
        .assert()
        .success()
        .stdout(contains("html"));

    memstead()
        .current_dir(tmp.path())
        .args(["--json", "export", "--format", "html", "-o", "out.html"])
        .assert()
        .success()
        .stdout(contains("\"format\": \"html\""));
    let html = std::fs::read_to_string(tmp.path().join("out.html")).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(!html.contains("<img"), "self-contained");

    memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "html", "--mem", "nope"])
        .assert()
        .failure()
        .stderr(contains("UNKNOWN_MEM"));
}
