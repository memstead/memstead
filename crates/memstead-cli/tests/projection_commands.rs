//! Integration tests for the `memstead projection` command tree.
//!
//! This slice ships the `migrate` leaf (gen-2 four-primitive → v1 bindings,
//! D10 gen-2 path). The tests build a fixture gen-2 workspace on disk, run
//! `memstead projection migrate`, and assert: the produced v1 binding
//! round-trips and carries the merged build operations; the merged ingest is
//! removed; `refinement` mode and a dangling ingest→projection ref each refuse
//! with a typed `PROJECTION_*` code (exit 5); and `--dry-run` writes nothing.

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
