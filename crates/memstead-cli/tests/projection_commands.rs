//! Integration tests for the `memstead projection` command tree.
//!
//! Two leaves ship here: `init` (D8 — non-interactive v1 scaffold) and
//! `migrate` (D10 gen-2 path — four-primitive → v1 bindings).
//!
//! `init` tests assert: a codebase/filesystem source scaffolds all three files
//! (`mediums`/`facets`/`projections`) with `operations:[build,sync,verify]` and
//! a round-trippable v1 binding; a `web` source scaffolds build-only with a
//! deferral warning; the `--json` output matches D8's pinned byte-shape; and a
//! re-run on an existing id refuses `PROJECTION_EXISTS` without touching disk
//! (the three files are byte-identical after the refused second run).
//!
//! `migrate` tests build a fixture gen-2 workspace on disk, run the migration,
//! and assert: the produced v1 binding round-trips and carries the merged build
//! operations; the merged ingest is removed; `refinement` mode and a dangling
//! ingest→projection ref each refuse with a typed `PROJECTION_*` code (exit 5);
//! and `--dry-run` writes nothing.

use assert_cmd::Command;
use memstead_base::binding::{BindingV1, BuildMode};
use memstead_base::pipeline::IngestTrigger;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// Write `contents` to `<root>/.memstead/<rel>`, creating parent dirs.
fn write_store(root: &Path, rel: &str, contents: &str) {
    let path = root.join(".memstead").join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A bare workspace: just the `.memstead/workspace.toml` marker.
fn bare_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write_store(tmp.path(), "workspace.toml", "");
    tmp
}

/// A minimal gen-2 workspace: the workspace marker plus one codebase medium,
/// one source facet, one projection, and one flat ingest naming it. `mode` and
/// `deny` parameterise the ingest.
fn fixture(mode: &str, deny: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_store(root, "workspace.toml", "");
    write_store(
        root,
        "mediums/engine/src.json",
        r#"{"name":"src","type":"codebase","pointer":"../public"}"#,
    );
    write_store(
        root,
        "facets/engine/source-tree.json",
        r#"{"name":"source-tree","medium":"src","scope":[{"path":"../public/**/*.rs","mode":"allow"}]}"#,
    );
    write_store(
        root,
        "projections/engine/graph.json",
        r#"{"intent":"the engine graph","source_facets":["source-tree"],"reference_mems":["plugin"],"destination_mem":"engine","rules":{"routing":"r"}}"#,
    );
    write_store(
        root,
        "ingests/engine-graph.json",
        &format!(
            r#"{{"projection":"engine/graph","mode":"{mode}","trigger":"loop","batch_size":20,"deny_paths":[{deny}],"post_actions":{{"archive_source":true}}}}"#
        ),
    );
    tmp
}

fn read_binding(root: &Path) -> BindingV1 {
    let bytes = std::fs::read(root.join(".memstead/projections/engine/graph.json")).unwrap();
    serde_json::from_slice(&bytes).expect("promoted projection file must parse as a v1 binding")
}

/// A discovery ingest migrates: the projection file is promoted to a v1
/// binding carrying the merged build operation, and the merged ingest is gone.
#[test]
fn migrate_promotes_projection_to_v1_binding() {
    let tmp = fixture("discovery", r#""dev","**/VISION.md""#);
    let root = tmp.path();

    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "migrate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).expect("--json migrate must emit JSON");
    assert_eq!(env["ok"], true);
    assert_eq!(env["migrated"], 1);
    assert_eq!(env["bindings"][0], "engine/graph");

    // The projection file now parses as a v1 binding with the merged build op.
    let b = read_binding(root);
    assert_eq!(b.version, 1);
    assert_eq!(b.destination_mem, "engine");
    assert_eq!(b.intent.as_deref(), Some("the engine graph"));
    assert_eq!(b.reference_mems, vec!["plugin".to_string()]);
    assert_eq!(b.operations.build.mode, BuildMode::Discovery);
    assert_eq!(b.operations.build.trigger, IngestTrigger::Loop);
    assert_eq!(b.operations.build.batch_size, 20);
    assert_eq!(
        b.operations.build.post_actions,
        Some(serde_json::json!({ "archive_source": true }))
    );
    // Build-only: sync/verify are enabled later, never fabricated by migrate.
    assert!(b.operations.sync.is_none());
    assert!(b.operations.verify.is_none());
    // deny_paths moved up; the bare `dev` segment converted to the glob dialect.
    assert_eq!(
        b.deny_paths,
        vec!["dev/**".to_string(), "**/VISION.md".to_string()]
    );

    // Serde round-trip is lossless.
    let json = serde_json::to_string(&b).unwrap();
    let back: BindingV1 = serde_json::from_str(&json).unwrap();
    assert_eq!(back, b);

    // The merged flat ingest was removed.
    assert!(!root.join(".memstead/ingests/engine-graph.json").exists());

    // A dialect-rewrite warning was reported.
    let warnings = env["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w["kind"] == "note" && w["message"].as_str().unwrap_or("").contains("dev/**")),
        "expected a deny-dialect note, got {warnings:?}"
    );
}

/// `--dry-run` reports the migration but writes nothing.
#[test]
fn migrate_dry_run_writes_nothing() {
    let tmp = fixture("discovery", "");
    let root = tmp.path();

    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "migrate", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["dry_run"], true);
    assert_eq!(env["migrated"], 1);

    // Disk untouched: the flat ingest survives and the projection file is still
    // the gen-2 shape (no `version` / `operations` keys).
    assert!(root.join(".memstead/ingests/engine-graph.json").exists());
    let raw =
        std::fs::read_to_string(root.join(".memstead/projections/engine/graph.json")).unwrap();
    assert!(
        !raw.contains("\"version\""),
        "gen-2 shape must be untouched"
    );
    assert!(!raw.contains("operations"), "gen-2 shape must be untouched");
}

/// A codebase binding validates clean — no capability warnings.
#[test]
fn migrate_legal_codebase_binding_validates_clean() {
    let tmp = fixture("discovery", "");
    let root = tmp.path();
    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "migrate", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    let warnings = env["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().all(|w| w["kind"] != "capability"),
        "a legal codebase binding must not surface a capability refusal: {warnings:?}"
    );
}

/// A facet declaring a preparation surfaces the D6 capability refusal as a
/// migrate warning (the format still carries it faithfully).
#[test]
fn migrate_surfaces_preparation_capability_warning() {
    let tmp = fixture("discovery", "");
    let root = tmp.path();
    // Overwrite the facet to declare an (unimplemented) preparation step.
    write_store(
        root,
        "facets/engine/source-tree.json",
        r#"{"name":"source-tree","medium":"src","scope":[{"path":"../public/**/*.rs","mode":"allow"}],"preparation":"pdf-to-markdown"}"#,
    );
    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "migrate", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    let warnings = env["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w["kind"] == "capability"
            && w["message"]
                .as_str()
                .unwrap_or("")
                .contains("pdf-to-markdown")),
        "expected a preparation capability warning, got {warnings:?}"
    );
}

/// `refinement` mode refuses with the typed `PROJECTION_MIGRATE_REFINEMENT`
/// code (exit 5) and writes nothing.
#[test]
fn migrate_refinement_mode_refuses_typed() {
    let tmp = fixture("refinement", "");
    let root = tmp.path();
    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "migrate"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["code"], "PROJECTION_MIGRATE_REFINEMENT");
    // All-or-nothing: the ingest survives untouched.
    assert!(root.join(".memstead/ingests/engine-graph.json").exists());
}

/// A dangling ingest→projection ref refuses with the typed
/// `PROJECTION_MIGRATE_DANGLING_REF` code (exit 5).
#[test]
fn migrate_dangling_ref_refuses_typed() {
    let tmp = fixture("discovery", "");
    let root = tmp.path();
    // Repoint the ingest at a projection that does not exist.
    write_store(
        root,
        "ingests/engine-graph.json",
        r#"{"projection":"engine/missing","mode":"discovery","trigger":"loop","batch_size":20,"deny_paths":[]}"#,
    );
    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "migrate"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["code"], "PROJECTION_MIGRATE_DANGLING_REF");
}

/// Running outside a workspace refuses with the shared, single-sourced
/// `WORKSPACE_NOT_INITIALISED` code — never a generic/internal leak.
#[test]
fn migrate_outside_workspace_is_typed() {
    let tmp = TempDir::new().unwrap();
    let output = memstead()
        .current_dir(tmp.path())
        .args(["--json", "projection", "migrate"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["code"], "WORKSPACE_NOT_INITIALISED");
    assert_ne!(env["code"], "INTERNAL");
}

// ---------------------------------------------------------------------------
// projection init (D8)
// ---------------------------------------------------------------------------

/// Read the three scaffolded files' raw bytes as a comparable triple.
fn triple_bytes(root: &Path, mem: &str, stem: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let m = root.join(format!(".memstead/mediums/{mem}/{stem}.json"));
    let f = root.join(format!(".memstead/facets/{mem}/{stem}.json"));
    let p = root.join(format!(".memstead/projections/{mem}/{stem}.json"));
    (
        std::fs::read(m).unwrap(),
        std::fs::read(f).unwrap(),
        std::fs::read(p).unwrap(),
    )
}

/// A codebase source scaffolds all three files, the binding declares
/// build+sync+verify (matrix-permitting), the on-disk binding round-trips, and
/// the `--json` output matches D8's pinned byte-shape.
#[test]
fn init_codebase_scaffolds_all_three_with_full_operations() {
    let tmp = bare_workspace();
    let root = tmp.path();

    let output = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "init",
            "--mem",
            "engine",
            "--source",
            "../public",
            "--medium-type",
            "codebase",
            "--intent",
            "model the engine",
            "--name",
            "graph",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).expect("--json init must emit JSON");

    // D8 pinned contract byte-shape: { binding, created, operations, warnings }.
    assert_eq!(env["binding"], "engine/graph");
    assert_eq!(
        env["created"],
        serde_json::json!([
            ".memstead/mediums/engine/graph.json",
            ".memstead/facets/engine/graph.json",
            ".memstead/projections/engine/graph.json",
        ])
    );
    assert_eq!(
        env["operations"],
        serde_json::json!(["build", "sync", "verify"])
    );
    assert_eq!(env["warnings"], serde_json::json!([]));
    // Exactly the four contract keys — no extras leaked.
    let keys: Vec<&str> = env
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, vec!["binding", "created", "operations", "warnings"]);

    // All three files exist on disk.
    assert!(root.join(".memstead/mediums/engine/graph.json").is_file());
    assert!(root.join(".memstead/facets/engine/graph.json").is_file());
    assert!(
        root.join(".memstead/projections/engine/graph.json")
            .is_file()
    );

    // The projection file parses as a v1 binding and round-trips losslessly.
    let bytes = std::fs::read(root.join(".memstead/projections/engine/graph.json")).unwrap();
    let b: BindingV1 = serde_json::from_slice(&bytes).expect("scaffold must be a v1 binding");
    assert_eq!(b.version, 1);
    assert_eq!(b.destination_mem, "engine");
    assert_eq!(b.intent.as_deref(), Some("model the engine"));
    assert_eq!(b.source_facets, vec!["graph".to_string()]);
    assert_eq!(b.operations.build.mode, BuildMode::Discovery);
    assert!(b.operations.sync.is_some());
    assert!(b.operations.verify.is_some());
    let round = serde_json::to_string(&b).unwrap();
    let back: BindingV1 = serde_json::from_str(&round).unwrap();
    assert_eq!(back, b);
}

/// A filesystem source likewise scaffolds build+sync+verify (the matrix marks
/// it path-shaped with a change signal).
#[test]
fn init_filesystem_scaffolds_full_operations() {
    let tmp = bare_workspace();
    let root = tmp.path();
    let output = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "init",
            "--mem",
            "docs",
            "--source",
            "../docs",
            "--medium-type",
            "filesystem",
            "--name",
            "manual",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["binding"], "docs/manual");
    assert_eq!(
        env["operations"],
        serde_json::json!(["build", "sync", "verify"])
    );
    assert_eq!(env["warnings"], serde_json::json!([]));
}

/// A `web` source scaffolds build-only, with the deferral named in `warnings[]`
/// (operator decision 7). The binding on disk carries no sync/verify block.
#[test]
fn init_web_source_scaffolds_build_only_with_warning() {
    let tmp = bare_workspace();
    let root = tmp.path();
    let output = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "init",
            "--mem",
            "research",
            "--source",
            "https://example.com/docs",
            "--medium-type",
            "web",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    // Stem derived from the source's final path component.
    assert_eq!(env["binding"], "research/docs");
    assert_eq!(env["operations"], serde_json::json!(["build"]));
    let warnings = env["warnings"].as_array().unwrap();
    assert!(!warnings.is_empty(), "web must warn about the deferral");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("out of scope")
                && w.as_str().unwrap_or("").contains("operator decision 7")),
        "expected a deferral warning, got {warnings:?}"
    );

    // On disk: build-only binding.
    let bytes = std::fs::read(root.join(".memstead/projections/research/docs.json")).unwrap();
    let b: BindingV1 = serde_json::from_slice(&bytes).unwrap();
    assert!(b.operations.sync.is_none());
    assert!(b.operations.verify.is_none());
}

/// Re-running `init` on an existing binding id refuses `PROJECTION_EXISTS`
/// (exit 5) and touches nothing — the three files are byte-identical after the
/// refused second run.
#[test]
fn init_existing_binding_refuses_without_touching_disk() {
    let tmp = bare_workspace();
    let root = tmp.path();
    let args = [
        "projection",
        "init",
        "--mem",
        "engine",
        "--source",
        "../public",
        "--medium-type",
        "codebase",
        "--name",
        "graph",
    ];

    memstead().current_dir(root).args(args).assert().success();
    let before = triple_bytes(root, "engine", "graph");

    // Second run refuses.
    let output = memstead()
        .current_dir(root)
        .args(
            ["--json"]
                .iter()
                .chain(args.iter())
                .copied()
                .collect::<Vec<_>>(),
        )
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["code"], "PROJECTION_EXISTS");
    assert_eq!(env["details"]["binding"], "engine/graph");

    // No partial writes: all three files are byte-identical.
    let after = triple_bytes(root, "engine", "graph");
    assert_eq!(before, after, "refused init must not touch disk");
}

/// `init` outside a workspace refuses with the shared, single-sourced
/// `WORKSPACE_NOT_INITIALISED` code — never a generic/internal leak.
#[test]
fn init_outside_workspace_is_typed() {
    let tmp = TempDir::new().unwrap();
    let output = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "projection",
            "init",
            "--mem",
            "m",
            "--source",
            "../x",
            "--medium-type",
            "codebase",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["code"], "WORKSPACE_NOT_INITIALISED");
    assert_ne!(env["code"], "INTERNAL");
}

// ---------------------------------------------------------------------------
// projection enable (D6 — the remedy a refused mutating op cites)
// ---------------------------------------------------------------------------

/// A gen-2 fixture migrated to a build-only v1 `engine/graph` binding — the
/// substrate for `enable` tests (migrate produces no sync/verify block).
fn migrated_build_only_workspace() -> TempDir {
    let tmp = fixture("discovery", "");
    memstead()
        .current_dir(tmp.path())
        .args(["projection", "migrate"])
        .assert()
        .success();
    tmp
}

/// Enabling `sync` on a codebase binding that lacked it adds the block (with
/// sensible defaults) and round-trips; every other field is untouched, and
/// `verify` stays absent.
#[test]
fn enable_sync_adds_block_to_codebase_binding() {
    let tmp = migrated_build_only_workspace();
    let root = tmp.path();

    let before = read_binding(root);
    assert!(
        before.operations.sync.is_none(),
        "precondition: no sync block"
    );

    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "enable", "sync", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).expect("--json enable must emit JSON");
    assert_eq!(env["binding"], "engine/graph");
    assert_eq!(env["enabled"], "sync");
    assert_eq!(env["operations"], serde_json::json!(["build", "sync"]));

    let after = read_binding(root);
    // The sync block appeared, with the manual trigger and build's batch_size.
    let sync = after
        .operations
        .sync
        .as_ref()
        .expect("sync block was added");
    assert_eq!(sync.trigger, IngestTrigger::Manual);
    assert_eq!(sync.batch_size, before.operations.build.batch_size);
    // verify stays absent — enable adds only the named operation.
    assert!(after.operations.verify.is_none());
    // Every other field is the same declaration.
    assert_eq!(after.version, before.version);
    assert_eq!(after.intent, before.intent);
    assert_eq!(after.source_facets, before.source_facets);
    assert_eq!(after.reference_mems, before.reference_mems);
    assert_eq!(after.destination_mem, before.destination_mem);
    assert_eq!(after.deny_paths, before.deny_paths);
    assert_eq!(after.coverage_semantics, before.coverage_semantics);
    assert_eq!(after.rules, before.rules);
    assert_eq!(after.operations.build, before.operations.build);

    // Round-trips losslessly.
    let json = serde_json::to_string(&after).unwrap();
    let back: BindingV1 = serde_json::from_str(&json).unwrap();
    assert_eq!(back, after);
}

/// Enabling `sync` on a `web`-medium binding refuses with the capability error
/// and leaves the binding file byte-identical (no partial write).
#[test]
fn enable_sync_on_web_refuses_and_leaves_file_identical() {
    let tmp = bare_workspace();
    let root = tmp.path();
    // Scaffold a build-only web binding (init strips sync/verify over web).
    memstead()
        .current_dir(root)
        .args([
            "projection",
            "init",
            "--mem",
            "research",
            "--source",
            "https://example.com/docs",
            "--medium-type",
            "web",
        ])
        .assert()
        .success();

    let path = root.join(".memstead/projections/research/docs.json");
    let before = std::fs::read(&path).unwrap();

    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "enable", "sync", "research/docs"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["code"], "PROJECTION_CAPABILITY_UNSUPPORTED");
    assert!(
        env["message"]
            .as_str()
            .unwrap_or("")
            .contains("out of scope"),
        "capability refusal must state the gap: {env}"
    );

    // The file is untouched by the refused enable.
    let after = std::fs::read(&path).unwrap();
    assert_eq!(before, after, "refused enable must not touch disk");
}

/// Enabling an already-present operation refuses `PROJECTION_OP_ALREADY_ENABLED`
/// and does not corrupt the binding. `build` is always present, so enabling it
/// always lands here.
#[test]
fn enable_already_present_op_refuses() {
    let tmp = migrated_build_only_workspace();
    let root = tmp.path();

    // `build` is always present on any binding.
    let path = root.join(".memstead/projections/engine/graph.json");
    let before = std::fs::read(&path).unwrap();
    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "enable", "build", "engine/graph"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["code"], "PROJECTION_OP_ALREADY_ENABLED");
    assert_eq!(env["details"]["operation"], "build");
    assert_eq!(std::fs::read(&path).unwrap(), before, "refusal is a no-op");

    // Enable sync once (succeeds), then again → already-enabled, still clean.
    memstead()
        .current_dir(root)
        .args(["projection", "enable", "sync", "engine/graph"])
        .assert()
        .success();
    let with_sync = std::fs::read(&path).unwrap();
    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "enable", "sync", "engine/graph"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["code"], "PROJECTION_OP_ALREADY_ENABLED");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        with_sync,
        "re-enable is a no-op and does not corrupt the binding"
    );
    // Still a valid v1 binding with exactly one sync block.
    let b = read_binding(root);
    assert!(b.operations.sync.is_some());
    assert!(b.operations.verify.is_none());
}

/// Enabling an operation on a missing binding refuses `PROJECTION_NOT_FOUND`
/// (exit 3, NotFound) — never a generic/internal leak.
#[test]
fn enable_missing_binding_is_not_found() {
    let tmp = bare_workspace();
    let root = tmp.path();
    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "enable", "sync", "engine/nope"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["code"], "PROJECTION_NOT_FOUND");
    assert_eq!(env["details"]["binding"], "engine/nope");
}

/// A malformed binding id (no `/`) refuses `PROJECTION_INVALID_NAME` before any
/// disk access.
#[test]
fn enable_malformed_binding_id_refuses() {
    let tmp = bare_workspace();
    let root = tmp.path();
    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "enable", "verify", "noslash"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["code"], "PROJECTION_INVALID_NAME");
}

/// `enable` outside a workspace refuses with the shared, single-sourced
/// `WORKSPACE_NOT_INITIALISED` code — never a generic/internal leak.
#[test]
fn enable_outside_workspace_is_typed() {
    let tmp = TempDir::new().unwrap();
    let output = memstead()
        .current_dir(tmp.path())
        .args(["--json", "projection", "enable", "sync", "engine/graph"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["code"], "WORKSPACE_NOT_INITIALISED");
    assert_ne!(env["code"], "INTERNAL");
}
