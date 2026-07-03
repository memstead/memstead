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

    // First install: copied + registered.
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("copied into cache"))
        .stdout(contains("registered as a read-mem"));

    // Second install: no-op on both sides.
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("already in cache"))
        .stdout(contains("already registered as a read-mem"));
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
