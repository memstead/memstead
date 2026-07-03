#![cfg(feature = "mem-repo")]
// `memstead workspace ...` config commands ship only in the full build.

//! Integration tests for the `memstead workspace` write-side subcommand
//! family.
//!
//! Full flavour only — lean CLIs don't expose `memstead workspace`. Each
//! test seeds a fresh workspace (the minimum-viable `workspace.toml`
//! that `memstead mem-repo init` materialises), runs one or more
//! subcommands, and asserts on the resulting TOML and the engine's
//! parse of it.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

const DEFAULT_BODY: &str =
    "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n";

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn seed_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let memstead_dir = tmp.path().join(".memstead");
    fs::create_dir_all(&memstead_dir).unwrap();
    fs::write(memstead_dir.join("workspace.toml"), DEFAULT_BODY).unwrap();
    fs::create_dir_all(tmp.path().join("mem-repo").join(".git")).unwrap();
    tmp
}

fn read_toml(workspace_root: &Path) -> String {
    fs::read_to_string(workspace_root.join(".memstead").join("workspace.toml")).unwrap()
}

#[test]
fn allow_create_writes_rule_then_re_derives_through_cli() {
    let ws = seed_workspace();
    memstead()
        .current_dir(ws.path())
        .args([
            "workspace",
            "allow-create",
            "exec-*",
            "--schema",
            "default@1.0.0",
        ])
        .assert()
        .success();

    let body = read_toml(ws.path());
    assert!(body.contains("[[mem_management.create]]"), "got:\n{body}");
    assert!(body.contains("pattern = \"exec-*\""), "got:\n{body}");
    assert!(
        body.contains("schemas = [\"default@1.0.0\"]"),
        "got:\n{body}"
    );
}

#[test]
fn allow_create_supports_before_flag() {
    let ws = seed_workspace();
    memstead()
        .current_dir(ws.path())
        .args([
            "workspace",
            "allow-create",
            "z-*",
            "--schema",
            "default@1.0.0",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args([
            "workspace",
            "allow-create",
            "a-*",
            "--schema",
            "default@1.0.0",
            "--before",
            "z-*",
        ])
        .assert()
        .success();

    let body = read_toml(ws.path());
    let a_idx = body.find("pattern = \"a-*\"").expect("a-* missing");
    let z_idx = body.find("pattern = \"z-*\"").expect("z-* missing");
    assert!(a_idx < z_idx, "a-* must precede z-*; got:\n{body}");
}

#[test]
fn allow_create_supports_cross_links_flag() {
    let ws = seed_workspace();
    memstead()
        .current_dir(ws.path())
        .args([
            "workspace",
            "allow-create",
            "exec-*",
            "--schema",
            "default@1.0.0",
            "--cross-link",
            "engine,plugin",
        ])
        .assert()
        .success();

    let body = read_toml(ws.path());
    assert!(body.contains("default_cross_links"), "got:\n{body}");
    assert!(body.contains("\"engine\""), "got:\n{body}");
    assert!(body.contains("\"plugin\""), "got:\n{body}");
}

#[test]
fn revoke_create_removes_rule() {
    let ws = seed_workspace();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "allow-create", "exec-*", "--schema", "default"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "revoke-create", "exec-*"])
        .assert()
        .success();
    let body = read_toml(ws.path());
    assert!(!body.contains("pattern = \"exec-*\""), "got:\n{body}");
}

#[test]
fn allow_delete_then_revoke_delete_roundtrip() {
    let ws = seed_workspace();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "allow-delete", "exec-*"])
        .assert()
        .success();
    let body = read_toml(ws.path());
    assert!(body.contains("[[mem_management.delete]]"), "got:\n{body}");
    assert!(body.contains("pattern = \"exec-*\""), "got:\n{body}");
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "revoke-delete", "exec-*"])
        .assert()
        .success();
    let body = read_toml(ws.path());
    assert!(!body.contains("pattern = \"exec-*\""), "got:\n{body}");
}

#[test]
fn grant_cross_link_named_and_wildcard() {
    let ws = seed_workspace();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "grant-cross-link", "plugin", "engine"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "grant-cross-link", "specs", "*"])
        .assert()
        .success();
    let body = read_toml(ws.path());
    assert!(body.contains("[cross_mem_links]"), "got:\n{body}");
    assert!(body.contains("plugin = [\"engine\"]"), "got:\n{body}");
    assert!(body.contains("specs = \"*\""), "got:\n{body}");
}

#[test]
fn revoke_cross_link_drops_key_when_empty() {
    let ws = seed_workspace();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "grant-cross-link", "plugin", "engine"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "revoke-cross-link", "plugin", "engine"])
        .assert()
        .success();
    let body = read_toml(ws.path());
    assert!(!body.contains("plugin = "), "got:\n{body}");
}

#[test]
fn set_mutations_require_notes_toggles() {
    let ws = seed_workspace();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "set-mutations", "--require-notes", "true"])
        .assert()
        .success();
    let body = read_toml(ws.path());
    assert!(body.contains("[mutations]"), "got:\n{body}");
    assert!(body.contains("require_notes = true"), "got:\n{body}");
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "set-mutations", "--require-notes", "false"])
        .assert()
        .success();
    let body = read_toml(ws.path());
    assert!(body.contains("require_notes = false"), "got:\n{body}");
}

/// Duplicate `allow-create`
/// is idempotent. The second invocation exits 0 (script-friendly) but
/// emits a `RULE_ALREADY_PRESENT` notice so the operator sees that
/// the file wasn't touched.
///
/// In markdown-default mode
/// the warning rides on stdout under `## Warnings` to match every
/// other CLI mutation's render shape; in `--json` mode the warning
/// stays on stderr to keep the envelope shape unchanged. Both shapes
/// surface the typed code so an operator can recover from either
/// channel.
#[test]
fn duplicate_pattern_is_idempotent_with_warning() {
    let ws = seed_workspace();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "allow-create", "exec-*", "--schema", "default"])
        .assert()
        .success();
    let assertion = memstead()
        .current_dir(ws.path())
        .args(["workspace", "allow-create", "exec-*", "--schema", "default"])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assertion.get_output().stdout).unwrap();
    assert!(
        stdout.contains("## Warnings"),
        "markdown-default mode must surface a `## Warnings` block; got stdout:\n{stdout}",
    );
    assert!(
        stdout.contains("RULE_ALREADY_PRESENT"),
        "second allow-create must emit RULE_ALREADY_PRESENT under `## Warnings`; got stdout:\n{stdout}",
    );

    // `--json` mode keeps the warning on stderr so the envelope shape
    // (`{action, detail}`) stays untouched.
    let json_assertion = memstead()
        .current_dir(ws.path())
        .args([
            "--json",
            "workspace",
            "allow-create",
            "exec-*",
            "--schema",
            "default",
        ])
        .assert()
        .success();
    let json_stderr = std::str::from_utf8(&json_assertion.get_output().stderr).unwrap();
    assert!(
        json_stderr.contains("RULE_ALREADY_PRESENT"),
        "--json mode must keep the warning on stderr; got stderr:\n{json_stderr}",
    );
}

#[test]
fn missing_workspace_returns_workspace_not_initialised_code() {
    let tmp = TempDir::new().unwrap();
    let output = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "workspace",
            "allow-create",
            "exec-*",
            "--schema",
            "default",
        ])
        .assert()
        .failure()
        .get_output()
        // Under `--json` the error envelope rides stdout.
        .stdout
        .clone();
    let body = std::str::from_utf8(&output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(body.trim()).expect("JSON envelope");
    assert_eq!(parsed["code"], "WORKSPACE_NOT_INITIALISED");
}

/// `memstead workspace show` renders the active workspace
/// configuration in markdown by default; `--json` emits a structured
/// document covering mem_management, cross_mem_links, mutations,
/// and plugin sections.
#[test]
fn workspace_show_renders_markdown_by_default() {
    let ws = seed_workspace();
    memstead()
        .current_dir(ws.path())
        .args([
            "workspace",
            "allow-create",
            "exec-*",
            "--schema",
            "default@1.0.0",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "grant-cross-link", "plugin", "engine"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "set-mutations", "--require-notes", "true"])
        .assert()
        .success();

    let output = memstead()
        .current_dir(ws.path())
        .args(["workspace", "show"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body = String::from_utf8(output).unwrap();
    assert!(body.contains("# Workspace configuration"), "got:\n{body}");
    assert!(body.contains("Mem management"), "got:\n{body}");
    assert!(body.contains("`exec-*`"), "got:\n{body}");
    assert!(body.contains("Cross-mem links"), "got:\n{body}");
    assert!(body.contains("`plugin`"), "got:\n{body}");
    assert!(body.contains("Mutations"), "got:\n{body}");
    assert!(body.contains("`require_notes`: `true`"), "got:\n{body}");
}

#[test]
fn workspace_show_json_includes_all_sections() {
    let ws = seed_workspace();
    memstead()
        .current_dir(ws.path())
        .args([
            "workspace",
            "allow-create",
            "exec-*",
            "--schema",
            "default@1.0.0",
            "--cross-link",
            "engine",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "allow-delete", "exec-*"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "grant-cross-link", "plugin", "engine"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "grant-cross-link", "specs", "*"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "set-mutations", "--require-notes", "false"])
        .assert()
        .success();

    let output = memstead()
        .current_dir(ws.path())
        .args(["--json", "workspace", "show"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    assert!(parsed["workspace_root"].is_string());

    let create = &parsed["mem_management"]["create"];
    assert!(create.is_array());
    assert_eq!(create[0]["pattern"], "exec-*");
    assert_eq!(create[0]["schemas"][0], "default@1.0.0");
    assert_eq!(create[0]["default_cross_links"][0], "engine");

    let delete = &parsed["mem_management"]["delete"];
    assert_eq!(delete[0]["pattern"], "exec-*");

    let links = &parsed["cross_mem_links"];
    assert_eq!(links["plugin"][0], "engine");
    assert_eq!(links["specs"], "*");

    assert_eq!(parsed["mutations"]["require_notes"], false);
}

#[test]
fn workspace_show_empty_workspace_reports_unset_sections() {
    let ws = seed_workspace();
    let output = memstead()
        .current_dir(ws.path())
        .args(["workspace", "show"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body = String::from_utf8(output).unwrap();
    assert!(
        body.contains("no agent-driven mem creation allowed"),
        "got:\n{body}"
    );
    assert!(body.contains("default-deny"), "got:\n{body}");
    assert!(body.contains("`require_notes`: (unset"), "got:\n{body}");
}

/// The engine-semantically-owned sections of the workspace config can
/// be re-derived structurally end-to-end via the CLI:
///
/// > The engine-semantically-owned sections of `memstead/.memstead/workspace.toml`
/// > (`[[mem_management.*]]`, `[cross_mem_links]`, `[mutations]`,
/// > `schemas_dir`, `format`, `[persistence_adapter]`) can be re-derived
/// > structurally end-to-end by running the CLI commands in sequence
/// > against a fresh init.
///
/// This test stitches the seven commands together and re-derives the
/// shape that `memstead/.memstead/workspace.toml` would take, then loads the
/// result through `FileWorkspaceStore` to confirm the engine accepts
/// it. `[plugin.*]` is intentionally out of scope.
#[test]
fn end_to_end_rederive_loads_through_engine() {
    use memstead_base::{FileWorkspaceStore, WorkspaceStoreAdapter};
    let ws = seed_workspace();

    memstead()
        .current_dir(ws.path())
        .args([
            "workspace",
            "allow-create",
            "planning/plan-*",
            "--schema",
            "planning@0.1.0",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "allow-delete", "planning/plan-*"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args([
            "workspace",
            "allow-create",
            "exec-*",
            "--schema",
            "default@1.0.0",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "allow-delete", "exec-*"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "grant-cross-link", "plugin", "engine"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "grant-cross-link", "macos", "engine"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args(["workspace", "set-mutations", "--require-notes", "true"])
        .assert()
        .success();

    let workspace = FileWorkspaceStore::new()
        .load(ws.path())
        .expect("re-derived workspace.toml must parse through the engine loader");
    assert_eq!(workspace.settings.mem_create_rules.len(), 2);
    assert_eq!(workspace.settings.mem_delete_rules.len(), 2);
    assert!(
        workspace
            .settings
            .mem_create_rules
            .iter()
            .any(|r| r.pattern == "planning/plan-*")
    );
    assert!(workspace.settings.cross_mem_links.contains_key("plugin"));
    assert!(workspace.settings.cross_mem_links.contains_key("macos"));
    assert_eq!(workspace.settings.mutations.require_notes, Some(true));
}

/// Markdown-default mode
/// renders a heading + bullet block matching every other CLI
/// mutation's shape (no more `workspace: <action> — <JSON>`
/// single-line shape). The pattern, schemas, position, and cross-link
/// defaults all surface as bullets.
#[test]
fn allow_create_markdown_default_renders_block_not_inline_json() {
    let ws = seed_workspace();
    let assertion = memstead()
        .current_dir(ws.path())
        .args([
            "workspace",
            "allow-create",
            "exec-*",
            "--schema",
            "default@1.0.0",
        ])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assertion.get_output().stdout).unwrap();
    assert!(
        !stdout.contains("workspace: allow-create —"),
        "pre-fix single-line `workspace: action — JSON` shape must be gone; got:\n{stdout}",
    );
    assert!(
        stdout.contains("# Workspace allow-create rule `exec-*`"),
        "heading must name the action and the pattern; got:\n{stdout}",
    );
    assert!(
        stdout.contains("- Pattern: `exec-*`"),
        "pattern bullet must surface; got:\n{stdout}",
    );
    assert!(
        stdout.contains("- Schemas: [default@1.0.0]"),
        "schemas bullet must surface; got:\n{stdout}",
    );
    assert!(
        stdout.contains("- Position: appended"),
        "position bullet must surface; got:\n{stdout}",
    );
}

/// `grant-cross-link` follows the
/// same markdown-block shape across all seven subcommands.
#[test]
fn grant_cross_link_markdown_default_renders_block() {
    let ws = seed_workspace();
    let assertion = memstead()
        .current_dir(ws.path())
        .args(["workspace", "grant-cross-link", "test", "other"])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assertion.get_output().stdout).unwrap();
    assert!(
        !stdout.contains("workspace: grant-cross-link —"),
        "pre-fix shape must be gone; got:\n{stdout}",
    );
    assert!(
        stdout.contains("# Workspace grant-cross-link `test` → `other`"),
        "heading must name the grant direction; got:\n{stdout}",
    );
    assert!(
        stdout.contains("- From: `test`") && stdout.contains("- To: `other`"),
        "from/to bullets must surface; got:\n{stdout}",
    );
}

/// The `--json` envelope is unchanged: the `{action, detail}` shape every
/// script consumes today survives.
#[test]
fn allow_create_json_envelope_unchanged_post_fix() {
    let ws = seed_workspace();
    let assertion = memstead()
        .current_dir(ws.path())
        .args([
            "--json",
            "workspace",
            "allow-create",
            "exec-*",
            "--schema",
            "default@1.0.0",
        ])
        .assert()
        .success();
    let stdout = std::str::from_utf8(&assertion.get_output().stdout).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--json must emit valid JSON");
    assert_eq!(parsed["action"], "allow-create");
    assert_eq!(parsed["detail"]["pattern"], "exec-*");
    assert_eq!(parsed["detail"]["schemas"][0], "default@1.0.0");
}
