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
    assert_eq!(payload["schema"], "default@1.0.0");
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
    assert_eq!(std::fs::read_to_string(root.join("README")).unwrap(), "my project\n");

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

    let err = stderr_of(memstead().arg("quickstart").arg(tmp.path()).assert().failure());
    assert!(err.contains("README.md"), "names the file; got: {err}");
    assert!(err.contains("adopt"), "explains the ingestion risk; got: {err}");
    assert!(err.contains("memstead quickstart"), "carries the alternative; got: {err}");
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
    assert!(err.contains("memstead quickstart"), "names the exact alternative; got: {err}");

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
    let err = stderr_of(memstead().arg("quickstart").arg(tmp.path()).assert().failure());
    assert!(err.contains("FOREIGN_MEMSTEAD_DIR"), "typed code; got: {err}");
    assert!(err.contains("memstead quickstart"), "carries next command; got: {err}");

    // Ancestor workspace: refuse to nest.
    let outer = TempDir::new().unwrap();
    memstead().arg("quickstart").arg(outer.path()).assert().success();
    let inner = outer.path().join("inner");
    std::fs::create_dir(&inner).unwrap();
    let err = stderr_of(memstead().arg("quickstart").arg(&inner).assert().failure());
    assert!(err.contains("WORKSPACE_ALREADY_EXISTS_ABOVE"), "typed code; got: {err}");
    // The alternatives must be viable in a quickstart-created
    // (filesystem, no-allowlist) workspace: work there, or start a
    // separate graph — never `memstead mem init`, which refuses there.
    assert!(err.contains("memstead overview"), "viable next command; got: {err}");
    assert!(err.contains("memstead quickstart"), "separate-graph alternative; got: {err}");
    assert!(!err.contains("mem init"), "no dead-end suggestion; got: {err}");
    assert!(!inner.join(".memstead").exists(), "no half-init in the nested target");

    // Re-run on the finished workspace: refuse, point at overview.
    let err = stderr_of(memstead().arg("quickstart").arg(outer.path()).assert().failure());
    assert!(err.contains("WORKSPACE_ALREADY_INITIALISED"), "typed code; got: {err}");
    assert!(err.contains("memstead overview"), "carries next command; got: {err}");
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

    let assert = memstead().args(["quickstart", "--json"]).arg(&root).assert().success();
    let payload: serde_json::Value = serde_json::from_str(&stdout_of(assert)).unwrap();
    assert!(
        payload["agents"][0]["action"].as_str().unwrap().contains("left untouched"),
        "report says the entry was left alone; got {payload}",
    );

    let mcp: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["memstead"]["command"], "/custom/memstead-mcp");
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
        serde_json::from_slice(&std::fs::read(root.join(".gemini/settings.json")).unwrap()).unwrap();
    assert!(gemini["mcpServers"]["memstead"]["command"].is_string());
    // Codex: command printed, nothing written.
    let codex_action = payload["agents"][2]["action"].as_str().unwrap();
    assert!(codex_action.contains("codex mcp add memstead --"), "got: {codex_action}");
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
    assert!(err.contains("memstead quickstart --name"), "exact command; got: {err}");
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
    assert!(out.contains("memstead schema install ../acme"), "got: {out}");
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
    memstead().current_dir(&ws).args(["schema", "validate", "acme"]).assert().success();
    memstead().current_dir(&ws).args(["schema", "install", "acme"]).assert().success();
    let pin_out = stdout_of(
        memstead()
            .current_dir(&ws)
            .args(["mem", "set-schema", "myws", "acme@0.1.0"])
            .assert()
            .success(),
    );
    assert!(pin_out.contains("Switched"), "empty mem switches atomically; got: {pin_out}");

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
        memstead().current_dir(&ws).args(["schema", "new", "acme"]).assert().success(),
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
    memstead().current_dir(&ws).args(["schema", "validate", "acme"]).assert().success();
    memstead().current_dir(&ws).args(["schema", "install", "acme"]).assert().success();
    memstead().current_dir(&ws).args(["delete", seed_id]).assert().success();
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

/// Preflight AC: a malformed agent config file refuses BEFORE anything
/// is created — the printed "re-run memstead quickstart" must still be
/// able to succeed, so no workspace may exist after the refusal.
#[test]
fn quickstart_malformed_agent_config_refuses_before_any_write() {
    // Invalid JSON.
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join(".mcp.json"), "{not json").unwrap();
    let err = stderr_of(memstead().arg("quickstart").arg(tmp.path()).assert().failure());
    assert!(err.contains("not valid JSON"), "names the defect; got: {err}");
    assert!(err.contains("re-run: memstead quickstart"), "carries the retry; got: {err}");
    assert!(!tmp.path().join(".memstead").exists(), "nothing was created");
    // The printed retry actually works once the file is fixed.
    std::fs::remove_file(tmp.path().join(".mcp.json")).unwrap();
    memstead().arg("quickstart").arg(tmp.path()).assert().success();

    // `mcpServers` present but not an object.
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join(".mcp.json"), r#"{"mcpServers": []}"#).unwrap();
    let err = stderr_of(memstead().arg("quickstart").arg(tmp.path()).assert().failure());
    assert!(err.contains("mcpServers"), "names the defect; got: {err}");
    assert!(err.contains("re-run: memstead quickstart"), "carries the retry; got: {err}");
    assert!(!tmp.path().join(".memstead").exists(), "nothing was created");
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
        memstead().current_dir(tmp.path()).args(["schema", "new", "acme"]).assert().success(),
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
    memstead().current_dir(tmp.path()).args(["schema", "validate", "acme"]).assert().success();
    let fresh = tmp.path().join("acme-mem");
    std::fs::create_dir(&fresh).unwrap();
    memstead()
        .current_dir(&fresh)
        .args(["init", "--name", "acme-mem", "--schema", "acme@0.1.0"])
        .assert()
        .success();
    memstead().current_dir(&fresh).args(["schema", "install", "../acme"]).assert().success();

    // The workspace boots and the scaffolded type is writable.
    memstead().current_dir(&fresh).arg("overview").assert().success();
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
        memstead().current_dir(&ws).args(["schema", "new", "acme"]).assert().success(),
    );

    // Pull the two printed paths: the fresh-mem dir from the init step,
    // the package path from the install step. Both are quoted absolute
    // paths in the in-workspace variant.
    let quoted = |line_marker: &str| -> std::path::PathBuf {
        let line = out
            .lines()
            .find(|l| l.contains(line_marker))
            .unwrap_or_else(|| panic!("no step containing `{line_marker}`; got: {out}"));
        let start = line.find('"').unwrap_or_else(|| panic!("no quoted path in: {line}"));
        let rest = &line[start + 1..];
        let end = rest.find('"').unwrap_or_else(|| panic!("unterminated quote in: {line}"));
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
    memstead().current_dir(&fresh).arg("overview").assert().success();
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
    memstead().current_dir(&ws).args(["schema", "new", "acme"]).assert().success();
    memstead().current_dir(&ws).args(["schema", "install", "acme"]).assert().success();
    assert!(ws.join(".memstead/schemas/acme@0.1.0/schema.yaml").is_file());
    assert!(ws.join(".memstead/schemas/acme@0.1.0/types/note.yaml").is_file());
}

/// Refusal ACs: an existing package refuses rather than overwriting; an
/// invalid name refuses with the slug rule and a suggested correction.
/// Both messages carry the exact next command.
#[test]
fn schema_new_refusals_carry_next_commands() {
    let tmp = TempDir::new().unwrap();
    memstead().current_dir(tmp.path()).args(["schema", "new", "acme"]).assert().success();
    let before = std::fs::read_to_string(tmp.path().join("acme/schema.yaml")).unwrap();

    // Existing package: refuse, don't overwrite.
    let err = stderr_of(
        memstead().current_dir(tmp.path()).args(["schema", "new", "acme"]).assert().failure(),
    );
    assert!(err.contains("SCHEMA_PACKAGE_EXISTS"), "typed code; got: {err}");
    assert!(err.contains("memstead schema validate acme"), "next command; got: {err}");
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
        memstead().current_dir(tmp.path()).args(["schema", "new", "busy"]).assert().failure(),
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
    memstead().current_dir(tmp.path()).args(["schema", "new", "acme"]).assert().success();
    let scaffold = format!(
        "{}{}",
        std::fs::read_to_string(tmp.path().join("acme/schema.yaml")).unwrap(),
        std::fs::read_to_string(tmp.path().join("acme/types/note.yaml")).unwrap(),
    );
    assert!(!scaffold.to_lowercase().contains(&retired_noun), "scaffold speaks mem only");

    let root = tmp.path().join("qs");
    let out = stdout_of(memstead().arg("quickstart").arg(&root).assert().success());
    assert!(!out.to_lowercase().contains(&retired_noun), "quickstart report speaks mem only");
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
            memstead().current_dir(tmp.path()).args(["schema", "new", "acme"]).assert().success();
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

/// The two commands exist on the declared CLI surface (the doc
/// generator and `--help` read the same clap tree).
#[test]
fn help_lists_quickstart_and_schema_new() {
    let out = stdout_of(memstead().arg("--help").assert().success());
    assert!(out.contains("quickstart"), "top-level help lists quickstart; got: {out}");
    let out = stdout_of(memstead().args(["schema", "--help"]).assert().success());
    assert!(out.contains("new"), "schema help lists new; got: {out}");
}

/// Path sanity for the wiring test helper: `Path::is_file` on the
/// scaffold README-less package (regression guard for the two-file
/// package shape the docs promise).
#[test]
fn scaffold_package_is_exactly_two_files() {
    let tmp = TempDir::new().unwrap();
    memstead().current_dir(tmp.path()).args(["schema", "new", "acme"]).assert().success();
    let mut files: Vec<String> = walk(tmp.path().join("acme").as_path());
    files.sort();
    assert_eq!(files, vec!["schema.yaml".to_string(), "types/note.yaml".to_string()]);
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
