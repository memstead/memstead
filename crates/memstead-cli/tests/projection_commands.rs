//! Integration tests for the `memstead projection` command tree.
//!
//! Leaves covered: `brief` (D9 — render a binding's run-brief, and its
//! typed-refusal paths), `init` (D8 — non-interactive v1 scaffold), `migrate`
//! (D10 — four-primitive → v1 bindings), `enable`, and `advance`.
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
    assert_eq!(
        b.operations.build.as_ref().unwrap().mode,
        BuildMode::Discovery
    );
    assert_eq!(
        b.operations.build.as_ref().unwrap().trigger,
        IngestTrigger::Loop
    );
    assert_eq!(b.operations.build.as_ref().unwrap().batch_size, 20);
    assert_eq!(
        b.operations.build.as_ref().unwrap().post_actions,
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
    assert_eq!(
        b.operations.build.as_ref().unwrap().mode,
        BuildMode::Discovery
    );
    assert!(b.operations.sync.is_some());
    assert!(b.operations.verify.is_some());
    // F1 — a git-backed (codebase) source scaffolds a prune block with the
    // strongest supported guarantee: never-clobber (base leg retrievable).
    assert_eq!(
        b.prune.as_ref().unwrap().guarantee,
        memstead_base::binding::PruneGuarantee::NeverClobber
    );
    let round = serde_json::to_string(&b).unwrap();
    let back: BindingV1 = serde_json::from_str(&round).unwrap();
    assert_eq!(back, b);
}

/// Round-trip pin (Rust half): `projection init` still emits **exactly** the
/// committed golden binding the plugin's v1 schema test validates against
/// `binding.schema.json`. The JS half (in the v1 validator suite) proves the
/// golden validates against the schema; this proves init still produces that
/// golden. Together they keep the plugin's `memstead-plugin/v1` binding schema
/// and the engine's emitter from drifting apart: change the emitter's shape and
/// this fails until the golden (and thus the schema check) is revisited.
#[test]
fn init_output_matches_the_v1_schema_golden() {
    let tmp = bare_workspace();
    let root = tmp.path();

    // Args chosen to match the committed golden's content (mem, intent, name;
    // the source pointer lands only in the medium file, not the binding).
    memstead()
        .current_dir(root)
        .args([
            "projection",
            "init",
            "--mem",
            "docs",
            "--source",
            "../src",
            "--medium-type",
            "codebase",
            "--intent",
            "Keep the reference mem true to the source tree",
            "--name",
            "guide",
        ])
        .assert()
        .success();

    let emitted: Value = serde_json::from_slice(
        &std::fs::read(root.join(".memstead/projections/docs/guide.json")).unwrap(),
    )
    .unwrap();

    // The golden ships with the plugin's v1 schema examples (repo-root-relative
    // to the cli crate: two levels up to `public/`, then the schemas tree).
    let golden_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../plugins/claude-code/schemas/memstead-plugin/v1/examples/binding.from-init.json",
    );
    let golden: Value = serde_json::from_slice(&std::fs::read(&golden_path).unwrap_or_else(|e| {
        panic!(
            "golden fixture unreadable at {}: {e}",
            golden_path.display()
        )
    }))
    .unwrap();

    assert_eq!(
        emitted,
        golden,
        "`projection init` output drifted from the committed v1 binding golden \
         ({}). Update the golden AND re-check binding.schema.json — the two must \
         move together.",
        golden_path.display()
    );
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
    assert_eq!(
        sync.batch_size,
        before.operations.build.as_ref().unwrap().batch_size
    );
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

// ---------------------------------------------------------------------------
// projection advance (D7)
// ---------------------------------------------------------------------------

/// Run `git` in `repo`, panicking on failure (deterministic committer identity).
fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_head(repo: &Path) -> String {
    String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}

/// Build a bootable **filesystem** workspace (no `mem-repo/.git`) with one
/// writable folder mem `engine`, a v1 binding `engine/graph` over a git source
/// tree at `<root>/src`, and the source moved from a base commit to `head1`
/// (a.rs modified, b.rs deleted). The base commit's sha is pre-seeded into the
/// mem's `syncState` so `advance` sees a real changed slice. Written directly
/// into the mem config (not via `mem set-sync-state`) so the test is
/// flavour-independent — the lean CLI has no `mem` subcommand.
fn advance_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Workspace adapter + engine folder mount.
    write_store(
        root,
        "workspace.toml",
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    );
    write_store(
        root,
        "state/mounts.json",
        r#"{"format":"memstead-mounts-3","mounts":[{"mem":"engine","schema":"default@1.0.0","storage":{"type":"folder","path":"engine-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}]}"#,
    );

    // v1 binding store: medium (git codebase at `src`), facet, binding.
    write_store(
        root,
        "mediums/engine/graph.json",
        r#"{"name":"graph","type":"codebase","pointer":"src","change_detection":"git"}"#,
    );
    write_store(
        root,
        "facets/engine/source-tree.json",
        r#"{"name":"source-tree","medium":"graph","scope":[{"path":"src/**/*.rs","mode":"allow"}]}"#,
    );
    write_store(
        root,
        "projections/engine/graph.json",
        r#"{"version":1,"intent":"model the engine","source_facets":["source-tree"],"reference_mems":[],"destination_mem":"engine","deny_paths":[],"coverage_semantics":"exhaustive","operations":{"build":{"mode":"discovery","trigger":"loop","batch_size":20},"sync":{"trigger":"manual","batch_size":20}}}"#,
    );

    // The git source tree: base (a.rs + b.rs), then head1 (modify a.rs, delete b.rs).
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q"]);
    std::fs::write(src.join("a.rs"), "one").unwrap();
    std::fs::write(src.join("b.rs"), "bee").unwrap();
    git(&src, &["add", "a.rs", "b.rs"]);
    git(&src, &["commit", "-qm", "base"]);
    let baseline = git_head(&src);
    std::fs::write(src.join("a.rs"), "one-longer").unwrap();
    std::fs::remove_file(src.join("b.rs")).unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-qm", "head1"]);

    // The destination mem's config, with the sync baseline pre-seeded so the
    // changed slice (a.rs modified, b.rs deleted) is presented.
    let mem_meta = root.join("engine-mem").join(".memstead");
    std::fs::create_dir_all(&mem_meta).unwrap();
    std::fs::write(
        mem_meta.join("config.json"),
        format!(
            r#"{{"format":1,"schema":"default@1.0.0","syncState":{{"engine/graph/source-tree#synced":"{baseline}"}}}}"#
        ),
    )
    .unwrap();

    tmp
}

/// End-to-end through the CLI (three separate processes, proving on-disk
/// resumability): advance a partial disposition, refuse an unknown artifact
/// atomically, then complete — the `#synced` token advancing.
#[test]
fn advance_records_dispositions_completes_and_gates_unknown() {
    let tmp = advance_workspace();
    let root = tmp.path();

    // (1) Dispose a.rs → remainder = b.rs (deleted), not complete.
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "advance",
            "engine/graph",
            "--dispositions",
            r#"{"src/a.rs": "worked"}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).expect("advance --json must emit JSON");
    assert_eq!(env["binding"], "engine/graph");
    assert_eq!(env["completed"], false);
    assert_eq!(env["pending"], 1);
    assert_eq!(env["disposed"], 1);
    assert_eq!(env["remainder"]["deleted"], serde_json::json!(["src/b.rs"]));
    assert_eq!(env["remainder"]["modified"], serde_json::json!([]));

    // (2) An unknown artifact id refuses the whole call atomically.
    let store_path = root.join(".memstead/state/advance/engine/graph.json");
    let before = std::fs::read(&store_path).unwrap();
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "advance",
            "engine/graph",
            "--dispositions",
            r#"{"src/never.rs": "worked"}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], "PROJECTION_ADVANCE_UNKNOWN_ARTIFACT");
    let after = std::fs::read(&store_path).unwrap();
    assert_eq!(before, after, "refused advance must not touch the store");

    // (3) Dispose the rest → complete → the `#synced` token advances. The a.rs
    // disposition from step (1) persisted across processes (resumability).
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "advance",
            "engine/graph",
            "--dispositions",
            r#"{"src/b.rs": "worked"}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["completed"], true);
    assert_eq!(env["pending"], 0);
    assert_eq!(env["disposed"], 2, "a.rs (persisted) + b.rs (this call)");
    assert_eq!(
        env["tokens_written"],
        serde_json::json!(["engine/graph/source-tree#synced"])
    );
    // The durable store was dropped on completion.
    assert!(!store_path.exists());
}

/// `advance` on a missing binding refuses with `PROJECTION_NOT_FOUND` (NotFound
/// exit) — before any engine boot.
#[test]
fn advance_missing_binding_is_typed() {
    let tmp = bare_workspace();
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "projection",
            "advance",
            "engine/nope",
            "--dispositions",
            "{}",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], "PROJECTION_NOT_FOUND");
    assert_ne!(env["code"], "INTERNAL");
}

/// `advance` with a malformed `--dispositions` payload refuses with
/// `PROJECTION_INVALID_DISPOSITIONS` before touching configs or an engine.
#[test]
fn advance_invalid_dispositions_is_typed() {
    let tmp = bare_workspace();
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "projection",
            "advance",
            "engine/graph",
            "--dispositions",
            "not-json",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], "PROJECTION_INVALID_DISPOSITIONS");
}

/// `projection exclude` records an authored exclusion for a **stable in-scope**
/// artifact (not in any changed slice), gates a non-member atomically, and
/// rejects a malformed payload — the direct write path for the exclusion ledger.
#[test]
fn exclude_records_authored_exclusion_and_gates_non_member() {
    let tmp = advance_workspace();
    let root = tmp.path();
    // S(D) for this binding = files on disk matching `src/**/*.rs` = {src/a.rs}
    // (b.rs was deleted at head1). a.rs is a stable member — declarable excluded.
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "exclude",
            "engine/graph",
            "--exclusions",
            r#"{"src/a.rs": "mined; warrants no entity"}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).expect("exclude --json must emit JSON");
    assert_eq!(env["binding"], "engine/graph");
    assert_eq!(env["added"], 1);
    assert_eq!(env["excluded"], 1);

    // The exclusion + rationale persisted to the durable ledger.
    let store: Value = serde_json::from_slice(
        &std::fs::read(root.join(".memstead/state/advance/engine/graph.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(store["exclusions"]["src/a.rs"], "mined; warrants no entity");

    // An artifact outside S(D) refuses the whole call atomically.
    let before = std::fs::read(root.join(".memstead/state/advance/engine/graph.json")).unwrap();
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "exclude",
            "engine/graph",
            "--exclusions",
            r#"{"src/not-a-file.rs": "x"}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], "PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER");
    let after = std::fs::read(root.join(".memstead/state/advance/engine/graph.json")).unwrap();
    assert_eq!(before, after, "refused call must not touch the ledger");

    // A malformed payload refuses with the typed parse code.
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "exclude",
            "engine/graph",
            "--exclusions",
            "not-json",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], "PROJECTION_INVALID_EXCLUSIONS");
}

/// `advance` outside a workspace refuses with the shared, single-sourced
/// `WORKSPACE_NOT_INITIALISED` code — never a generic/internal leak.
#[test]
fn advance_outside_workspace_is_typed() {
    let tmp = TempDir::new().unwrap();
    let out = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "projection",
            "advance",
            "engine/graph",
            "--dispositions",
            "{}",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], "WORKSPACE_NOT_INITIALISED");
    assert_ne!(env["code"], "INTERNAL");
}

// ── brief (D9) ───────────────────────────────────────────────────────────────

/// `projection brief <mem>/<stem>` renders a binding's discovery run-brief,
/// headed by the canonical binding id (D3/D9). Scaffold a binding with
/// `projection init`, then render it.
#[cfg(feature = "mem-repo")]
#[test]
fn brief_renders_for_scaffolded_binding() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    memstead()
        .current_dir(&ws)
        .args([
            "projection",
            "init",
            "--mem",
            "ws",
            "--source",
            "../src",
            "--medium-type",
            "codebase",
            "--name",
            "code",
        ])
        .assert()
        .success();

    let out = memstead()
        .current_dir(&ws)
        .args(["projection", "brief", "ws/code"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let brief = String::from_utf8(out).unwrap();
    assert!(
        brief.contains("ws/code"),
        "brief must name the canonical binding id; got:\n{brief}"
    );
    assert!(
        brief.contains("## Situation"),
        "a discovery brief carries the Situation block; got:\n{brief}"
    );
}

/// `projection brief --all` on a workspace with NO bindings configured reports
/// a distinct `no_bindings` outcome (exit 0) — not the all-backing-off
/// `skipped` outcome, which would otherwise collapse into the same `None`. A
/// caller (the plugin's setup ramp, a status display) branches on this to
/// prompt first-time setup rather than retry a no-op pass.
#[cfg(feature = "mem-repo")]
#[test]
fn brief_all_empty_store_reports_no_bindings() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();

    // JSON: the distinct `{ "no_bindings": true }` envelope.
    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "projection", "brief", "--all"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["no_bindings"], Value::Bool(true));
    assert!(
        env.get("skipped").is_none(),
        "empty store must NOT report the backing-off `skipped` outcome; got:\n{env}"
    );

    // Markdown: a distinct, human-readable no-bindings line (not "backing off").
    let out = memstead()
        .current_dir(&ws)
        .args(["projection", "brief", "--all"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let md = String::from_utf8(out).unwrap();
    assert!(
        md.contains("No bindings configured"),
        "empty store gets a distinct no-bindings message; got:\n{md}"
    );
    assert!(
        !md.contains("backing off"),
        "empty store must not use the backing-off message; got:\n{md}"
    );
}

/// `projection brief <binding> --verify` renders the verify brief (group C):
/// measurement + capped-adjudication instructions only, with the explicit
/// no-mutation refusal and NO repair block. Read-only on the mem.
#[cfg(feature = "mem-repo")]
#[test]
fn brief_verify_renders_measurement_only() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    memstead()
        .current_dir(&ws)
        .args([
            "projection",
            "init",
            "--mem",
            "ws",
            "--source",
            "../src",
            "--medium-type",
            "codebase",
            "--name",
            "code",
        ])
        .assert()
        .success();

    let out = memstead()
        .current_dir(&ws)
        .args(["projection", "brief", "ws/code", "--verify"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let brief = String::from_utf8(out).unwrap();
    assert!(brief.contains("## Verify — measure fidelity, do not mutate"));
    assert!(
        brief.contains("Verify writes **nothing** into the destination mem"),
        "C1 refusal present; got:\n{brief}"
    );
    // C1/C2 refusal: the verify brief carries NO repair block.
    assert!(
        !brief.contains("## How to repair"),
        "verify brief must not carry repair instructions; got:\n{brief}"
    );
    assert!(!brief.contains("## Open findings to repair"));
}

/// `projection brief <binding> --sync` renders the sync brief (group C): the
/// sole-maintenance-writer prompt with the absorbed reconcile conservatism. A
/// fresh mem (no anchors, never synced) triggers the adopt / first-sync framing.
#[cfg(feature = "mem-repo")]
#[test]
fn brief_sync_renders_sole_writer_with_conservatism() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    memstead()
        .current_dir(&ws)
        .args([
            "projection",
            "init",
            "--mem",
            "ws",
            "--source",
            "../src",
            "--medium-type",
            "codebase",
            "--name",
            "code",
        ])
        .assert()
        .success();

    let out = memstead()
        .current_dir(&ws)
        .args(["projection", "brief", "ws/code", "--sync"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let brief = String::from_utf8(out).unwrap();
    assert!(brief.contains("## Sync — repair the graph to match the source"));
    assert!(brief.contains("sole maintenance writer"));
    assert!(brief.contains("Sync commits nothing."));
    // Fresh mem → adopt / first-sync framing (E1 brief half).
    assert!(
        brief.contains("## First sync — adopting `ws`"),
        "fresh mem gets adopt framing; got:\n{brief}"
    );
    // Absorbed reconcile conservatism (C3).
    assert!(brief.contains("## How to repair — be conservative"));
    assert!(brief.contains("A dropped dependency FLAGS, it does not auto-remove."));
    assert!(brief.contains("`[commit <hash>]` log-style entries"));
}

/// `projection brief --verify` / `--sync` without a binding id refuses with a
/// typed `PROJECTION_BRIEF_BINDING_REQUIRED` — they render one binding, never an
/// `--all` rotation.
#[cfg(feature = "mem-repo")]
#[test]
fn brief_verify_sync_require_a_binding() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();

    for flag in ["--verify", "--sync"] {
        let out = memstead()
            .current_dir(&ws)
            .args(["--json", "projection", "brief", flag])
            .assert()
            .failure()
            .get_output()
            .stdout
            .clone();
        let env: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(env["code"], "PROJECTION_BRIEF_BINDING_REQUIRED");
        assert_ne!(env["code"], "INTERNAL");
    }
}

/// `projection brief` on an unknown binding id refuses `PROJECTION_NOT_FOUND`
/// (NotFound exit) — never a generic/internal leak.
#[cfg(feature = "mem-repo")]
#[test]
fn brief_missing_binding_refuses() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "projection", "brief", "engine/nope"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], "PROJECTION_NOT_FOUND");
    assert_ne!(env["code"], "INTERNAL");
}

/// `projection brief` outside a workspace refuses with the shared,
/// single-sourced `WORKSPACE_NOT_INITIALISED` code — never a generic/internal
/// leak. Runs on both build flavours (no engine is built before the check).
#[test]
fn brief_outside_workspace_is_typed() {
    let tmp = TempDir::new().unwrap();
    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "projection", "brief", "engine/graph"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], "WORKSPACE_NOT_INITIALISED");
    assert_ne!(env["code"], "INTERNAL");
}

// ── migrate: gen-1 root-folder path (folded from the retired `pipeline migrate`) ──

/// A gen-1 root-folder workspace (`scopes|projections|ingests/` at the root)
/// migrates straight to a v1 binding in one `projection migrate` pass (D10,
/// gen-1 path — folded from the retired `pipeline migrate` command).
#[test]
fn migrate_gen1_root_folder_promotes_to_v1_binding() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_store(root, "workspace.toml", "");

    let write_root = |rel: &str, contents: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    };
    write_root(
        "scopes/engine/src.json",
        r#"{"type":"codebase","scope":{"tree":[{"path":"../public/**/*.rs","mode":"allow"}]}}"#,
    );
    write_root(
        "projections/engine/graph.json",
        r#"{"intent":"the engine graph","sources":[{"scope_ref":"src"}],"destinations":[{"mem":"engine"}]}"#,
    );
    write_root(
        "ingests/engine-graph.json",
        r#"{"projection":"engine/graph","mode":"discovery","trigger":"loop","batch_size":20,"deny_paths":[]}"#,
    );

    let output = memstead()
        .current_dir(root)
        .args(["--json", "projection", "migrate"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(env["migrated"], 1);
    assert_eq!(env["bindings"][0], "engine/graph");

    // The projection was promoted to a v1 binding in the `.memstead/` store.
    let b = read_binding(root);
    assert_eq!(b.version, 1);
    assert_eq!(b.destination_mem, "engine");
    assert_eq!(b.source_facets, vec!["src".to_string()]);
    assert_eq!(
        b.operations.build.as_ref().unwrap().mode,
        BuildMode::Discovery
    );
    // The merged flat ingest was consumed; medium + facet were materialized.
    assert!(!root.join(".memstead/ingests/engine-graph.json").exists());
    assert!(root.join(".memstead/mediums/engine/src.json").exists());
    assert!(root.join(".memstead/facets/engine/src.json").exists());
}

/// `--dry-run` on a gen-1 root-folder workspace previews the promotion without
/// materializing the gen-2 store or touching the root-folder layout.
#[test]
fn migrate_gen1_dry_run_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_store(root, "workspace.toml", "");
    let write_root = |rel: &str, contents: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    };
    write_root(
        "scopes/engine/src.json",
        r#"{"type":"codebase","scope":{"tree":[{"path":"../public/**/*.rs","mode":"allow"}]}}"#,
    );
    write_root(
        "projections/engine/graph.json",
        r#"{"intent":"the engine graph","sources":[{"scope_ref":"src"}],"destinations":[{"mem":"engine"}]}"#,
    );
    write_root(
        "ingests/engine-graph.json",
        r#"{"projection":"engine/graph","mode":"discovery","trigger":"loop","batch_size":20,"deny_paths":[]}"#,
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
    assert_eq!(env["dry_run"], true);
    assert_eq!(env["migrated"], 1);
    // Nothing materialized under `.memstead/` (no gen-2 store written).
    assert!(
        !root
            .join(".memstead/projections/engine/graph.json")
            .exists()
    );
    assert!(!root.join(".memstead/mediums/engine/src.json").exists());
}

// ── AC4: absent-operation-block refusal + `projection enable` remedy ─────────

/// D6/AC4: `projection brief` on a binding with **no build block** refuses with
/// the `projection enable build <binding>` remedy, and that command — run
/// verbatim — makes the same brief succeed.
#[test]
fn brief_refuses_absent_build_then_enable_build_remedy_succeeds() {
    let tmp = advance_workspace();
    let root = tmp.path();
    // Strip the build block — a verify-only binding (verify is read-only, never
    // a refusal, so it is a legal build-less shape).
    write_store(
        root,
        "projections/engine/graph.json",
        r#"{"version":1,"intent":"model the engine","source_facets":["source-tree"],"reference_mems":[],"destination_mem":"engine","deny_paths":[],"coverage_semantics":"exhaustive","operations":{"verify":{"trigger":"manual","batch_size":20}}}"#,
    );

    // brief refuses with the one-command remedy.
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "brief", "engine/graph"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).expect("brief refusal must emit JSON");
    assert_eq!(env["code"], "PROJECTION_BUILD_NOT_ENABLED");
    assert!(
        env["message"]
            .as_str()
            .unwrap_or("")
            .contains("memstead projection enable build engine/graph"),
        "message must carry the verbatim remedy: {env}",
    );

    // The cited command, run verbatim, enables build.
    memstead()
        .current_dir(root)
        .args(["projection", "enable", "build", "engine/graph"])
        .assert()
        .success();

    // The same brief now succeeds.
    memstead()
        .current_dir(root)
        .args(["projection", "brief", "engine/graph"])
        .assert()
        .success();
}

/// D6/AC4: `projection advance` on a binding with **no sync block** refuses with
/// the `projection enable sync <binding>` remedy, and that command — run
/// verbatim — makes the same advance succeed.
#[test]
fn advance_refuses_absent_sync_then_enable_sync_remedy_succeeds() {
    let tmp = advance_workspace();
    let root = tmp.path();
    // Strip the sync block so the advance (sync) path has none to run.
    write_store(
        root,
        "projections/engine/graph.json",
        r#"{"version":1,"intent":"model the engine","source_facets":["source-tree"],"reference_mems":[],"destination_mem":"engine","deny_paths":[],"coverage_semantics":"exhaustive","operations":{"build":{"mode":"discovery","trigger":"loop","batch_size":20}}}"#,
    );
    assert!(read_binding(root).operations.sync.is_none());

    // advance (the sync path) refuses with the one-command remedy.
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "advance",
            "engine/graph",
            "--dispositions",
            "{}",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).expect("advance refusal must emit JSON");
    assert_eq!(env["code"], "PROJECTION_SYNC_NOT_ENABLED");
    assert!(
        env["message"]
            .as_str()
            .unwrap_or("")
            .contains("memstead projection enable sync engine/graph"),
        "message must carry the verbatim remedy: {env}",
    );

    // The cited command, run verbatim, enables sync.
    memstead()
        .current_dir(root)
        .args(["projection", "enable", "sync", "engine/graph"])
        .assert()
        .success();

    // The same advance now succeeds (empty dispositions re-present the slice).
    memstead()
        .current_dir(root)
        .args([
            "projection",
            "advance",
            "engine/graph",
            "--dispositions",
            "{}",
        ])
        .assert()
        .success();
}

/// Verify-path resolution succeeds with **no verify block** (defaults, never a
/// refusal): a build-only binding renders its brief clean.
#[test]
fn brief_succeeds_with_no_verify_block() {
    let tmp = advance_workspace();
    // The migrated binding is build-only (no verify). Its brief renders.
    memstead()
        .current_dir(tmp.path())
        .args(["projection", "brief", "engine/graph"])
        .assert()
        .success();
}

// ── AC12: `projection migrate` consumes reconcile-cursors.json (D10) ─────────

/// D10/AC12: `projection migrate` seeds the destination binding's `#synced`
/// token from a `reconcile-cursors.json` entry whose absolute-keyed path
/// resolves to the binding's medium pointer, then deletes the cursor file.
#[test]
fn migrate_consumes_reconcile_cursors_seeds_synced_and_deletes_it() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Workspace adapter + a folder-mounted `engine` mem (so set_mem_sync_state
    // has a writable mem with a loaded config).
    write_store(
        root,
        "workspace.toml",
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    );
    write_store(
        root,
        "state/mounts.json",
        r#"{"format":"memstead-mounts-3","mounts":[{"mem":"engine","schema":"default@1.0.0","storage":{"type":"folder","path":"engine-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}]}"#,
    );
    let mem_meta = root.join("engine-mem").join(".memstead");
    std::fs::create_dir_all(&mem_meta).unwrap();
    std::fs::write(
        mem_meta.join("config.json"),
        br#"{"format":1,"schema":"default@1.0.0"}"#,
    )
    .unwrap();

    // A real source dir the medium pointer resolves to.
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.rs"), "x").unwrap();

    // Gen-2 store: medium (codebase → `src`), facet, projection, flat ingest.
    write_store(
        root,
        "mediums/engine/src.json",
        r#"{"name":"src","type":"codebase","pointer":"src"}"#,
    );
    write_store(
        root,
        "facets/engine/source-tree.json",
        r#"{"name":"source-tree","medium":"src","scope":[{"path":"src/**/*.rs","mode":"allow"}]}"#,
    );
    write_store(
        root,
        "projections/engine/graph.json",
        r#"{"intent":"engine graph","source_facets":["source-tree"],"reference_mems":[],"destination_mem":"engine"}"#,
    );
    write_store(
        root,
        "ingests/engine-graph.json",
        r#"{"projection":"engine/graph","mode":"discovery","trigger":"loop","batch_size":20}"#,
    );

    // A skill-written reconcile-cursors.json keyed to `src`'s absolute path.
    let src_abs = std::fs::canonicalize(&src).unwrap();
    write_store(
        root,
        "reconcile-cursors.json",
        &format!(r#"{{"engine:{}":"cafebabe0000"}}"#, src_abs.display()),
    );

    // Migrate.
    memstead()
        .current_dir(root)
        .args(["projection", "migrate"])
        .assert()
        .success();

    // The `#synced` baseline was seeded from the cursor's sha, on the mem config.
    let cfg: Value =
        serde_json::from_slice(&std::fs::read(mem_meta.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        cfg["syncState"]["engine/graph/source-tree#synced"], "cafebabe0000",
        "migrate seeded #synced from the absolute-keyed cursor sha: {cfg}",
    );

    // The cursor file was consumed (deleted).
    assert!(
        !root.join(".memstead/reconcile-cursors.json").exists(),
        "reconcile-cursors.json must be deleted by the migration",
    );
}

/// A cursorless migrate leaves the binding never-synced and writes no baseline.
#[test]
fn migrate_without_cursor_leaves_never_synced() {
    let tmp = migrated_build_only_workspace();
    let root = tmp.path();
    // No reconcile-cursors.json existed → no #synced token anywhere. The
    // migrate succeeded (asserted by the helper) and left no cursor artifact.
    assert!(!root.join(".memstead/reconcile-cursors.json").exists());
}
