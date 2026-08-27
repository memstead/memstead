//! Integration tests for the two happy-path commands: `memstead
//! quickstart` (one-command cold start) and `memstead schema new`
//! (schema scaffold). Both run the real binary via `assert_cmd`, so
//! stdin is a pipe — every test exercises the non-TTY contract (no
//! prompts; defaults and typed refusals instead).

use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn stdout_of(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).expect("stdout is UTF-8")
}

fn stderr_of(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).expect("stderr is UTF-8")
}

// ---------------------------------------------------------------------
// quickstart
// ---------------------------------------------------------------------

/// The headline AC: in a fresh empty directory, one command leaves a
/// bootable workspace, a default-schema mem, a seed entity, and MCP
/// wiring; `memstead overview` immediately works. Non-interactive with
/// no `--agent` defaults to Claude Code and says so.
#[test]
fn quickstart_fresh_dir_bootstraps_workspace_seed_and_wiring() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("my-fresh-graph");

    let assert = memstead()
        .args(["quickstart", "--json"])
        .arg(&root)
        .assert()
        .success();
    let payload: serde_json::Value =
        serde_json::from_str(&stdout_of(assert)).expect("quickstart --json emits JSON");

    // Derived name + default schema pin.
    assert_eq!(payload["name"], "my-fresh-graph");
    assert_eq!(payload["schema"], "default@1.3.0");
    // Non-TTY, no --agent: Claude Code default, explicitly flagged.
    assert_eq!(payload["agents_defaulted"], true);
    assert_eq!(payload["agents"][0]["target"], "claude-code");

    // Workspace on disk: marker + config.
    assert!(root.join(".memstead").join("workspace.toml").is_file());
    assert!(root.join(".memstead").join("config.json").is_file());

    // Seed entity exists as a markdown file at the mem root.
    let seed_id = payload["seed_entity"].as_str().expect("seed entity id");
    assert_eq!(seed_id, "my-fresh-graph--welcome-to-memstead");
    assert!(root.join("welcome-to-memstead.md").is_file());

    // `.mcp.json` server entry launches the resolved memstead-mcp.
    let mcp: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".mcp.json")).unwrap()).unwrap();
    let command = mcp["mcpServers"]["memstead"]["command"]
        .as_str()
        .expect("server entry has a command");
    assert!(
        command.contains("memstead-mcp"),
        "command must launch memstead-mcp, got: {command}",
    );

    // Output names the single next action.
    assert!(
        payload["next_action"].as_str().unwrap().contains("Restart"),
        "next action must name the restart, got: {}",
        payload["next_action"],
    );

    // The workspace boots: `memstead overview` works immediately.
    memstead()
        .current_dir(&root)
        .arg("overview")
        .assert()
        .success();
}

/// Tolerance AC: dotfiles and README-grade files don't block, and are
/// never ingested — the graph afterwards contains exactly the seed
/// entity.
#[test]
fn quickstart_tolerates_dotfiles_and_readme_without_ingesting() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(root.join("README"), "my project\n").unwrap();
    std::fs::write(root.join("LICENSE"), "MIT\n").unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();

    memstead().arg("quickstart").arg(&root).assert().success();

    // Pre-existing files untouched.
    assert_eq!(
        std::fs::read_to_string(root.join("README")).unwrap(),
        "my project\n"
    );

    // Exactly one entity — the seed. Nothing was ingested.
    let assert = memstead()
        .current_dir(&root)
        .args(["list", "--json"])
        .assert()
        .success();
    let listed: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
    let hits = listed["hits"]
        .as_array()
        .unwrap_or_else(|| panic!("list --json carries hits[]; got {listed}"));
    assert_eq!(hits.len(), 1, "seed entity only; got {hits:?}");
}

/// A `.md` file — even a README — is a genuine conflict, not a
/// tolerated one: the folder backend would adopt it as an entity, and
/// quickstart never silently ingests user content. The refusal says
/// exactly that.
#[test]
fn quickstart_refuses_markdown_readme_naming_the_ingestion_risk() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("README.md"), "# my project\n").unwrap();

    let err = stderr_of(
        memstead()
            .arg("quickstart")
            .arg(tmp.path())
            .assert()
            .failure(),
    );
    assert!(err.contains("README.md"), "names the file; got: {err}");
    assert!(
        err.contains("adopt"),
        "explains the ingestion risk; got: {err}"
    );
    assert!(
        err.contains("memstead quickstart"),
        "carries the alternative; got: {err}"
    );
    assert!(!tmp.path().join(".memstead").exists(), "no half-init");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("README.md")).unwrap(),
        "# my project\n",
        "the README is untouched",
    );
}

/// Refusal AC: genuinely conflicting content refuses with one typed
/// error naming the conflict and the exact alternative — and the
/// target is left untouched (no half-initialisation).
#[test]
fn quickstart_refuses_conflicting_content_without_half_init() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::write(root.join("main.py"), "print()\n").unwrap();

    let assert = memstead().arg("quickstart").arg(&root).assert().failure();
    let err = stderr_of(assert);
    assert!(err.contains("TARGET_NOT_EMPTY"), "typed code; got: {err}");
    assert!(err.contains("main.py"), "names the conflict; got: {err}");
    assert!(
        err.contains("memstead quickstart"),
        "names the exact alternative; got: {err}"
    );

    // Never half-initialises.
    assert!(!root.join(".memstead").exists());
    assert!(!root.join(".mcp.json").exists());
}

/// Refusal AC: a foreign `.memstead/` (not a workspace) and an ancestor
/// workspace both refuse with typed errors carrying the next command.
#[test]
fn quickstart_refuses_foreign_memstead_dir_and_ancestor_workspace() {
    // Foreign `.memstead/` without workspace.toml.
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
    std::fs::write(tmp.path().join(".memstead").join("junk"), "x").unwrap();
    let err = stderr_of(
        memstead()
            .arg("quickstart")
            .arg(tmp.path())
            .assert()
            .failure(),
    );
    assert!(
        err.contains("FOREIGN_MEMSTEAD_DIR"),
        "typed code; got: {err}"
    );
    assert!(
        err.contains("memstead quickstart"),
        "carries next command; got: {err}"
    );

    // Ancestor workspace: refuse to nest.
    let outer = TempDir::new().unwrap();
    memstead()
        .arg("quickstart")
        .arg(outer.path())
        .assert()
        .success();
    let inner = outer.path().join("inner");
    std::fs::create_dir(&inner).unwrap();
    let err = stderr_of(memstead().arg("quickstart").arg(&inner).assert().failure());
    assert!(
        err.contains("WORKSPACE_ALREADY_EXISTS_ABOVE"),
        "typed code; got: {err}"
    );
    // The alternatives must be viable in a quickstart-created
    // (filesystem, no-allowlist) workspace: work there, or start a
    // separate graph — never `memstead mem init`, which refuses there.
    assert!(
        err.contains("memstead overview"),
        "viable next command; got: {err}"
    );
    assert!(
        err.contains("memstead quickstart"),
        "separate-graph alternative; got: {err}"
    );
    assert!(
        !err.contains("mem init"),
        "no dead-end suggestion; got: {err}"
    );
    assert!(
        !inner.join(".memstead").exists(),
        "no half-init in the nested target"
    );

    // Re-run on the finished workspace: refuse, point at overview.
    let err = stderr_of(
        memstead()
            .arg("quickstart")
            .arg(outer.path())
            .assert()
            .failure(),
    );
    assert!(
        err.contains("WORKSPACE_ALREADY_INITIALISED"),
        "typed code; got: {err}"
    );
    assert!(
        err.contains("memstead overview"),
        "carries next command; got: {err}"
    );
}

/// Wiring AC: an existing `.mcp.json` server entry is never
/// overwritten, and foreign entries in the same file survive the merge.
#[test]
fn quickstart_never_overwrites_existing_mcp_server_entry() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::write(
        root.join(".mcp.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "mcpServers": {
                "memstead": { "command": "/custom/memstead-mcp", "args": ["--flag"] },
                "other": { "command": "/bin/other" },
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let assert = memstead()
        .args(["quickstart", "--json"])
        .arg(&root)
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
    assert!(
        payload["agents"][0]["action"]
            .as_str()
            .unwrap()
            .contains("left untouched"),
        "report says the entry was left alone; got {payload}",
    );

    let mcp: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(
        mcp["mcpServers"]["memstead"]["command"],
        "/custom/memstead-mcp"
    );
    assert_eq!(mcp["mcpServers"]["memstead"]["args"][0], "--flag");
    assert_eq!(mcp["mcpServers"]["other"]["command"], "/bin/other");
}

/// `--agent` selects targets without any prompt: Cursor and Gemini get
/// project config files, Codex gets the `codex mcp add` command line.
#[test]
fn quickstart_agent_flags_wire_cursor_gemini_codex() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let assert = memstead()
        .args([
            "quickstart",
            "--json",
            "--agent",
            "cursor",
            "--agent",
            "gemini",
            "--agent",
            "codex",
        ])
        .arg(&root)
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
    assert_eq!(payload["agents_defaulted"], false);

    let cursor: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".cursor/mcp.json")).unwrap()).unwrap();
    assert!(cursor["mcpServers"]["memstead"]["command"].is_string());
    let gemini: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".gemini/settings.json")).unwrap())
            .unwrap();
    assert!(gemini["mcpServers"]["memstead"]["command"].is_string());
    // Codex: command printed, nothing written.
    let codex_action = payload["agents"][2]["action"].as_str().unwrap();
    assert!(
        codex_action.contains("codex mcp add memstead --"),
        "got: {codex_action}"
    );
    assert!(!root.join(".codex").exists());
    // No Claude Code wiring — it was not selected.
    assert!(!root.join(".mcp.json").exists());
}

/// Non-TTY with an underivable directory name refuses with the exact
/// `--name` command instead of prompting; `--name` bypasses derivation.
#[test]
fn quickstart_underivable_name_refuses_with_flag_command_non_tty() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("日本語");
    std::fs::create_dir(&root).unwrap();

    let err = stderr_of(memstead().arg("quickstart").arg(&root).assert().failure());
    assert!(err.contains("--name"), "refusal names the flag; got: {err}");
    assert!(
        err.contains("memstead quickstart --name"),
        "exact command; got: {err}"
    );
    assert!(!root.join(".memstead").exists(), "no half-init");

    memstead()
        .args(["quickstart", "--name", "nihongo"])
        .arg(&root)
        .assert()
        .success();
    assert!(root.join(".memstead").join("workspace.toml").is_file());
}

// ---------------------------------------------------------------------
// schema new
// ---------------------------------------------------------------------

/// Scaffold AC: the generated package passes `schema validate`
/// unmodified, and the output prints the three follow-up commands.
#[test]
fn schema_new_scaffold_validates_unmodified() {
    let tmp = TempDir::new().unwrap();
    let out = stdout_of(
        memstead()
            .current_dir(tmp.path())
            .args(["schema", "new", "acme"])
            .assert()
            .success(),
    );
    assert!(out.contains("memstead schema validate acme"), "got: {out}");
    #[cfg(feature = "mem-repo")]
    assert!(out.contains("memstead schema install acme"), "got: {out}");
    #[cfg(not(feature = "mem-repo"))]
    assert!(
        out.contains("memstead schema install ../acme"),
        "got: {out}"
    );
    assert!(
        out.contains("acme@0.1.0"),
        "pin step names the version; got: {out}",
    );

    assert!(tmp.path().join("acme/schema.yaml").is_file());
    assert!(tmp.path().join("acme/types/note.yaml").is_file());
    memstead()
        .current_dir(tmp.path())
        .args(["schema", "validate", "acme"])
        .assert()
        .success();
}

/// Follow-up AC: the printed three-command sequence, executed verbatim
/// from a workspace, ends with the mem pinned to `acme@0.1.0` and
/// accepting a `memstead create --type note`. (`mem set-schema` lives
/// in the mem-repo-featured binary; the lean flavour covers the
/// scaffold/validate/install prefix in the test above and below.)
#[cfg(feature = "mem-repo")]
#[test]
fn schema_new_follow_up_commands_end_in_pinned_mem_accepting_create() {
    let tmp = TempDir::new().unwrap();
    // The mem name is path-derived — the directory basename is the
    // authoritative identity, so it must match `--name`.
    let ws = tmp.path().join("myws");
    memstead()
        .args(["init", "--name", "myws", "--schema", "default@1.0.0"])
        .arg(&ws)
        .assert()
        .success();

    // Step 0: scaffold inside the workspace (where the printed steps
    // resolve the real mem name).
    let out = stdout_of(
        memstead()
            .current_dir(&ws)
            .args(["schema", "new", "acme"])
            .assert()
            .success(),
    );
    assert!(
        out.contains("memstead mem set-schema myws acme@0.1.0"),
        "pin step names the workspace's mem; got: {out}",
    );
    assert!(
        !out.contains("memstead delete"),
        "no seed in an init workspace, so no delete step; got: {out}",
    );

    // Steps 1-3 verbatim.
    memstead()
        .current_dir(&ws)
        .args(["schema", "validate", "acme"])
        .assert()
        .success();
    memstead()
        .current_dir(&ws)
        .args(["schema", "install", "acme"])
        .assert()
        .success();
    let pin_out = stdout_of(
        memstead()
            .current_dir(&ws)
            .args(["mem", "set-schema", "myws", "acme@0.1.0"])
            .assert()
            .success(),
    );
    assert!(
        pin_out.contains("Switched"),
        "empty mem switches atomically; got: {pin_out}"
    );

    // The pinned mem accepts the scaffolded example type.
    memstead()
        .current_dir(&ws)
        .args([
            "create",
            "--type",
            "note",
            "--title",
            "First note",
            "--section",
            "summary=It works.",
        ])
        .assert()
        .success();
}

/// The newcomer path end-to-end: from a *quickstart* workspace (which
/// carries the seed entity), the printed follow-up includes a delete
/// step for the seed, and the printed commands executed verbatim end
/// with the mem atomically pinned (`Switched`, not a dual-pin
/// migration) and accepting the scaffolded type.
#[cfg(feature = "mem-repo")]
#[test]
fn schema_new_follow_up_from_quickstart_workspace_ends_pinned() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("my-graph");
    memstead().arg("quickstart").arg(&ws).assert().success();

    let out = stdout_of(
        memstead()
            .current_dir(&ws)
            .args(["schema", "new", "acme"])
            .assert()
            .success(),
    );
    let seed_id = "my-graph--welcome-to-memstead";
    assert!(
        out.contains(&format!("memstead delete {seed_id}")),
        "follow-up includes the seed delete step; got: {out}",
    );
    assert!(
        out.contains("memstead mem set-schema my-graph acme@0.1.0"),
        "pin step names the quickstart mem; got: {out}",
    );

    // The printed commands, verbatim.
    memstead()
        .current_dir(&ws)
        .args(["schema", "validate", "acme"])
        .assert()
        .success();
    memstead()
        .current_dir(&ws)
        .args(["schema", "install", "acme"])
        .assert()
        .success();
    memstead()
        .current_dir(&ws)
        .args(["delete", seed_id])
        .assert()
        .success();
    let pin_out = stdout_of(
        memstead()
            .current_dir(&ws)
            .args(["mem", "set-schema", "my-graph", "acme@0.1.0"])
            .assert()
            .success(),
    );
    assert!(
        pin_out.contains("Switched"),
        "seedless mem switches atomically, no migration; got: {pin_out}",
    );
    memstead()
        .current_dir(&ws)
        .args([
            "create",
            "--type",
            "note",
            "--title",
            "First note",
            "--section",
            "summary=It works.",
        ])
        .assert()
        .success();
}

/// `schema install <builtin>@<version>` resolves every RETAINED
/// built-in version: the registry registers all generations, and the
/// collect path scans the suffixed retention directories
/// (`planning-0.3`, …) instead of refusing everything but the
/// name-exact directory's version. An unregistered version still
/// refuses.
#[cfg(feature = "mem-repo")]
#[test]
fn schema_install_resolves_retained_builtin_versions() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("retained");
    memstead().arg("quickstart").arg(&ws).assert().success();

    // planning@0.3.0 lives in the retained `planning-0.3` directory
    // (name-exact `planning/` holds 0.1.0) — resolve + collect must
    // both succeed through the real command.
    memstead()
        .current_dir(&ws)
        .args(["schema", "install", "planning@0.3.0"])
        .assert()
        .success();

    let err = stderr_of(
        memstead()
            .current_dir(&ws)
            .args(["schema", "install", "planning@9.9.9"])
            .assert()
            .failure(),
    );
    assert!(
        err.contains("planning@9.9.9"),
        "unregistered version refuses, naming the pin; got: {err}"
    );
}

/// Preflight AC: a malformed agent config file refuses BEFORE anything
/// is created — the printed "re-run memstead quickstart" must still be
/// able to succeed, so no workspace may exist after the refusal.
#[test]
fn quickstart_malformed_agent_config_refuses_before_any_write() {
    // Invalid JSON.
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join(".mcp.json"), "{not json").unwrap();
    let err = stderr_of(
        memstead()
            .arg("quickstart")
            .arg(tmp.path())
            .assert()
            .failure(),
    );
    assert!(
        err.contains("not valid JSON"),
        "names the defect; got: {err}"
    );
    assert!(
        err.contains("re-run: memstead quickstart"),
        "carries the retry; got: {err}"
    );
    assert!(
        !tmp.path().join(".memstead").exists(),
        "nothing was created"
    );
    // The printed retry actually works once the file is fixed.
    std::fs::remove_file(tmp.path().join(".mcp.json")).unwrap();
    memstead()
        .arg("quickstart")
        .arg(tmp.path())
        .assert()
        .success();

    // `mcpServers` present but not an object.
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join(".mcp.json"), r#"{"mcpServers": []}"#).unwrap();
    let err = stderr_of(
        memstead()
            .arg("quickstart")
            .arg(tmp.path())
            .assert()
            .failure(),
    );
    assert!(err.contains("mcpServers"), "names the defect; got: {err}");
    assert!(
        err.contains("re-run: memstead quickstart"),
        "carries the retry; got: {err}"
    );
    assert!(
        !tmp.path().join(".memstead").exists(),
        "nothing was created"
    );
}

/// Lean-flavour follow-up end-to-end: without `mem set-schema`, the
/// printed sequence routes through a fresh mem — init pins the custom
/// schema, then `schema install ../<name>` from inside the new folder
/// makes the workspace boot. Executed as printed, it ends with a
/// working workspace accepting a `create --type note` (regression: an
/// earlier sequence pinned without installing, leaving a workspace
/// where every engine-booting command died with INTERNAL).
#[cfg(not(feature = "mem-repo"))]
#[test]
fn schema_new_lean_follow_up_ends_in_working_fresh_mem() {
    let tmp = TempDir::new().unwrap();
    let out = stdout_of(
        memstead()
            .current_dir(tmp.path())
            .args(["schema", "new", "acme"])
            .assert()
            .success(),
    );
    assert!(
        out.contains("memstead init --name acme-mem --schema acme@0.1.0"),
        "lean follow-up routes through a fresh init; got: {out}",
    );
    assert!(
        out.contains("memstead schema install ../acme"),
        "install step targets the new workspace; got: {out}",
    );
    assert!(
        !out.contains("mem set-schema"),
        "lean never prints the full-only subcommand; got: {out}",
    );

    // The printed sequence, step by step (`mkdir && cd` become the
    // test's directory handling).
    memstead()
        .current_dir(tmp.path())
        .args(["schema", "validate", "acme"])
        .assert()
        .success();
    let fresh = tmp.path().join("acme-mem");
    std::fs::create_dir(&fresh).unwrap();
    memstead()
        .current_dir(&fresh)
        .args(["init", "--name", "acme-mem", "--schema", "acme@0.1.0"])
        .assert()
        .success();
    memstead()
        .current_dir(&fresh)
        .args(["schema", "install", "../acme"])
        .assert()
        .success();

    // The workspace boots and the scaffolded type is writable.
    memstead()
        .current_dir(&fresh)
        .arg("overview")
        .assert()
        .success();
    memstead()
        .current_dir(&fresh)
        .args([
            "create",
            "--type",
            "note",
            "--title",
            "First note",
            "--section",
            "summary=It works.",
        ])
        .assert()
        .success();
}

/// Lean follow-up scaffolded from INSIDE an existing workspace: the
/// printed fresh-mem path must land outside it (workspaces don't nest,
/// and the lean binary has no `memstead mem init` to fall back on).
/// The test executes the paths exactly as printed and ends in a
/// working mem.
#[cfg(not(feature = "mem-repo"))]
#[test]
fn schema_new_lean_follow_up_from_inside_workspace_lands_outside() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("my-graph");
    memstead().arg("quickstart").arg(&ws).assert().success();

    let out = stdout_of(
        memstead()
            .current_dir(&ws)
            .args(["schema", "new", "acme"])
            .assert()
            .success(),
    );

    // Pull the two printed paths: the fresh-mem dir from the init step,
    // the package path from the install step. Both are quoted absolute
    // paths in the in-workspace variant.
    let quoted = |line_marker: &str| -> std::path::PathBuf {
        let line = out
            .lines()
            .find(|l| l.contains(line_marker))
            .unwrap_or_else(|| panic!("no step containing `{line_marker}`; got: {out}"));
        let start = line
            .find('"')
            .unwrap_or_else(|| panic!("no quoted path in: {line}"));
        let rest = &line[start + 1..];
        let end = rest
            .find('"')
            .unwrap_or_else(|| panic!("unterminated quote in: {line}"));
        std::path::PathBuf::from(&rest[..end])
    };
    let fresh = quoted("memstead init --name acme-mem");
    let pkg = quoted("memstead schema install");

    // The fresh mem lands outside the workspace.
    let ws_canon = std::fs::canonicalize(&ws).unwrap();
    assert!(
        !fresh.starts_with(&ws_canon) && !fresh.starts_with(&ws),
        "fresh-mem dir {} must not nest inside the workspace {}",
        fresh.display(),
        ws.display(),
    );

    // Execute as printed: mkdir + init in the fresh dir, install the
    // package by its printed path, and the workspace works.
    std::fs::create_dir_all(&fresh).unwrap();
    memstead()
        .current_dir(&fresh)
        .args(["init", "--name", "acme-mem", "--schema", "acme@0.1.0"])
        .assert()
        .success();
    memstead()
        .current_dir(&fresh)
        .args(["schema", "install"])
        .arg(&pkg)
        .assert()
        .success();
    memstead()
        .current_dir(&fresh)
        .arg("overview")
        .assert()
        .success();
    memstead()
        .current_dir(&fresh)
        .args([
            "create",
            "--type",
            "note",
            "--title",
            "First note",
            "--section",
            "summary=It works.",
        ])
        .assert()
        .success();
}

/// `schema install` accepts the scaffolded package on the folder
/// backend regardless of binary flavour (the lean prefix of the
/// follow-up flow).
#[test]
fn schema_new_package_installs_into_folder_workspace() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    memstead()
        .args(["init", "--name", "myws", "--schema", "default@1.0.0"])
        .arg(&ws)
        .assert()
        .success();
    memstead()
        .current_dir(&ws)
        .args(["schema", "new", "acme"])
        .assert()
        .success();
    memstead()
        .current_dir(&ws)
        .args(["schema", "install", "acme"])
        .assert()
        .success();
    assert!(
        ws.join(".memstead/schemas/acme@0.1.0/schema.yaml")
            .is_file()
    );
    assert!(
        ws.join(".memstead/schemas/acme@0.1.0/types/note.yaml")
            .is_file()
    );
}

/// Refusal ACs: an existing package refuses rather than overwriting; an
/// invalid name refuses with the slug rule and a suggested correction.
/// Both messages carry the exact next command.
#[test]
fn schema_new_refusals_carry_next_commands() {
    let tmp = TempDir::new().unwrap();
    memstead()
        .current_dir(tmp.path())
        .args(["schema", "new", "acme"])
        .assert()
        .success();
    let before = std::fs::read_to_string(tmp.path().join("acme/schema.yaml")).unwrap();

    // Existing package: refuse, don't overwrite.
    let err = stderr_of(
        memstead()
            .current_dir(tmp.path())
            .args(["schema", "new", "acme"])
            .assert()
            .failure(),
    );
    assert!(
        err.contains("SCHEMA_PACKAGE_EXISTS"),
        "typed code; got: {err}"
    );
    assert!(
        err.contains("memstead schema validate acme"),
        "next command; got: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("acme/schema.yaml")).unwrap(),
        before,
        "the existing package is untouched",
    );

    // Invalid (non-slug) name: rule + suggestion + exact retry command.
    let err = stderr_of(
        memstead()
            .current_dir(tmp.path())
            .args(["schema", "new", "Acme Corp!"])
            .assert()
            .failure(),
    );
    assert!(err.contains("lowercase"), "states the rule; got: {err}");
    assert!(
        err.contains("memstead schema new acme-corp"),
        "suggested correction as a runnable command; got: {err}",
    );

    // Non-empty non-package directory: refuse, name the finding.
    std::fs::create_dir(tmp.path().join("busy")).unwrap();
    std::fs::write(tmp.path().join("busy/x.txt"), "x").unwrap();
    let err = stderr_of(
        memstead()
            .current_dir(tmp.path())
            .args(["schema", "new", "busy"])
            .assert()
            .failure(),
    );
    assert!(err.contains("TARGET_NOT_EMPTY"), "typed code; got: {err}");
    assert!(err.contains("x.txt"), "names the finding; got: {err}");
}

/// Vocabulary AC helper: the artifacts the two commands generate carry
/// no retired unit noun — checked on the scaffold and the quickstart
/// report (source-level grep is part of the review gate).
#[test]
fn generated_artifacts_speak_mem_vocabulary_only() {
    // The retired unit noun stays retired even in this test's source —
    // assemble it at runtime so a source-level grep stays at zero hits.
    let retired_noun = ["va", "ult"].concat();

    let tmp = TempDir::new().unwrap();
    memstead()
        .current_dir(tmp.path())
        .args(["schema", "new", "acme"])
        .assert()
        .success();
    let scaffold = format!(
        "{}{}",
        std::fs::read_to_string(tmp.path().join("acme/schema.yaml")).unwrap(),
        std::fs::read_to_string(tmp.path().join("acme/types/note.yaml")).unwrap(),
    );
    assert!(
        !scaffold.to_lowercase().contains(&retired_noun),
        "scaffold speaks mem only"
    );

    let root = tmp.path().join("qs");
    let out = stdout_of(memstead().arg("quickstart").arg(&root).assert().success());
    assert!(
        !out.to_lowercase().contains(&retired_noun),
        "quickstart report speaks mem only"
    );
}

/// Errors-as-tutorial sweep: every refusal reachable on the two paths
/// prints an exact next command (a `memstead …` or `codex …`
/// invocation), not just a reason.
#[test]
fn every_refusal_on_these_paths_names_a_next_command() {
    let tmp = TempDir::new().unwrap();

    // quickstart refusals.
    let dirty = tmp.path().join("dirty");
    std::fs::create_dir(&dirty).unwrap();
    std::fs::write(dirty.join("code.rs"), "x").unwrap();
    let cases: Vec<String> = vec![
        // Conflicting content.
        stderr_of(memstead().arg("quickstart").arg(&dirty).assert().failure()),
        // Underivable name (non-TTY).
        {
            let weird = tmp.path().join("统一");
            std::fs::create_dir(&weird).unwrap();
            stderr_of(memstead().arg("quickstart").arg(&weird).assert().failure())
        },
        // schema new: existing package.
        {
            memstead()
                .current_dir(tmp.path())
                .args(["schema", "new", "acme"])
                .assert()
                .success();
            stderr_of(
                memstead()
                    .current_dir(tmp.path())
                    .args(["schema", "new", "acme"])
                    .assert()
                    .failure(),
            )
        },
        // schema new: invalid name.
        stderr_of(
            memstead()
                .current_dir(tmp.path())
                .args(["schema", "new", "BAD NAME"])
                .assert()
                .failure(),
        ),
    ];
    for (i, err) in cases.iter().enumerate() {
        assert!(
            err.contains("memstead "),
            "refusal #{i} must include an exact next command; got: {err}",
        );
    }
}

// ---------------------------------------------------------------------
// workspace-shape disclosure
// ---------------------------------------------------------------------

/// Every assertion the shape disclosure has to satisfy, in one place:
/// which shape, one concrete thing it cannot do, and the way to the
/// other shape. Applied to `quickstart` and `init` alike.
///
/// The "cannot" half is flavour-specific on purpose. The full build
/// names `memstead install` and the typed code it refuses with, because
/// that command exists there. The lean build has no `install`
/// subcommand at all, so it states the limit without borrowing a verb
/// the reader could not run — see the FILESYSTEM_CANNOT gate in
/// `setup.rs`. Both must still name `memstead mem-repo init`, which is
/// a pointer at the other shape, not an invitation to run it here.
fn assert_filesystem_shape_disclosure(out: &str, ctx: &str) {
    let mut needles = vec![
        "filesystem-mem",
        "cannot install mems from the registry",
        "memstead mem-repo init",
    ];
    if cfg!(feature = "mem-repo") {
        needles.push("memstead install");
        needles.push("UNSUPPORTED_WORKSPACE_SHAPE");
    } else {
        needles.push("this lean build does not carry them");
    }
    for needle in needles {
        assert!(
            out.contains(needle),
            "{ctx}: shape disclosure must name `{needle}`; got:\n{out}",
        );
    }
}

/// The fork `quickstart` decides silently is stated in the receipt the
/// newcomer is already reading — not discovered later by being refused.
#[test]
fn quickstart_receipt_discloses_the_shape_it_picked() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("disclosed-graph");
    let out = stdout_of(
        memstead()
            .args(["quickstart", "--agent", "claude-code"])
            .arg(&root)
            .assert()
            .success(),
    );
    assert_filesystem_shape_disclosure(&out, "quickstart receipt");
}

/// `memstead init` picks the same fork and discloses it the same way.
#[test]
fn init_receipt_discloses_the_shape_it_picked() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("strict-graph");
    let out = stdout_of(
        memstead()
            .args([
                "init",
                "--name",
                "strict-graph",
                "--schema",
                "default@1.3.0",
            ])
            .arg(&root)
            .assert()
            .success(),
    );
    assert_filesystem_shape_disclosure(&out, "init receipt");
}

/// Symmetry: the mem-repo verb reports its shape too, so the
/// disclosure reads as a fork rather than as a warning bolted onto one
/// branch. It names what mem-repo costs and the command for the other
/// shape.
#[cfg(feature = "mem-repo")]
#[test]
fn mem_repo_init_discloses_its_shape_symmetrically() {
    let tmp = TempDir::new().unwrap();
    let out = stdout_of(
        memstead()
            .args(["mem-repo", "init"])
            .arg(tmp.path())
            .assert()
            .success(),
    );
    for needle in ["mem-repo", "git", "memstead quickstart"] {
        assert!(
            out.contains(needle),
            "mem-repo init receipt must name `{needle}`; got:\n{out}",
        );
    }
}

/// The `--json` receipt carries the whole disclosure, not just the
/// label. The agent surface is the primary consumer here; a bare
/// `"workspace_shape": "filesystem-mem"` names the fork without
/// disclosing it, which is the failure this block exists to end.
#[test]
fn json_receipts_carry_the_whole_disclosure_not_just_the_label() {
    fn assert_disclosure(payload: &serde_json::Value, want_shape: &str, ctx: &str) {
        let d = &payload["workspace_shape_disclosure"];
        assert_eq!(d["shape"], want_shape, "{ctx}: shape; got {payload}");
        for key in ["summary", "cannot", "other_shape", "other_shape_command"] {
            let v = d[key].as_str().unwrap_or_default();
            assert!(
                !v.is_empty(),
                "{ctx}: `{key}` must be present and non-empty; got {d}",
            );
        }
        assert_ne!(
            d["other_shape"], want_shape,
            "{ctx}: the other shape must differ from this one; got {d}",
        );
    }

    let tmp = TempDir::new().unwrap();

    let assert = memstead()
        .args(["quickstart", "--json", "--agent", "claude-code"])
        .arg(tmp.path().join("json-qs"))
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
    assert_disclosure(&payload, "filesystem-mem", "quickstart --json");

    let assert = memstead()
        .args([
            "init",
            "--json",
            "--name",
            "json-init",
            "--schema",
            "default@1.3.0",
        ])
        .arg(tmp.path().join("json-init"))
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
    assert_disclosure(&payload, "filesystem-mem", "init --json");
}

/// Symmetric machine surface: `mem-repo init --json` carries the same
/// disclosure shape, pointing the other way.
#[cfg(feature = "mem-repo")]
#[test]
fn mem_repo_init_json_carries_the_whole_disclosure() {
    let tmp = TempDir::new().unwrap();
    let assert = memstead()
        .args(["mem-repo", "init", "--json"])
        .arg(tmp.path())
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
    let d = &payload["workspace_shape_disclosure"];
    assert_eq!(d["shape"], "mem-repo", "got {payload}");
    assert_eq!(d["other_shape"], "filesystem-mem", "got {d}");
    assert!(
        d["other_shape_command"]
            .as_str()
            .unwrap_or_default()
            .contains("memstead quickstart"),
        "must name the other shape's command; got {d}",
    );
}

/// `mem-repo init` inside (or at the root of) a git repository emits
/// the source-layout hint — the out-of-root trade-off and the
/// common-parent recipe — at the moment the layout decision is made,
/// and names `.memstead/` as intentionally trackable next to the
/// `.gitignore` append (backlog-sweep plan 06, decisions 14/15).
/// Complement: under no git repo at all, neither line appears.
#[cfg(feature = "mem-repo")]
#[test]
fn mem_repo_init_inside_git_repo_hints_layout_and_trackability() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );

    // Workspace AT the repo root — the recipe's own recommended layout.
    let assert = memstead()
        .args(["mem-repo", "init"])
        .arg(&repo)
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        stderr.contains("common parent directory"),
        "layout hint with the recipe expected on stderr, got:\n{stderr}"
    );
    assert!(
        stderr.contains(".memstead/` is intentionally trackable"),
        "trackability note expected next to the gitignore line, got:\n{stderr}"
    );
    // The append itself reached the repo's own .gitignore (workspace ==
    // repo root — the case the old parent-first walk skipped).
    let gitignore = std::fs::read_to_string(repo.join(".gitignore")).unwrap();
    assert!(gitignore.contains("mem-repo/"), "got:\n{gitignore}");

    // Complement: no git repo anywhere above → no hint, no note.
    let free = tmp.path().join("free-standing");
    let assert = memstead()
        .args(["mem-repo", "init"])
        .arg(&free)
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        !stderr.contains("common parent directory") && !stderr.contains("intentionally trackable"),
        "no hint noise where the shape is unaffected, got:\n{stderr}"
    );
}

/// A verb the receipt names must either exist in the binary that
/// printed it, or be named together with the statement that this build
/// does not carry it. Anything else sends the reader to
/// `unrecognized subcommand`.
///
/// Both halves are live. The lean build's "cannot" clause no longer
/// borrows `memstead install` (it has none), while its pointer at the
/// other shape still names `memstead mem-repo init` — legitimately,
/// because the same sentence says a different build is needed first.
#[test]
fn every_verb_the_receipt_names_is_runnable_or_flagged_as_absent() {
    let tmp = TempDir::new().unwrap();
    let out = stdout_of(
        memstead()
            .args(["quickstart", "--agent", "claude-code"])
            .arg(tmp.path().join("named-cmds"))
            .assert()
            .success(),
    );

    let help = stdout_of(memstead().arg("--help").assert().success());
    let disowned =
        out.contains("this lean build has no") || out.contains("this lean build does not carry");
    for verb in ["install", "mem-repo", "quickstart", "overview", "delete"] {
        if !out.contains(&format!("memstead {verb}")) {
            continue;
        }
        // `--help` lists subcommands one per line, name first.
        let listed = help.lines().any(|l| l.trim_start().starts_with(verb));
        assert!(
            listed || disowned,
            "the receipt names `memstead {verb}`, this build's help does not list it, and the \
             receipt never says the build lacks it \u{2014} the reader would hit `unrecognized \
             subcommand`.\n--- receipt ---\n{out}\n--- help ---\n{help}",
        );
    }
}

/// Disclosure is not permission: a mem-repo-only subcommand on the
/// shape `quickstart` produces still refuses with the same typed code,
/// and the message still names the recovering command.
#[cfg(feature = "mem-repo")]
#[test]
fn mem_repo_only_subcommand_still_refuses_after_disclosure() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("refusing-graph");
    memstead()
        .args(["quickstart", "--agent", "claude-code"])
        .arg(&root)
        .assert()
        .success();

    // `install` / `uninstall` deliberately stopped being shape-gated on
    // 2026-08-27 — a read-mem attaches to the workspace roster, which every
    // shape carries. `mem init` is still genuinely mem-repo-only (a second
    // writable mem needs the multi-mem backend), so it carries the disclosure
    // contract this test exists for.
    let assert = memstead()
        .current_dir(&root)
        .args([
            "mem",
            "init",
            "second",
            "--schema",
            "default@1.0.0",
            "--json",
        ])
        .assert()
        .failure();
    let body = stdout_of(assert);
    let envelope: serde_json::Value =
        serde_json::from_str(body.trim()).expect("--json refusal is JSON");
    assert_eq!(
        envelope["code"], "UNSUPPORTED_WORKSPACE_SHAPE",
        "a mem-repo-only verb must still refuse by shape; got: {envelope}",
    );
    let message = envelope["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("mem-repo"),
        "refusal must still name the recovering shape; got: {message}",
    );
}

/// F5: an agent session that has just run onboarding cannot restart
/// itself, so the receipt names a check that works from inside that
/// session — and still names the restart for what the restart does.
#[test]
fn quickstart_receipt_names_in_session_verification_and_the_restart() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("verifiable-graph");
    let assert = memstead()
        .args(["quickstart", "--json", "--agent", "claude-code"])
        .arg(&root)
        .assert()
        .success();
    let payload: serde_json::Value =
        serde_json::from_str(&stdout_of(assert)).expect("quickstart --json emits JSON");

    let next = payload["next_action"].as_str().unwrap_or_default();
    assert!(
        next.contains("Restart") && next.contains("registers"),
        "the restart must still be named for what it does; got: {next}",
    );

    let verify = payload["verify_now"]
        .as_array()
        .expect("receipt carries in-session verification steps");
    let rendered = verify
        .iter()
        .map(|v| v["command"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("--version"),
        "verification must exercise the binary the wiring points at; got:\n{rendered}",
    );
    assert!(
        rendered.contains("memstead overview"),
        "verification must name a read of the graph itself; got:\n{rendered}",
    );

    // The named check is real: the wired binary answers right now.
    let wired = payload["mcp_command"].as_str().expect("mcp_command");
    let status = std::process::Command::new(wired)
        .arg("--version")
        .status()
        .expect("the wired memstead-mcp binary must be runnable");
    assert!(status.success(), "`{wired} --version` must succeed");

    // …and the markdown receipt says the same thing.
    let root2 = tmp.path().join("verifiable-graph-md");
    let out = stdout_of(
        memstead()
            .args(["quickstart", "--agent", "claude-code"])
            .arg(&root2)
            .assert()
            .success(),
    );
    assert!(
        out.contains("--version") && out.contains("Restart"),
        "markdown receipt must carry both the in-session check and the restart; got:\n{out}",
    );
}

/// Every command the receipt prints must run verbatim, from the
/// directory the reader is standing in — across the awkward cases, on
/// both output surfaces, for every agent target.
///
/// Three earlier versions of this guard passed while printed commands
/// were unrunnable, each time because the test sampled a subset: it
/// checked `verify_now` but not the markdown lines, one agent target
/// but not Codex, a space in the path but not a leading dash, and a
/// `PATH` that always happened to contain `memstead`. So this one
/// *extracts* every backticked command from the markdown receipt and
/// every command-bearing JSON field, and runs the lot.
#[test]
fn the_receipts_printed_commands_run_verbatim_from_the_callers_cwd() {
    // Pull every `backticked` span out of the markdown receipt, keeping
    // the ones that look like commands (they name a binary we ship, or
    // start with `cd `). Extraction rather than enumeration is the
    // point: a new printed command is covered the day it is added.
    fn commands_in_markdown(out: &str) -> Vec<String> {
        out.split('`')
            .skip(1)
            .step_by(2)
            .map(str::trim)
            .filter(|s| {
                // A printed command always carries at least a
                // subcommand or flag. Requiring a space is what keeps
                // prose mentions (the backticked `memstead` in "the
                // `memstead` MCP server") and the seed entity id
                // (`<mem>--welcome-to-memstead`) out — and, unlike a
                // first-word match, it still recognises a command whose
                // program is quoted because its path holds a space.
                s.contains(' ')
                    && (s.starts_with("cd ") || s.starts_with("codex ") || s.contains("memstead"))
            })
            // `memstead quickstart` in recovery hints is a real command
            // but would create a second workspace; the disclosure block's
            // `memstead install <scope>/<name>` is a placeholder, not a
            // literal. Both are covered by their own tests.
            .filter(|s| !s.contains("quickstart") && !s.contains('<'))
            .map(str::to_string)
            .collect()
    }

    // The `PATH` a case runs under. It is applied to BOTH the
    // `quickstart` invocation and the commands its receipt prints —
    // a reader runs both in the same shell, so generating the receipt
    // under one environment and testing it under another would prove
    // nothing about what they see.
    fn path_for(has_memstead: bool) -> String {
        if has_memstead {
            format!(
                "{}:{}",
                Path::new(env!("CARGO_BIN_EXE_memstead"))
                    .parent()
                    .unwrap()
                    .display(),
                std::env::var("PATH").unwrap_or_default(),
            )
        } else {
            "/usr/bin:/bin".to_string()
        }
    }

    fn run(command: &str, cwd: &Path, path_has_memstead: bool) {
        let path = path_for(path_has_memstead);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .env("PATH", path)
            .output()
            .expect("spawn sh");
        assert!(
            out.status.success(),
            "the receipt printed `{command}`, which fails when run as printed \
             (PATH carries memstead: {path_has_memstead}):\n--- stdout ---\n{}\n\
             --- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    // Each case is (directory name, whether `memstead` is on PATH,
    // whether the binary itself sits under a path with a space).
    // The dash-prefixed name is why the `cd` carries `--`; the
    // PATH-less case is why the receipt names the binary it was
    // actually invoked as; the awkward-binary-path case is why that
    // name is quoted wherever it is printed — including in the shape
    // disclosure, which lives in a different module and was the last
    // printed command still interpolating it raw.
    let cases = [
        ("My Graph", true, false),
        ("-dashed-graph", true, false),
        ("bob's graph", true, false),
        ("offpath-graph", false, false),
        ("awkward-binary-graph", false, true),
    ];

    for (dir, on_path, awkward_binary) in cases {
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        std::fs::create_dir_all(&outer).unwrap();

        // Copy the pair into a directory whose name would break any
        // unquoted interpolation, and invoke through that copy.
        let bin = if awkward_binary {
            let dir = tmp.path().join("bob's bin dir");
            std::fs::create_dir_all(&dir).unwrap();
            for name in ["memstead", "memstead-mcp"] {
                let src = Path::new(env!("CARGO_BIN_EXE_memstead"))
                    .parent()
                    .unwrap()
                    .join(name);
                if src.is_file() {
                    std::fs::copy(&src, dir.join(name)).unwrap();
                }
            }
            dir.join("memstead")
        } else {
            Path::new(env!("CARGO_BIN_EXE_memstead")).to_path_buf()
        };

        // `--` so clap does not read `-dashed-graph` as a flag.
        let assert = Command::new(&bin)
            .current_dir(&outer)
            .env("PATH", path_for(on_path))
            .args(["quickstart", "--agent", "claude-code", "--"])
            .arg(dir)
            .assert()
            .success();
        let out = stdout_of(assert);
        // The lean receipt names `mem-repo init` while stating that this
        // build does not carry it — a pointer at another build, not an
        // instruction for here. `every_verb_the_receipt_names_is_runnable_
        // or_flagged_as_absent` is what holds that case honest.
        let disowned = out.contains("this lean build has no")
            || out.contains("this lean build does not carry");
        for command in commands_in_markdown(&out) {
            if disowned && command.contains("mem-repo") {
                continue;
            }
            run(&command, &outer, on_path);
        }
    }

    // The JSON surface, including the Codex target — whose wiring IS a
    // command the reader runs, so it has to survive the same paths.
    let tmp = TempDir::new().unwrap();
    let outer = tmp.path().join("outer");
    std::fs::create_dir_all(&outer).unwrap();
    let assert = memstead()
        .current_dir(&outer)
        .args([
            "quickstart",
            "--json",
            "--agent",
            "claude-code",
            "--agent",
            "codex",
            "--",
            "My Graph",
        ])
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();

    let steps = payload["verify_now"]
        .as_array()
        .expect("verify_now is an array of steps");
    assert!(!steps.is_empty(), "got {payload}");
    for s in steps {
        let c = s["command"].as_str().unwrap_or_default();
        assert!(
            !c.contains('`') && !c.starts_with("- "),
            "machine surface must carry a bare command, got: {c}",
        );
        run(c, &outer, true);
    }
    run(
        payload["seed_entity_delete_command"]
            .as_str()
            .expect("seed delete command"),
        &outer,
        true,
    );

    // `next_action`'s trailing command must be runnable too — it is the
    // most prominent line in the receipt.
    let next = payload["next_action"].as_str().unwrap_or_default();
    let (_, tail) = next
        .rsplit_once("then try: ")
        .unwrap_or_else(|| panic!("next_action names a follow-up command; got: {next}"));
    run(tail, &outer, true);

    // Codex's wiring line is a command too. It is not run (that would
    // need Codex installed), but it must parse into the argv we intend
    // — the bug this catches is an unquoted path splitting into extra
    // words, which `set --` reproduces exactly.
    let codex_action = payload["agents"]
        .as_array()
        .and_then(|a| a.iter().find(|w| w["target"] == "codex"))
        .map(|w| w["action"].as_str().unwrap_or_default().to_string())
        .expect("codex wiring action");
    let codex_cmd = codex_action.split('`').nth(1).unwrap_or_else(|| {
        panic!("codex action carries a backticked command; got: {codex_action}")
    });
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("set -- {codex_cmd}; echo $#"))
        .output()
        .expect("spawn sh");
    let argc: usize = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    assert_eq!(
        argc, 6,
        "`{codex_cmd}` must parse as exactly `codex mcp add memstead -- <path>` (6 words); \
         an unquoted path with a space would split into more",
    );

    // Absolute paths, so a relative argument does not leak into a field
    // an agent will resolve against its own cwd.
    for key in ["workspace_root", "config_path"] {
        let v = payload[key].as_str().unwrap_or_default();
        assert!(
            Path::new(v).is_absolute(),
            "`{key}` must be absolute for a machine consumer, got: {v}",
        );
    }
}

/// F8: `--relation` is no longer refused on a filesystem-mem
/// workspace — the MCP surface has always performed this operation on
/// this shape, and the CLI-local guard made the limit look like the
/// engine's. The edge is readable afterwards.
#[test]
fn create_relation_lands_edges_on_a_filesystem_mem_workspace() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("edge-graph");
    memstead()
        .args(["quickstart", "--agent", "claude-code"])
        .arg(&root)
        .assert()
        .success();

    memstead()
        .current_dir(&root)
        .args([
            "create",
            "--title",
            "Edge Source",
            "--type",
            "concept",
            "--section",
            "definition=A concept that points at the seed entity.",
            "--section",
            "explanation=Its only job is to carry one inline relation, so the edge is \
             observable after creation.",
            "--relation",
            "CONTRASTS_WITH:edge-graph--welcome-to-memstead",
        ])
        .assert()
        .success();

    let out = stdout_of(
        memstead()
            .current_dir(&root)
            .args(["entity", "edge-graph--edge-source", "--include-relations"])
            .assert()
            .success(),
    );
    assert!(
        out.contains("welcome-to-memstead"),
        "the inline relation must be readable after creation; got:\n{out}",
    );
}

/// The `--help` text no longer claims a restriction that is gone.
#[test]
fn create_help_no_longer_claims_a_mem_repo_only_relation_limit() {
    let out = stdout_of(memstead().args(["create", "--help"]).assert().success());
    assert!(
        !out.contains("Mem-repo workspaces only"),
        "create --help must not claim a lifted restriction; got:\n{out}",
    );
}

/// The two commands exist on the declared CLI surface (the doc
/// generator and `--help` read the same clap tree).
#[test]
fn help_lists_quickstart_and_schema_new() {
    let out = stdout_of(memstead().arg("--help").assert().success());
    assert!(
        out.contains("quickstart"),
        "top-level help lists quickstart; got: {out}"
    );
    let out = stdout_of(memstead().args(["schema", "--help"]).assert().success());
    assert!(out.contains("new"), "schema help lists new; got: {out}");
}

/// Path sanity for the wiring test helper: `Path::is_file` on the
/// scaffold README-less package (regression guard for the two-file
/// package shape the docs promise).
#[test]
fn scaffold_package_is_exactly_two_files() {
    let tmp = TempDir::new().unwrap();
    memstead()
        .current_dir(tmp.path())
        .args(["schema", "new", "acme"])
        .assert()
        .success();
    let mut files: Vec<String> = walk(tmp.path().join("acme").as_path());
    files.sort();
    assert_eq!(
        files,
        vec!["schema.yaml".to_string(), "types/note.yaml".to_string()]
    );
}

fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.path().is_dir() {
            for sub in walk(&entry.path()) {
                out.push(format!("{name}/{sub}"));
            }
        } else {
            out.push(name);
        }
    }
    out
}

// ---------------------------------------------------------------------
// quickstart --repo — the guided point-at-your-repo path
// ---------------------------------------------------------------------

/// A repository the way a stranger's actually looks: source files, `.md`
/// docs at the root and below, and a git history. Returns its path.
fn fixture_repo(parent: &Path, name: &str) -> std::path::PathBuf {
    let repo = parent.join(name);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::write(repo.join("src/main.rs"), b"fn main() {}\n").unwrap();
    std::fs::write(repo.join("README.md"), b"# The App\n\nWhat it does.\n").unwrap();
    std::fs::write(repo.join("docs/design.md"), b"# Design\n\nHow it works.\n").unwrap();
    for args in [
        vec!["init", "-q", "."],
        vec!["add", "-A"],
        vec![
            "-c",
            "user.email=fixture@example.com",
            "-c",
            "user.name=Fixture",
            "commit",
            "-qm",
            "initial",
        ],
    ] {
        let status = std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .status()
            .expect("git must be available");
        assert!(status.success(), "git {args:?} failed");
    }
    repo
}

/// Run one printed command line verbatim through a shell, from `dir`.
fn replay(dir: &Path, command: &str) -> std::process::Output {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .output()
        .expect("shell must run")
}

/// The headline AC: from an existing repository, one invocation leaves a
/// workspace, a mem, and a codebase binding carrying the scaffold deny
/// defaults — and adopts none of the repository's own files.
#[test]
fn quickstart_repo_mode_binds_the_repo_without_adopting_its_files() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "the-app");

    let assert = memstead()
        .current_dir(&repo)
        .args(["quickstart", "--json", "--repo", "."])
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();

    // The repository IS the workspace; the mem takes a folder of its own.
    assert_eq!(
        std::fs::canonicalize(payload["workspace_root"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(&repo).unwrap(),
    );
    assert_eq!(payload["name"], "the-app");
    assert_eq!(payload["mem_folder"], "the-app");
    assert!(repo.join(".memstead").join("workspace.toml").is_file());
    assert!(
        repo.join("the-app")
            .join(".memstead")
            .join("config.json")
            .is_file()
    );

    // The binding: codebase over the repo, scaffold deny defaults, and the
    // record where `projection init` would have put it.
    assert_eq!(payload["binding"]["id"], "the-app/the-app");
    assert_eq!(payload["binding"]["pointer"], ".");
    let record = repo.join(payload["binding"]["record"].as_str().unwrap());
    assert!(record.is_file(), "binding record must exist at {record:?}");
    let binding: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    assert_eq!(binding["version"], 2);
    assert_eq!(binding["destination_mem"], "the-app");
    assert_eq!(binding["sources"][0]["type"], "codebase");
    let deny: Vec<String> = serde_json::from_value(binding["deny_paths"].clone()).unwrap();
    assert_eq!(
        deny,
        vec![
            "**/.DS_Store",
            "**/.git/**",
            "**/node_modules/**",
            "**/Thumbs.db"
        ],
        "the record materialises the scaffold deny defaults as deletable entries",
    );

    // Complement: none of the repository's files became entities. The mem
    // folder holds exactly the seed, and the graph counts exactly one.
    let mem_files: Vec<String> = std::fs::read_dir(repo.join("the-app"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".md"))
        .collect();
    assert_eq!(mem_files, vec!["welcome-to-memstead.md"]);
    let overview = stdout_of(
        memstead()
            .current_dir(&repo)
            .arg("overview")
            .assert()
            .success(),
    );
    assert!(
        overview.contains("_entity_count: 1"),
        "the repo's files must not be adopted; overview:\n{overview}",
    );

    // Complement: the repository's tracked tree is untouched — the only
    // additions are the ones the receipt names.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo)
        .output()
        .unwrap();
    let mut added: Vec<String> = String::from_utf8(status.stdout)
        .unwrap()
        .lines()
        .map(|l| l.trim().to_string())
        .collect();
    added.sort();
    assert_eq!(
        added,
        vec!["?? .mcp.json", "?? .memstead/", "?? the-app/"],
        "nothing beyond the receipt's own artifacts may appear in the tree",
    );
    let receipt_names: Vec<String> = serde_json::from_value(payload["brief"].clone()).unwrap();
    let written = receipt_names
        .iter()
        .find(|l| l.starts_with("Written into your repository"))
        .expect("the brief names what it wrote into the repo");
    for named in [".memstead/", "the-app/", ".mcp.json"] {
        assert!(
            written.contains(named),
            "brief must name `{named}`: {written}"
        );
    }
}

/// Complement to the guided path: without `--repo`, the same fixture is
/// still refused by the tolerant-emptiness gate, with the same code and
/// the same reason. The guided mode adds a door; it opens none.
#[test]
fn quickstart_without_repo_flag_still_refuses_a_populated_repo() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "untouched-app");

    let err = stderr_of(
        memstead()
            .args(["quickstart"])
            .arg(&repo)
            .assert()
            .failure(),
    );
    assert!(
        err.contains("TARGET_NOT_EMPTY") && err.contains("README.md"),
        "the plain path must still refuse the populated repo; got:\n{err}",
    );
    assert!(
        err.contains("silently adopt them into the graph"),
        "the refusal must still name the adoption risk; got:\n{err}",
    );
    assert!(
        !repo.join(".memstead").exists(),
        "a refused quickstart writes nothing",
    );
}

/// Every command the guided receipt prints runs verbatim from the
/// directory the receipt states, with no placeholder to edit first.
#[test]
fn quickstart_repo_receipt_commands_replay_verbatim() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "replay-app");

    let assert = memstead()
        .current_dir(&repo)
        .args(["quickstart", "--json", "--repo", "."])
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();

    let mut replayed = 0;
    for check in payload["verify_now"].as_array().unwrap() {
        let command = check["command"].as_str().unwrap();
        // The `memstead-mcp --version` check names a binary that only a
        // full install carries; every other check is this binary's own.
        if command.contains("memstead-mcp") {
            continue;
        }
        let out = replay(&repo, command);
        assert!(
            out.status.success(),
            "printed command must run verbatim: {command}\n{}",
            String::from_utf8_lossy(&out.stderr),
        );
        replayed += 1;
    }
    assert!(replayed >= 2, "the receipt must print runnable checks");

    // The brief's own command — the one that starts the ingest loop —
    // is a printed command too, and carries no placeholder.
    let brief: Vec<String> = serde_json::from_value(payload["brief"].clone()).unwrap();
    let growth = brief
        .iter()
        .find(|l| l.starts_with("Growth:"))
        .expect("the brief states how the mem grows");
    // Take what is BETWEEN the backticks, not everything after the opening
    // one: the line may carry prose after the command, and a trailing-trim
    // would hand the shell that prose too.
    let command = growth
        .split_once("Start with: `")
        .expect("the growth line names a runnable command")
        .1
        .split('`')
        .next()
        .expect("the command is backtick-delimited");
    assert!(
        !command.contains('<') && !command.contains("..."),
        "no printed command may carry a placeholder: {command}",
    );
    let out = replay(&repo, command);
    assert!(
        out.status.success(),
        "the ingest brief must render: {command}\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
}

/// The layout guidance fires at the layout decision: a workspace beside
/// the repository is supported, and the receipt says what it costs and
/// how to avoid it.
#[test]
fn quickstart_repo_outside_the_workspace_warns_with_the_relocation_recipe() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "beside-app");

    let assert = memstead()
        .current_dir(tmp.path())
        .args(["quickstart", "--json", "graph", "--repo"])
        .arg(&repo)
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();

    assert_eq!(payload["binding"]["pointer"], "../beside-app");
    // The mem still collapses onto the workspace root in this layout.
    assert!(payload["mem_folder"].as_str().unwrap() == ".");
    let warnings: Vec<String> = serde_json::from_value(payload["warnings"].clone()).unwrap();
    let layout = warnings
        .iter()
        .find(|w| w.contains("resolves outside the workspace root"))
        .expect("the out-of-root layout must be named");
    assert!(
        layout.contains("root the workspace at the common parent"),
        "the warning must carry the relocation recipe: {layout}",
    );
    assert!(
        layout.contains("supported"),
        "the shape is supported, not refused: {layout}",
    );
    // Complement: the repository itself stays clean in this layout.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        String::from_utf8(status.stdout).unwrap().trim().is_empty(),
        "a workspace beside the repo writes nothing into it",
    );
}

/// The mem folder is a folder the graph then owns, so a collision with
/// something the repository already uses refuses — it is never adopted —
/// and the refusal carries the flag that resolves it.
#[test]
fn quickstart_repo_mem_folder_collision_refuses_with_the_name_remedy() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "collide-app");
    std::fs::create_dir_all(repo.join("collide-app")).unwrap();
    std::fs::write(repo.join("collide-app").join("notes.md"), b"# mine\n").unwrap();

    let err = stderr_of(
        memstead()
            .current_dir(&repo)
            .args(["quickstart", "--repo", "."])
            .assert()
            .failure(),
    );
    assert!(
        err.contains("TARGET_NOT_EMPTY") && err.contains("--name"),
        "the collision must refuse with the --name remedy; got:\n{err}",
    );
    assert!(
        !repo.join(".memstead").exists(),
        "a refused guided quickstart writes nothing",
    );
    assert_eq!(
        std::fs::read(repo.join("collide-app").join("notes.md")).unwrap(),
        b"# mine\n",
        "the folder it refused to take is untouched",
    );
}

/// A `--repo` that names nothing refuses before any write: quickstart
/// creates workspaces, never repositories.
#[test]
fn quickstart_repo_must_already_exist() {
    let tmp = TempDir::new().unwrap();
    let err = stderr_of(
        memstead()
            .current_dir(tmp.path())
            .args(["quickstart", "graph", "--repo", "./typo"])
            .assert()
            .failure(),
    );
    assert!(
        err.contains("INVALID_INPUT") && err.contains("--repo"),
        "a missing repo must refuse naming the flag; got:\n{err}",
    );
    assert!(!tmp.path().join("graph").exists(), "nothing is created");
}

/// The guided scaffold does not widen the silent dead-deny exemption:
/// its own defaults stay quiet, a user-authored entry that matches
/// nothing still gets the loud lint.
#[test]
fn quickstart_repo_scaffold_does_not_widen_the_dead_deny_exemption() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "lint-app");

    memstead()
        .current_dir(&repo)
        .args(["quickstart", "--json", "--repo", "."])
        .assert()
        .success();

    let record = repo.join(".memstead/projections/lint-app/lint-app.json");
    let mut binding: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    binding["deny_paths"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!("nowhere-near/**"));
    std::fs::write(&record, serde_json::to_vec_pretty(&binding).unwrap()).unwrap();

    let brief = stdout_of(
        memstead()
            .current_dir(&repo)
            .args(["projection", "brief", "lint-app/lint-app"])
            .assert()
            .success(),
    );
    assert!(
        brief.contains("nowhere-near/**"),
        "a user-authored dead deny entry must still be reported; got:\n{brief}",
    );
    for scaffolded in ["**/node_modules/**", "**/Thumbs.db"] {
        assert!(
            !brief.contains(&format!("`{scaffolded}`")),
            "the scaffold's own default must stay silent: {scaffolded}\n{brief}",
        );
    }
}

/// A workspace INSIDE the repository is still a mem-in-its-own-folder
/// layout, and that is what makes the receipt's scope claim true: the
/// mem's own entity file must not come back round as a source artifact
/// of the binding that points at the tree containing it.
#[test]
fn quickstart_repo_workspace_inside_the_repo_keeps_the_mem_out_of_its_own_scope() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "inside-app");

    let assert = memstead()
        .current_dir(&repo)
        .args(["quickstart", "--json", "memgraph", "--repo", "."])
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();

    // The workspace is a subdirectory of the repo, so the mem still takes
    // a folder of its own — the containment test, not path-argument
    // absence, decides the layout.
    assert_eq!(payload["mem_folder"], "memgraph");
    assert_eq!(payload["binding"]["pointer"], "..");

    let workspace = repo.join("memgraph");
    let measured = memstead()
        .current_dir(&workspace)
        .args([
            "projection",
            "verify",
            payload["binding"]["id"].as_str().unwrap(),
            "--full",
            "--include",
            "uncovered_artifacts",
        ])
        .assert()
        .success();
    let report = stdout_of(measured);
    assert!(
        !report.contains("welcome-to-memstead.md"),
        "the mem's own entity must not enumerate as a source artifact; report:\n{report}",
    );
    for repo_file in ["../README.md", "../src/main.rs"] {
        assert!(
            report.contains(repo_file),
            "the repository's files are the denominator; missing {repo_file}:\n{report}",
        );
    }

    // …and the printed claim says exactly that, naming the folder that
    // carries the exclusion in this layout.
    let brief: Vec<String> = serde_json::from_value(payload["brief"].clone()).unwrap();
    let scope = brief
        .iter()
        .find(|l| l.starts_with("Scope:"))
        .expect("the brief states the scope");
    // The machine brief speaks workspace-relative, which is what an agent
    // resolves against `workspace_root`. The markdown brief's frame is
    // covered by `quickstart_repo_printed_mem_folder_resolves_from_the_readers_cwd`.
    assert!(
        scope.contains("the mem's own folder `memgraph/`"),
        "the scope claim must name the excluded folder: {scope}",
    );
}

/// The "what appeared in your repository" claim is stated in the
/// READER's frame. With the workspace nested inside the repo, one new
/// directory appears — not the three paths inside it, which are
/// workspace-relative and would not resolve from the repo root.
#[test]
fn quickstart_repo_nested_workspace_names_what_the_repo_actually_gained() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "nested-app");

    let assert = memstead()
        .current_dir(&repo)
        .args(["quickstart", "--json", "wsdir", "--repo", "."])
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();

    let brief: Vec<String> = serde_json::from_value(payload["brief"].clone()).unwrap();
    let written = brief
        .iter()
        .find(|l| l.starts_with("Written into your repository"))
        .expect("the brief states what the repository gained");
    assert!(
        written.contains("`wsdir/`"),
        "the one new path must be named: {written}",
    );
    for workspace_relative in ["`.memstead/`", "`.mcp.json`"] {
        assert!(
            !written.contains(workspace_relative),
            "a workspace-relative path must not be presented as repo-relative: {written}",
        );
    }

    // …and that is exactly what git sees.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(status.stdout).unwrap().trim(),
        "?? wsdir/",
        "the receipt's claim and the tracked tree must agree",
    );
}

/// A wiring file that already existed was MODIFIED, not added — and the
/// receipt says so, because `git status` will.
#[test]
fn quickstart_repo_names_a_modified_wiring_file_as_modified() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "wired-app");
    std::fs::write(
        repo.join(".mcp.json"),
        b"{\n  \"mcpServers\": {\n    \"other\": { \"command\": \"/bin/other\" }\n  }\n}\n",
    )
    .unwrap();
    for args in [
        vec!["add", "-A"],
        vec![
            "-c",
            "user.email=fixture@example.com",
            "-c",
            "user.name=Fixture",
            "commit",
            "-qm",
            "wiring",
        ],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .status()
            .unwrap();
    }

    let assert = memstead()
        .current_dir(&repo)
        .args(["quickstart", "--json", "--repo", "."])
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
    let brief: Vec<String> = serde_json::from_value(payload["brief"].clone()).unwrap();
    let written = brief
        .iter()
        .find(|l| l.starts_with("Written into your repository"))
        .expect("the brief states what the repository gained");
    assert!(
        written.contains("`.mcp.json` (agent wiring added to it)"),
        "a pre-existing file is modified, not added: {written}",
    );

    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo)
        .output()
        .unwrap();
    let out = String::from_utf8(status.stdout).unwrap();
    assert!(
        out.contains("M .mcp.json"),
        "git must agree the file was modified: {out}",
    );
}

/// The out-of-root warning recommends rooting the workspace at the common
/// parent of the source trees. That recipe must be reachable from the
/// front door that prints it.
#[test]
fn quickstart_repo_at_the_common_parent_is_reachable() {
    let tmp = TempDir::new().unwrap();
    let parent = tmp.path().join("parent");
    std::fs::create_dir_all(&parent).unwrap();
    fixture_repo(&parent, "repo-one");

    let assert = memstead()
        .current_dir(tmp.path())
        .args([
            "quickstart",
            "--json",
            "parent",
            "--repo",
            "parent/repo-one",
            "--name",
            "parent-ws",
        ])
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();

    // The whole point of the recipe: an in-root pointer, so no `../…`
    // artifact ids and no out-of-root caveat.
    assert_eq!(payload["binding"]["pointer"], "repo-one");
    let warnings: Vec<String> = serde_json::from_value(payload["warnings"].clone()).unwrap();
    assert!(
        !warnings.iter().any(|w| w.contains("resolves outside")),
        "the recommended layout must not warn about itself: {warnings:?}",
    );
    // The workspace is not inside the repository, so nothing was written
    // into it and the receipt claims nothing about its tree.
    let brief: Vec<String> = serde_json::from_value(payload["brief"].clone()).unwrap();
    assert!(
        !brief
            .iter()
            .any(|l| l.starts_with("Written into your repository")),
        "no repository claim belongs here: {brief:?}",
    );
}

/// The JSON receipt's `config_path` names the MEM's config, and the mem's
/// config lives with the mem's entities — which, in every guided layout
/// that gives the mem a folder, is not the workspace root.
#[test]
fn quickstart_repo_json_config_path_names_a_file_that_exists() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "config-app");

    for args in [
        vec!["quickstart", "--json", "--repo", "."],
        vec!["quickstart", "--json", "sub-ws", "--repo", "."],
    ] {
        let fresh = fixture_repo(tmp.path(), &format!("c{}", args.len()));
        let assert = memstead()
            .current_dir(&fresh)
            .args(&args)
            .assert()
            .success();
        let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
        let config = std::path::PathBuf::from(payload["config_path"].as_str().unwrap());
        assert!(
            config.is_file(),
            "config_path must name a file that exists ({args:?}): {config:?}",
        );
        // …and it is the mem's, not some other workspace's.
        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(parsed["schema"], payload["schema"]);
    }
    drop(repo);
}

/// A path the receipt tells the reader to open resolves from where the
/// receipt's own commands leave them standing — the commands carry a
/// `cd` when the workspace is not the cwd, and a bare workspace-relative
/// path beside them would not resolve.
#[test]
fn quickstart_repo_record_path_resolves_from_the_readers_cwd() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "record-app");

    let out = stdout_of(
        memstead()
            .current_dir(&repo)
            .args([
                "quickstart",
                "--agent",
                "claude-code",
                "sub-ws",
                "--repo",
                ".",
            ])
            .assert()
            .success(),
    );
    let edit_line = out
        .lines()
        .find(|l| l.contains("yours to edit"))
        .expect("the brief names the record to edit");
    let path = edit_line
        .rsplit_once("yours to edit: `")
        .expect("the line ends in a backticked path")
        .1
        .trim_end_matches('`');
    assert!(
        repo.join(path).is_file(),
        "the record path must resolve from the reader's cwd: {path}",
    );
}

/// The mem-folder path the receipt prints is a path the reader is told
/// their entities are in — so it resolves from where the receipt leaves
/// them, like every other path it prints. The nested layout is where the
/// workspace frame and the reader's frame diverge.
#[test]
fn quickstart_repo_printed_mem_folder_resolves_from_the_readers_cwd() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "framed-app");

    let out = stdout_of(
        memstead()
            .current_dir(&repo)
            .args([
                "quickstart",
                "--agent",
                "claude-code",
                "sub-ws",
                "--repo",
                ".",
                "--name",
                "kb",
            ])
            .assert()
            .success(),
    );

    // The artifact line and the shape disclosure name the same folder;
    // both must open from the directory the reader is standing in.
    let folder_line = out
        .lines()
        .find(|l| l.starts_with("- Mem folder:"))
        .expect("the receipt names the mem folder");
    let printed = folder_line
        .split('`')
        .nth(1)
        .expect("the folder is backticked")
        .trim_end_matches('/');
    assert!(
        repo.join(printed).is_dir(),
        "the printed mem folder must resolve from the reader's cwd: {printed}",
    );
    assert!(
        repo.join(printed).join("welcome-to-memstead.md").is_file(),
        "…and be the folder that actually holds the entities: {printed}",
    );
    assert!(
        out.contains(&format!("plain `.md` files in `{printed}/`")),
        "the disclosure must name the same resolvable folder; got:\n{out}",
    );
}

/// A refusal's retry command reproduces the invocation that hit it. A
/// guided run retried without `--repo` lands on the tolerant-emptiness
/// gate instead of succeeding.
#[test]
fn quickstart_repo_name_refusal_retry_keeps_the_repo_flag() {
    let tmp = TempDir::new().unwrap();
    // A directory name nothing valid survives slugging from.
    let repo = fixture_repo(tmp.path(), "日本語");

    let err = stderr_of(
        memstead()
            .current_dir(&repo)
            .args(["quickstart", "--repo", "."])
            .assert()
            .failure(),
    );
    let retry = err
        .split("pass one explicitly: ")
        .nth(1)
        .expect("the refusal carries a retry command")
        .trim();
    assert!(
        retry.contains("--repo"),
        "the retry must reproduce the guided invocation: {retry}",
    );
    let out = replay(&repo, retry);
    assert!(
        out.status.success(),
        "the printed retry must run verbatim: {retry}\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
}

/// `--repo` takes any directory. A plain folder has files, not history —
/// the brief must not describe a git repository that is not there.
#[test]
fn quickstart_repo_non_git_directory_claims_no_history() {
    let tmp = TempDir::new().unwrap();
    let plain = tmp.path().join("plain-dir");
    std::fs::create_dir_all(plain.join("src")).unwrap();
    std::fs::write(plain.join("src/a.rs"), b"fn a() {}\n").unwrap();
    let ws = tmp.path().join("ws");

    let assert = memstead()
        .current_dir(tmp.path())
        .args(["quickstart", "--json"])
        .arg(&ws)
        .arg("--repo")
        .arg(&plain)
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
    let brief: Vec<String> = serde_json::from_value(payload["brief"].clone()).unwrap();
    let not_yet = brief
        .iter()
        .find(|l| l.starts_with("Not yet:"))
        .expect("the brief states what the mem does not hold");
    assert!(
        not_yet.contains("Its files are"),
        "a non-git directory has files, not history: {not_yet}",
    );
}

/// The wiring line names a file the reader is told to restart an agent
/// for. It resolves from their cwd, like every other path the receipt
/// prints — for every target that writes a file.
#[test]
fn quickstart_repo_printed_wiring_paths_resolve_from_the_readers_cwd() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "wiring-app");

    let out = stdout_of(
        memstead()
            .current_dir(&repo)
            .args([
                "quickstart",
                "sub-ws",
                "--repo",
                ".",
                "--agent",
                "claude-code",
                "--agent",
                "cursor",
                "--agent",
                "gemini",
            ])
            .assert()
            .success(),
    );

    let mut checked = 0;
    for line in out.lines().filter(|l| l.contains("(server `memstead`)")) {
        let printed = line
            .split('`')
            .nth(1)
            .expect("the config file is backticked");
        assert!(
            repo.join(printed).is_file(),
            "the printed wiring path must resolve from the reader's cwd: {printed}\n{out}",
        );
        checked += 1;
    }
    assert_eq!(
        checked, 3,
        "all three file-writing targets are named:\n{out}"
    );
}

/// The `--json` payload is the agent's surface: its paths resolve against
/// `workspace_root`, not against whatever directory the human ran from.
/// One rendering serving both frames is how a payload ends up carrying a
/// path that resolves in neither.
#[test]
fn quickstart_repo_json_paths_resolve_against_the_workspace_root() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "frames-app");

    let assert = memstead()
        .current_dir(&repo)
        .args(["quickstart", "--json", "sub-ws", "--repo", "."])
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
    let root = std::path::PathBuf::from(payload["workspace_root"].as_str().unwrap());

    assert!(
        root.join(payload["binding"]["record"].as_str().unwrap())
            .is_file(),
        "binding.record resolves against workspace_root",
    );
    assert!(
        root.join(payload["mem_folder"].as_str().unwrap()).is_dir(),
        "mem_folder resolves against workspace_root",
    );
    assert!(
        root.join(
            payload["agents"][0]["action"]
                .as_str()
                .unwrap()
                .split('`')
                .nth(1)
                .unwrap()
        )
        .is_file(),
        "the agent action's path resolves against workspace_root",
    );

    let brief: Vec<String> = serde_json::from_value(payload["brief"].clone()).unwrap();
    let edit = brief
        .iter()
        .find(|l| l.contains("yours to edit"))
        .expect("the brief names the record");
    let path = edit
        .rsplit_once("yours to edit: `")
        .unwrap()
        .1
        .trim_end_matches('`');
    assert!(
        root.join(path).is_file(),
        "the JSON brief's paths resolve against workspace_root too: {path}",
    );
}

/// Every path the markdown receipt prints resolves from the directory the
/// reader is standing in, and is the thing the receipt calls it. Invoked
/// from a third directory — neither the workspace nor the repository —
/// because that is the only vantage point where all three frames differ.
/// Layout A cannot expose this: there they coincide.
#[test]
fn quickstart_repo_every_printed_path_resolves_from_a_third_directory() {
    let tmp = TempDir::new().unwrap();
    let parent = tmp.path().join("parent");
    std::fs::create_dir_all(&parent).unwrap();
    let repo = fixture_repo(&parent, "third-app");
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    let out = stdout_of(
        memstead()
            .current_dir(&elsewhere)
            .args(["quickstart", "--agent", "claude-code"])
            // Nested inside the repo, so the mem takes a folder of its
            // own — and the reader is in neither.
            .arg("../parent/third-app/ws")
            .arg("--repo")
            .arg("../parent/third-app")
            .arg("--name")
            .arg("kb")
            .assert()
            .success(),
    );

    let printed = |prefix: &str| -> String {
        let line = out
            .lines()
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("receipt has no `{prefix}` line:\n{out}"));
        line.split('`')
            .nth(if prefix.starts_with("- Binding") {
                3
            } else {
                1
            })
            .unwrap_or_else(|| panic!("no backticked path in: {line}"))
            .trim_end_matches('/')
            .to_string()
    };

    // The mem folder: exists, and holds the entities.
    let mem = elsewhere.join(printed("- Mem folder:"));
    assert!(
        mem.join("welcome-to-memstead.md").is_file(),
        "mem folder must resolve and hold the seed: {mem:?}",
    );

    // The binding pointer: exists, and is the source — not some other
    // directory that merely happens to exist at that relative path.
    let pointer = elsewhere.join(printed("- Binding:"));
    assert!(
        pointer.join("README.md").is_file() && pointer.join(".git").exists(),
        "the pointer must resolve to the repository: {pointer:?}",
    );
    assert_eq!(
        pointer.canonicalize().unwrap(),
        repo.canonicalize().unwrap(),
        "…and to THAT repository, not another directory at the same path",
    );

    // The wiring file the reader is told to restart an agent for.
    let wiring = out
        .lines()
        .find(|l| l.contains("(server `memstead`)"))
        .expect("the receipt names the wiring file");
    let wiring = elsewhere.join(wiring.split('`').nth(1).unwrap());
    assert!(wiring.is_file(), "wiring path must resolve: {wiring:?}");

    // The record the reader is told to edit.
    let edit = out
        .lines()
        .find(|l| l.contains("yours to edit"))
        .expect("the brief names the record");
    let record = elsewhere.join(
        edit.rsplit_once("yours to edit: `")
            .unwrap()
            .1
            .trim_end_matches('`'),
    );
    assert!(record.is_file(), "record path must resolve: {record:?}");
}

/// The collapsed layout — workspace outside the repository — claims only
/// engine state, because the mem folder is not in the source tree at all
/// and no exclusion has to fire for it.
#[test]
fn quickstart_repo_outside_layout_claims_only_engine_state() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "outside-app");

    let assert = memstead()
        .current_dir(tmp.path())
        .args(["quickstart", "--json", "graph", "--repo"])
        .arg(&repo)
        .assert()
        .success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
    assert_eq!(payload["mem_folder"], ".");

    let brief: Vec<String> = serde_json::from_value(payload["brief"].clone()).unwrap();
    let scope = brief
        .iter()
        .find(|l| l.starts_with("Scope:"))
        .expect("the brief states the scope");
    assert!(
        scope.contains("engine state (`.memstead/`)") && !scope.contains("mem's own folder"),
        "the collapsed layout claims only what it excludes: {scope}",
    );
}

/// The guided receipt's shape disclosure holds — and tells the truth
/// about where the entities actually are.
#[test]
fn quickstart_repo_receipt_discloses_the_shape_and_the_mem_folder() {
    let tmp = TempDir::new().unwrap();
    let repo = fixture_repo(tmp.path(), "disclose-app");
    let out = stdout_of(
        memstead()
            .current_dir(&repo)
            .args(["quickstart", "--agent", "claude-code", "--repo", "."])
            .assert()
            .success(),
    );
    assert_filesystem_shape_disclosure(&out, "guided quickstart receipt");
    assert!(
        out.contains("plain `.md` files in `disclose-app/`"),
        "the disclosure must point at the mem's actual folder; got:\n{out}",
    );
    assert!(
        !out.contains("plain `.md` files in this folder"),
        "the collapsed-shape wording would be untrue here; got:\n{out}",
    );
}

/// Criterion 1's other half: `--format llms-txt` is backend-uniform, so it
/// produces the same document shape on a **filesystem** mem as on a mem-repo
/// one. `quickstart` is the shortest route to a real filesystem workspace.
///
/// This exists because "backend-uniform" is the kind of claim that holds until
/// nobody checks: the mem-repo path is what every other export test exercises.
#[test]
fn llms_txt_export_works_on_a_filesystem_mem() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("fs-graph");
    memstead()
        .args(["quickstart", "--json"])
        .arg(&root)
        .assert()
        .success();

    let doc = stdout_of(
        memstead()
            .current_dir(&root)
            .args(["export", "--format", "llms-txt"])
            .assert()
            .success(),
    );

    assert!(doc.starts_with("# fs-graph"), "header names the mem: {doc}");
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
        doc.contains("_Type: "),
        "the seed entity carries a visible type line: {doc}"
    );
    assert!(doc.contains("\n---\n"), "entities are separated: {doc}");
    assert!(!doc.contains("[["), "no raw wiki-link syntax: {doc}");
    assert!(
        !doc.contains("this deployment vouches"),
        "a CLI export claims no deployment provenance: {doc}"
    );
}
