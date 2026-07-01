#![cfg(feature = "vault-repo")]
// `memstead export --format vault` round-trips through `memstead install` here,
// and `install` is a vault-repo-only subcommand. Skip the whole binary
// under `--no-default-features` rather than try to project the basis
// half (which has no `install` to round-trip into).

//! Integration tests for `memstead export` and `memstead install`.
//!
//! Exercises the full share-a-vault flow end-to-end:
//!
//! 1. Build a fixture write vault A with two entities.
//! 2. Run `memstead export --format vault -o out.mem` against A.
//! 3. Build a separate empty write vault B.
//! 4. Run `memstead install ./out.mem` against B.
//! 5. Verify the installed read vault's entities are discoverable from B
//!    via `memstead entity` / `memstead search`.
//!
//! Uses `MEMSTEAD_VAULT_CACHE` to keep the global cache writes inside a tempdir.
//! Env access is unsafe in Rust 2024, and `std::env::set_var` is globally
//! visible across threads — so this test binary runs single-threaded and
//! serializes cache-touching tests via a process-local `Mutex`, matching
//! `memstead-git-branch/tests/read_vaults.rs`.

use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_vault_repo_from_disk;
use predicates::str::contains;
use tempfile::TempDir;

/// Serializes env mutations across tests in this binary.
fn cache_guard() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Commit the `(rel_path, bytes)` pairs to `refs/heads/<vault_name>` of
/// `<root>/vault-repo/.git/`. The branch-walk export reads archive `.md`
/// content from this branch tip — disk-resident `.md` files alone are not
/// sufficient under the GitObject default.
fn commit_vault_branch(root: &Path, vault_name: &str, entries: &[(&str, &str)]) {
    use memstead_git_branch::storage::git_tree::GitTreeVaultWriter;
    use memstead_git_branch::storage::VaultWriter;
    use memstead_git_branch::vcs::CommitContext;

    let gitdir = root.join("vault-repo").join(".git");
    let writer = GitTreeVaultWriter::new(gitdir, format!("refs/heads/{vault_name}"));
    for (rel, content) in entries {
        writer
            .write_entity(Path::new(rel), content.as_bytes())
            .unwrap();
    }
    writer.commit("seed", &CommitContext::internal()).unwrap();
}

/// Build a write vault at `<root>/sender-vault/` with version set
/// (required for vault-archive export) and two minimal spec entities.
/// Returns the vault's absolute path. The dir basename equals the
/// declared `name: "sender-vault"` per the basename-invariant.
///
/// Also lays down `<root>/vault-repo/.git/` so the CLI's workspace walk-up
/// resolves `<root>` and the engine's fail-fast accepts the workspace.
///
/// Also commits the disk `.md` content to `refs/heads/sender-vault` so the
/// branch-walk export produces a non-empty archive.
fn make_sender_vault(root: &Path) -> std::path::PathBuf {
    let dir = root.join("sender-vault");
    fs::create_dir_all(&dir).unwrap();
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{
  "version": "1.0.0",
  "description": "Fixture write vault used to exercise export → install",
  "schema": "default@1.0.0"
}"#,
    )
    .unwrap();
    init_real_vault_repo_from_disk(root, &[(&dir, "sender-vault")]);

    let alpha_body = r#"---
type: spec
created_date: 2026-01-01
last_modified: 2026-01-01
level: M0
---
# Alpha

## Identity

First entity in the sender vault. Used to verify a read vault's entities
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

Second entity in the sender vault.

## Purpose

Provides a second hit for the search test below.
"#;
    fs::write(dir.join("beta.md"), beta_body).unwrap();

    commit_vault_branch(
        root,
        "sender-vault",
        &[("alpha.md", alpha_body), ("beta.md", beta_body)],
    );
    dir
}

/// Build an empty write vault (no entities) under `<root>/receiver-vault/`
/// the consumer installs into. Returns the vault's absolute path.
///
/// Also lays down `<root>/vault-repo/.git/` so the CLI's workspace walk-up
/// resolves `<root>` and the engine's fail-fast accepts the workspace.
fn make_receiver_vault(root: &Path) -> std::path::PathBuf {
    let dir = root.join("receiver-vault");
    fs::create_dir_all(&dir).unwrap();
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{ "version": "0.1.0", "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    init_real_vault_repo_from_disk(root, &[(&dir, "receiver-vault")]);
    dir
}

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

#[test]
fn export_markdown_default_runs() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_sender_vault(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .arg("export")
        .assert()
        .success()
        .stdout(contains("Export — markdown"));
}

/// Workspace-wide
/// `memstead export --format markdown` against a vault-repo workspace
/// (every mount is `MountStorage::GitBranch`) completes the folder
/// mounts (zero, in this fixture) and lists the declined git-branch
/// mounts under `## Skipped mounts` in markdown mode. The exit code
/// stays 0 (partial-success path).
#[test]
fn export_markdown_workspace_wide_reports_skipped_git_branch_mounts() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_sender_vault(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .arg("export")
        .assert()
        .success()
        .stdout(contains("Export — markdown"))
        .stdout(contains("Skipped mounts"))
        .stdout(contains("sender-vault"))
        .stdout(contains("git-branch"))
        .stdout(contains("backend_does_not_support_markdown_export"));
}

/// Per-vault
/// `memstead export --format markdown --vault-name <git-branch-vault>`
/// returns the typed `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` refusal.
/// Exit code is the validation-class code; the stderr message carries
/// the recovery hint (`--format vault`). The pre-fix shape — exit-0
/// with `Written: 0, Unchanged: 0` — is unreachable here.
#[test]
fn export_markdown_per_vault_refuses_on_git_branch_backend() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_sender_vault(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "markdown", "--vault", "sender-vault"])
        .assert()
        .failure()
        .stderr(contains("MARKDOWN_EXPORT_UNSUPPORTED_BACKEND"))
        .stderr(contains("git-branch"))
        .stderr(contains("--format vault"));
}

/// The `--json` envelope under
/// per-vault refusal carries the typed `code` and structured details
/// (`vault`, `active_backend`, `supported_backends`). Agents key on
/// the code to branch their recovery.
#[test]
fn export_markdown_per_vault_refuses_with_json_envelope() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_sender_vault(tmp.path());

    let assert = memstead()
        .current_dir(tmp.path())
        .args([
            "--json",
            "export",
            "--format",
            "markdown",
            "--vault",
            "sender-vault",
        ])
        .assert()
        .failure();

    // Under `--json` the error envelope rides stdout.
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let env: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON error envelope on stdout; got:\n{stdout}\n({e})"));
    assert_eq!(env["code"], "MARKDOWN_EXPORT_UNSUPPORTED_BACKEND");
    assert_eq!(env["details"]["vault"], "sender-vault");
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
    let _vault = make_sender_vault(tmp.path());

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
    let skipped = env["skipped_mounts"]
        .as_array()
        .expect("skipped_mounts must be a JSON array under the workspace-wide partial-success shape");
    assert_eq!(skipped.len(), 1, "one git-branch-mount in this fixture");
    assert_eq!(skipped[0]["vault"], "sender-vault");
    assert_eq!(skipped[0]["active_backend"], "git-branch");
    assert_eq!(
        skipped[0]["reason"],
        "backend_does_not_support_markdown_export"
    );
}

#[test]
fn export_vault_produces_memstead_archive() {
    let _guard = cache_guard().lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let vault = make_sender_vault(tmp.path());

    let output_path = vault.join("sender-vault-1.0.0.mem");

    memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "vault", "-o"])
        .arg(&output_path)
        .assert()
        .success()
        .stdout(contains("Exported `sender-vault` v1.0.0"))
        .stdout(contains("sender-vault-1.0.0.mem"));

    assert!(output_path.exists(), "archive must be written to disk");
    assert!(
        fs::metadata(&output_path).unwrap().len() > 0,
        "archive must not be empty"
    );
}

#[test]
fn export_vault_fails_without_version() {
    let tmp = TempDir::new().unwrap();
    // Dir basename equals the declared name so the basename-invariant
    // does not reject this fixture before the version-missing check fires.
    let vault = tmp.path().join("unversioned");
    let memstead_dir = vault.join(".memstead");
    fs::create_dir_all(&memstead_dir).unwrap();
    // Version deliberately omitted.
    fs::write(
        memstead_dir.join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    init_real_vault_repo_from_disk(tmp.path(), &[(&vault, "unversioned")]);

    memstead()
        .current_dir(tmp.path())
        .args(["export", "--format", "vault", "-o"])
        .arg(vault.join("out.mem"))
        .assert()
        .failure()
        .stderr(contains("version"));
}

/// F1: `memstead vault set-version` against a vault-repo workspace bumps
/// the version on the `__MEMSTEAD:vaults/<name>/config.json` blob (via
/// the backend's `write_vault_config` trait method). Backend-symmetric
/// counterpart to `memstead-cli/tests/write_commands.rs::vault_set_version_persists_through_filesystem_backend`,
/// which covers the folder backend. Verifies on-disk persistence by
/// running `set-version` a second time in a fresh CLI process: the
/// second call's `Old version` line is sourced from the persisted
/// `__MEMSTEAD`-mirrored config (re-loaded across the process boundary),
/// not from in-memory engine state — so seeing the first bump there
/// proves it survived.
#[test]
fn vault_set_version_persists_through_vault_repo_backend() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_sender_vault(tmp.path());

    // Bump the fixture's seeded version (1.0.0) to 1.5.0.
    memstead()
        .current_dir(tmp.path())
        .args(["vault", "set-version", "sender-vault", "1.5.0"])
        .assert()
        .success()
        .stdout(contains("New version: 1.5.0"));

    // Second bump in a fresh process — the `Old version: 1.5.0` line
    // can only come from the persisted on-disk config (the previous
    // engine instance has exited). Proves the prior write reached
    // `__MEMSTEAD:vaults/sender-vault/config.json`, not just RAM.
    memstead()
        .current_dir(tmp.path())
        .args(["vault", "set-version", "sender-vault", "2.0.0"])
        .assert()
        .success()
        .stdout(contains("Old version: 1.5.0"))
        .stdout(contains("New version: 2.0.0"));

    // Malformed semver refuses with INVALID_INPUT.
    memstead()
        .current_dir(tmp.path())
        .args(["vault", "set-version", "sender-vault", "not-a-semver"])
        .assert()
        .failure();
}

/// `set-version`
/// accepts `--note` like the other commit-producing vault-lifecycle
/// commands, and the note rides the `__MEMSTEAD` version-bump commit body.
#[test]
fn vault_set_version_note_lands_on_commit() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_sender_vault(tmp.path());

    memstead()
        .current_dir(tmp.path())
        .args([
            "vault",
            "set-version",
            "sender-vault",
            "1.5.0",
            "--note",
            "bump for the auth release",
        ])
        .assert()
        .success()
        .stdout(contains("New version: 1.5.0"));

    // The note lands on the config-commit body on `refs/heads/__MEMSTEAD`
    // (the ref the version bump writes through).
    let gitdir = tmp.path().join("vault-repo").join(".git");
    let repo = gix::open(&gitdir).expect("open vault-repo gitdir");
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

/// `memstead vault set-sync-state` persists an opaque token into the
/// `__MEMSTEAD:vaults/<name>/config.json` blob (via the backend's
/// `write_vault_config`), exactly like `set-version`. Verifies on-disk
/// persistence and the set-vs-overwrite-vs-clear reporting by running
/// the command across fresh CLI processes (the previous engine instance
/// has exited, so the `overwrote`/`cleared` lines can only come from the
/// persisted config).
#[test]
fn vault_set_sync_state_persists_and_reports() {
    let tmp = TempDir::new().unwrap();
    let _vault = make_sender_vault(tmp.path());

    // First write: a fresh key is "set".
    memstead()
        .current_dir(tmp.path())
        .args([
            "vault",
            "set-sync-state",
            "sender-vault",
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
            "vault",
            "set-sync-state",
            "sender-vault",
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
            "vault",
            "set-sync-state",
            "sender-vault",
            "engine-graph/source-files",
            "",
        ])
        .assert()
        .success()
        .stdout(contains("sync state cleared"));

    // Unknown vault refuses (UNKNOWN_VAULT → non-zero exit).
    memstead()
        .current_dir(tmp.path())
        .args([
            "vault",
            "set-sync-state",
            "no-such-vault",
            "k",
            "v",
        ])
        .assert()
        .failure();
}

#[test]
fn install_round_trip_entities_discoverable() {
    let _guard = cache_guard().lock().unwrap();
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_vault = make_sender_vault(sender.path());
    let _receiver_vault = make_receiver_vault(receiver.path());

    let archive = sender_vault.join("sender-vault-1.0.0.mem");

    // Step 1: export the sender vault as a .memstead archive.
    memstead()
        .current_dir(sender.path())
        .args(["export", "--format", "vault", "-o"])
        .arg(&archive)
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success();

    assert!(archive.exists(), "export must produce the archive");

    // Step 2: install the archive into the receiver project.
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("Installed `sender-vault`"));

    // Step 3: sender vault's entities are now discoverable from receiver.
    memstead()
        .current_dir(receiver.path())
        .args(["search", "Alpha"])
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("sender-vault--alpha"));

    // The entity itself is readable directly by ID.
    memstead()
        .current_dir(receiver.path())
        .args(["entity", "sender-vault--alpha"])
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("# Alpha"));
}

/// The install pipeline refuses an archive whose authoritative name
/// matches a writable mount in the target workspace. Otherwise the
/// install would report success while the boot-time `hydrate_read_vaults`
/// silently skipped the read-vault registration because writable shadows
/// the read-vault — net effect a no-op-with-success-message.
#[test]
fn install_refuses_when_archive_shadows_writable() {
    let _guard = cache_guard().lock().unwrap();
    let sender = TempDir::new().unwrap();
    // The "receiver" workspace is set up with a writable vault
    // whose name *matches* the sender archive's authoritative name
    // (`sender-vault`). Installing the archive must refuse rather
    // than silently no-op.
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_vault = make_sender_vault(sender.path());

    // Build the archive in the sender workspace.
    let archive = sender_vault.join("sender-vault-1.0.0.mem");
    memstead()
        .current_dir(sender.path())
        .args(["export", "--format", "vault", "-o"])
        .arg(&archive)
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success();

    // Seed the receiver workspace with a writable vault named
    // `sender-vault` so the install would collide.
    let receiver_vault_dir = receiver.path().join("sender-vault");
    fs::create_dir_all(&receiver_vault_dir).unwrap();
    let memstead_dir = receiver_vault_dir.join(".memstead");
    fs::create_dir_all(&memstead_dir).unwrap();
    fs::write(
        memstead_dir.join("config.json"),
        r#"{ "version": "0.1.0", "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    init_real_vault_repo_from_disk(receiver.path(), &[(&receiver_vault_dir, "sender-vault")]);

    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .failure()
        .stderr(contains("READ_VAULT_SHADOWS_WRITABLE"))
        .stderr(contains("already exists as a writable mount"));
}

#[test]
fn install_is_idempotent() {
    let _guard = cache_guard().lock().unwrap();
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_vault = make_sender_vault(sender.path());
    let _receiver_vault = make_receiver_vault(receiver.path());

    let archive = sender_vault.join("sender-vault-1.0.0.mem");

    memstead()
        .current_dir(sender.path())
        .args(["export", "--format", "vault", "-o"])
        .arg(&archive)
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success();

    // First install: copied + registered.
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("copied into cache"))
        .stdout(contains("registered as a read-vault"));

    // Second install: no-op on both sides.
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success()
        .stdout(contains("already in cache"))
        .stdout(contains("already registered as a read-vault"));
}

/// After installing an archive as an RO mount: `memstead workspace dump
/// --json` must complete without `VAULT_ERROR` (otherwise the dump would
/// iterate every vault and call `gitdir_for` unconditionally, crashing on
/// the first RO mount). `memstead type --vault <ro-name>` must resolve
/// against the RO mount (otherwise the resolver walks `writable_vault_names`
/// only). And the schema_ref on the dump's RO entry matches the archive's
/// bundled config (otherwise `hydrate_read_vaults` hardcodes `default@1.0.0`
/// ignoring the archive's actual pin).
#[test]
fn workspace_dump_and_type_resolve_for_ro_mount_after_install() {
    let _guard = cache_guard().lock().unwrap();
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let sender_vault = make_sender_vault(sender.path());
    let _receiver_vault = make_receiver_vault(receiver.path());

    let archive = sender_vault.join("sender-vault-1.0.0.mem");
    memstead()
        .current_dir(sender.path())
        .args(["export", "--format", "vault", "-o"])
        .arg(&archive)
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success();
    memstead()
        .current_dir(receiver.path())
        .arg("install")
        .arg(&archive)
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success();

    // Workspace dump completes; the RO mount entry carries the
    // `capability: "read_only"` discriminator plus a matching
    // `schema_ref` (pin-fidelity: the value matches what the archive
    // bundled).
    let dump = memstead()
        .current_dir(receiver.path())
        .args(["workspace", "dump", "--json"])
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let dump_str = String::from_utf8(dump).unwrap();
    let dump_json: serde_json::Value = serde_json::from_str(&dump_str)
        .expect("workspace dump must emit valid JSON");
    let vaults = dump_json["vaults"]
        .as_array()
        .expect("dump must carry a vaults array");
    let ro_entry = vaults
        .iter()
        .find(|v| v["name"] == "sender-vault")
        .expect("installed sender-vault must appear in dump");
    assert_eq!(
        ro_entry["capability"], "read_only",
        "RO mount must carry the `read_only` capability marker; got {ro_entry}"
    );
    assert_eq!(
        ro_entry["schema_ref"], "default@1.0.0",
        "RO mount's schema_ref must match the archive's bundled pin (pre-fix \
         the dump would have crashed before reaching this assertion); got {ro_entry}"
    );

    // `memstead type --vault <ro-name>` resolves and lists the RO
    // mount's schema types — the resolver reaches read-only mounts, not
    // only writable ones.
    memstead()
        .current_dir(receiver.path())
        .args(["type", "--vault", "sender-vault"])
        .env("MEMSTEAD_VAULT_CACHE", cache.path())
        .assert()
        .success();
}
