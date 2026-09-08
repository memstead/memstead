//! Integration tests for the `memstead projection` command tree.
//!
//! Leaves covered: `brief` (D9 — render a binding's run-brief, and its
//! typed-refusal paths), `init` (D8 — non-interactive v2 scaffold), `enable`,
//! `edit`, and `advance`.
//!
//! `init` tests assert: a codebase/filesystem source scaffolds all three files
//! (`mediums`/`facets`/`projections`) with `operations:[build,sync,verify]` and
//! a round-trippable v1 binding; a `web` source scaffolds build-only with a
//! deferral warning; the `--json` output matches D8's pinned byte-shape; and a
//! re-run on an existing id refuses `PROJECTION_EXISTS` without touching disk
//! (the three files are byte-identical after the refused second run).

use assert_cmd::Command;
use memstead_base::binding::{Binding, BuildMode};
use memstead_base::pipeline::IngestTrigger;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// The binary's own path, for replaying a printed command through a shell.
fn memstead_bin() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("memstead")
}

/// Write `contents` to `<root>/.memstead/<rel>`, creating parent dirs.
fn write_store(root: &Path, rel: &str, contents: &str) {
    let path = root.join(".memstead").join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// The smallest workspace store the engine boots: a `workspace.toml` naming
/// its format and adapter, no mounts. `projection init` and `projection edit`
/// boot a scoped engine to read the destination schema for the intent rule,
/// so a fixture with an empty marker file (which the engine has never
/// accepted) would refuse `WORKSPACE_STORE_PARSE` before the command under
/// test runs.
const BARE_WORKSPACE_TOML: &str =
    "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n";

/// A bare workspace: just the `.memstead/workspace.toml` store, no mounts.
fn bare_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write_store(tmp.path(), "workspace.toml", BARE_WORKSPACE_TOML);
    tmp
}

/// A workspace holding one build-only v2 binding `engine/graph` over a
/// codebase source `source-tree` (pointer `../public`, scope `**/*.rs`),
/// written raw. The substrate for the `enable` and `edit` tests.
fn build_only_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_store(root, "workspace.toml", BARE_WORKSPACE_TOML);
    write_store(
        root,
        "projections/engine/graph.json",
        r#"{"version":2,"intent":"the engine graph","sources":[{"name":"source-tree","type":"codebase","pointer":"../public","scope":[{"path":"**/*.rs","mode":"allow"}]}],"reference_mems":["plugin"],"destination_mem":"engine","deny_paths":[],"rules":{"routing":"r"},"operations":{"build":{"mode":"discovery","trigger":"loop","batch_size":20}}}"#,
    );
    tmp
}

fn read_binding(root: &Path) -> Binding {
    let bytes = std::fs::read(root.join(".memstead/projections/engine/graph.json")).unwrap();
    serde_json::from_slice(&bytes).expect("binding file must parse as a v2 binding")
}

// ---------------------------------------------------------------------------
// projection init
// ---------------------------------------------------------------------------

/// Read the scaffolded binding file's raw bytes.
fn scaffold_bytes(root: &Path, mem: &str, stem: &str) -> Vec<u8> {
    std::fs::read(root.join(format!(".memstead/projections/{mem}/{stem}.json"))).unwrap()
}

/// A codebase source scaffolds ONE v2 record with the source inline, the
/// binding declares build+sync+verify (matrix-permitting), the on-disk
/// binding round-trips, and the `--json` output matches the pinned
/// byte-shape.
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

    // Pinned contract byte-shape: { binding, created, operations, warnings }.
    assert_eq!(env["binding"], "engine/graph");
    assert_eq!(
        env["created"],
        serde_json::json!([".memstead/projections/engine/graph.json"])
    );
    assert_eq!(
        env["operations"],
        serde_json::json!(["build", "sync", "verify"])
    );
    // `--source ../public` resolves outside this workspace root — init
    // warns with the works/degrades split and the common-parent recipe
    // (never the retired "anchors orphan" claim — post pointer-join,
    // out-of-root anchors resolve) and still succeeds.
    // Two warnings: the layout caveat, and — because this fixture's source
    // tree does not exist — the absent-source note. A declared pointer that
    // resolves to nothing is legal but never silent.
    let warnings = env["warnings"].as_array().expect("warnings array");
    assert_eq!(warnings.len(), 2, "got: {warnings:?}");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("which does not exist")),
        "the absent source must be named: {warnings:?}",
    );
    let w = warnings[0].as_str().unwrap();
    assert!(w.contains("outside the workspace root"), "got: {w}");
    assert!(
        w.contains("anchor resolution all work"),
        "warning must state what works: {w}"
    );
    assert!(
        w.contains("common parent directory"),
        "warning must carry the relocation recipe: {w}"
    );
    assert!(
        !w.contains("orphan"),
        "the retired anchors-orphan claim must not reappear: {w}"
    );
    // Exactly the four contract keys — no extras leaked.
    let keys: Vec<&str> = env
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, vec!["binding", "created", "operations", "warnings"]);

    // Exactly one file exists on disk — no mediums/facets trees appear.
    assert!(
        root.join(".memstead/projections/engine/graph.json")
            .is_file()
    );
    assert!(!root.join(".memstead/mediums").exists());
    assert!(!root.join(".memstead/facets").exists());

    // The projection file parses as a v2 binding and round-trips losslessly.
    let bytes = scaffold_bytes(root, "engine", "graph");
    let b: Binding = serde_json::from_slice(&bytes).expect("scaffold must be a v2 binding");
    assert_eq!(b.version, 2);
    assert_eq!(b.destination_mem, "engine");
    assert_eq!(b.intent.as_deref(), Some("model the engine"));
    assert_eq!(b.sources.len(), 1);
    assert_eq!(b.sources[0].name, "graph");
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
    let back: Binding = serde_json::from_str(&round).unwrap();
    assert_eq!(back, b);
}

/// Complement to the out-of-root warning: an IN-root source scaffolds with
/// no warnings at all — the layout hint fires only where the shape is
/// affected, never as ambient noise.
#[test]
fn init_in_root_codebase_source_warns_nothing() {
    let tmp = bare_workspace();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();

    let output = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "init",
            "--mem",
            "engine",
            "--source",
            "src",
            "--medium-type",
            "codebase",
            "--intent",
            "model the engine",
            "--name",
            "inroot",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).expect("--json init must emit JSON");
    assert_eq!(
        env["warnings"],
        serde_json::json!([]),
        "in-root pointer must produce zero hint noise"
    );
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

    // The golden lives with the v1 format schemas under docs/ (repo-root-relative
    // to the cli crate: two levels up to `public/`, then the schemas tree).
    let golden_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/schemas/memstead-plugin/v1/examples/binding.from-init.json");
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
    // `../docs` is outside the workspace root — the consequence-naming
    // warning fires here too (filesystem medium) — and the fixture's tree
    // does not exist, so the absent-source note joins it.
    let warnings = env["warnings"].as_array().expect("warnings array");
    assert_eq!(warnings.len(), 2, "got: {warnings:?}");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("which does not exist")),
        "the absent source must be named: {warnings:?}",
    );
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .contains("outside the workspace root"),
        "got: {warnings:?}"
    );
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
                && w.as_str().unwrap_or("").contains("no change signal")),
        "expected a deferral warning, got {warnings:?}"
    );

    // On disk: build-only binding.
    let bytes = std::fs::read(root.join(".memstead/projections/research/docs.json")).unwrap();
    let b: Binding = serde_json::from_slice(&bytes).unwrap();
    assert!(b.operations.sync.is_none());
    assert!(b.operations.verify.is_none());
}

/// Re-running `init` on an existing binding id refuses `PROJECTION_EXISTS`
/// (exit 5) and touches nothing — the record is byte-identical after the
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
    let before = scaffold_bytes(root, "engine", "graph");

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

    // No partial writes: the record is byte-identical.
    let after = scaffold_bytes(root, "engine", "graph");
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

/// Enabling `sync` on a codebase binding that lacked it adds the block (with
/// sensible defaults) and round-trips; every other field is untouched, and
/// `verify` stays absent.
#[test]
fn enable_sync_adds_block_to_codebase_binding() {
    let tmp = build_only_workspace();
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
    assert_eq!(after.sources, before.sources);
    assert_eq!(after.reference_mems, before.reference_mems);
    assert_eq!(after.destination_mem, before.destination_mem);
    assert_eq!(after.deny_paths, before.deny_paths);
    assert_eq!(after.coverage_semantics, before.coverage_semantics);
    assert_eq!(after.rules, before.rules);
    assert_eq!(after.operations.build, before.operations.build);

    // Round-trips losslessly.
    let json = serde_json::to_string(&after).unwrap();
    let back: Binding = serde_json::from_str(&json).unwrap();
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
    let tmp = build_only_workspace();
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
// projection advance
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
/// into the mem config (not via `mem set-sync-state`) so the test
/// needs no mem-repo workspace.
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
        "projections/engine/graph.json",
        r#"{"version":2,"intent":"model the engine","sources":[{"name":"source-tree","type":"codebase","pointer":"src","change_detection":"git","scope":[{"path":"**/*.rs","mode":"allow"}]}],"reference_mems":[],"destination_mem":"engine","deny_paths":[],"coverage_semantics":"exhaustive","operations":{"build":{"mode":"discovery","trigger":"loop","batch_size":20},"sync":{"trigger":"manual","batch_size":20}}}"#,
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

/// A medium-relative artifact id (`a.rs` where the slice printed `src/a.rs`)
/// refuses with the corrected workspace-relative id in the message AND the
/// `corrected_artifacts` details map — and the dialect never widens: the
/// medium-relative form is refused, never accepted.
#[test]
fn advance_medium_relative_id_refuses_with_corrected_id() {
    let tmp = advance_workspace();
    let root = tmp.path();

    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "advance",
            "engine/graph",
            "--dispositions",
            r#"{"a.rs": "worked"}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], "PROJECTION_ADVANCE_UNKNOWN_ARTIFACT");
    let message = env["message"].as_str().unwrap();
    assert!(
        message.contains("workspace-relative"),
        "message names the expected dialect: {message}"
    );
    assert!(
        message.contains("`a.rs` → `src/a.rs`"),
        "message carries the concrete corrected id: {message}"
    );
    assert_eq!(
        env["details"]["corrected_artifacts"]["a.rs"], "src/a.rs",
        "the remedy is machine-readable in details"
    );
    // Nothing was written — the refused medium-relative id was not accepted
    // in any form (the gate did not widen).
    assert!(
        !root
            .join(".memstead/state/advance/engine/graph.json")
            .exists()
    );
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

// ── brief ───────────────────────────────────────────────────────────────

/// `projection brief <mem>/<stem>` renders a binding's discovery run-brief,
/// headed by the canonical binding id (D3/D9). Scaffold a binding with
/// `projection init`, then render it.
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

/// Re-expressed against the pointer
/// channel: the ACTIVE-BINDING pointer derives only from CONSUMING renders.
/// A peek-only brief (any named render — the `--consume` flag requires
/// `--all`, so named briefs are peeks by construction) leaves every cache
/// byte-identical and never points enforcement at the peeked binding; a
/// consuming `--all --consume` render publishes the picked binding's id.
#[test]
fn active_binding_pointer_derives_only_from_consuming_renders() {
    fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push((p.display().to_string(), std::fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    for name in ["alpha", "beta"] {
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
                name,
            ])
            .assert()
            .success();
    }
    let cache_file = ws
        .join(".memstead.cache")
        .join("projection")
        .join("active-binding.json");

    // Peek binding beta: a pure read — the pointer is not created.
    memstead()
        .current_dir(&ws)
        .args(["projection", "brief", "ws/beta"])
        .assert()
        .success();
    assert!(
        !cache_file.exists(),
        "a peek-only render must not create the active-binding pointer"
    );

    // Repeated peeks are byte-idempotent on ALL caches.
    let before = snapshot(&ws.join(".memstead.cache"));
    memstead()
        .current_dir(&ws)
        .args(["projection", "brief", "ws/beta"])
        .assert()
        .success();
    assert_eq!(
        before,
        snapshot(&ws.join(".memstead.cache")),
        "repeated peeks must leave every cache byte-identical"
    );

    // A consuming rotation render publishes the PICKED binding's id.
    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "projection", "brief", "--all", "--consume"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let payload: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let brief = payload["brief"].as_str().expect("rotation renders a brief");
    let cache: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&cache_file)
            .expect("a consuming render must publish the active-binding pointer"),
    )
    .unwrap();
    let guarded = cache["binding"].as_str().unwrap();
    let stem = guarded.split_once('/').map(|(_, s)| s).unwrap_or(guarded);
    assert!(
        brief.contains(stem),
        "enforcement must target the binding whose brief was consumed: pointer names \
         `{guarded}`, brief:\n{brief}"
    );
}

/// `projection check-path`: single and `--batch` stdin forms answer deny
/// verdicts against a named binding, naming the matched deny entry on a
/// block; the directory-prefix rule and `..`-escaping candidates produce the
/// verdicts the retired hook dialect produced.
#[test]
fn check_path_answers_single_and_batch() {
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
            "alpha",
        ])
        .assert()
        .success();
    // The author adds denies to the scaffolded record: a subtree and a
    // workspace-escaping entry (this project's own cross-medium dialect).
    let record_path = ws
        .join(".memstead")
        .join("projections")
        .join("ws")
        .join("alpha.json");
    let mut record: Value = serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    let denies = record["deny_paths"].as_array_mut().unwrap();
    denies.push(Value::String("dev/**".into()));
    denies.push(Value::String("../CLAUDE.md".into()));
    std::fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();

    // Single form: a denied path names the matched entry.
    let out = memstead()
        .current_dir(&ws)
        .args([
            "--json",
            "projection",
            "check-path",
            "--binding",
            "ws/alpha",
            "dev/notes/a.md",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["binding"], "ws/alpha");
    assert_eq!(v["results"][0]["denied"], true);
    assert_eq!(v["results"][0]["matched"], "dev/**");

    // Batch form: one process answers every candidate — the directory-
    // targeted read (`dev` itself), a `..`-escaping sibling candidate, a
    // recursing Glob pattern, and an allowed path.
    let batch = serde_json::json!({
        "cwd": ws.to_str().unwrap(),
        "paths": [
            "dev",
            tmp.path().join("CLAUDE.md").to_str().unwrap(),
            "dev/**/*.md",
            "src/lib.rs",
        ],
    });
    let out = memstead()
        .current_dir(&ws)
        .args([
            "--json",
            "projection",
            "check-path",
            "--binding",
            "ws/alpha",
            "--batch",
        ])
        .write_stdin(batch.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    let results = v["results"].as_array().unwrap();
    assert_eq!(results.len(), 4);
    assert_eq!(
        results[0]["denied"], true,
        "a read of the denied directory itself is blocked (prefix rule)"
    );
    assert_eq!(
        results[1]["denied"], true,
        "a `..`-escaping candidate matches a `../` deny entry"
    );
    assert_eq!(results[1]["matched"], "../CLAUDE.md");
    assert_eq!(
        results[2]["denied"], true,
        "a Glob pattern recursing the denied subtree is blocked"
    );
    assert_eq!(results[3]["denied"], false);
    assert_eq!(results[3]["matched"], Value::Null);
}

/// `projection check-path` refusals are typed, never "allowed" by omission:
/// an unknown binding, a quarantined binding, a missing active pointer, and
/// a malformed batch each refuse with their own code.
#[test]
fn check_path_refuses_typed() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();

    // Unknown binding → PROJECTION_NOT_FOUND (exit 3).
    memstead()
        .current_dir(&ws)
        .args([
            "projection",
            "check-path",
            "--binding",
            "ws/ghost",
            "dev/a.md",
        ])
        .assert()
        .failure()
        .code(3)
        .stderr(predicates::str::contains("PROJECTION_NOT_FOUND"));

    // No --binding and no active pointer → NO_ACTIVE_BINDING (exit 3).
    memstead()
        .current_dir(&ws)
        .args(["projection", "check-path", "dev/a.md"])
        .assert()
        .failure()
        .code(3)
        .stderr(predicates::str::contains("NO_ACTIVE_BINDING"));

    // A quarantined binding refuses with its typed reason, never answering
    // "allowed": a version-gate-failing record file lands in quarantine.
    write_store(
        &ws,
        "projections/ws/legacy.json",
        r#"{ "version": 1, "destination_mem": "ws" }"#,
    );
    memstead()
        .current_dir(&ws)
        .args([
            "projection",
            "check-path",
            "--binding",
            "ws/legacy",
            "dev/a.md",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("PROJECTION_QUARANTINED"));

    // A malformed batch refuses whole (INVALID_INPUT, exit 5) — never a
    // part-answer.
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
            "alpha",
        ])
        .assert()
        .success();
    for bad in ["not json", r#"{"cwd": "/x"}"#, r#"{"paths": ["ok", 42]}"#] {
        memstead()
            .current_dir(&ws)
            .args([
                "projection",
                "check-path",
                "--binding",
                "ws/alpha",
                "--batch",
            ])
            .write_stdin(bad)
            .assert()
            .failure()
            .code(5)
            .stderr(predicates::str::contains("INVALID_INPUT"));
    }
}

/// With `--binding` omitted, `check-path` answers against the ACTIVE binding
/// — the one the last consuming render published — so enforcement follows
/// the loop with no list ever cached: after the pointer moves, the previous
/// binding's denies are no longer enforced (the stale-enforcement regression,
/// re-expressed at the new seam).
#[test]
fn check_path_follows_the_active_binding() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["mem-repo", "init", ws.to_str().unwrap(), "--no-gitignore"])
        .assert()
        .success();
    for name in ["alpha", "beta"] {
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
                name,
            ])
            .assert()
            .success();
    }
    // Alpha denies `secrets/**` on top of the scaffold defaults.
    let record_path = ws
        .join(".memstead")
        .join("projections")
        .join("ws")
        .join("alpha.json");
    let mut record: Value = serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    record["deny_paths"]
        .as_array_mut()
        .unwrap()
        .push(Value::String("secrets/**".into()));
    std::fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();

    let pointer = ws
        .join(".memstead.cache")
        .join("projection")
        .join("active-binding.json");
    std::fs::create_dir_all(pointer.parent().unwrap()).unwrap();

    let check = |expect_denied: bool| {
        let out = memstead()
            .current_dir(&ws)
            .args(["--json", "projection", "check-path", "secrets/key.txt"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["results"][0]["denied"], expect_denied);
    };

    std::fs::write(&pointer, r#"{"binding":"ws/alpha"}"#).unwrap();
    check(true);
    // The loop moves to beta: alpha's extra deny must not survive, because
    // nothing of alpha's list is cached anywhere — only the pointer moved.
    std::fs::write(&pointer, r#"{"binding":"ws/beta"}"#).unwrap();
    check(false);
}

/// `projection brief --all` on a workspace with NO bindings configured reports
/// a distinct `no_bindings` outcome (exit 0) — not the all-backing-off
/// `skipped` outcome, which would otherwise collapse into the same `None`. A
/// caller (the plugin's setup ramp, a status display) branches on this to
/// prompt first-time setup rather than retry a no-op pass.
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

/// `projection brief <binding> --verify` renders the verify brief:
/// measurement + capped-adjudication instructions only, with the explicit
/// no-mutation refusal and NO repair block. Read-only on the mem.
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
    // Reworded 2026-08-20 with the engine-side assertion in
    // `ingest::brief::tests`: the brief used to claim verify writes
    // "**nothing**", which is false — a completed run records findings,
    // backfills anchor hashes and writes a `#verified` baseline. The refusal
    // being pinned is about ENTITY CONTENT, so the claim is narrowed to what
    // holds rather than dropped.
    assert!(
        brief.contains("Verify writes **no entity content**"),
        "C1 refusal present; got:\n{brief}"
    );
    // C1/C2 refusal: the verify brief carries NO repair block.
    assert!(
        !brief.contains("## How to repair"),
        "verify brief must not carry repair instructions; got:\n{brief}"
    );
    assert!(!brief.contains("## Open findings to repair"));
}

/// `projection brief <binding> --sync` renders the sync brief: the
/// sole-maintenance-writer prompt with the absorbed reconcile conservatism. A
/// fresh mem (no anchors, never synced) triggers the adopt / first-sync framing.
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
    // Fresh mem → adopt / first-sync framing (the brief half).
    assert!(
        brief.contains("## First sync — adopting `ws`"),
        "fresh mem gets adopt framing; got:\n{brief}"
    );
    // Absorbed reconcile conservatism (C3).
    assert!(brief.contains("## How to repair — be conservative"));
    assert!(brief.contains("A dropped dependency FLAGS, it does not auto-remove."));
    assert!(brief.contains("`[commit <hash>]` log-style entries"));
}

/// `projection brief <binding> --sync` against a binding whose sync operation
/// is not enabled refuses typed with the enable remedy in details — a loop
/// must not spend a work slot rendering a brief the engine will refuse to
/// apply (an earlier plan, decision 13). Complement: `projection
/// enable sync` makes the identical call succeed.
#[test]
fn brief_sync_refuses_sync_disabled_binding_with_remedy() {
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

    // Strip the scaffolded sync block — the store is operator-editable JSON;
    // direct edits take effect on the next load (documented store contract).
    let record_path = ws.join(".memstead/projections/ws/code.json");
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    record["operations"].as_object_mut().unwrap().remove("sync");
    std::fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "projection", "brief", "ws/code", "--sync"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let envelope: serde_json::Value =
        serde_json::from_slice(&out).expect("JSON envelope on stdout");
    assert_eq!(
        envelope["code"], "PROJECTION_SYNC_NOT_ENABLED",
        "got: {envelope}"
    );
    assert_eq!(
        envelope["details"]["remedy"]["cli"], "memstead projection enable sync ws/code",
        "the one-command remedy rides details: {envelope}"
    );

    // Complement: running the named remedy makes the same call succeed.
    memstead()
        .current_dir(&ws)
        .args(["projection", "enable", "sync", "ws/code"])
        .assert()
        .success();
    memstead()
        .current_dir(&ws)
        .args(["projection", "brief", "ws/code", "--sync"])
        .assert()
        .success();
}

/// `projection brief --verify` / `--sync` without a binding id refuses with a
/// typed `PROJECTION_BRIEF_BINDING_REQUIRED` — they render one binding, never an
/// `--all` rotation.
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
/// leak. No engine is built before the check.
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

/// The absent-destination remedy must work in the workspace the reader is
/// standing in. `memstead mem init` is mem-repo-only, so in the
/// filesystem-mem shape `memstead quickstart` produces it refuses — the
/// brief there must name the repointing fix instead. The mem-repo variant
/// of this test cannot catch that, which is why this one exists.
#[test]
fn brief_absent_destination_remedy_suits_the_workspace_shape() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("app");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/a.rs"), b"pub fn a() {}\n").unwrap();
    memstead()
        .current_dir(&repo)
        .args(["quickstart", "--repo", ".", "--agent", "claude-code"])
        .assert()
        .success();

    // Repoint the binding at a mem that is not there.
    let record = repo.join(".memstead/projections/app/app.json");
    let mut binding: Value = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    binding["destination_mem"] = Value::String("ghost".into());
    std::fs::write(&record, serde_json::to_vec_pretty(&binding).unwrap()).unwrap();

    let out = String::from_utf8(
        memstead()
            .current_dir(&repo)
            .args(["projection", "brief", "app/app"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        out.contains("This mem does not exist in this workspace yet"),
        "the absence must be named:\n{out}",
    );
    assert!(
        !out.contains("memstead mem init"),
        "`mem init` refuses in a filesystem-mem workspace — the brief must not \
         name it here:\n{out}",
    );
    // The only assertion that matters: FOLLOW the remedy, verbatim, and the
    // brief's own mandate must then succeed. Two earlier versions of this
    // message were each defensible as prose and each left the reader stuck
    // — one naming a command that refuses in this shape, one naming a field
    // whose edit does not move the record, so every anchored write still
    // failed INVALID_ANCHOR.
    let remedy = out
        .lines()
        .find(|l| l.contains("does not exist in this workspace yet"))
        .expect("the remedy line");
    let commands: Vec<&str> = remedy
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|c| c.starts_with("rm ") || c.starts_with("memstead "))
        .collect();
    assert!(
        commands.len() >= 2,
        "the remedy must carry runnable commands, got {commands:?} from: {remedy}",
    );
    for command in &commands {
        let run = std::process::Command::new("sh")
            .arg("-c")
            .arg(command.replace("memstead ", &format!("{} ", memstead_bin().display())))
            .current_dir(&repo)
            .output()
            .expect("shell runs");
        assert!(
            run.status.success(),
            "the remedy's command must run: {command}\n{}",
            String::from_utf8_lossy(&run.stderr),
        );
    }

    // …and now the anchored write the brief mandates lands.
    memstead()
        .current_dir(&repo)
        .args([
            "create",
            "--type",
            "concept",
            "--title",
            "After Remedy",
            "--section",
            "definition=d",
            "--section",
            "explanation=e",
            "--anchor",
            r#"{"artifact":"src/a.rs","grain":"file","class":"anchored","source":"app"}"#,
        ])
        .assert()
        .success();
}

/// A source pointer that resolves to nothing on disk is named as such.
/// The brief tells the agent to read from it; an absent tree is
/// indistinguishable from an empty one unless the brief says so.
#[test]
fn brief_names_a_source_that_does_not_resolve() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("app2");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/a.rs"), b"pub fn a() {}\n").unwrap();
    memstead()
        .current_dir(&repo)
        .args(["quickstart", "--repo", ".", "--agent", "claude-code"])
        .assert()
        .success();

    let record = repo.join(".memstead/projections/app2/app2.json");
    let mut binding: Value = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    binding["sources"][0]["pointer"] = Value::String("no-such-tree".into());
    std::fs::write(&record, serde_json::to_vec_pretty(&binding).unwrap()).unwrap();

    let out = String::from_utf8(
        memstead()
            .current_dir(&repo)
            .args(["projection", "brief", "app2/app2"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        out.contains("`no-such-tree`") && out.contains("does not resolve to anything on disk"),
        "the brief must print the pointer and name its absence:\n{out}",
    );
}

/// The brief says a wrong `source` name "usually refuses" because the path
/// no longer joins — and that it is NOT refused when the path resolves
/// workspace-relative anyway. Both halves, so the sentence cannot drift
/// back into promising a gate that the legacy tolerance deliberately does
/// not provide.
#[test]
fn an_undeclared_anchor_source_refuses_on_the_path_but_is_tolerated_when_it_resolves() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("app3");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/a.rs"), b"pub fn a() {}\n").unwrap();
    memstead()
        .current_dir(&repo)
        .args(["quickstart", "--repo", ".", "--agent", "claude-code"])
        .assert()
        .success();

    // Source-relative path + undeclared name: the join fails, so it refuses.
    let out = memstead()
        .current_dir(&repo)
        .args([
            "--json", "create", "--type", "concept", "--title", "Probe",
            "--section", "definition=d", "--section", "explanation=e",
            "--anchor",
            r#"{"artifact":"nowhere/a.rs","grain":"file","class":"anchored","source":"not-declared"}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).expect("refusal must emit JSON");
    assert_eq!(env["code"], "INVALID_ANCHOR");

    // …but a path that resolves workspace-relative is written even with an
    // undeclared name. This is the documented legacy tolerance, not an
    // oversight — a brief that promised a refusal here would be wrong.
    memstead()
        .current_dir(&repo)
        .args([
            "create",
            "--type",
            "concept",
            "--title",
            "Tolerated",
            "--section",
            "definition=d",
            "--section",
            "explanation=e",
            "--anchor",
            r#"{"artifact":"src/a.rs","grain":"file","class":"anchored","source":"not-declared"}"#,
        ])
        .assert()
        .success();
}

/// A binding scaffolded before its mem exists still renders — `projection
/// init` deliberately allows that order — but the brief SAYS the destination
/// is not there and names the command that creates it. The agent's mandate is
/// to mutate that mem; discovering its absence on the first create means the
/// surface that sent it said something untrue.
///
/// Fixture needs a mem-repo workspace (`mem-repo init`).
#[test]
fn brief_names_a_destination_mem_that_does_not_exist_yet() {
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
            "absent-mem",
            "--source",
            "../src",
            "--medium-type",
            "codebase",
            "--name",
            "code",
        ])
        .assert()
        .success();

    let out = String::from_utf8(
        memstead()
            .current_dir(&ws)
            .args(["projection", "brief", "absent-mem/code"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        out.contains("This mem does not exist in this workspace yet"),
        "the brief must name the absent destination:\n{out}",
    );
    assert!(
        !out.contains("<name@version>"),
        "the remedy must name a concrete pin, not a placeholder the reader has \
         to fetch vocabulary for:\n{out}",
    );

    // Follow the remedy verbatim — the sibling filesystem-shape test earned
    // this method three times over. Wording assertions passed while every
    // wrong version of this message shipped; running it is what caught them.
    let remedy = out
        .lines()
        .find(|l| l.contains("does not exist in this workspace yet"))
        .expect("the remedy line");
    let spans: Vec<&str> = remedy
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|c| c.starts_with("memstead "))
        .collect();
    // The sentence names the bare verb (`memstead mem init`) to say it
    // refuses on its own, then gives the full invocation. A mention that is a
    // proper prefix of an actual command is not a command — running it would
    // fail on missing arguments and prove nothing about the remedy.
    let commands: Vec<&str> = spans
        .iter()
        .copied()
        .filter(|c| {
            !spans
                .iter()
                .any(|o| o != c && o.starts_with(&format!("{c} ")))
        })
        .collect();
    assert!(
        !commands.is_empty(),
        "the remedy must carry runnable commands, got {spans:?} from: {remedy}",
    );
    for command in &commands {
        let run = std::process::Command::new("sh")
            .arg("-c")
            .arg(command.replace("memstead ", &format!("{} ", memstead_bin().display())))
            .current_dir(&ws)
            .output()
            .expect("shell runs");
        assert!(
            run.status.success(),
            "the remedy's command must run: {command}\n{}",
            String::from_utf8_lossy(&run.stderr),
        );
    }

    // …and the mem the brief said was missing is now there, described as
    // present by the same block that reported it absent.
    let after = String::from_utf8(
        memstead()
            .current_dir(&ws)
            .args(["projection", "brief", "absent-mem/code"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        !after.contains("This mem does not exist in this workspace yet"),
        "after the remedy the destination must be present:\n{after}",
    );
    assert!(
        after.contains("absent-mem") && after.contains("schema:"),
        "the Destination block must describe the created mem:\n{after}",
    );
}

/// A refusal names `projection enable <op>` as the remedy. Over a medium
/// whose capability row cannot carry the operation, that remedy refuses too
/// — so the reader is bounced from run-time refusal to enable to capability
/// gap with nothing they can do. The gap must be the answer at the first
/// refusal, and the remedy must survive where it IS honest. Both halves, run.
#[test]
fn absent_sync_names_the_enable_remedy_only_where_the_medium_can_carry_it() {
    let tmp = TempDir::new().unwrap();

    // (a) a web source: sync is out of scope, so no remedy may be offered.
    let web = tmp.path().join("web-ws");
    std::fs::create_dir_all(&web).unwrap();
    memstead()
        .current_dir(&web)
        .args(["quickstart", "--name", "web-check"])
        .assert()
        .success();
    memstead()
        .current_dir(&web)
        .args([
            "projection",
            "init",
            "--mem",
            "web-check",
            "--source",
            "https://example.com/docs",
            "--medium-type",
            "web",
        ])
        .assert()
        .success();
    let out = memstead()
        .current_dir(&web)
        .args(["--json", "projection", "brief", "web-check/docs", "--sync"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).expect("refusal must emit JSON");
    assert_eq!(env["code"], "PROJECTION_CAPABILITY_UNSUPPORTED");
    let message = env["message"].as_str().unwrap_or_default();
    assert!(
        !message.contains("projection enable"),
        "a remedy that would itself refuse must not be offered:\n{message}",
    );
    assert!(
        message.contains("cannot carry one"),
        "the capability gap must be named:\n{message}",
    );

    // (b) a codebase source with its sync block stripped: the remedy is real,
    // and running it verbatim makes the same brief render.
    let repo = tmp.path().join("code-ws");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/a.rs"), b"pub fn a() {}\n").unwrap();
    memstead()
        .current_dir(&repo)
        .args(["quickstart", "--repo", ".", "--agent", "claude-code"])
        .assert()
        .success();
    let record = repo.join(".memstead/projections/code-ws/code-ws.json");
    let mut binding: Value = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    binding["operations"]
        .as_object_mut()
        .unwrap()
        .remove("sync");
    std::fs::write(&record, serde_json::to_vec_pretty(&binding).unwrap()).unwrap();

    let out = memstead()
        .current_dir(&repo)
        .args(["--json", "projection", "brief", "code-ws/code-ws", "--sync"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).expect("refusal must emit JSON");
    assert_eq!(env["code"], "PROJECTION_SYNC_NOT_ENABLED");
    let remedy = env["details"]["remedy"]["cli"]
        .as_str()
        .expect("the remedy command")
        .to_string();
    let run = std::process::Command::new("sh")
        .arg("-c")
        .arg(remedy.replace("memstead ", &format!("{} ", memstead_bin().display())))
        .current_dir(&repo)
        .output()
        .expect("shell runs");
    assert!(
        run.status.success(),
        "the remedy must run: {remedy}\n{}",
        String::from_utf8_lossy(&run.stderr),
    );
    memstead()
        .current_dir(&repo)
        .args(["projection", "brief", "code-ws/code-ws", "--sync"])
        .assert()
        .success();
}

// ── AC4: absent-operation-block refusal + `projection enable` remedy ─────────

/// D6/AC4: `projection brief` on a binding with **no build block** refuses with
/// the `projection enable build <binding>` remedy, and that command — run
/// verbatim — makes the same brief succeed.
#[test]
fn brief_refuses_absent_build_then_enable_build_remedy_succeeds() {
    let tmp = advance_workspace();
    let root = tmp.path();
    // Strip the build block — a verify-only binding (verify has no mutating
    // operation to gate, so an absent block is never a refusal and this is a
    // legal build-less shape).
    write_store(
        root,
        "projections/engine/graph.json",
        r#"{"version":2,"intent":"model the engine","sources":[{"name":"source-tree","type":"codebase","pointer":"src","change_detection":"git","scope":[{"path":"**/*.rs","mode":"allow"}]}],"reference_mems":[],"destination_mem":"engine","deny_paths":[],"coverage_semantics":"exhaustive","operations":{"verify":{"trigger":"manual","batch_size":20}}}"#,
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
        r#"{"version":2,"intent":"model the engine","sources":[{"name":"source-tree","type":"codebase","pointer":"src","change_detection":"git","scope":[{"path":"**/*.rs","mode":"allow"}]}],"reference_mems":[],"destination_mem":"engine","deny_paths":[],"coverage_semantics":"exhaustive","operations":{"build":{"mode":"discovery","trigger":"loop","batch_size":20}}}"#,
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
    // The binding is build-only (no verify). Its brief renders.
    memstead()
        .current_dir(tmp.path())
        .args(["projection", "brief", "engine/graph"])
        .assert()
        .success();
}

// ── `brief --all --operation` (operation-aware rotation) ────────────────────

/// A mem-repo workspace with one scaffolded binding `ws/code` over a real
/// sibling `src/` dir (init defaults: build `trigger: loop`, sync + verify
/// `trigger: manual`). Returns the TempDir and the workspace path.
fn operation_workspace() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.rs"), "x").unwrap();
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
    (tmp, ws)
}

/// Rewrite one operation block's `trigger` on the scaffolded `ws/code` binding.
fn set_trigger(ws: &Path, op: &str, trigger: &str) {
    let path = ws.join(".memstead/projections/ws/code.json");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["operations"][op]["trigger"] = Value::String(trigger.to_string());
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
}

/// `brief --all` without `--operation` keeps the classic build rotation
/// (back-compat for the ingest router) and the JSON output gains the additive
/// `operation` field next to `brief` — explicit `--operation build` behaves
/// identically.
#[test]
fn brief_all_defaults_to_build_and_names_the_operation() {
    let (_tmp, ws) = operation_workspace();

    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "projection", "brief", "--all"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["operation"], "build", "additive operation field: {env}");
    let brief = env["brief"].as_str().expect("brief must stay a string");
    assert!(
        brief.contains("## Situation"),
        "default rotation renders the build brief; got:\n{brief}"
    );

    // Explicit `--operation build` — same rotation, same brief shape.
    let out = memstead()
        .current_dir(&ws)
        .args([
            "--json",
            "projection",
            "brief",
            "--all",
            "--operation",
            "build",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["operation"], "build");
    assert!(env["brief"].as_str().unwrap().contains("## Situation"));
}

/// `--operation any` honours the per-operation eligibility gate (`trigger:
/// loop` in the declaration): with build flipped to manual and verify to loop,
/// the rotation selects the verify pair and dispatches to the verify renderer,
/// naming the operation in the JSON output.
#[test]
fn brief_all_any_dispatches_to_the_loop_declared_operation() {
    let (_tmp, ws) = operation_workspace();
    set_trigger(&ws, "build", "manual");
    set_trigger(&ws, "verify", "loop");

    let out = memstead()
        .current_dir(&ws)
        .args([
            "--json",
            "projection",
            "brief",
            "--all",
            "--operation",
            "any",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["operation"], "verify",
        "manual build is ineligible; loop verify is due (never verified): {env}"
    );
    assert!(
        env["brief"]
            .as_str()
            .unwrap()
            .contains("## Verify — measure fidelity, do not mutate"),
        "the verify renderer produced the brief: {env}"
    );
}

/// A loop-declared sync pair with an unmoved source and no open findings is
/// not due — the rotation yields the quiet `skipped` outcome, not a brief.
#[test]
fn brief_all_sync_yields_quietly_when_nothing_due() {
    let (_tmp, ws) = operation_workspace();
    set_trigger(&ws, "sync", "loop");

    let out = memstead()
        .current_dir(&ws)
        .args([
            "--json",
            "projection",
            "brief",
            "--all",
            "--operation",
            "sync",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["skipped"],
        Value::Bool(true),
        "never-synced + no findings → sync is not due: {env}"
    );
}

/// `--operation` binds to the `--all` rotation: without `--all` it is a usage
/// error, and it conflicts with the single-binding `--sync` / `--verify` modes.
#[test]
fn brief_operation_flag_requires_all_and_conflicts_with_group_c() {
    let (_tmp, ws) = operation_workspace();

    // Named binding + --operation, no --all → clap usage error.
    memstead()
        .current_dir(&ws)
        .args(["projection", "brief", "ws/code", "--operation", "any"])
        .assert()
        .failure();

    // --operation conflicts with --sync / --verify.
    for flag in ["--sync", "--verify"] {
        memstead()
            .current_dir(&ws)
            .args(["projection", "brief", "--all", "--operation", "any", flag])
            .assert()
            .failure();
    }
}

/// A plain `--all` render is a pure read: it mints no scheduler state and
/// repeats byte-identically; `--consume` is the act that takes the rotation
/// slot. The JSON envelope also discloses the (binding, op) pairs the filter
/// admits but the bindings never loop-declare (`not_rotated`).
#[test]
fn brief_all_is_pure_without_consume_and_advances_with_it() {
    let (tmp, ws) = operation_workspace();
    // A second build-loop binding so the rotation has two pairs to move
    // between: `ws/code#build` < `ws/code2#build`.
    let src2 = tmp.path().join("src2");
    std::fs::create_dir_all(&src2).unwrap();
    std::fs::write(src2.join("b.rs"), "y").unwrap();
    memstead()
        .current_dir(&ws)
        .args([
            "projection",
            "init",
            "--mem",
            "ws",
            "--source",
            "../src2",
            "--medium-type",
            "codebase",
            "--name",
            "code2",
        ])
        .assert()
        .success();

    let cursor_path = ws.join(".memstead.cache/ingest/ingest-cursor.json");
    let render = |consume: bool| -> Value {
        let mut args = vec![
            "--json",
            "projection",
            "brief",
            "--all",
            "--operation",
            "any",
        ];
        if consume {
            args.push("--consume");
        }
        let out = memstead()
            .current_dir(&ws)
            .args(&args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&out).unwrap()
    };

    // Two peeks: a brief renders, but no scheduler state appears.
    for _ in 0..2 {
        let env = render(false);
        assert_eq!(env["operation"], "build", "peek renders a brief: {env}");
        assert!(
            !cursor_path.exists(),
            "a plain --all render must not mint the rotation cursor"
        );
        // Sync and verify stay manual on both bindings — the filter admits
        // them, the declarations don't, and the envelope says so.
        let not_rotated = env["not_rotated"].as_array().expect("not_rotated array");
        assert_eq!(not_rotated.len(), 4, "2 bindings x (sync, verify): {env}");
    }

    // Consume: the slot is taken, the cursor lands on the first pair.
    render(true);
    let cursor: Value = serde_json::from_slice(&std::fs::read(&cursor_path).unwrap()).unwrap();
    assert_eq!(cursor["last"], "ws/code#build");

    // A peek between consumes leaves the cursor untouched...
    let bytes_before = std::fs::read(&cursor_path).unwrap();
    render(false);
    assert_eq!(
        std::fs::read(&cursor_path).unwrap(),
        bytes_before,
        "peek left the cursor byte-identical"
    );

    // ...and the next consume advances to the pair the peek would have shown.
    render(true);
    let cursor: Value = serde_json::from_slice(&std::fs::read(&cursor_path).unwrap()).unwrap();
    assert_eq!(cursor["last"], "ws/code2#build");
}

/// `--consume` binds to the `--all` rotation: on a named-binding render it is
/// a usage error (a single-binding brief has no rotation slot to take).
#[test]
fn brief_consume_requires_all() {
    let (_tmp, ws) = operation_workspace();
    memstead()
        .current_dir(&ws)
        .args(["projection", "brief", "ws/code", "--consume"])
        .assert()
        .failure();
}

// ── verify: prepared-hash backfill + deterministic drift ─────────────────────

/// `advance_workspace` plus a verify operation on the binding and an anchors
/// sidecar carrying one HASH-LESS `anchored` anchor on `src/a.rs` — the
/// fixture for the verify command's backfill/adjudication legs.
fn verify_workspace() -> TempDir {
    let tmp = advance_workspace();
    let root = tmp.path();
    write_store(
        root,
        "projections/engine/graph.json",
        r#"{"version":2,"intent":"model the engine","sources":[{"name":"source-tree","type":"codebase","pointer":"src","change_detection":"git","scope":[{"path":"**/*.rs","mode":"allow"}]}],"reference_mems":[],"destination_mem":"engine","deny_paths":[],"coverage_semantics":"exhaustive","operations":{"build":{"mode":"discovery","trigger":"loop","batch_size":20},"sync":{"trigger":"manual","batch_size":20},"verify":{"trigger":"manual","batch_size":20,"adjudication_cap":50,"full_resync_every":20}}}"#,
    );
    // The entity the sidecar row is keyed to. A row whose entity the mem does
    // not hold is DANGLING (consistency-sweep 03/02) and leaves the population
    // before any figure counts it, so a fixture without the entity would be
    // measuring the wrong thing.
    std::fs::write(
        root.join("engine-mem").join("covers-a.md"),
        "---\ntype: decision\n---\n\n# Covers A\n\n## Decision\n\nBody.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("engine-mem").join(".memstead").join("anchors.json"),
        r#"{"version":1,"entities":{"engine--covers-a":[{"artifact":"src/a.rs","grain":"file","class":"anchored","hash_stability":"stable"}]}}"#,
    )
    .unwrap();
    tmp
}

/// End-to-end through the CLI (separate processes): the first `projection
/// verify` backfills the hash-less anchor's prepared-content hash into the
/// sidecar (`hash_backfilled: 1`); a re-run backfills nothing (idempotent);
/// after a source change a verify adjudicates `drifted` deterministically —
/// no queued deferral, no LLM leg.
///
/// Deliberately a BARE verify throughout (C6, 2026-09-03): `--advance` gates
/// the `#verified` freshness token only. The backfill is measurement
/// machinery, and this test is the evidence for that split — gating it too
/// was tried and reverted when the anchor stayed in `recheck` and the third
/// leg's drift never appeared.
#[test]
fn verify_backfills_hashless_anchor_then_adjudicates_drift() {
    let tmp = verify_workspace();
    let root = tmp.path();

    // (1) First verify: the hash-less anchored anchor gains its prepared hash.
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["hash_backfilled"], 1,
        "one hash-less anchor backfilled: {env}"
    );
    assert_eq!(env["backlog"], 0, "backfill queues nothing: {env}");
    let sidecar = std::fs::read_to_string(root.join("engine-mem/.memstead/anchors.json")).unwrap();
    assert!(
        sidecar.contains("\"hash\""),
        "the sidecar now records the prepared-content hash: {sidecar}"
    );

    // (2) Idempotent: a second verify observes an empty worklist.
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["hash_backfilled"], 0, "backfill happens once: {env}");
    assert_eq!(
        env["report"]["anchors"]["resolves"], 1,
        "the recorded hash matches the source — the anchor resolves: {env}"
    );

    // (3) The anchored artifact changes; verify adjudicates drift
    //     deterministically from the hash comparison alone.
    let src = root.join("src");
    std::fs::write(src.join("a.rs"), "one-drifted").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-qm", "drift"]);

    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["hash_backfilled"], 0,
        "a recorded hash is never overwritten: {env}"
    );
    assert_eq!(
        env["report"]["anchors"]["drifted"], 1,
        "stable-medium hash mismatch → deterministic drifted: {env}"
    );
    assert_eq!(
        env["report"]["findings_by_class"]["drifted"], 1,
        "the drift lands as a durable finding: {env}"
    );
    assert_eq!(
        env["backlog"], 0,
        "nothing queued — the hash leg needs no sampling: {env}"
    );
}

/// C9 AC1: a quarantined DESTINATION mem is refused by both verify and
/// brief, and the run leaves all three stores byte-identical.
///
/// Filed 2026-09-02: a tick under a binary missing a schema version loaded
/// `engine` quarantined. Verify measured the empty view rather than refusing,
/// recorded 583 bogus `uncovered` findings and a `#verified` baseline, and
/// pruned 181 authored dispositions out of the advance file. Three separate
/// stores dirtied by a run that had nothing true to say, which is why this
/// test compares all three rather than only the one the report mentions.
///
/// Distinct from the pre-existing quarantine test in this file, which covers
/// a quarantined BINDING RECORD (a version-gate failure on the stored file).
/// This is the destination mem being unserviceable while the binding itself
/// loads fine.
#[test]
fn a_quarantined_destination_is_refused_and_all_three_stores_are_untouched() {
    let tmp = verify_workspace();
    let root = tmp.path();

    // Seed every store a bad run would touch, so "untouched" has something
    // to prove: findings + backfill from a real run, a `#verified` token,
    // and an authored exclusion in the advance ledger.
    memstead()
        .current_dir(root)
        .args(["projection", "verify", "engine/graph", "--advance"])
        .assert()
        .success();
    memstead()
        .current_dir(root)
        .args([
            "projection",
            "exclude",
            "engine/graph",
            "--exclusions",
            r#"{"src/a.rs": "probe rationale"}"#,
        ])
        .assert()
        .success();

    let findings_path = root.join(".memstead/state/findings/engine/graph.json");
    let advance_path = root.join(".memstead/state/advance/engine/graph.json");
    let config_path = root.join("engine-mem/.memstead/config.json");
    let findings_before = std::fs::read(&findings_path).unwrap();
    let advance_before = std::fs::read(&advance_path).unwrap();
    let config_before = std::fs::read(&config_path).unwrap();
    assert!(
        String::from_utf8_lossy(&config_before).contains("#verified"),
        "fixture must carry a baseline token"
    );
    assert!(
        String::from_utf8_lossy(&advance_before).contains("probe rationale"),
        "fixture must carry an authored exclusion"
    );

    // Quarantine the DESTINATION: repoint ONLY the schema pin at a version no
    // installed schema resolves to, leaving `syncState` in place. The binding
    // record itself stays valid.
    //
    // Rewriting the whole config would have dropped the `#verified` token, and
    // the comparison below would then have proved only that an absent token
    // stayed absent — which is not what its message claims (grader finding,
    // 2026-09-03). Preserving the token makes the assertion mean what it says.
    let quarantined_cfg = {
        let mut cfg: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        cfg["schema"] = Value::String("default@9.9.9".to_string());
        serde_json::to_vec_pretty(&cfg).unwrap()
    };
    std::fs::write(&config_path, &quarantined_cfg).unwrap();
    let config_quarantined = std::fs::read(&config_path).unwrap();
    assert!(
        String::from_utf8_lossy(&config_quarantined).contains("#verified"),
        "the quarantined fixture must still carry the token the comparison is about"
    );

    for (label, args) in [
        ("verify", vec!["projection", "verify", "engine/graph"]),
        (
            "brief",
            vec!["projection", "brief", "engine/graph", "--verify"],
        ),
    ] {
        let out = memstead()
            .current_dir(root)
            .args(&args)
            .assert()
            .failure()
            .get_output()
            .clone();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            text.contains("MEM_QUARANTINED"),
            "{label} refuses with the shared quarantine code: {text}"
        );
        assert!(
            text.contains("engine"),
            "{label} names the destination mem: {text}"
        );

        assert_eq!(
            std::fs::read(&findings_path).unwrap(),
            findings_before,
            "{label}: the findings store must be byte-identical"
        );
        assert_eq!(
            std::fs::read(&advance_path).unwrap(),
            advance_before,
            "{label}: the advance file must be byte-identical (no disposition prune)"
        );
        assert_eq!(
            std::fs::read(&config_path).unwrap(),
            config_quarantined,
            "{label}: the mem config, and so the #verified token, must be byte-identical"
        );
    }
}

/// C6 AC1: a BARE `projection verify` leaves the destination mem's config
/// byte-identical, findings and report unaffected; the same call with
/// `--advance` moves the `#verified` token. The entry this pins was filed on
/// a grader that verified in order to READ and dirtied the tree it was
/// checking, then reverted its own bump by hand.
///
/// The config is the right thing to compare: `#verified` is written through
/// `set_mem_sync_state`, which merges into the mem's `.memstead/config.json`.
/// The fixture seeds a STALE token first, so "unchanged" cannot pass by the
/// key simply being absent on both sides.
///
/// The anchors sidecar is deliberately NOT part of this comparison. The
/// prepared-hash backfill writes it on every completed run, gated or not,
/// because it is measurement machinery rather than a freshness claim:
/// withheld, an anchor never leaves `recheck` and drift stops being
/// adjudicated. What the plan was filed on is the freshness token, and that
/// is what the flag gates.
#[test]
fn bare_verify_leaves_the_mem_config_untouched_and_advance_moves_the_token() {
    let tmp = verify_workspace();
    let root = tmp.path();
    let config_path = root.join("engine-mem/.memstead/config.json");

    // Let the prepared-hash backfill land first. It writes the anchors
    // sidecar and so commits to the mem, which stamps the config — real, and
    // outside what this criterion is about. Once the anchor carries its hash
    // the backfill is idempotent and the settled state is the one a gate
    // actually re-verifies.
    memstead()
        .current_dir(root)
        .args(["projection", "verify", "engine/graph"])
        .assert()
        .success();

    // Seed a stale baseline, so the key is present and WRONG before the
    // measurement runs: "unchanged" must not be able to pass by the key
    // simply being absent on both sides. Written into the config directly,
    // the way this fixture writes its store and its anchors sidecar — the
    // `mem set-sync-state` verb needs a mem-repo workspace and this
    // file's workspace is the folder shape.
    let seeded = {
        let mut cfg: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        cfg["syncState"]["engine/graph/source-tree#verified"] =
            Value::String("staletoken".to_string());
        serde_json::to_vec_pretty(&cfg).unwrap()
    };
    std::fs::write(&config_path, &seeded).unwrap();
    let before = std::fs::read(&config_path).unwrap();
    assert!(
        String::from_utf8_lossy(&before).contains("staletoken"),
        "fixture must seed the stale token"
    );

    // (1) Bare verify: measures, records findings, writes nothing into the mem.
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["advanced"], false, "a bare run does not advance: {env}");
    assert!(
        env["verified_baseline"].as_array().unwrap().is_empty(),
        "no baseline key written: {env}"
    );
    assert!(
        env["report"].is_object(),
        "the measurement still happened and rendered: {env}"
    );
    let after_bare = std::fs::read(&config_path).unwrap();
    assert_eq!(
        after_bare,
        before,
        "a bare verify must leave the mem config byte-identical\nbefore: {}\nafter:  {}",
        String::from_utf8_lossy(&before),
        String::from_utf8_lossy(&after_bare),
    );

    // The human surface says so rather than staying silent, so an empty
    // baseline is never read as "there was nothing to record".
    let human = memstead()
        .current_dir(root)
        .args(["projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8_lossy(&human).contains("Baseline not advanced"),
        "the bare run must announce that it did not advance the baseline: {}",
        String::from_utf8_lossy(&human)
    );
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        before,
        "the human-mode run writes no more than the JSON one"
    );

    // (2) `--advance`: the token moves off the seeded stale value.
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "verify",
            "engine/graph",
            "--advance",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["advanced"], true, "the flag advances: {env}");
    assert!(
        !env["verified_baseline"].as_array().unwrap().is_empty(),
        "the baseline keys are reported: {env}"
    );
    let after = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !after.contains("staletoken"),
        "--advance replaces the stale token: {after}"
    );
}

/// `projection verify --full` measures completely: the JSON decision is
/// `forced` (full-enumeration walk, scheduler bypassed, cap unlimited), the
/// criterion-level backfill still happens, nothing queues, and the rendered
/// report states the full measurement with no sampling caveat. Without the
/// flag, the sampled behavior over the same workspace is what it was.
#[test]
fn verify_full_walks_everything_and_reports_forced() {
    let tmp = verify_workspace();
    let root = tmp.path();

    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph", "--full"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["full_resync"]["state"], "forced",
        "an explicit full measurement reports the forced walk: {env}"
    );
    assert_eq!(
        env["hash_backfilled"], 1,
        "--full includes the prepared-hash backfill: {env}"
    );
    assert_eq!(env["backlog"], 0, "cap unlimited — nothing queued: {env}");

    // Human-readable mode states the full measurement up front.
    let out = memstead()
        .current_dir(root)
        .args(["projection", "verify", "engine/graph", "--full"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("Full measurement (`--full`)"),
        "the rendered report leads with the full-measurement statement: {text}"
    );
    assert!(
        text.contains("not sampled"),
        "no sampling caveat — the figures are stated as computed: {text}"
    );

    // A no-flag run over the same workspace still succeeds on the sampled
    // path (byte-compatible economics; the scheduled decision, not forced).
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_ne!(
        env["full_resync"]["state"], "forced",
        "a no-flag run never reports a forced walk: {env}"
    );
}

/// REFUSAL — `verify --full` over a non-enumerable (web) medium refuses with
/// the existing typed capability error, exit-coded as validation, and renders
/// no report: a fabricated-complete report is never an answer.
#[test]
fn verify_full_refuses_non_enumerable_medium() {
    let tmp = verify_workspace();
    let root = tmp.path();
    write_store(
        root,
        "projections/engine/manual.json",
        r#"{"version":2,"intent":"the manual","sources":[{"name":"manual","type":"web","pointer":"https://example.com/docs","scope":[]}],"reference_mems":[],"destination_mem":"engine","deny_paths":[],"coverage_semantics":"curated","operations":{"verify":{"trigger":"manual","batch_size":20}}}"#,
    );

    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/manual", "--full"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["code"], "PROJECTION_CAPABILITY_UNSUPPORTED",
        "the existing typed capability error: {env}"
    );
    assert_eq!(env["details"]["medium_type"], "web");
    assert!(
        env["message"]
            .as_str()
            .unwrap_or("")
            .contains("non-enumerable"),
        "the refusal states why: {env}"
    );
}

// ---------------------------------------------------------------------------
// CI gate — `projection verify --fail-on-findings`
// ---------------------------------------------------------------------------

/// The three-outcome contract, all three demonstrated against the same
/// fixture so the codes are provably pairwise distinct: a completed clean run
/// exits 0, a completed run with a seeded drift exits **6**, and an
/// operational failure keeps its own typed code (3, not found). The whole
/// point of a dedicated findings code is that a CI job can tell "the mem
/// drifted" from "the engine could not run" — that distinction is what these
/// assertions pin.
#[test]
fn gate_exits_zero_clean_six_on_findings_and_typed_code_on_error() {
    let tmp = verify_workspace();
    let root = tmp.path();

    // (1) Clean fixture in gate mode → 0. (Runs once ungated first so the
    //     hash backfill has landed and the anchor adjudicates deterministically.)
    memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success();
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "verify",
            "engine/graph",
            "--fail-on-findings",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["rollup"]["findings_total"], 0,
        "the fixture is clean before the drift is seeded: {env}"
    );
    assert_eq!(
        env["rollup"]["verdict"], "clean",
        "a substantive pass with no findings is clean: {env}"
    );

    // (2) Seed a drift in the anchored artifact → the dedicated findings code.
    let src = root.join("src");
    std::fs::write(src.join("a.rs"), "one-drifted").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-qm", "drift"]);

    let assertion = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "verify",
            "engine/graph",
            "--fail-on-findings",
        ])
        .assert()
        .code(6);
    let stdout = assertion.get_output().stdout.clone();
    let text = String::from_utf8_lossy(&stdout);

    // (3) An operational failure over the same workspace keeps its own code,
    //     and it is not 6 — that is the distinction the gate exists to draw.
    memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "verify",
            "engine/nonexistent",
            "--fail-on-findings",
        ])
        .assert()
        .code(3);

    // Criterion 2: the report is emitted before the findings exit fires. In
    // `--json` mode stdout carries both the report envelope and the typed
    // error envelope, so a pipeline consumer can read either.
    assert!(
        text.contains("memstead-verify/v1"),
        "the report envelope lands before the gate fails: {text}"
    );
    assert!(
        text.contains("PROJECTION_VERIFY_FINDINGS"),
        "the typed error envelope still reaches stdout: {text}"
    );
}

/// An unreadable anchors sidecar is an operational failure, not findings.
///
/// The regression this pins was live: the anchor readers degrade a malformed
/// sidecar to "no anchors", so a fidelity pass read every artifact as
/// uncovered, recorded that as findings, and exited 6 — a red CI build
/// blaming the mem for a file the engine could not parse, with nothing on
/// stderr. The distinction the whole exit-code contract rests on is that 6
/// means the measurement SUCCEEDED; one made over an unreadable input did not.
#[test]
fn an_unreadable_anchors_sidecar_refuses_and_never_returns_the_findings_code() {
    let tmp = verify_workspace();
    let root = tmp.path();
    std::fs::write(root.join("engine-mem/.memstead/anchors.json"), "{ broken").unwrap();
    // Three ways to be unreadable, and the first fix caught only this one.
    // A grade found the other two still producing a confident "every
    // artifact uncovered" and exit 6, so they are pinned here beside it.

    let assertion = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "verify",
            "engine/graph",
            "--fail-on-findings",
        ])
        .assert()
        .code(5);
    let out = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    let env: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(env["code"], "ANCHORS_SIDECAR_UNREADABLE", "{env}");
    assert_eq!(env["details"]["mem"], "engine", "{env}");

    // Ungated too: the refusal is about the measurement being untrustworthy,
    // not about the gate flag.
    memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .code(5);

    // An EMPTY sidecar. `AnchorSidecar::from_bytes` tolerates whitespace-only
    // bytes as "no anchors" — right for a reader, wrong here: an interrupted
    // write leaves exactly this state, and it is not a mem that never had
    // anchors.
    std::fs::write(root.join("engine-mem/.memstead/anchors.json"), "   \n").unwrap();
    memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "verify",
            "engine/graph",
            "--fail-on-findings",
        ])
        .assert()
        .code(5);
}

/// A source directory that exists but cannot be entered refuses, rather than
/// enumerating nothing and calling the result drift.
///
/// The guard used to test existence alone, so an unreadable tree walked past
/// it: the pass enumerated zero artifacts, every anchor came back
/// unresolvable, and the verdict blamed a mem that had not moved — with the
/// report's own denominator saying `non-enumerable` two screens down.
#[cfg(unix)]
#[test]
fn an_unreadable_source_directory_refuses_rather_than_reporting_drift() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = verify_workspace();
    let root = tmp.path();
    let src = root.join("src");
    let restore = std::fs::metadata(&src).unwrap().permissions();
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o000)).unwrap();

    let assertion = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "verify",
            "engine/graph",
            "--fail-on-findings",
        ])
        .assert()
        .code(5);
    let out = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    let env: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(env["code"], "SOURCE_UNREACHABLE", "{env}");

    std::fs::set_permissions(&src, restore).unwrap();
}

/// A binding declaring `change_detection: "git"` over a tree with no `.git`
/// cannot support a green verdict.
///
/// This is the CI shape the guide's `fetch-depth: 0` advice circles: a
/// `git archive`, a Docker `COPY`, a vendored drop. The declaration used to
/// be honoured without probing, so the capability row asserted a signal that
/// could not be read and the rollup called the pass "substantive on every
/// axis" while `source_head` was empty and no baseline was written.
#[test]
fn a_declared_git_binding_without_a_git_root_cannot_verdict_clean() {
    let tmp = verify_workspace();
    let root = tmp.path();
    std::fs::remove_dir_all(root.join("src/.git")).unwrap();

    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["rollup"]["verdict"], "inconclusive",
        "a git binding with no git root is not a clean bill of health: {env}"
    );
    assert!(
        !env["rollup"]["blind_spots"].as_array().unwrap().is_empty(),
        "the blindness is named: {env}"
    );
}

/// The inconclusive gate (the graph-plans 03 pilot):
/// `--fail-on-inconclusive` exits **6** with the typed
/// `PROJECTION_VERIFY_INCONCLUSIVE` on a completed-but-blind run, after
/// rendering the report; the same run without the flag keeps its
/// long-standing exit 0 (the compatibility half); a substantive clean
/// run with the flag still exits 0; and a findings run with BOTH gate
/// flags exits with the findings code — the findings gate evaluates
/// first, a substantive result outranks a blindness report.
#[test]
fn inconclusive_gate_exits_six_typed_and_stays_opt_in() {
    // (1) Substantive clean run with the flag → 0.
    let tmp = verify_workspace();
    let root = tmp.path();
    memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success();
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "verify",
            "engine/graph",
            "--fail-on-inconclusive",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["rollup"]["verdict"], "clean", "{env}");

    // (2) Blind run (git binding, no git root): WITHOUT the flag the
    //     long-standing exit 0 holds — the compatibility promise.
    std::fs::remove_dir_all(root.join("src/.git")).unwrap();
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["rollup"]["verdict"], "inconclusive", "{env}");

    // (3) The same blind run WITH the flag → 6, typed, report first.
    let assertion = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "verify",
            "engine/graph",
            "--fail-on-inconclusive",
        ])
        .assert()
        .code(6);
    let text = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        text.contains("memstead-verify/v1"),
        "the report envelope lands before the gate fails: {text}"
    );
    assert!(
        text.contains("PROJECTION_VERIFY_INCONCLUSIVE"),
        "the typed inconclusive code names the gate that fired: {text}"
    );

    // (4) Ordering: a findings run with BOTH flags exits with the
    //     findings code, not the inconclusive one.
    let tmp2 = verify_workspace();
    let root2 = tmp2.path();
    memstead()
        .current_dir(root2)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success();
    let src2 = root2.join("src");
    std::fs::write(src2.join("a.rs"), "one-drifted").unwrap();
    git(&src2, &["add", "-A"]);
    git(&src2, &["commit", "-qm", "drift"]);
    let assertion = memstead()
        .current_dir(root2)
        .args([
            "--json",
            "projection",
            "verify",
            "engine/graph",
            "--fail-on-findings",
            "--fail-on-inconclusive",
        ])
        .assert()
        .code(6);
    let text = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        text.contains("PROJECTION_VERIFY_FINDINGS")
            && !text.contains("PROJECTION_VERIFY_INCONCLUSIVE"),
        "the findings gate outranks the inconclusive gate: {text}"
    );
}

/// The gate is opt-in: a bare `projection verify` over a drifted fixture
/// exits 0 exactly as it always has. This is the compatibility promise — a
/// silent default flip would break every existing consumer, including this
/// project's own loops.
#[test]
fn gate_is_opt_in_bare_verify_still_exits_zero_with_findings() {
    let tmp = verify_workspace();
    let root = tmp.path();

    memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success();

    let src = root.join("src");
    std::fs::write(src.join("a.rs"), "one-drifted").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-qm", "drift"]);

    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["report"]["findings_by_class"]["drifted"], 1,
        "the drift IS present — the ungated run simply does not gate on it: {env}"
    );
    assert_eq!(
        env["rollup"]["verdict"], "drifted",
        "the verdict reports the drift even ungated: {env}"
    );
}

/// The human report opens with the rollup verdict and the concrete actions —
/// making the fidelity-contract page's long-standing claim true.
#[test]
fn human_report_opens_with_verdict_and_actions() {
    let tmp = verify_workspace();
    let root = tmp.path();

    memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success();

    let src = root.join("src");
    std::fs::write(src.join("a.rs"), "one-drifted").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-qm", "drift"]);

    let out = memstead()
        .current_dir(root)
        .args(["projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8_lossy(&out);
    let verdict_at = text.find("**Verdict: DRIFTED**").unwrap_or_else(|| {
        panic!("the report opens with the rollup verdict: {text}");
    });
    let provenance_at = text
        .find("Coverage semantics")
        .expect("the denominator-provenance block still renders");
    assert!(
        verdict_at < provenance_at,
        "the verdict comes BEFORE the provenance a reader used to open on: {text}"
    );
    assert!(
        text.contains("**Do next:**"),
        "the rollup carries top concrete actions: {text}"
    );
    // The coverage rule (consistency-sweep 04/08, found by the plan's
    // grade): human mode must carry the stamp the JSON envelope
    // carries, or the human reader gets a verdict with no coverage.
    assert!(
        text.contains(
            "**Verdict coverage:** examined=projection,anchors; advisory=; not_examined="
        ),
        "human mode carries the verdict-coverage stamp: {text}"
    );
}

/// The machine payload carries the pinned version marker, in the house style
/// the two existing external envelopes established. A consumer asserts this
/// before parsing, so a future shape change fails loudly.
#[test]
fn verify_json_carries_the_pinned_format_marker() {
    let tmp = verify_workspace();
    let root = tmp.path();

    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["format"], "memstead-verify/v1",
        "the external contract is versioned: {env}"
    );
    assert!(
        env["rollup"]["verdict"].is_string(),
        "the rollup ships in the envelope, not only in the markdown: {env}"
    );
    assert!(
        env["report"]["findings_by_class"].is_object(),
        "the closed finding-class vocabulary still ships: {env}"
    );

    // Every field the guide names as contract, asserted by name. A grade
    // found six of these documented and pinned by nothing: a rename would
    // reshape the `v1` payload and falsify the guide while the marker kept
    // saying `memstead-verify/v1`, which is exactly the silent break the
    // marker exists to prevent. `is_null()` rather than a value check —
    // the point is that the key survives a refactor, not what it holds.
    for field in [
        "verdict",
        "findings_total",
        "because",
        "blind_spots",
        "actions",
    ] {
        assert!(
            !env["rollup"][field].is_null(),
            "rollup.{field} is documented as contract but missing: {env}"
        );
    }
    for facet in env["report"]["capabilities"].as_array().unwrap() {
        for field in [
            "facet",
            "medium_type",
            "enumerable",
            "change_signal",
            "base_version_retrievable",
            "anchor_namespace",
            "signal",
        ] {
            assert!(
                !facet[field].is_null(),
                "capabilities[].{field} is documented as contract but missing: {facet}"
            );
        }
    }
    for facet in env["report"]["freshness"].as_array().unwrap() {
        for field in ["facet", "signal", "change_detectable"] {
            assert!(
                !facet[field].is_null(),
                "freshness[].{field} is documented as contract but missing: {facet}"
            );
        }
        // `synced` and `verified` are legitimately null when never recorded,
        // so the !is_null idiom above cannot cover them — the KEY has to be
        // present. The guide documents both as contract; a rename would
        // falsify it while the marker still said `memstead-verify/v1`.
        for field in ["synced", "verified"] {
            assert!(
                facet.get(field).is_some(),
                "freshness[].{field} is documented as contract but the key is gone: {facet}"
            );
        }
    }
    // The denominator is INTERNALLY tagged on `kind`. Pinned because the
    // guide documents that exact shape as external contract, and serde's
    // default for an enum is externally tagged — dropping the
    // `#[serde(tag = "kind")]` attribute would silently reshape a payload
    // consumers branch on, while still passing an `is_object()` check.
    let denom = &env["report"]["coverage"]["denominator"];
    assert!(
        denom.is_object(),
        "the denominator union still ships: {env}"
    );
    assert_eq!(
        denom["kind"], "enumerated",
        "denominator is internally tagged on `kind`, as the guide documents: {env}"
    );
    assert!(
        denom["count"].is_number(),
        "an enumerated denominator carries its count alongside the tag: {env}"
    );
}

// ── graph-medium fidelity: a two-mem graph→graph binding, end to end ────────
//
// The S1b pilot drove a source change through a graph binding end-to-end and
// then watched a deliberately stale anchor over the changed entity go
// unflagged: anchor resolution 0/0, coverage 0/0, drift undetected, while the
// capability matrix claimed full parity. This fixture is that scenario,
// re-run through the CLI as separate processes — it cannot pass silently again.

/// A workspace with two folder mems: `srcmem` (the source graph) and `dest`
/// (the destination), bound by a graph-medium binding scoped to the whole
/// source mem. `dest` holds two entities anchored at source entities, both
/// hash-less so the first verify backfills them — the same shape the codebase
/// fixture uses, proving the backfill path is namespace-agnostic.
fn graph_binding_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    write_store(
        root,
        "workspace.toml",
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    );
    write_store(
        root,
        "state/mounts.json",
        r#"{"format":"memstead-mounts-3","mounts":[
            {"mem":"srcmem","schema":"default@1.0.0","storage":{"type":"folder","path":"src-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false},
            {"mem":"dest","schema":"default@1.0.0","storage":{"type":"folder","path":"dest-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}
        ]}"#,
    );

    // A graph source selects entities, so its scope is the entity vocabulary.
    // `deny_paths` stays empty — a glob deny is illegal over this namespace.
    write_store(
        root,
        "projections/dest/mirror.json",
        r#"{"version":2,"intent":"mirror srcmem into dest","sources":[{"name":"src-graph","type":"graph","pointer":"srcmem","scope":[{"path":"*","mode":"allow"}]}],"reference_mems":[],"destination_mem":"dest","deny_paths":[],"coverage_semantics":"exhaustive","prune":{"guarantee":"conflict-flag"},"operations":{"build":{"mode":"discovery","trigger":"loop","batch_size":20},"sync":{"trigger":"manual","batch_size":20},"verify":{"trigger":"manual","batch_size":20,"adjudication_cap":50,"full_resync_every":20}}}"#,
    );

    // `concept` with definition/explanation is a real `default@1.0.0` type with
    // its real required sections. An earlier draft wrote `type: decision`,
    // which that schema does not declare — the raw-markdown read path tolerates
    // it while `memstead create` refuses it, so the fixture would have been
    // passing on an asymmetry rather than on the behaviour under test.
    let entity = |dir: &Path, slug: &str, ty: &str, title: &str, body: &str| {
        std::fs::write(
            dir.join(format!("{slug}.md")),
            format!(
                "---\ntype: {ty}\n---\n\n# {title}\n\n## Definition\n\n{title} is a fixture concept.\n\n## Explanation\n\n{body}\n"
            ),
        )
        .unwrap();
    };

    // Source mem: three entities. `gamma` is deliberately unprojected — it must
    // surface as an uncovered member of S(D), which a 0/0 denominator could
    // never do.
    let src_mem = root.join("src-mem");
    std::fs::create_dir_all(src_mem.join(".memstead")).unwrap();
    std::fs::write(
        src_mem.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0"}"#,
    )
    .unwrap();
    entity(&src_mem, "alpha", "concept", "Alpha", "Alpha body.");
    entity(&src_mem, "beta", "concept", "Beta", "Beta body.");
    entity(&src_mem, "gamma", "concept", "Gamma", "Gamma body.");

    // Destination mem: two entities, each anchored at a source ENTITY (not a
    // path), hash-less so the first verify backfills.
    let dest_mem = root.join("dest-mem");
    std::fs::create_dir_all(dest_mem.join(".memstead")).unwrap();
    std::fs::write(
        dest_mem.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0"}"#,
    )
    .unwrap();
    entity(
        &dest_mem,
        "from-alpha",
        "decision",
        "From alpha",
        "Mirrors alpha.",
    );
    entity(
        &dest_mem,
        "from-beta",
        "decision",
        "From beta",
        "Mirrors beta.",
    );
    std::fs::write(
        dest_mem.join(".memstead").join("anchors.json"),
        r#"{"version":1,"entities":{
            "dest--from-alpha":[{"artifact":"srcmem--alpha","grain":"entity","class":"anchored","hash_stability":"stable"}],
            "dest--from-beta":[{"artifact":"srcmem--beta","grain":"entity","class":"anchored","hash_stability":"stable"}]
        }}"#,
    )
    .unwrap();

    tmp
}

/// Criterion 1 + 2, positively, through the CLI: a graph binding's verify
/// reports a REAL enumerated denominator (the source mem's in-scope entities),
/// surfaces the unprojected source entity as uncovered, backfills its entity
/// anchors, and — after one source entity changes — adjudicates that anchor
/// `drifted` while the untouched one still resolves.
#[test]
fn graph_binding_verify_enumerates_and_detects_entity_drift() {
    let tmp = graph_binding_workspace();
    let root = tmp.path();

    // (1) First verify: a real denominator, and the hash-less entity anchors
    //     gain their prepared hashes.
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "dest/mirror"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();

    assert_eq!(
        env["report"]["coverage"]["denominator"]["kind"], "enumerated",
        "a graph source enumerates for real — not the 'no S(D) denominator' bail: {env}"
    );
    assert_eq!(
        env["report"]["coverage"]["denominator"]["count"], 3,
        "S(D) is the source mem's three in-scope entities: {env}"
    );
    assert_eq!(
        env["hash_backfilled"], 2,
        "both hash-less ENTITY anchors backfill, exactly as path anchors do: {env}"
    );

    // (2) Idempotent, and the unprojected source entity is a real gap.
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "dest/mirror"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["hash_backfilled"], 0, "backfill happens once: {env}");
    assert_eq!(
        env["report"]["anchors"]["resolves"], 2,
        "both entity anchors resolve against the live graph — not 'unobserved': {env}"
    );
    let body = serde_json::to_string(&env).unwrap();
    assert!(
        body.contains("srcmem--gamma"),
        "the unprojected source entity surfaces as an uncovered member of S(D): {env}"
    );

    // (3) The pilot's move: change ONE source entity. Its anchor must drift.
    std::fs::write(
        root.join("src-mem").join("alpha.md"),
        "---\ntype: decision\n---\n\n# Alpha\n\n## Decision\n\nAlpha body, rewritten.\n",
    )
    .unwrap();

    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "dest/mirror"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["report"]["anchors"]["drifted"], 1,
        "the stale-pinned anchor over the CHANGED source entity is drifted — \
         this is the pilot failure that went unflagged: {env}"
    );
    assert_eq!(
        env["report"]["anchors"]["resolves"], 1,
        "the anchor over the untouched entity still resolves: {env}"
    );
}

/// Criterion 1's exclude clause: `projection exclude` gates on S(D)
/// membership, so a genuine entity of the source mem is accepted and a
/// non-member is refused. Before graph enumeration, S(D) was empty and the
/// gate refused *every* id — the command was unusable over a graph binding.
#[test]
fn graph_binding_exclude_accepts_a_real_source_entity() {
    let tmp = graph_binding_workspace();
    let root = tmp.path();

    memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "exclude",
            "dest/mirror",
            "--exclusions",
            r#"{"srcmem--gamma": "out of scope for this mem"}"#,
        ])
        .assert()
        .success();

    // The complement: an id that is not in S(D) is still refused.
    memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "exclude",
            "dest/mirror",
            "--exclusions",
            r#"{"srcmem--not-a-real-entity": "typo"}"#,
        ])
        .assert()
        .failure();
}

/// Criterion 2's complement, at its sharpest: an **unmounted source mem** must
/// refuse, not resolve as a mem full of deleted entities.
///
/// Every entity anchor into an absent mem misses the store and observes as
/// ABSENT — a definite `orphaned`, not an honest "unobserved". The pass would
/// report drift, instruct the reader to repoint or unset anchors that are
/// perfectly fine, and — because `orphaned` is the one state satisfying
/// prune's all-orphaned gate — let prune propose deleting the destination
/// entities. The path mediums have always refused a vanished source; the graph
/// medium needs the same refusal for a worse consequence.
#[test]
fn an_unmounted_graph_source_refuses_instead_of_reporting_deletions() {
    let tmp = graph_binding_workspace();
    let root = tmp.path();

    // Baseline: mounted, the anchors resolve.
    memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "dest/mirror"])
        .assert()
        .success();

    // Now unmount the source mem, leaving the binding pointing at it.
    write_store(
        root,
        "state/mounts.json",
        r#"{"format":"memstead-mounts-3","mounts":[
            {"mem":"dest","schema":"default@1.0.0","storage":{"type":"folder","path":"dest-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}
        ]}"#,
    );

    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "dest/mirror"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["code"], "SOURCE_UNREACHABLE",
        "an absent source mem is a typed refusal, not a measurement: {env}"
    );
    let body = serde_json::to_string(&env).unwrap();
    assert!(
        !body.contains("orphaned"),
        "nothing is scored orphaned — an unmounted mem must never be \
         indistinguishable from a deleted one: {env}"
    );

    // The polarity that matters more, and that verify's refusal does not
    // cover: PRUNE reaches anchor resolution by its own path, so the sync
    // brief must not propose deleting entities whose source is merely
    // unmounted. An earlier fix guarded only `run_verify`, and this test
    // asserted only the block above — it passed while the brief went on
    // recommending the deletion of every destination entity.
    let brief = String::from_utf8(
        memstead()
            .current_dir(root)
            .args(["projection", "brief", "dest/mirror", "--sync"])
            .assert()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        !brief.contains("artifact(s) gone"),
        "nothing is reported as gone from the source when the mem is merely \
         unmounted — a data-loss suggestion to the sole maintenance writer: {brief}"
    );
}

/// Criterion 5's complement, on the path that matters: a graph facet carrying
/// a scope pattern nothing interprets must be REFUSED WHEN IT RUNS, not only
/// when it is edited.
///
/// Scaffolding the right shape protects only bindings this engine wrote. Every
/// graph binding scaffolded before the entity vocabulary existed carries the
/// path glob `**/*`, and hand-editing the record is a route the CLI's own
/// name-collision refusal points people at. Left unguarded, such a binding ran
/// clean over an S(D) of zero and recorded a `#verified` baseline for a
/// measurement that never happened.
#[test]
fn a_graph_scope_nothing_interprets_is_refused_on_the_run_path() {
    let tmp = graph_binding_workspace();
    let root = tmp.path();

    // Hand-edit the binding to the pre-vocabulary path glob.
    let binding = root.join(".memstead/projections/dest/mirror.json");
    let text = std::fs::read_to_string(&binding).unwrap();
    std::fs::write(
        &binding,
        text.replace(
            r#"{"path":"*","mode":"allow"}"#,
            r#"{"path":"**/*","mode":"allow"}"#,
        ),
    )
    .unwrap();

    // Every run path refuses — not just the edit paths that call
    // `validate_binding`. Verify is the one that used to record a `#verified`
    // baseline over an empty walk.
    for args in [
        vec!["--json", "projection", "verify", "dest/mirror"],
        vec!["--json", "projection", "brief", "dest/mirror"],
    ] {
        let out = memstead()
            .current_dir(root)
            .args(&args)
            .assert()
            .failure()
            .get_output()
            .stdout
            .clone();
        let env: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            env["code"], "PROJECTION_SCOPE_UNINTERPRETABLE",
            "`{args:?}` must refuse a scope nothing interprets: {env}"
        );
        let msg = env["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("type:") && msg.contains("id:"),
            "the refusal names the legal forms rather than leaving them to be \
             found: {env}"
        );
    }
}

/// Criterion 3's complement, second clause: a capability the matrix CLAIMS but
/// the pass could not deliver renders as a degradation, never as silence. The
/// degradation block only ever spoke for media already marked non-enumerable,
/// so an enumerable medium whose walk came back empty printed
/// `Degradations: (none)` beside a report with no denominator.
#[test]
fn an_empty_walk_on_an_enumerable_medium_renders_a_degradation() {
    let tmp = graph_binding_workspace();
    let root = tmp.path();

    // A well-formed selector that legitimately matches nothing — no
    // uninterpretable-scope hole involved.
    let binding = root.join(".memstead/projections/dest/mirror.json");
    let text = std::fs::read_to_string(&binding).unwrap();
    std::fs::write(
        &binding,
        text.replace(
            r#"{"path":"*","mode":"allow"}"#,
            r#"{"path":"type:nosuchtype","mode":"allow"}"#,
        ),
    )
    .unwrap();

    let out = String::from_utf8(
        memstead()
            .current_dir(root)
            .args(["projection", "verify", "dest/mirror"])
            .assert()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();

    assert!(
        out.contains("enumeration-empty"),
        "the unavailable enumeration is named as a degradation: {out}"
    );
    assert!(
        !out.contains("_(none)_") || !out.contains("## Degradations\n\n_(none)_"),
        "the Degradations block is not silent about it: {out}"
    );
}

/// Criteria 1 and 3's complements, in the shape that hid them: a MIXED
/// binding, where one facet walks and another does not.
///
/// Both guards were binding-level — the `--full` refusal tested the union of
/// every facet's walk, and the degradation flag was computed over that same
/// union while being rendered per facet. One facet with artifacts therefore
/// spoke for a sibling that had none: `--full` returned clean over a scope
/// nobody looked at, and the Degradations block stayed silent about a
/// capability the matrix claimed and the pass never delivered.
#[test]
fn an_empty_facet_is_not_excused_by_a_sibling_that_walked() {
    let tmp = graph_binding_workspace();
    let root = tmp.path();

    // Add a second, path-medium facet that DOES walk, alongside a graph facet
    // narrowed to a well-formed selector matching nothing.
    std::fs::create_dir_all(root.join("code")).unwrap();
    std::fs::write(root.join("code").join("a.rs"), "fn a() {}\n").unwrap();
    write_store(
        root,
        "projections/dest/mirror.json",
        r#"{"version":2,"intent":"mixed","sources":[
            {"name":"src-graph","type":"graph","pointer":"srcmem","scope":[{"path":"type:nosuchtype","mode":"allow"}]},
            {"name":"code-facet","type":"filesystem","pointer":"code","change_detection":"mtime","scope":[{"path":"**/*","mode":"allow"}]}
        ],"reference_mems":[],"destination_mem":"dest","deny_paths":[],"coverage_semantics":"exhaustive","operations":{"build":{"mode":"discovery","trigger":"loop","batch_size":20},"sync":{"trigger":"manual","batch_size":20},"verify":{"trigger":"manual","batch_size":20,"adjudication_cap":50,"full_resync_every":20}}}"#,
    );

    // (1) `--full` must refuse, naming the empty facet — not pass because the
    //     sibling produced a non-empty union.
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "dest/mirror", "--full"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["code"], "PROJECTION_CAPABILITY_UNSUPPORTED",
        "a full measurement refuses when any enumerable facet walked nothing: {env}"
    );
    assert_eq!(
        env["details"]["facet"], "src-graph",
        "the refusal names the facet that walked nothing, not the whole binding: {env}"
    );

    // (2) A plain pass measures what it can — and says what it could not.
    let out = String::from_utf8(
        memstead()
            .current_dir(root)
            .args(["projection", "verify", "dest/mirror"])
            .assert()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        out.contains("enumeration-empty:`src-graph`"),
        "the empty facet is named as a degradation even though a sibling \
         walked: {out}"
    );
    assert!(
        !out.contains("enumeration-empty:`code-facet`"),
        "the facet that DID walk is not accused of walking nothing: {out}"
    );
}

/// Criterion 4's complement, on the path a grade found: the brief must never
/// instruct an agent to write a scope the engine then refuses.
///
/// An unscoped facet's remedy was medium-agnostic — "write `**/*` in the facet
/// scope" — printed at a graph source whose run path refuses exactly that as
/// not an entity selector. The engine told the agent to do the one thing it
/// would reject. This asserts the round trip: whatever the brief tells you to
/// write must actually run.
#[test]
fn the_unscoped_remedy_is_one_the_medium_accepts() {
    let tmp = graph_binding_workspace();
    let root = tmp.path();

    let binding = root.join(".memstead/projections/dest/mirror.json");
    let text = std::fs::read_to_string(&binding).unwrap();
    std::fs::write(
        &binding,
        text.replace(r#""scope":[{"path":"*","mode":"allow"}]"#, r#""scope":[]"#),
    )
    .unwrap();

    let brief = String::from_utf8(
        memstead()
            .current_dir(root)
            .args(["projection", "brief", "dest/mirror"])
            .assert()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        brief.contains("unscoped facet"),
        "the unscoped facet is reported: {brief}"
    );
    assert!(
        !brief.contains("write `**/*`"),
        "a graph source is never told to write a path glob — the engine refuses \
         that scope, so printing it is an instruction to fail: {brief}"
    );
    assert!(
        brief.contains("write `*` in the"),
        "the remedy names a selector this medium actually accepts: {brief}"
    );

    // The round trip: take the brief's advice and the binding must RUN.
    std::fs::write(
        &binding,
        std::fs::read_to_string(&binding)
            .unwrap()
            .replace(r#""scope":[]"#, r#""scope":[{"path":"*","mode":"allow"}]"#),
    )
    .unwrap();
    memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "dest/mirror"])
        .assert()
        .success();
}

/// Criterion 5's complement, second clause, for the medium it was left on. A
/// `web` facet has no scope vocabulary at all, so any rule is uninterpretable —
/// yet the declaration gate checked graph alone and the brief printed
/// `Paths: **/*` at the agent as selection. Fixing the medium where a defect
/// was demonstrated and leaving its twin standing is how this class survived a
/// round.
#[test]
fn a_web_facet_cannot_carry_scope_either() {
    let tmp = graph_binding_workspace();
    let root = tmp.path();

    write_store(
        root,
        "projections/dest/web.json",
        r#"{"version":2,"intent":"web","sources":[{"name":"web-facet","type":"web","pointer":"https://example.com","scope":[{"path":"**/*","mode":"allow"}]}],"reference_mems":[],"destination_mem":"dest","deny_paths":[],"operations":{"build":{"mode":"discovery","trigger":"loop","batch_size":20}}}"#,
    );

    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "brief", "dest/web"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["code"], "PROJECTION_SCOPE_UNINTERPRETABLE",
        "a web facet's scope rule is refused, not printed as selection: {env}"
    );
}

/// Criterion 6 proper: build→sync→verify over a graph binding whose source mem
/// is **git-branch backed**, so a real snapshot token and a real changed slice
/// exist. The folder-mem fixture above cannot reach this — a folder mount
/// tracks no head, so its graph source can only ever report "snapshot missing",
/// which pins the token's *absence* rather than the token.
///
/// This pins the working change-detection half the plan requires stay
/// observably unchanged: the snapshot token appears in the findings key, the
/// changed slice names the modified source entity in the sync brief, and
/// `advance` writes the baseline forward.
///
/// `--storage git-branch` needs a mem-repo workspace. The
/// folder-mem graph tests above carry the coverage
/// and drift criteria; this one adds the git-backed change-detection half.
#[test]
fn graph_binding_over_a_git_backed_source_pins_token_slice_and_baseline() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // A mem-repo workspace — the shape `--storage git-branch` requires.
    std::process::Command::new("git")
        .args(["init", "-q", "mem-repo"])
        .current_dir(root)
        .output()
        .unwrap();
    write_store(
        root,
        "workspace.toml",
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    );

    let run = |args: &[&str]| {
        memstead()
            .current_dir(root)
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone()
    };

    run(&[
        "workspace",
        "allow-create",
        "*",
        "--schema",
        "default@1.0.0",
    ]);
    run(&[
        "mem",
        "init",
        "srcmem",
        "--schema",
        "default@1.0.0",
        "--storage",
        "git-branch",
    ]);
    run(&[
        "mem",
        "init",
        "dest",
        "--schema",
        "default@1.0.0",
        "--storage",
        "git-branch",
    ]);

    for title in ["Alpha", "Beta", "Gamma"] {
        run(&[
            "create",
            "--mem",
            "srcmem",
            "--title",
            title,
            "--type",
            "concept",
            "--section",
            &format!("definition={title} is a fixture concept."),
            "--section",
            &format!("explanation=Body of {title}."),
        ]);
    }

    write_store(
        root,
        "projections/dest/mirror.json",
        r#"{"version":2,"intent":"mirror srcmem into dest","sources":[{"name":"src-graph","type":"graph","pointer":"srcmem","scope":[{"path":"*","mode":"allow"}]}],"reference_mems":[],"destination_mem":"dest","deny_paths":[],"coverage_semantics":"exhaustive","operations":{"build":{"mode":"discovery","trigger":"loop","batch_size":20},"sync":{"trigger":"manual","batch_size":20},"verify":{"trigger":"manual","batch_size":20,"adjudication_cap":50,"full_resync_every":20}}}"#,
    );

    // (1) BUILD: the build leg of a graph binding is a discovery brief plus
    //     agent writes — there is no `projection build` command. Render the
    //     brief the agent works from, then make the write it prescribes:
    //     a destination entity anchored at a source ENTITY. That anchor is
    //     what verify measures coverage and drift against below, so the
    //     chain is genuinely build→sync→verify in one workspace rather than
    //     a verify fixture with the build hand-seeded on disk.
    let build_brief = String::from_utf8(run(&["projection", "brief", "dest/mirror"])).unwrap();
    assert!(
        build_brief.contains("**src-graph** (graph, primary)"),
        "the build brief names the graph source: {build_brief}"
    );
    assert!(
        build_brief.contains("Entities: *"),
        "a graph source's scope renders on the entity axis, not as `Paths`: {build_brief}"
    );
    assert!(
        build_brief.contains("memstead_search mem=srcmem"),
        "the brief hands the agent an executable route to the source baseline \
         — a changed slice alone is a delta with no baseline: {build_brief}"
    );
    assert!(
        !build_brief.contains("Paths:"),
        "no path-glob guidance is rendered for an entity-namespace source: {build_brief}"
    );

    run(&[
        "create",
        "--mem",
        "dest",
        "--title",
        "From alpha",
        "--type",
        "concept",
        "--section",
        "definition=Mirrors the alpha concept.",
        "--section",
        "explanation=Written from the build brief.",
        "--anchor",
        r#"{"artifact":"srcmem--alpha","grain":"entity","class":"anchored","source":"src-graph"}"#,
    ]);

    // (2) VERIFY: a real enumerated denominator AND a real snapshot token.
    //     The token is the half a folder-mem source can never produce.
    //     `--advance` because the leg below asserts what the `#verified`
    //     baseline holds after a sync advances `#synced`; since C6 the
    //     baseline moves only when asked for, so a test about the baseline
    //     has to ask (2026-09-03).
    let env: Value = serde_json::from_slice(&run(&[
        "--json",
        "projection",
        "verify",
        "dest/mirror",
        "--advance",
    ]))
    .unwrap();
    assert_eq!(
        env["report"]["coverage"]["denominator"]["count"], 3,
        "the git-backed source mem's three entities are S(D): {env}"
    );
    // The build leg's write is what verify measures: the entity it anchored is
    // covered, the two it did not are gaps. Without this the build leg would
    // be decorative — a brief rendered and a write nothing checks.
    let uncovered = serde_json::to_string(&env["report"]).unwrap();
    assert!(
        uncovered.contains("srcmem--beta") && uncovered.contains("srcmem--gamma"),
        "the two unprojected source entities are gaps: {env}"
    );
    assert!(
        !env["report"]["coverage"]["uncovered"]
            .as_array()
            .map(|a| a.iter().any(|v| v == "srcmem--alpha"))
            .unwrap_or(false),
        "the entity the build leg anchored is NOT a gap — the write is measured: {env}"
    );

    let source_head = env["key"]["source_head"].as_str().unwrap().to_string();
    let token = source_head
        .strip_prefix("src-graph=")
        .expect("the findings key carries the per-source snapshot token")
        .to_string();
    assert_eq!(
        token.len(),
        40,
        "the graph snapshot token is the source mem's head SHA, not an empty \
         placeholder: {source_head}"
    );

    // (3) SYNC: with that token recorded as the baseline, a change to one
    //     source entity must surface as the changed slice — the half the
    //     brief steers by.
    run(&[
        "mem",
        "set-sync-state",
        "dest",
        "dest/mirror/src-graph#synced",
        &token,
    ]);
    run(&[
        "update",
        "srcmem--alpha",
        "--auto-hash",
        "--section",
        "explanation=Alpha body, rewritten.",
    ]);

    let brief = String::from_utf8(run(&["projection", "brief", "dest/mirror"])).unwrap();
    assert!(
        brief.contains("Source changes since the last sync"),
        "the source moved, so the brief presents a changed slice: {brief}"
    );
    assert!(
        brief.contains("srcmem--alpha"),
        "the changed slice names the modified source ENTITY: {brief}"
    );
    assert!(
        !brief.contains("srcmem--beta") && !brief.contains("srcmem--gamma"),
        "only the changed entity is in the slice — a delta, not the whole source: {brief}"
    );

    // (4) ADVANCE: disposing the whole slice completes the pass and writes the
    //     baseline forward to the source's new head.
    let env: Value = serde_json::from_slice(&run(&[
        "--json",
        "projection",
        "advance",
        "dest/mirror",
        "--dispositions",
        r#"{"srcmem--alpha": "worked"}"#,
    ]))
    .unwrap();
    assert_eq!(
        env["completed"], true,
        "the slice's only artifact is disposed — the pass completes: {env}"
    );

    assert_eq!(
        env["tokens_written"],
        serde_json::json!(["dest/mirror/src-graph#synced"]),
        "a completed pass writes the per-source baseline token: {env}"
    );

    let dump: Value = serde_json::from_slice(&run(&["--json", "workspace", "dump"])).unwrap();
    let sync_state = dump["mems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "dest")
        .and_then(|m| m["sync_state"].as_object())
        .expect("the destination mem carries sync state")
        .clone();

    let synced = sync_state["dest/mirror/src-graph#synced"].as_str().unwrap();
    assert_ne!(
        synced, token,
        "the #synced baseline advanced past the head the pass started on: {sync_state:?}"
    );
    assert_eq!(
        synced.len(),
        40,
        "it advanced to a real head SHA: {sync_state:?}"
    );

    // The #verified baseline is a DIFFERENT key and legitimately still holds
    // the head verify ran at — advancing sync must not move it. Asserting the
    // two independently is the point: one dump-wide substring check would
    // have read verify's untouched token as a failure to advance sync.
    assert_eq!(
        sync_state["dest/mirror/src-graph#verified"]
            .as_str()
            .unwrap(),
        token,
        "verify's baseline stays at the head it measured: {sync_state:?}"
    );
}

// ---------------------------------------------------------------------------
// projection edit (the general binding-field patch over pipeline_edit)
// ---------------------------------------------------------------------------

/// Editing the source list replaces the block whole and round-trips: the
/// serve/bridge shape — adding a second and third source beside the existing
/// one — lands, every other field is untouched, and the record re-reads with
/// all three names.
#[test]
fn edit_replaces_the_source_list_and_preserves_the_rest() {
    let tmp = build_only_workspace();
    let root = tmp.path();
    let before = read_binding(root);

    let output = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "edit",
            "engine/graph",
            "--patch",
            r#"{"sources": [
                {"name":"src","type":"codebase","pointer":"../public"},
                {"name":"serve","type":"codebase","pointer":"../serve"},
                {"name":"bridge","type":"codebase","pointer":"../bridge"}
            ]}"#,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).expect("--json edit must emit JSON");
    assert_eq!(env["binding"], "engine/graph");
    assert_eq!(
        env["sources"],
        serde_json::json!(["src", "serve", "bridge"])
    );

    let after = read_binding(root);
    assert_eq!(after.sources.len(), 3);
    assert_eq!(after.destination_mem, before.destination_mem);
    assert_eq!(
        after.operations.build.is_some(),
        before.operations.build.is_some(),
        "operations untouched by a sources patch"
    );
}

/// A patch that would introduce a validation refusal (duplicate source name)
/// refuses typed and writes nothing — the file stays byte-identical.
#[test]
fn edit_refuses_an_introduced_refusal_and_writes_nothing() {
    let tmp = build_only_workspace();
    let root = tmp.path();
    let raw_before =
        std::fs::read_to_string(root.join(".memstead/projections/engine/graph.json")).unwrap();

    let output = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "edit",
            "engine/graph",
            "--patch",
            r#"{"sources": [
                {"name":"src","type":"codebase","pointer":"../public"},
                {"name":"src","type":"codebase","pointer":"../serve"}
            ]}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&output).expect("refusal must emit JSON");
    assert_eq!(env["code"], "PROJECTION_EDIT_REFUSED", "got: {env}");
    let raw_after =
        std::fs::read_to_string(root.join(".memstead/projections/engine/graph.json")).unwrap();
    assert_eq!(raw_before, raw_after, "no write on refusal");
}

/// A missing binding refuses PROJECTION_NOT_FOUND; a malformed patch refuses
/// PROJECTION_EDIT_INVALID_JSON.
#[test]
fn edit_refusals_are_typed() {
    let tmp = build_only_workspace();
    let root = tmp.path();

    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "edit",
            "engine/ghost",
            "--patch",
            "{}",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], "PROJECTION_NOT_FOUND", "got: {env}");

    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "edit",
            "engine/graph",
            "--patch",
            "{nope",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], "PROJECTION_EDIT_INVALID_JSON", "got: {env}");
}

/// A ledger-excluded artifact is not an uncovered FINDING — the verdict and
/// the findings store obey the exclusion the way the report body already
/// did. Before the 2026-08-28 fix the second verify below still recorded
/// `uncovered: 1` (the excluded artifact) while rendering its rationale
/// right beside the count.
#[test]
fn verify_does_not_record_a_ledger_excluded_artifact_as_uncovered() {
    let tmp = advance_workspace();
    let root = tmp.path();
    // The folder mount the workspace declares, with the binding's pinned
    // schema, so verify can boot the destination mem.
    write_store(
        root,
        "../engine-mem/.memstead/config.json",
        r#"{ "schema": "default@1.0.0" }"#,
    );
    memstead()
        .current_dir(root)
        .args(["projection", "enable", "verify", "engine/graph"])
        .assert()
        .success();

    // First verify: a.rs is in scope with no anchor — uncovered.
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).expect("verify --json must emit JSON");
    let uncovered_before = env["report"]["findings_by_class"]["uncovered"]
        .as_u64()
        .unwrap_or(0);
    assert!(uncovered_before >= 1, "precondition: a.rs uncovered: {env}");

    // Exclude it with a rationale.
    memstead()
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
        .success();

    // Second verify: the exclusion gates the recording, not only the report
    // decoration — the uncovered count drops by one and the rationale shows.
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).expect("verify --json must emit JSON");
    let uncovered_after = env["report"]["findings_by_class"]["uncovered"]
        .as_u64()
        .unwrap_or(0);
    assert_eq!(
        uncovered_after,
        uncovered_before - 1,
        "the excluded artifact must not be recorded as uncovered: {env}"
    );
    assert_eq!(
        env["report"]["disposed_excluded"], 1,
        "rationale kept: {env}"
    );
}

// ---------------------------------------------------------------------------
// Binding intent against the destination vocabulary
// ---------------------------------------------------------------------------

/// A folder-mount workspace whose `engine` mem pins the built-in `software`
/// schema, with one git codebase binding `engine/graph` carrying `intent`.
/// The shape every intent test runs against: a real schema, so the vocabulary
/// the rule reads is the one the engine loads, never a fixture list.
fn intent_workspace(intent: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_store(
        root,
        "workspace.toml",
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    );
    write_store(
        root,
        "state/mounts.json",
        r#"{"format":"memstead-mounts-3","mounts":[{"mem":"engine","schema":"software@0.5.0","storage":{"type":"folder","path":"engine-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}]}"#,
    );
    write_store(
        root,
        "projections/engine/graph.json",
        &format!(
            r#"{{"version":2,"intent":{},"sources":[{{"name":"source-tree","type":"codebase","pointer":"src","change_detection":"git","scope":[{{"path":"**/*.rs","mode":"allow"}}]}}],"reference_mems":[],"destination_mem":"engine","deny_paths":[],"coverage_semantics":"exhaustive","operations":{{"build":{{"mode":"discovery","trigger":"loop","batch_size":20}},"sync":{{"trigger":"manual","batch_size":20}},"verify":{{"trigger":"manual","batch_size":20,"adjudication_cap":50,"full_resync_every":20}}}}}}"#,
            serde_json::to_string(intent).unwrap()
        ),
    );
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q"]);
    std::fs::write(src.join("a.rs"), "one").unwrap();
    git(&src, &["add", "a.rs"]);
    git(&src, &["commit", "-qm", "base"]);
    let mem_meta = root.join("engine-mem").join(".memstead");
    std::fs::create_dir_all(&mem_meta).unwrap();
    std::fs::write(
        mem_meta.join("config.json"),
        r#"{"format":1,"schema":"software@0.5.0"}"#,
    )
    .unwrap();
    tmp
}

const INTENT_CODE: &str = "BINDING_INTENT_UNKNOWN_RELATIONSHIP";

/// A stored binding whose intent names `PROVIDED_BY` — absent from the
/// software vocabulary — keeps loading: the build brief, the verify brief and
/// the verify report each carry the typed finding naming the token and the
/// schema, and an edit that leaves the intent alone still lands. Only a
/// write of an intent that names the token refuses: `projection edit` with a
/// patch that sets one, and `projection init --intent`, both with the same
/// code and nothing written.
#[test]
fn intent_unknown_relationship_is_reported_on_load_and_refused_on_write() {
    let tmp = intent_workspace("Every crate is PROVIDED_BY its workspace; it DEPENDS_ON others.");
    let root = tmp.path();

    // (1) The build brief carries the finding next to the intent.
    let out = memstead()
        .current_dir(root)
        .args(["projection", "brief", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let md = String::from_utf8(out).unwrap();
    assert!(
        md.contains(INTENT_CODE),
        "build brief carries the code:\n{md}"
    );
    assert!(md.contains("`PROVIDED_BY`"), "names the token:\n{md}");
    assert!(md.contains("software@0.5.0"), "names the schema:\n{md}");
    assert!(md.contains("`DEPENDS_ON`"), "names the vocabulary:\n{md}");

    // (2) The verify brief carries it too.
    let out = memstead()
        .current_dir(root)
        .args(["projection", "brief", "engine/graph", "--verify"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let md = String::from_utf8(out).unwrap();
    assert!(
        md.contains(INTENT_CODE),
        "verify brief carries the code:\n{md}"
    );

    // (3) The verify report: a completed run (never a refusal) with the
    //     typed finding in the report, JSON and markdown alike.
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    let findings = env["report"]["intent_findings"]
        .as_array()
        .expect("intent_findings array");
    assert_eq!(findings.len(), 1, "one unknown token: {env}");
    assert_eq!(findings[0]["code"], INTENT_CODE);
    assert_eq!(findings[0]["token"], "PROVIDED_BY");
    assert_eq!(findings[0]["schema"], "software@0.5.0");
    assert!(
        findings[0]["vocabulary"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "DEPENDS_ON"),
        "the vocabulary rides the finding: {env}"
    );
    let report_md = env["report_markdown"].as_str().unwrap();
    assert!(
        report_md.contains("## Binding intent"),
        "rendered section:\n{report_md}"
    );
    assert!(
        report_md.contains(INTENT_CODE),
        "rendered code:\n{report_md}"
    );

    // (4) An edit that leaves the intent alone lands on the old record.
    let before = scaffold_bytes(root, "engine", "graph");
    memstead()
        .current_dir(root)
        .args([
            "projection",
            "edit",
            "engine/graph",
            "--patch",
            r#"{"deny_paths":["vendor/**"]}"#,
        ])
        .assert()
        .success();
    assert_ne!(before, scaffold_bytes(root, "engine", "graph"));

    // (5) An edit that SETS an intent naming the token refuses, byte-stable.
    let before = scaffold_bytes(root, "engine", "graph");
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "edit",
            "engine/graph",
            "--patch",
            r#"{"intent":"Still PROVIDED_BY the workspace."}"#,
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], INTENT_CODE, "{env}");
    assert_eq!(env["details"]["tokens"], serde_json::json!(["PROVIDED_BY"]));
    assert_eq!(env["details"]["schema"], "software@0.5.0");
    assert_eq!(
        before,
        scaffold_bytes(root, "engine", "graph"),
        "refused edit writes nothing"
    );

    // (6) `init --intent` naming the token refuses with the same code and
    //     scaffolds nothing.
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "init",
            "--mem",
            "engine",
            "--source",
            "src",
            "--medium-type",
            "codebase",
            "--name",
            "other",
            "--intent",
            "Modules are PROVIDED_BY crates.",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(env["code"], INTENT_CODE, "{env}");
    assert!(
        !root
            .join(".memstead/projections/engine/other.json")
            .exists(),
        "refused init scaffolds nothing"
    );

    // (7) A clean intent edits through, and the brief goes quiet.
    memstead()
        .current_dir(root)
        .args([
            "projection",
            "edit",
            "engine/graph",
            "--patch",
            r#"{"intent":"Every crate DEPENDS_ON others; read CLAUDE.md first."}"#,
        ])
        .assert()
        .success();
    let out = memstead()
        .current_dir(root)
        .args(["projection", "brief", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let md = String::from_utf8(out).unwrap();
    assert!(
        !md.contains(INTENT_CODE),
        "a clean intent renders no finding:\n{md}"
    );
}

/// The silence half: an intent naming only vocabulary relationships (plus a
/// file name and a prose acronym) renders no finding on the brief and an
/// empty `intent_findings` on the verify report, and `init --intent` with
/// such text scaffolds.
#[test]
fn intent_naming_only_the_vocabulary_is_silent() {
    let tmp = intent_workspace(
        "Rust source. A crate DEPENDS_ON its neighbours and USES the CLI over JSON; \
         CLAUDE.md carries dev notes.",
    );
    let root = tmp.path();

    let out = memstead()
        .current_dir(root)
        .args(["projection", "brief", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let md = String::from_utf8(out).unwrap();
    assert!(
        !md.contains(INTENT_CODE),
        "no finding on a clean brief:\n{md}"
    );

    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/graph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        env["report"]["intent_findings"],
        serde_json::json!([]),
        "{env}"
    );
    assert!(
        !env["report_markdown"]
            .as_str()
            .unwrap()
            .contains("## Binding intent"),
        "a clean report carries no intent section"
    );

    memstead()
        .current_dir(root)
        .args([
            "projection",
            "init",
            "--mem",
            "engine",
            "--source",
            "src",
            "--medium-type",
            "codebase",
            "--name",
            "other",
            "--intent",
            "Modules DEPENDS_ON crates; see DATABASE.md.",
        ])
        .assert()
        .success();
    assert!(
        root.join(".memstead/projections/engine/other.json")
            .exists()
    );
}
