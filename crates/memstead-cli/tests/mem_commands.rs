#![cfg(feature = "mem-repo")]
// `memstead export --format mem` round-trips through `memstead install` here,
// and `install` is a mem-repo-only subcommand. Skip the whole binary
// under `--no-default-features` rather than try to project the lean
// half (which has no `install` to round-trip into).

//! Integration tests for `memstead export` and `memstead install`.
//!
//! Exercises the full share-a-mem flow end-to-end:
//!
//! 1. Build a fixture write mem A with two entities.
//! 2. Run `memstead export --format mem -o out.mem` against A.
//! 3. Build a separate empty write mem B.
//! 4. Run `memstead install ./out.mem` against B.
//! 5. Verify the installed read mem's entities are discoverable from B
//!    via `memstead entity` / `memstead search`.
//!
//! Uses `MEMSTEAD_MEM_CACHE` to keep the global cache writes inside a tempdir.
//! Env access is unsafe in Rust 2024, and `std::env::set_var` is globally
//! visible across threads — so this test binary runs single-threaded and
//! serializes cache-touching tests via a process-local `Mutex`, matching
//! `memstead-git-branch/tests/read_mems.rs`.

use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use predicates::str::contains;
use tempfile::TempDir;

/// Serializes env mutations across tests in this binary.
fn cache_guard() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Commit the `(rel_path, bytes)` pairs to `refs/heads/<mem_name>` of
/// `<root>/mem-repo/.git/`. The branch-walk export reads archive `.md`
/// content from this branch tip — disk-resident `.md` files alone are not
/// sufficient under the GitObject default.
fn commit_mem_branch(root: &Path, mem_name: &str, entries: &[(&str, &str)]) {
    use memstead_git_branch::storage::MemWriter;
    use memstead_git_branch::storage::git_tree::GitTreeMemWriter;
    use memstead_git_branch::vcs::CommitContext;

    let gitdir = root.join("mem-repo").join(".git");
    let writer = GitTreeMemWriter::new(gitdir, format!("refs/heads/{mem_name}"));
    for (rel, content) in entries {
        writer
            .write_entity(Path::new(rel), content.as_bytes())
            .unwrap();
    }
    writer.commit("seed", &CommitContext::internal()).unwrap();
}

/// Build a write mem at `<root>/sender-mem/` with version set
/// (required for mem-archive export) and two minimal spec entities.
/// Returns the mem's absolute path. The dir basename equals the
/// declared `name: "sender-mem"` per the basename-invariant.
///
/// Also lays down `<root>/mem-repo/.git/` so the CLI's workspace walk-up
/// resolves `<root>` and the engine's fail-fast accepts the workspace.
///
/// Also commits the disk `.md` content to `refs/heads/sender-mem` so the
/// branch-walk export produces a non-empty archive.
fn make_sender_mem(root: &Path) -> std::path::PathBuf {
    let dir = root.join("sender-mem");
    fs::create_dir_all(&dir).unwrap();
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{
  "version": "1.0.0",
  "description": "Fixture write mem used to exercise export → install",
  "schema": "default@1.0.0"
}"#,
    )
    .unwrap();
    init_real_mem_repo_from_disk(root, &[(&dir, "sender-mem")]);

    let alpha_body = r#"---
type: spec
created_date: 2026-01-01
last_modified: 2026-01-01
level: M0
---
# Alpha

## Identity

First entity in the sender mem. Used to verify a read mem's entities
become discoverable from a second project after install.

## Purpose

Exercises the export → install round trip via the CLI.
"#;
    fs::write(dir.join("alpha.md"), alpha_body).unwrap();

    let beta_body = r#"---
type: spec
created_date: 2026-01-02
last_modified: 2026-01-02
level: M0
---
# Beta

## Identity

Second entity in the sender mem.

## Purpose

Provides a second hit for the search test below.
"#;
    fs::write(dir.join("beta.md"), beta_body).unwrap();

    commit_mem_branch(
        root,
        "sender-mem",
        &[("alpha.md", alpha_body), ("beta.md", beta_body)],
    );
    dir
}

/// Build an empty write mem (no entities) under `<root>/receiver-mem/`
/// the consumer installs into. Returns the mem's absolute path.
///
/// Also lays down `<root>/mem-repo/.git/` so the CLI's workspace walk-up
/// resolves `<root>` and the engine's fail-fast accepts the workspace.
fn make_receiver_mem(root: &Path) -> std::path::PathBuf {
    let dir = root.join("receiver-mem");
    fs::create_dir_all(&dir).unwrap();
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{ "version": "0.1.0", "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    init_real_mem_repo_from_disk(root, &[(&dir, "receiver-mem")]);
    dir
}

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

#[test]
fn export_markdown_default_runs() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_sender_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .arg("export")
        .assert()
        .success()
        .stdout(contains("Export — markdown"));
}

/// Workspace-wide
/// `memstead export --format markdown` against a mem-repo workspace
/// (every mount is `MountStorage::GitBranch`) completes the folder
/// mounts (zero, in this fixture) and lists the declined git-branch
/// mounts under `## Skipped mounts` in markdown mode. The exit code
/// stays 0 (partial-success path).
#[test]
fn export_markdown_workspace_wide_reports_skipped_git_branch_mounts() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_sender_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .arg("export")
        .assert()
        .success()
        .stdout(contains("Export — markdown"))
        .stdout(contains("Skipped mounts"))
        .stdout(contains("sender-mem"))
        .stdout(contains("git-branch"))
        .stdout(contains("backend_does_not_support_markdown_export"));
}

/// Per-mem
/// `memstead export --format markdown --mem-name <git-branch-mem>`
/// returns the typed `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` refusal.
/// Exit code is the validation-class code; the stderr message carries
/// the recovery hint (`--format mem`). The pre-fix shape — exit-0
/// with `Written: 0, Unchanged: 0` — is unreachable here.
#[test]
fn export_markdown_per_mem_refuses_on_git_branch_backend() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_sender_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "markdown", "--mem", "sender-mem"])
        .assert()
        .failure()
        .stderr(contains("MARKDOWN_EXPORT_UNSUPPORTED_BACKEND"))
        .stderr(contains("git-branch"))
        .stderr(contains("--format mem"));
}

/// The `--json` envelope under
/// per-mem refusal carries the typed `code` and structured details
/// (`mem`, `active_backend`, `supported_backends`). Agents key on
/// the code to branch their recovery.
#[test]
fn export_markdown_per_mem_refuses_with_json_envelope() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_sender_mem(tmp.path());

    let assert = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "export",
            "--format",
            "markdown",
            "--mem",
            "sender-mem",
        ])
        .assert()
        .failure();

    // Under `--json` the error envelope rides stdout.
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let env: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("expected JSON error envelope on stdout; got:\n{stdout}\n({e})")
    });
    assert_eq!(env["code"], "MARKDOWN_EXPORT_UNSUPPORTED_BACKEND");
    assert_eq!(env["details"]["mem"], "sender-mem");
    assert_eq!(env["details"]["active_backend"], "git-branch");
    assert_eq!(
        env["details"]["supported_backends"],
        serde_json::json!(["folder"])
    );
}

/// Workspace-wide `--json`
/// envelope carries `skipped_mounts` as a structured array. Scripts
/// branch on this without parsing the markdown stdout.
#[test]
fn export_markdown_workspace_wide_json_carries_skipped_mounts() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_sender_mem(tmp.path());

    let assert = memstead()
        .current_dir(tmp.path())
        .args(["--json", "export"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let env: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON envelope on stdout; got:\n{stdout}\n({e})"));
    assert_eq!(env["written"], 0);
    assert_eq!(env["unchanged"], 0);
    let skipped = env["skipped_mounts"].as_array().expect(
        "skipped_mounts must be a JSON array under the workspace-wide partial-success shape",
    );
    assert_eq!(skipped.len(), 1, "one git-branch-mount in this fixture");
    assert_eq!(skipped[0]["mem"], "sender-mem");
    assert_eq!(skipped[0]["active_backend"], "git-branch");
    assert_eq!(
        skipped[0]["reason"],
        "backend_does_not_support_markdown_export"
    );
}

#[test]
fn export_mem_produces_memstead_archive() {
    let _guard = cache_guard().lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let mem = make_sender_mem(tmp.path());

    let output_path = mem.join("sender-mem-1.0.0.mem");

    memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "mem", "-o"])
        .arg(&output_path)
        .assert()
        .success()
        .stdout(contains("Exported `sender-mem` v1.0.0"))
        .stdout(contains("sender-mem-1.0.0.mem"));

    assert!(output_path.exists(), "archive must be written to disk");
    assert!(
        fs::metadata(&output_path).unwrap().len() > 0,
        "archive must not be empty"
    );
}

#[test]
fn export_mem_fails_without_version() {
    let tmp = TempDir::new().unwrap();
    // Dir basename equals the declared name so the basename-invariant
    // does not reject this fixture before the version-missing check fires.
    let mem = tmp.path().join("unversioned");
    let memstead_dir = mem.join(".memstead");
    fs::create_dir_all(&memstead_dir).unwrap();
    // Version deliberately omitted.
    fs::write(
        memstead_dir.join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    init_real_mem_repo_from_disk(tmp.path(), &[(&mem, "unversioned")]);

    memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "mem", "-o"])
        .arg(mem.join("out.mem"))
        .assert()
        .failure()
        .stderr(contains("version"));
}

/// F1: `memstead mem set-version` against a mem-repo workspace bumps
/// the version on the `__MEMSTEAD:mems/<name>/config.json` blob (via
/// the backend's `write_mem_config` trait method). Backend-symmetric
/// counterpart to `memstead-cli/tests/write_commands.rs::mem_set_version_persists_through_filesystem_backend`,
/// which covers the folder backend. Verifies on-disk persistence by
/// running `set-version` a second time in a fresh CLI process: the
/// second call's `Old version` line is sourced from the persisted
/// `__MEMSTEAD`-mirrored config (re-loaded across the process boundary),
/// not from in-memory engine state — so seeing the first bump there
/// proves it survived.
#[test]
fn mem_set_version_persists_through_mem_repo_backend() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_sender_mem(tmp.path());

    // Bump the fixture's seeded version (1.0.0) to 1.5.0.
    memstead()
        .current_dir(tmp.path())
        .args(["mem", "set-version", "sender-mem", "1.5.0"])
        .assert()
        .success()
        .stdout(contains("New version: 1.5.0"));

    // Second bump in a fresh process — the `Old version: 1.5.0` line
    // can only come from the persisted on-disk config (the previous
    // engine instance has exited). Proves the prior write reached
    // `__MEMSTEAD:mems/sender-mem/config.json`, not just RAM.
    memstead()
        .current_dir(tmp.path())
        .args(["mem", "set-version", "sender-mem", "2.0.0"])
        .assert()
        .success()
        .stdout(contains("Old version: 1.5.0"))
        .stdout(contains("New version: 2.0.0"));

    // Malformed semver refuses with INVALID_INPUT.
    memstead()
        .current_dir(tmp.path())
        .args(["mem", "set-version", "sender-mem", "not-a-semver"])
        .assert()
        .failure();
}

/// `memstead mem set-description` persists the one-line card text the
/// archive export embeds, survives a process boundary (the second
/// call's `Old:` line reads the persisted config), and an empty string
/// clears the field. Same backend path as set-version
/// (`write_mem_config` onto `__MEMSTEAD:mems/<name>/config.json`).
#[test]
fn mem_set_description_persists_and_clears() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_sender_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args([
            "mem",
            "set-description",
            "sender-mem",
            "Typed knowledge about senders.",
        ])
        .assert()
        .success()
        .stdout(contains("New: Typed knowledge about senders."));

    // Fresh process: the old value can only come from the persisted config.
    memstead()
        .current_dir(tmp.path())
        .args(["mem", "set-description", "sender-mem", "Sharper card text."])
        .assert()
        .success()
        .stdout(contains("Old: Typed knowledge about senders."))
        .stdout(contains("New: Sharper card text."));

    // Empty string clears.
    memstead()
        .current_dir(tmp.path())
        .args(["mem", "set-description", "sender-mem", ""])
        .assert()
        .success()
        .stdout(contains("New: <cleared>"));
}

/// `set-version`
/// accepts `--note` like the other commit-producing mem-lifecycle
/// commands, and the note rides the `__MEMSTEAD` version-bump commit body.
#[test]
fn mem_set_version_note_lands_on_commit() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_sender_mem(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args([
            "mem",
            "set-version",
            "sender-mem",
            "1.5.0",
            "--note",
            "bump for the auth release",
        ])
        .assert()
        .success()
        .stdout(contains("New version: 1.5.0"));

    // The note lands on the config-commit body on `refs/heads/__MEMSTEAD`
    // (the ref the version bump writes through).
    let gitdir = tmp.path().join("mem-repo").join(".git");
    let repo = gix::open(&gitdir).expect("open mem-repo gitdir");
    let commit = repo
        .find_reference("refs/heads/__MEMSTEAD")
        .expect("find __MEMSTEAD")
        .into_fully_peeled_id()
        .expect("peel __MEMSTEAD to id")
        .object()
        .expect("load __MEMSTEAD object")
        .try_into_commit()
        .expect("__MEMSTEAD is a commit");
    let message = commit.message_raw().expect("commit message").to_string();
    assert!(
        message.contains("bump for the auth release"),
        "version-bump note must land on the __MEMSTEAD commit body; got:\n{message}"
    );
}

/// `memstead mem set-sync-state` persists an opaque token into the
/// `__MEMSTEAD:mems/<name>/config.json` blob (via the backend's
/// `write_mem_config`), exactly like `set-version`. Verifies on-disk
/// persistence and the set-vs-overwrite-vs-clear reporting by running
/// the command across fresh CLI processes (the previous engine instance
/// has exited, so the `overwrote`/`cleared` lines can only come from the
/// persisted config).
#[test]
fn mem_set_sync_state_persists_and_reports() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_sender_mem(tmp.path());

    // First write: a fresh key is "set".
    memstead()
        .current_dir(tmp.path())
        .args([
            "mem",
            "set-sync-state",
            "sender-mem",
            "engine-graph/source-files",
            "cafef00d",
        ])
        .assert()
        .success()
        .stdout(contains("sync state set"))
        .stdout(contains("engine-graph/source-files"));

    // Second write in a fresh process — "overwrote" can only come from
    // the persisted on-disk config (the prior engine instance exited).
    memstead()
        .current_dir(tmp.path())
        .args([
            "mem",
            "set-sync-state",
            "sender-mem",
            "engine-graph/source-files",
            "f00dcafe",
        ])
        .assert()
        .success()
        .stdout(contains("sync state overwrote"));

    // Empty token clears the key.
    memstead()
        .current_dir(tmp.path())
        .args([
            "mem",
            "set-sync-state",
            "sender-mem",
            "engine-graph/source-files",
            "",
        ])
        .assert()
        .success()
        .stdout(contains("sync state cleared"));

    // Unknown mem refuses (UNKNOWN_MEM → non-zero exit).
    memstead()
        .current_dir(tmp.path())
        .args(["mem", "set-sync-state", "no-such-mem", "k", "v"])
        .assert()
        .failure();
}

#[test]
fn install_round_trip_entities_discoverable() {
    let _guard = cache_guard().lock().unwrap();
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_mem = make_sender_mem(sender.path());
    let _receiver_mem = make_receiver_mem(receiver.path());

    let archive = sender_mem.join("sender-mem-1.0.0.mem");

    // Step 1: export the sender mem as a .memstead archive.
    memstead()
        .current_dir(sender.path())
        .args(["export", "--format", "mem", "-o"])
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();

    assert!(archive.exists(), "export must produce the archive");

    // Step 2: install the archive into the receiver project.
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("Installed `sender-mem`"));

    // Step 3: sender mem's entities are now discoverable from receiver.
    memstead()
        .current_dir(receiver.path())
        .args(["search", "Alpha"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("sender-mem--alpha"));

    // The entity itself is readable directly by ID.
    memstead()
        .current_dir(receiver.path())
        .args(["entity", "sender-mem--alpha"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("# Alpha"));
}

/// The install pipeline refuses an archive whose authoritative name
/// matches a writable mount in the target workspace. Otherwise the
/// install would report success while the boot-time `hydrate_read_mems`
/// silently skipped the read-mem registration because writable shadows
/// the read-mem — net effect a no-op-with-success-message.
#[test]
fn install_refuses_when_archive_shadows_writable() {
    let _guard = cache_guard().lock().unwrap();
    let sender = TempDir::new().unwrap();
    // The "receiver" workspace is set up with a writable mem
    // whose name *matches* the sender archive's authoritative name
    // (`sender-mem`). Installing the archive must refuse rather
    // than silently no-op.
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_mem = make_sender_mem(sender.path());

    // Build the archive in the sender workspace.
    let archive = sender_mem.join("sender-mem-1.0.0.mem");
    memstead()
        .current_dir(sender.path())
        .args(["export", "--format", "mem", "-o"])
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();

    // Seed the receiver workspace with a writable mem named
    // `sender-mem` so the install would collide.
    let receiver_mem_dir = receiver.path().join("sender-mem");
    fs::create_dir_all(&receiver_mem_dir).unwrap();
    let memstead_dir = receiver_mem_dir.join(".memstead");
    fs::create_dir_all(&memstead_dir).unwrap();
    fs::write(
        memstead_dir.join("config.json"),
        r#"{ "version": "0.1.0", "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    init_real_mem_repo_from_disk(receiver.path(), &[(&receiver_mem_dir, "sender-mem")]);

    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .failure()
        .stderr(contains("READ_MEM_SHADOWS_WRITABLE"))
        .stderr(contains("already exists as a writable mount"));
}

#[test]
fn install_is_idempotent() {
    let _guard = cache_guard().lock().unwrap();
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_mem = make_sender_mem(sender.path());
    let _receiver_mem = make_receiver_mem(receiver.path());

    let archive = sender_mem.join("sender-mem-1.0.0.mem");

    memstead()
        .current_dir(sender.path())
        .args(["export", "--format", "mem", "-o"])
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();

    // First install: copied + mounted.
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("copied into cache"))
        .stdout(contains("registered as a workspace-level read-only mount"));

    // Second install: no-op on both sides.
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("already in cache"))
        .stdout(contains("already registered as a read-mem mount (unchanged)"));
}

/// After installing an archive as an RO mount: `memstead workspace dump
/// --json` must complete without `MEM_ERROR` (otherwise the dump would
/// iterate every mem and call `gitdir_for` unconditionally, crashing on
/// the first RO mount). `memstead type --mem <ro-name>` must resolve
/// against the RO mount (otherwise the resolver walks `writable_mem_names`
/// only). And the schema_ref on the dump's RO entry matches the archive's
/// bundled config (otherwise `hydrate_read_mems` hardcodes `default@1.0.0`
/// ignoring the archive's actual pin).
#[test]
fn workspace_dump_and_type_resolve_for_ro_mount_after_install() {
    let _guard = cache_guard().lock().unwrap();
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_mem = make_sender_mem(sender.path());
    let _receiver_mem = make_receiver_mem(receiver.path());

    let archive = sender_mem.join("sender-mem-1.0.0.mem");
    memstead()
        .current_dir(sender.path())
        .args(["export", "--format", "mem", "-o"])
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();

    // Workspace dump completes; the RO mount entry carries the
    // `capability: "read_only"` discriminator plus a matching
    // `schema_ref` (pin-fidelity: the value matches what the archive
    // bundled).
    let dump = memstead()
        .current_dir(receiver.path())
        .args(["workspace", "dump", "--json"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let dump_str = String::from_utf8(dump).unwrap();
    let dump_json: serde_json::Value =
        serde_json::from_str(&dump_str).expect("workspace dump must emit valid JSON");
    let mems = dump_json["mems"]
        .as_array()
        .expect("dump must carry a mems array");
    let ro_entry = mems
        .iter()
        .find(|v| v["name"] == "sender-mem")
        .expect("installed sender-mem must appear in dump");
    assert_eq!(
        ro_entry["capability"], "read_only",
        "RO mount must carry the `read_only` capability marker; got {ro_entry}"
    );
    assert_eq!(
        ro_entry["schema_ref"], "default@1.0.0",
        "RO mount's schema_ref must match the archive's bundled pin (pre-fix \
         the dump would have crashed before reaching this assertion); got {ro_entry}"
    );

    // `memstead type --mem <ro-name>` resolves and lists the RO
    // mount's schema types — the resolver reaches read-only mounts, not
    // only writable ones.
    memstead()
        .current_dir(receiver.path())
        .args(["type", "--mem", "sender-mem"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();
}

// ---------------------------------------------------------------------------
// Mem title + subject (display text, not identity)
// ---------------------------------------------------------------------------

/// `mem set-title` / `mem set-subject` round-trip: the roster prefers
/// the title and falls back to the name; clearing restores the
/// fallback; the subject sets and clears as a unit. Refusal
/// complements: identity is untouched — addressing a mem by its title
/// refuses UNKNOWN_MEM while the name keeps resolving.
#[test]
fn mem_title_and_subject_lifecycle() {
    let tmp = TempDir::new().unwrap();
    let _mem = make_sender_mem(tmp.path());

    // Set a title (non-ASCII, spaces — no slug grammar).
    memstead()
        .current_dir(tmp.path())
        .args([
            "mem",
            "set-title",
            "sender-mem",
            "Einrichtungsbezogene Impfpflicht Deutschland",
        ])
        .assert()
        .success()
        .stdout(contains("title updated"));

    // The roster prefers the title; the name stays visible.
    let out = memstead()
        .current_dir(tmp.path())
        .args(["--json", "mem", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let row = &json["mems"][0];
    assert_eq!(row["name"], "sender-mem");
    assert_eq!(row["title"], "Einrichtungsbezogene Impfpflicht Deutschland");
    memstead()
        .current_dir(tmp.path())
        .args(["mem", "list"])
        .assert()
        .success()
        .stdout(contains(
            "Einrichtungsbezogene Impfpflicht Deutschland (`sender-mem`)",
        ));

    // Identity untouched: the title does NOT address the mem…
    memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "search",
            "--mem",
            "Einrichtungsbezogene Impfpflicht Deutschland",
        ])
        .assert()
        .failure()
        .stdout(contains("UNKNOWN_MEM"));
    // …while the name keeps resolving exactly as before.
    memstead()
        .current_dir(tmp.path())
        .args(["--json", "search", "--mem", "sender-mem"])
        .assert()
        .success();

    // Subject sets with scope + method + ordered exclusions…
    memstead()
        .current_dir(tmp.path())
        .args([
            "mem",
            "set-subject",
            "sender-mem",
            "--scope",
            "Die Impfpflicht in Einrichtungen",
            "--method",
            "Primärquellen, händisch geprüft",
            "--exclusion",
            "Länderverordnungen nach 2023",
            "--exclusion",
            "Presseberichte",
        ])
        .assert()
        .success()
        .stdout(contains("subject updated"));

    // …method/exclusion without scope refuses…
    memstead()
        .current_dir(tmp.path())
        .args([
            "mem",
            "set-subject",
            "sender-mem",
            "--method",
            "nur Methode",
        ])
        .assert()
        .failure()
        .stderr(contains("--scope"));

    // …and no fields clears the block as a unit.
    memstead()
        .current_dir(tmp.path())
        .args(["--json", "mem", "set-subject", "sender-mem"])
        .assert()
        .success()
        .stdout(predicates::prelude::PredicateBooleanExt::not(contains(
            "\"new_subject\"",
        )));

    // Clearing the title restores the name fallback.
    memstead()
        .current_dir(tmp.path())
        .args(["mem", "set-title", "sender-mem", ""])
        .assert()
        .success();
    memstead()
        .current_dir(tmp.path())
        .args(["mem", "list"])
        .assert()
        .success()
        .stdout(contains("- `sender-mem`"));
}

// ---------------------------------------------------------------------------
// `export --format json` — the bulk read (one engine boot, whole entity set)
// ---------------------------------------------------------------------------

/// Section bodies written into the JSON-export fixture. Named consts so
/// the byte-faithful assertions below compare against the exact same
/// strings the fixture wrote — a round-trip check, not a re-typing.
const DELTA_IDENTITY: &str = "Multi-section fixture entity for the JSON bulk export.\n\nSecond paragraph with `inline code` and *emphasis* to make the\nround-trip non-trivial.";
const DELTA_PURPOSE: &str = "Verifies byte-faithful section content, metadata, relationships\n(including a cross-mem edge), and `_hash` in one document.";
const GAMMA_IDENTITY: &str = "Cross-mem edge target living in the second writable mem.";

/// Two writable git-branch mems in one workspace: `sender-mem` holding
/// `delta` (multi-section body, metadata, an explicit cross-mem USES
/// edge into `second-mem--gamma`) and `second-mem` holding `gamma`.
fn make_json_export_workspace(root: &Path) {
    let sender = root.join("sender-mem");
    let second = root.join("second-mem");
    for dir in [&sender, &second] {
        let store = dir.join(".memstead");
        fs::create_dir_all(&store).unwrap();
        fs::write(
            store.join("config.json"),
            r#"{ "version": "1.0.0", "schema": "default@1.0.0" }"#,
        )
        .unwrap();
    }

    let delta_body = format!(
        "---\ntype: spec\ncreated_date: 2026-02-01\nlast_modified: 2026-02-01\nlevel: M1\n---\n# Delta\n\n## Identity\n\n{DELTA_IDENTITY}\n\n## Purpose\n\n{DELTA_PURPOSE}\n\n## Relationships\n\n- **USES**: [[second-mem--gamma]]\n"
    );
    let gamma_body = format!(
        "---\ntype: spec\ncreated_date: 2026-02-02\nlast_modified: 2026-02-02\nlevel: M0\n---\n# Gamma\n\n## Identity\n\n{GAMMA_IDENTITY}\n"
    );
    fs::write(sender.join("delta.md"), &delta_body).unwrap();
    fs::write(second.join("gamma.md"), &gamma_body).unwrap();

    init_real_mem_repo_from_disk(root, &[(&sender, "sender-mem"), (&second, "second-mem")]);
    commit_mem_branch(root, "sender-mem", &[("delta.md", &delta_body)]);
    commit_mem_branch(root, "second-mem", &[("gamma.md", &gamma_body)]);
}

/// Run `export --format json` (plus `extra` args) and parse stdout.
fn export_json(root: &Path, extra: &[&str]) -> serde_json::Value {
    let out = memstead()
        .current_dir(root)
        .args(["export", "--format", "json"])
        .args(extra)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).expect("--format json must emit one valid JSON document")
}

/// Criterion 1 (git-branch leg): one invocation, one document, every
/// non-stub entity with id / type / title / metadata / sections /
/// relationships / `_hash` — section bodies byte-identical to what the
/// fixture wrote, the cross-mem edge present with its full target id.
#[test]
fn export_json_git_branch_round_trips_values() {
    let tmp = TempDir::new().unwrap();
    make_json_export_workspace(tmp.path());

    let doc = export_json(tmp.path(), &["--mem", "sender-mem"]);
    assert_eq!(doc["format"], "memstead-export/v1");

    let group = &doc["mems"]["sender-mem"];
    assert_eq!(group["schema"], "default@1.0.0");
    assert_eq!(group["read_only"], false);
    assert_eq!(group["entity_count"], 1);

    let entities = group["entities"].as_array().expect("entities array");
    let delta = entities
        .iter()
        .find(|e| e["id"] == "sender-mem--delta")
        .expect("delta must be exported");
    assert_eq!(delta["type"], "spec");
    assert_eq!(delta["title"], "Delta");
    assert_eq!(delta["mem"], "sender-mem");
    assert_eq!(delta["metadata"]["level"], "M1");
    assert_eq!(delta["metadata"]["created_date"], "2026-02-01");
    assert_eq!(
        delta["sections"]["identity"], DELTA_IDENTITY,
        "identity section must round-trip byte-faithfully; got {}",
        delta["sections"]["identity"]
    );
    assert_eq!(delta["sections"]["purpose"], DELTA_PURPOSE);

    let rels = delta["relationships"].as_array().expect("relationships");
    let uses = rels
        .iter()
        .find(|r| r["rel_type"] == "USES")
        .expect("cross-mem USES edge must be exported");
    assert_eq!(uses["target"], "second-mem--gamma");

    // The engine's `_hash` contract: sha-256 truncated to 16 hex chars
    // (`entity::parser::compute_hash`) — the same value optimistic
    // locking compares against.
    let hash = delta["_hash"].as_str().expect("_hash string");
    assert_eq!(hash.len(), 16, "truncated sha-256 hex expected; got {hash}");
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

/// Criterion 2: the workspace-wide form (no `--mem`) contains every
/// writable mem, grouped, from one invocation. The cross-mem target's
/// stub in `sender-mem`'s namespace must NOT surface as an entity
/// anywhere (non-stub contract).
#[test]
fn export_json_workspace_wide_groups_every_writable_mem() {
    let tmp = TempDir::new().unwrap();
    make_json_export_workspace(tmp.path());

    let doc = export_json(tmp.path(), &[]);
    let mems = doc["mems"].as_object().expect("mems object");
    assert!(mems.contains_key("sender-mem"), "sender-mem group missing");
    assert!(mems.contains_key("second-mem"), "second-mem group missing");

    let gamma = mems["second-mem"]["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "second-mem--gamma")
        .expect("gamma must be exported in its own group");
    assert_eq!(gamma["title"], "Gamma");
    assert_eq!(gamma["sections"]["identity"], GAMMA_IDENTITY);

    for (name, group) in mems {
        for e in group["entities"].as_array().unwrap() {
            assert!(
                e.get("_stub_kind").is_none(),
                "stub leaked into export group `{name}`: {e}"
            );
        }
    }
}

/// Criterion 3: a read-only mount is included when named via `--mem`
/// (with `read_only: true`) and absent from the workspace-wide default.
#[test]
fn export_json_read_only_mount_named_vs_default() {
    let _guard = cache_guard().lock().unwrap();
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_mem = make_sender_mem(sender.path());
    let _receiver_mem = make_receiver_mem(receiver.path());
    let archive = sender_mem.join("sender-mem-1.0.0.mem");
    memstead()
        .current_dir(sender.path())
        .args(["export", "--format", "mem", "-o"])
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();

    // Workspace-wide: only the writable receiver mem.
    let wide = memstead()
        .current_dir(receiver.path())
        .args(["export", "--format", "json"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let wide: serde_json::Value = serde_json::from_slice(&wide).unwrap();
    let mems = wide["mems"].as_object().unwrap();
    assert!(
        !mems.contains_key("sender-mem"),
        "read-only mount must be absent from the workspace-wide default; got {:?}",
        mems.keys().collect::<Vec<_>>()
    );
    assert!(mems.contains_key("receiver-mem"));

    // Named: the read-only mount exports, marked read_only.
    let named = memstead()
        .current_dir(receiver.path())
        .args(["export", "--format", "json", "--mem", "sender-mem"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let named: serde_json::Value = serde_json::from_slice(&named).unwrap();
    let group = &named["mems"]["sender-mem"];
    assert_eq!(group["read_only"], true);
    let ids: Vec<&str> = group["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"sender-mem--alpha") && ids.contains(&"sender-mem--beta"),
        "named RO export must carry the mount's entities; got {ids:?}"
    );
}

/// Criterion 4 (git-branch leg): the mem-repo's refs are identical
/// before and after the export — the command is observably read-only.
#[test]
fn export_json_leaves_mem_repo_refs_untouched() {
    let tmp = TempDir::new().unwrap();
    make_json_export_workspace(tmp.path());
    let gitdir = tmp.path().join("mem-repo").join(".git");

    let refs = |label: &str| -> String {
        let out = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&gitdir)
            .args(["for-each-ref", "--format=%(refname) %(objectname)"])
            .output()
            .unwrap_or_else(|e| panic!("git for-each-ref ({label}): {e}"));
        String::from_utf8(out.stdout).unwrap()
    };

    let before = refs("before");
    assert!(
        before.contains("refs/heads/sender-mem"),
        "fixture must have seeded the branch; got: {before}"
    );
    let _ = export_json(tmp.path(), &[]);
    let after = refs("after");
    assert_eq!(before, after, "export must not move any mem-repo ref");
}

/// Criteria 1 + 4 (folder-mem legs): the same invocation shape serves a
/// folder-backed mount — backend-uniform, byte-faithful — and no
/// mem-content file changes on disk.
#[test]
fn export_json_folder_mem_round_trips_and_mutates_nothing() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let store = root.join(".memstead");
    fs::create_dir_all(store.join("state")).unwrap();
    fs::write(
        store.join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    fs::write(
        store.join("state").join("mounts.json"),
        r#"{"format":"memstead-mounts-3","mounts":[{"mem":"engine","schema":"default@1.0.0","storage":{"type":"folder","path":"engine-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}]}"#,
    )
    .unwrap();
    let mem_dir = root.join("engine-mem");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0"}"#,
    )
    .unwrap();
    let epsilon_body = format!(
        "---\ntype: spec\ncreated_date: 2026-02-03\nlast_modified: 2026-02-03\nlevel: M0\n---\n# Epsilon\n\n## Identity\n\n{DELTA_IDENTITY}\n\n## Purpose\n\n{DELTA_PURPOSE}\n"
    );
    fs::write(mem_dir.join("epsilon.md"), &epsilon_body).unwrap();

    // Snapshot every mem-content file before the export.
    let snapshot = |label: &str| -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<(String, Vec<u8>)> = walk_files(&mem_dir)
            .into_iter()
            .map(|p| {
                let rel = p.strip_prefix(&mem_dir).unwrap().to_string_lossy().into_owned();
                let bytes = fs::read(&p).unwrap_or_else(|e| panic!("read {p:?} ({label}): {e}"));
                (rel, bytes)
            })
            .collect();
        files.sort();
        files
    };
    let before = snapshot("before");

    let doc = export_json(root, &["--mem", "engine"]);
    let group = &doc["mems"]["engine"];
    assert_eq!(group["read_only"], false);
    let epsilon = group["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "engine--epsilon")
        .expect("epsilon must be exported from the folder mount");
    assert_eq!(epsilon["title"], "Epsilon");
    assert_eq!(epsilon["sections"]["identity"], DELTA_IDENTITY);
    assert_eq!(epsilon["sections"]["purpose"], DELTA_PURPOSE);

    let after = snapshot("after");
    assert_eq!(before, after, "export must not change any mem-content file");
}

/// Recursively collect every file under `dir`.
fn walk_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            out.extend(walk_files(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// Criterion 5: unknown mem refuses with the existing typed not-found
/// code; `-o` (a `--format mem`-only flag) combined with `--format json`
/// refuses rather than silently ignoring; neither leaf is INTERNAL.
#[test]
fn export_json_refusals_are_typed() {
    let tmp = TempDir::new().unwrap();
    make_json_export_workspace(tmp.path());

    let unknown = memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "json", "--mem", "no-such-mem"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let unknown = String::from_utf8(unknown).unwrap();
    assert!(unknown.contains("UNKNOWN_MEM"), "got: {unknown}");
    assert!(!unknown.contains("INTERNAL"), "got: {unknown}");

    let with_output = memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "json", "-o", "dump.json"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let with_output = String::from_utf8(with_output).unwrap();
    assert!(with_output.contains("INVALID_INPUT"), "got: {with_output}");
    assert!(!with_output.contains("INTERNAL"), "got: {with_output}");
    assert!(
        !tmp.path().join("dump.json").exists(),
        "refusal must not have written the file"
    );
}

// ---------------------------------------------------------------------------
// `mem rename` — complete rename across every name-bearing surface
// ---------------------------------------------------------------------------

/// Build a two-mem workspace end-to-end through the CLI: `alpha` (two
/// entities, one full-id self-reference wiki-link, one anchor) and
/// `beta` (one entity with a cross-mem edge and a cross-mem wiki-link
/// into `alpha`), with the `beta → alpha` grant in place. Everything
/// goes through the product surface — no raw seeding — so the fixture
/// state is exactly what a real workspace reaches.
fn make_rename_workspace(root: &Path) {
    let m = |args: &[&str]| {
        memstead()
            .current_dir(root)
            .env("MEMSTEAD_OPERATOR_MODE", "1")
            .args(args)
            .assert()
            .success();
    };
    m(&["mem-repo", "init", "."]);
    m(&["mem", "init", "alpha", "--no-gitignore"]);
    m(&["mem", "init", "beta", "--no-gitignore"]);
    m(&["workspace", "grant-cross-link", "beta", "alpha"]);
    let corpus = serde_json::json!({ "creates": [
        {"title": "One", "entity_type": "spec", "mem": "alpha",
         "sections": {"identity": "First entity.", "purpose": "Rename fixture."}},
        {"title": "Two", "entity_type": "spec", "mem": "alpha",
         "sections": {"identity": "Second entity, see [[alpha--one]] full form.", "purpose": "Rename fixture."},
         "relations": [{"type": "USES", "to": "alpha--one"}]},
        {"title": "Watcher", "entity_type": "spec", "mem": "beta",
         "sections": {"identity": "Watches [[alpha--one]] from beta.", "purpose": "Cross-mem referrer."},
         "relations": [{"type": "DEPENDS_ON", "to": "alpha--two"}]},
    ]});
    let corpus_path = root.join("corpus.json");
    fs::write(&corpus_path, serde_json::to_string(&corpus).unwrap()).unwrap();
    m(&["batch-create", "--from", corpus_path.to_str().unwrap()]);
    fs::remove_file(&corpus_path).unwrap();
    m(&[
        "update",
        "alpha--one",
        "--auto-hash",
        "--append",
        "purpose= Anchored.",
        "--anchor",
        r#"{"artifact": "src/lib.rs", "grain": "file", "class": "anchored"}"#,
    ]);
    m(&["mem", "set-sync-state", "alpha", "alpha/graph/tree#synced", "abc123"]);
}

/// Criteria 1 + 2: the rename is complete across entity ids, cross-mem
/// edges, wiki-links, grants, anchors, and sync-state keys; commit
/// history is preserved (a branch move, not a fresh seed); and
/// `memstead health --strict` stays clean.
#[test]
fn mem_rename_git_branch_full_surface() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_rename_workspace(root);

    memstead()
        .current_dir(root)
        .env("MEMSTEAD_OPERATOR_MODE", "1")
        .args(["mem", "rename", "alpha", "gamma"])
        .assert()
        .success()
        .stdout(contains("renamed to `gamma`"));

    // New ids resolve; old ids do not.
    memstead()
        .current_dir(root)
        .args(["entity", "gamma--one"])
        .assert()
        .success();
    memstead()
        .current_dir(root)
        .args(["entity", "alpha--one"])
        .assert()
        .failure()
        .stderr(contains("ENTITY_NOT_FOUND"));

    // The cross-mem referrer in `beta` carries the new prefix in both
    // its wiki-link and its Relationships entry.
    memstead()
        .current_dir(root)
        .args(["entity", "beta--watcher"])
        .assert()
        .success()
        .stdout(contains("[[gamma--one]]"))
        .stdout(contains("[[gamma--two]]"))
        .stdout(predicates::prelude::PredicateBooleanExt::not(contains(
            "alpha--",
        )));

    // Grants rewritten (value side).
    let toml = fs::read_to_string(root.join(".memstead/workspace.toml")).unwrap();
    assert!(
        toml.contains(r#"beta = ["gamma"]"#),
        "grant must name the new mem; got:\n{toml}"
    );
    assert!(!toml.contains("alpha"), "no grant may still name the old mem:\n{toml}");

    // Branch moved with history: the create-time seed commit is still
    // reachable from the new branch, and the old branch is gone.
    let log = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(root.join("mem-repo/.git"))
        .args(["log", "--oneline", "refs/heads/gamma"])
        .output()
        .unwrap();
    let log = String::from_utf8(log.stdout).unwrap();
    assert!(
        log.contains("create mem alpha"),
        "history must be preserved across the rename; got:\n{log}"
    );
    let refs = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(root.join("mem-repo/.git"))
        .args(["for-each-ref", "--format=%(refname)"])
        .output()
        .unwrap();
    let refs = String::from_utf8(refs.stdout).unwrap();
    assert!(!refs.contains("refs/heads/alpha"), "old branch must be gone:\n{refs}");

    // Anchors sidecar re-keyed on the moved branch.
    let sidecar = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(root.join("mem-repo/.git"))
        .args(["show", "refs/heads/gamma:.memstead/anchors.json"])
        .output()
        .unwrap();
    let sidecar = String::from_utf8(sidecar.stdout).unwrap();
    assert!(
        sidecar.contains("gamma--one") && !sidecar.contains("alpha--one"),
        "anchors must key on the new id; got:\n{sidecar}"
    );

    // Sync-state keys follow the mem name.
    let config = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(root.join("mem-repo/.git"))
        .args(["show", "refs/heads/__MEMSTEAD:mems/gamma/config.json"])
        .output()
        .unwrap();
    let config = String::from_utf8(config.stdout).unwrap();
    assert!(
        config.contains("gamma/graph/tree#synced") && !config.contains("alpha/"),
        "sync-state keys must carry the new mem name; got:\n{config}"
    );

    // Criterion 2: health --strict is clean after the rename.
    memstead()
        .current_dir(root)
        .args(["health", "--strict"])
        .assert()
        .success();
}

/// Criteria 3 + 4 + 7: every refusal fires with its typed code, never
/// `INTERNAL`, and leaves the workspace byte-identical (refs,
/// workspace.toml, mounts.json all unchanged).
#[test]
fn mem_rename_refusals_are_typed_and_effect_free() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_rename_workspace(root);

    let snapshot = |label: &str| -> (String, String, String) {
        let refs = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(root.join("mem-repo/.git"))
            .args(["for-each-ref", "--format=%(refname) %(objectname)"])
            .output()
            .unwrap_or_else(|e| panic!("for-each-ref ({label}): {e}"));
        (
            String::from_utf8(refs.stdout).unwrap(),
            fs::read_to_string(root.join(".memstead/workspace.toml")).unwrap(),
            fs::read_to_string(root.join(".memstead/state/mounts.json")).unwrap(),
        )
    };
    let before = snapshot("before");

    let refusals: &[(&[&str], &str, bool)] = &[
        (&["mem", "rename", "nope", "other"], "UNKNOWN_MEM", true),
        (&["mem", "rename", "alpha", "beta"], "MEM_NAME_COLLISION", true),
        (&["mem", "rename", "alpha", "Bad Name"], "INVALID_MEM_NAME", true),
        (&["mem", "rename", "alpha", "alpha"], "INVALID_INPUT", true),
        // Agent mode (no operator env): no allowlist configured →
        // MEM_PATH_NOT_ALLOWED with the policy_table disambiguator.
        (&["mem", "rename", "alpha", "delta"], "MEM_PATH_NOT_ALLOWED", false),
    ];
    for (args, code, operator) in refusals {
        let mut cmd = memstead();
        cmd.current_dir(root);
        if *operator {
            cmd.env("MEMSTEAD_OPERATOR_MODE", "1");
        } else {
            cmd.env_remove("MEMSTEAD_OPERATOR_MODE");
        }
        let out = cmd.args(*args).assert().failure().get_output().stderr.clone();
        let stderr = String::from_utf8(out).unwrap();
        assert!(stderr.contains(code), "expected {code}; got: {stderr}");
        assert!(!stderr.contains("INTERNAL"), "INTERNAL leaked: {stderr}");
        if *code == "MEM_PATH_NOT_ALLOWED" {
            assert!(
                stderr.contains("mem_management.delete"),
                "policy_table disambiguator missing: {stderr}"
            );
        }
    }

    let after = snapshot("after");
    assert_eq!(before, after, "a refused rename must leave the workspace byte-identical");
}

/// Criterion 5: a half-applied rename — the peer mem already rewritten
/// to the new prefix, the identity flip not yet done — is (a) visible
/// to `health --strict` as findings, and (b) completed by re-issuing
/// the same `mem rename`.
#[test]
fn mem_rename_interrupted_half_state_detectable_and_completable() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_rename_workspace(root);

    // Simulate the interruption: `beta` has already been swept
    // (references `gamma--…`), `alpha` has not flipped yet. The sweep
    // commit is reproduced through the same backend path the real
    // sweep uses (a raw branch commit against beta's ref).
    let watcher_body = "---\ntype: spec\ncreated_date: 2026-02-01\nlast_modified: 2026-02-01\nlevel: M0\n---\n# Watcher\n\n## Identity\n\nWatches [[gamma--one]] from beta.\n\n## Purpose\n\nCross-mem referrer.\n\n## Relationships\n\n- **DEPENDS_ON**: [[gamma--two]]\n";
    commit_mem_branch(root, "beta", &[("watcher.md", watcher_body)]);

    // (a) The half-state is never silent: the dangling `gamma--…`
    // references (mem `gamma` not yet mounted) surface as a stub in
    // the health report. (Strict mode's exit code gates schema
    // violations, not stub counts — the criterion binds the
    // *reporting*, which is the stub line.)
    memstead()
        .current_dir(root)
        .args(["health", "--strict"])
        .assert()
        .stdout(contains("Stubs: 1"));

    // (b) Re-issuing the same rename completes it.
    memstead()
        .current_dir(root)
        .env("MEMSTEAD_OPERATOR_MODE", "1")
        .args(["mem", "rename", "alpha", "gamma"])
        .assert()
        .success();
    memstead()
        .current_dir(root)
        .args(["entity", "gamma--one"])
        .assert()
        .success();
    memstead()
        .current_dir(root)
        .args(["health", "--strict"])
        .assert()
        .success()
        .stdout(contains("Stubs: 0"));
}

/// Criterion 1 (folder leg): a folder-backed mount renames — the mount
/// identity flips, entities re-id, the folder's on-disk location stays.
#[test]
fn mem_rename_folder_mount() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let store = root.join(".memstead");
    fs::create_dir_all(store.join("state")).unwrap();
    fs::write(
        store.join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    // A folder-backed MOUNT inside a mem-repo workspace: the `mem`
    // subcommand family requires the mem-repo flavour (a folder-only
    // workspace boots the filesystem flavour, where `mem` refuses), so
    // the fixture lays down the bare mem-repo alongside the folder
    // mount.
    memstead_git_branch::test_support::init_real_mem_repo(root, &[]);
    fs::write(
        store.join("state").join("mounts.json"),
        r#"{"format":"memstead-mounts-3","mounts":[{"mem":"engine","schema":"default@1.0.0","storage":{"type":"folder","path":"engine-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}]}"#,
    )
    .unwrap();
    let mem_dir = root.join("engine-mem");
    fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0"}"#,
    )
    .unwrap();
    fs::write(
        mem_dir.join("solo.md"),
        "---\ntype: spec\ncreated_date: 2026-02-01\nlast_modified: 2026-02-01\nlevel: M0\n---\n# Solo\n\n## Identity\n\nFolder-mount rename fixture.\n\n## Purpose\n\nTesting.\n",
    )
    .unwrap();

    memstead()
        .current_dir(root)
        .env("MEMSTEAD_OPERATOR_MODE", "1")
        .args(["mem", "rename", "engine", "motor"])
        .assert()
        .success();

    memstead()
        .current_dir(root)
        .args(["entity", "motor--solo"])
        .assert()
        .success();
    memstead()
        .current_dir(root)
        .args(["entity", "engine--solo"])
        .assert()
        .failure();
    assert!(
        root.join("engine-mem").join("solo.md").exists(),
        "folder location is not identity — the directory stays in place"
    );
    let mounts = fs::read_to_string(store.join("state").join("mounts.json")).unwrap();
    assert!(
        mounts.contains(r#""motor""#) && !mounts.contains(r#""engine""#),
        "mounts.json must carry the new name; got:\n{mounts}"
    );
}

/// Constraint: read-only mounts cannot be renamed (their identity is
/// the archive's internal name).
#[test]
fn mem_rename_read_only_mount_refuses() {
    let _guard = cache_guard().lock().unwrap();
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_mem = make_sender_mem(sender.path());
    let _receiver_mem = make_receiver_mem(receiver.path());
    let archive = sender_mem.join("sender-mem-1.0.0.mem");
    memstead()
        .current_dir(sender.path())
        .args(["export", "--format", "mem", "-o"])
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();

    let out = memstead()
        .current_dir(receiver.path())
        .env("MEMSTEAD_OPERATOR_MODE", "1")
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .args(["mem", "rename", "sender-mem", "other-name"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8(out).unwrap();
    assert!(
        stderr.contains("READ_ONLY_MOUNT") || stderr.contains("read-only"),
        "renaming an RO mount must refuse with the read-only code; got: {stderr}"
    );
    assert!(!stderr.contains("INTERNAL"), "INTERNAL leaked: {stderr}");
}

// ---------------------------------------------------------------------------
// Read-mems as workspace-level mounts: install / uninstall / migration
// ---------------------------------------------------------------------------

/// Export `sender-mem` as an archive and return its path (fixture half
/// shared by the mount-model tests).
fn export_sender_archive(sender_root: &Path, sender_mem: &Path) -> std::path::PathBuf {
    let archive = sender_mem.join("sender-mem-1.0.0.mem");
    memstead()
        .current_dir(sender_root)
        .args(["export", "--format", "mem", "-o"])
        .arg(&archive)
        .assert()
        .success();
    archive
}

/// Criterion 1: install produces a workspace-level read-only mount —
/// `mem list` shows it with `read_only`, `mounts.json` carries the
/// archive mount, and no writable mem's config gains a `readMems` key.
#[test]
fn install_registers_workspace_read_only_mount() {
    let _guard = cache_guard().lock().unwrap_or_else(|e| e.into_inner());
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_mem = make_sender_mem(sender.path());
    let _receiver_mem = make_receiver_mem(receiver.path());
    let archive = export_sender_archive(sender.path(), &sender_mem);

    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("registered as a workspace-level read-only mount"));

    memstead()
        .current_dir(receiver.path())
        .args(["mem", "list"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("`sender-mem` (read_only)"));
    memstead()
        .current_dir(receiver.path())
        .args(["overview"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("sender-mem"));

    let mounts =
        fs::read_to_string(receiver.path().join(".memstead/state/mounts.json")).unwrap();
    assert!(
        mounts.contains(r#""sender-mem""#) && mounts.contains(r#""archive""#),
        "mounts.json must carry the archive mount; got:\n{mounts}"
    );

    // No writable mem's config gained a readMems entry.
    let cfg = memstead_git_branch::mem_repo_config::read_config(receiver.path(), "receiver-mem")
        .expect("receiver config readable");
    assert!(
        cfg.read_mems.is_empty(),
        "install must not write readMems; got {:?}",
        cfg.read_mems
    );
}

/// Criterion 2 + parity (fresh-install leg of criterion 5): uninstall
/// removes the mount (searchability gone), the cache copy survives, a
/// re-install re-registers cleanly, and reads against the installed
/// read-mem work — search hit plus a cross-mem wiki-link into it.
#[test]
fn uninstall_round_trip_cache_survives_and_reads_have_parity() {
    let _guard = cache_guard().lock().unwrap_or_else(|e| e.into_inner());
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_mem = make_sender_mem(sender.path());
    let _receiver_mem = make_receiver_mem(receiver.path());
    let archive = export_sender_archive(sender.path(), &sender_mem);

    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();

    // Reads: search reaches the installed content; a writable entity
    // may hold a cross-mem wiki-link into the read-mem (grant first —
    // same policy gate as before the model change).
    memstead()
        .current_dir(receiver.path())
        .args(["search", "Alpha"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("sender-mem--alpha"));
    memstead()
        .current_dir(receiver.path())
        .env("MEMSTEAD_OPERATOR_MODE", "1")
        .args(["workspace", "grant-cross-link", "receiver-mem", "sender-mem"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();
    memstead()
        .current_dir(receiver.path())
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .args([
            "create",
            "--mem",
            "receiver-mem",
            "--title",
            "Bridge",
            "--type",
            "spec",
            "--section",
            "identity=Links into [[sender-mem--alpha]] from the writable side.",
            "--section",
            "purpose=Cross-mem parity probe.",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(receiver.path())
        .args(["entity", "receiver-mem--bridge"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("[[sender-mem--alpha]]"));

    // Uninstall refuses while the bridge edge exists (incoming-refs
    // gate), then succeeds once the referrer is gone.
    memstead()
        .current_dir(receiver.path())
        .arg("uninstall")
        .arg("sender-mem")
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .failure()
        .stderr(contains("MEM_HAS_INCOMING_REFS"))
        .stderr(contains("receiver-mem--bridge"));
    memstead()
        .current_dir(receiver.path())
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .args(["delete", "receiver-mem--bridge"])
        .assert()
        .success();
    memstead()
        .current_dir(receiver.path())
        .arg("uninstall")
        .arg("sender-mem")
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("archive copy retained"));

    // Mount gone, cache copy survives.
    memstead()
        .current_dir(receiver.path())
        .args(["search", "Alpha"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(predicates::prelude::PredicateBooleanExt::not(contains(
            "sender-mem--alpha",
        )));
    let cached: Vec<_> = fs::read_dir(cache.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("sender-mem-") && n.ends_with(".mem"))
        .collect();
    assert_eq!(cached.len(), 1, "cache copy must survive uninstall: {cached:?}");

    // Re-install re-registers from the surviving cache copy.
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("already in cache"))
        .stdout(contains("registered as a workspace-level read-only mount"));
}

/// Criterion 3 + the pre-migration leg of criterion 5: a workspace with
/// legacy `readMems` entries boots, migrates them to mounts, removes
/// the legacy key, and surfaces one warning naming the migrated mems; a
/// second boot is silent; reads (search + cross-mem link) behave
/// identically on the migrated fixture.
#[test]
fn legacy_read_mems_config_migrates_at_boot() {
    let _guard = cache_guard().lock().unwrap_or_else(|e| e.into_inner());
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_mem = make_sender_mem(sender.path());
    let _receiver_mem = make_receiver_mem(receiver.path());
    let archive = export_sender_archive(sender.path(), &sender_mem);

    // Seed the global cache without touching the receiver workspace
    // (the legacy fixture must be the FIRST thing the receiver sees).
    unsafe {
        std::env::set_var("MEMSTEAD_MEM_CACHE", cache.path());
    }
    let cached = memstead_git_branch::mem_cache::install_to_cache(&archive, &[]).unwrap();
    unsafe {
        std::env::remove_var("MEMSTEAD_MEM_CACHE");
    }

    // Write the legacy readMems registration into the receiver's config
    // through the engine-owned writer — the exact shape the pre-mount
    // installer produced.
    let mut cfg =
        memstead_git_branch::mem_repo_config::read_config(receiver.path(), "receiver-mem")
            .expect("receiver config readable");
    cfg.read_mems.insert(
        "sender-mem".to_string(),
        memstead_schema::config::ReadMemSpec {
            source: memstead_schema::config::ReadMemSource::Local,
            cache_key: Some(cached.cache_key.clone()),
        },
    );
    let mut bytes = serde_json::to_vec_pretty(&cfg).unwrap();
    bytes.push(b'\n');
    memstead_git_branch::mem_repo_config::commit_config(
        receiver.path(),
        "receiver-mem",
        &bytes,
        &memstead_git_branch::vcs::CommitContext::internal(),
        "test: legacy readMems fixture",
    )
    .unwrap();

    // Boot 1: migration runs — warning names the mem, mounts.json gains
    // the archive mount, the legacy key is gone.
    let health = memstead()
        .current_dir(receiver.path())
        .args(["--json", "health"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let health = String::from_utf8(health).unwrap();
    assert!(
        health.contains("READ_MEMS_MIGRATED_TO_MOUNTS") && health.contains("sender-mem"),
        "boot must surface the migration warning naming the mem; got:\n{health}"
    );
    let mounts =
        fs::read_to_string(receiver.path().join(".memstead/state/mounts.json")).unwrap();
    assert!(
        mounts.contains(r#""sender-mem""#),
        "migrated mount must persist; got:\n{mounts}"
    );
    let cfg_after =
        memstead_git_branch::mem_repo_config::read_config(receiver.path(), "receiver-mem")
            .unwrap();
    assert!(
        cfg_after.read_mems.is_empty(),
        "legacy key must be removed; got {:?}",
        cfg_after.read_mems
    );

    // Boot 2: silent (the source key is gone).
    let health2 = memstead()
        .current_dir(receiver.path())
        .args(["--json", "health"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let health2 = String::from_utf8(health2).unwrap();
    assert!(
        !health2.contains("READ_MEMS_MIGRATED_TO_MOUNTS"),
        "second boot must not re-warn; got:\n{health2}"
    );

    // Parity on the migrated fixture: search + cross-mem link resolve
    // exactly as on a fresh install.
    memstead()
        .current_dir(receiver.path())
        .args(["search", "Alpha"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("sender-mem--alpha"));
    memstead()
        .current_dir(receiver.path())
        .env("MEMSTEAD_OPERATOR_MODE", "1")
        .args(["workspace", "grant-cross-link", "receiver-mem", "sender-mem"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success();
    memstead()
        .current_dir(receiver.path())
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .args([
            "create",
            "--mem",
            "receiver-mem",
            "--title",
            "Bridge",
            "--type",
            "spec",
            "--section",
            "identity=Links into [[sender-mem--alpha]] post-migration.",
            "--section",
            "purpose=Migration parity probe.",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(receiver.path())
        .args(["entity", "receiver-mem--bridge"])
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("[[sender-mem--alpha]]"));
}

/// Criterion 4 remainder: uninstalling an unknown name and a writable
/// mem refuse typed and side-effect-free; no INTERNAL on any refusal.
#[test]
fn uninstall_refusals_are_typed_and_effect_free() {
    let _guard = cache_guard().lock().unwrap_or_else(|e| e.into_inner());
    let receiver = TempDir::new().unwrap();
    let _receiver_mem = make_receiver_mem(receiver.path());

    let mounts_before =
        fs::read_to_string(receiver.path().join(".memstead/state/mounts.json")).unwrap();

    for (name, code) in [("no-such-mem", "UNKNOWN_MEM"), ("receiver-mem", "MEM_NOT_READ_ONLY")] {
        let out = memstead()
            .current_dir(receiver.path())
            .arg("uninstall")
            .arg(name)
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone();
        let stderr = String::from_utf8(out).unwrap();
        assert!(stderr.contains(code), "expected {code}; got: {stderr}");
        assert!(!stderr.contains("INTERNAL"), "INTERNAL leaked: {stderr}");
    }

    let mounts_after =
        fs::read_to_string(receiver.path().join(".memstead/state/mounts.json")).unwrap();
    assert_eq!(mounts_before, mounts_after, "refusals must not touch mount state");
}

// ---------------------------------------------------------------------------
// `verify-anchors` — the standalone drift statement (no binding required)
// ---------------------------------------------------------------------------

/// Build a one-mem workspace with an entity whose anchors span the four
/// verification states. Returns the workspace root.
fn make_anchor_workspace(root: &Path) {
    let m = |args: &[&str]| {
        memstead()
            .current_dir(root)
            .env("MEMSTEAD_OPERATOR_MODE", "1")
            .args(args)
            .assert()
            .success();
    };
    m(&["mem-repo", "init", "."]);
    m(&["mem", "init", "hold", "--no-gitignore"]);
    m(&[
        "create", "--mem", "hold", "--title", "Holder", "--type", "spec",
        "--section", "identity=Anchored fixture entity.",
        "--section", "purpose=Verification states.",
    ]);

    // Four source files; anchors record the ORIGINAL content's
    // prepared hash, then edits/deletes produce the states.
    for (name, content) in [
        ("src-a.txt", "alpha"),
        ("src-b.txt", "beta"),
        ("src-c.txt", "gamma"),
        ("src-d.txt", "delta"),
    ] {
        fs::write(root.join(name), content).unwrap();
    }
    let h = |content: &str| memstead_base::anchor::prepared_content_hash(content.as_bytes());
    let anchors = [
        // resolved: intact source, matching hash, stable.
        format!(
            r#"{{"artifact":"src-a.txt","grain":"file","class":"anchored","hash":"{}","hash_stability":"stable"}}"#,
            h("alpha")
        ),
        // drifted: source will be edited; stable stability.
        format!(
            r#"{{"artifact":"src-b.txt","grain":"file","class":"anchored","hash":"{}","hash_stability":"stable"}}"#,
            h("beta")
        ),
        // recheck: source will be edited; unstable stability.
        format!(
            r#"{{"artifact":"src-c.txt","grain":"file","class":"anchored","hash":"{}","hash_stability":"unstable"}}"#,
            h("gamma")
        ),
        // unresolvable: source will be deleted.
        format!(
            r#"{{"artifact":"src-d.txt","grain":"file","class":"anchored","hash":"{}","hash_stability":"stable"}}"#,
            h("delta")
        ),
    ];
    let mut args: Vec<String> = vec![
        "update".into(),
        "hold--holder".into(),
        "--auto-hash".into(),
        "--append".into(),
        "purpose= Anchored.".into(),
    ];
    for a in &anchors {
        args.push("--anchor".into());
        args.push(a.clone());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    m(&arg_refs);

    // Produce the states: edit b and c, delete d.
    fs::write(root.join("src-b.txt"), "beta CHANGED").unwrap();
    fs::write(root.join("src-c.txt"), "gamma CHANGED").unwrap();
    fs::remove_file(root.join("src-d.txt")).unwrap();
}

/// Criterion 1: all four states on a hand-authored mem with no binding,
/// in one run. Criterion 5's read-only complement rides the same test.
#[test]
fn verify_anchors_reports_four_states_without_binding() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_anchor_workspace(root);

    let refs_before = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(root.join("mem-repo/.git"))
        .args(["for-each-ref", "--format=%(refname) %(objectname)"])
        .output()
        .unwrap()
        .stdout;

    let out = memstead()
        .current_dir(root)
        .args(["--json", "verify-anchors", "--mem", "hold"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["resolved"], 1, "full: {v}");
    assert_eq!(v["drifted"], 1, "full: {v}");
    assert_eq!(v["recheck"], 1, "full: {v}");
    assert_eq!(v["unresolvable"], 1, "full: {v}");
    let state_of = |artifact: &str| -> String {
        v["anchors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["artifact"] == artifact)
            .map(|a| a["state"].as_str().unwrap().to_string())
            .unwrap_or_else(|| panic!("anchor for {artifact} missing: {v}"))
    };
    assert_eq!(state_of("src-a.txt"), "resolved");
    assert_eq!(state_of("src-b.txt"), "drifted");
    assert_eq!(state_of("src-c.txt"), "recheck");
    assert_eq!(state_of("src-d.txt"), "unresolvable");

    // Read-only: no ref moved.
    let refs_after = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(root.join("mem-repo/.git"))
        .args(["for-each-ref", "--format=%(refname) %(objectname)"])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(refs_before, refs_after, "verify-anchors must not move any ref");
}

/// Criterion 3: a mem whose bindings span multiple media no longer
/// nulls — path anchors resolve against their own recorded paths, and a
/// grain the mechanism does not reach reports `unresolvable`.
#[test]
fn verify_anchors_multi_binding_mem_no_longer_nulls() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_anchor_workspace(root);

    // Two v2 bindings on the mem — the shape that made
    // `single_path_medium_root` return None (ambiguous) pre-rework.
    let proj = root.join(".memstead/projections/hold");
    fs::create_dir_all(&proj).unwrap();
    for (name, pointer) in [("graph-a", "src-a.txt"), ("graph-b", "src-b.txt")] {
        fs::write(
            proj.join(format!("{name}.json")),
            format!(
                r#"{{"version":2,"intent":"t","sources":[{{"name":"s","type":"codebase","pointer":"{pointer}","change_detection":"git","scope":[{{"path":"**","mode":"allow"}}]}}],"reference_mems":[],"destination_mem":"hold","deny_paths":[],"coverage_semantics":"exhaustive","operations":{{"build":{{"mode":"discovery","trigger":"loop","batch_size":20}},"sync":{{"trigger":"manual","batch_size":20}}}}}}"#
            ),
        )
        .unwrap();
    }

    // Add a url-grain anchor — the mechanism doesn't reach it.
    memstead()
        .current_dir(root)
        .env("MEMSTEAD_OPERATOR_MODE", "1")
        .args([
            "update", "hold--holder", "--auto-hash",
            "--append", "purpose= Url anchor.",
            "--anchor",
            r#"{"artifact":"https://example.com/spec","grain":"url","class":"informed-by"}"#,
        ])
        .assert()
        .success();

    let out = memstead()
        .current_dir(root)
        .args(["--json", "verify-anchors", "--mem", "hold"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    // Path anchors still resolve per their own states — nothing nulls.
    assert_eq!(v["resolved"], 1, "{v}");
    assert_eq!(v["drifted"], 1, "{v}");
    assert_eq!(v["recheck"], 1, "{v}");
    // src-d (deleted) + the url anchor.
    assert_eq!(v["unresolvable"], 2, "{v}");
}

/// Criterion 4 + 5 remainders: the health anchors axis is include-gated
/// (absent by default), an anchor-less mem reports empty, and an
/// unknown mem refuses typed.
#[test]
fn verify_anchors_health_axis_and_refusals() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_anchor_workspace(root);

    // Default health carries no anchors key.
    let plain = memstead()
        .current_dir(root)
        .args(["--json", "health"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let plain: serde_json::Value = serde_json::from_slice(&plain).unwrap();
    assert!(
        plain.get("anchors").is_none(),
        "anchors axis must be include-gated: {plain}"
    );

    // With the include: per-mem four-state counts.
    let with = memstead()
        .current_dir(root)
        .args(["--json", "health", "--include", "anchors"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let with: serde_json::Value = serde_json::from_slice(&with).unwrap();
    let axis = with.get("anchors").expect("axis present under include");
    assert_eq!(axis["hold"]["drifted"], 1, "{with}");
    assert_eq!(axis["hold"]["unresolvable"], 1, "{with}");

    // Anchor-less mem: empty result, not an error.
    memstead()
        .current_dir(root)
        .env("MEMSTEAD_OPERATOR_MODE", "1")
        .args(["mem", "init", "bare", "--no-gitignore"])
        .assert()
        .success();
    let empty = memstead()
        .current_dir(root)
        .args(["--json", "verify-anchors", "--mem", "bare"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let empty: serde_json::Value = serde_json::from_slice(&empty).unwrap();
    assert_eq!(empty["resolved"], 0);
    assert_eq!(empty["anchors"].as_array().map(|a| a.len()), Some(0));

    // Unknown mem: typed refusal, no INTERNAL.
    let unknown = memstead()
        .current_dir(root)
        .args(["verify-anchors", "--mem", "nope"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let unknown = String::from_utf8(unknown).unwrap();
    assert!(unknown.contains("UNKNOWN_MEM"), "got: {unknown}");
    assert!(!unknown.contains("INTERNAL"), "INTERNAL leaked: {unknown}");
}
