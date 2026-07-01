#![cfg(feature = "vault-repo")]
//! Integration tests for the `--read-vault` install batch (V3 Task 3).
//!
//! Drives [`memstead_mcp::read_vaults::install_read_vaults`] directly — the
//! binary entry point is a thin wrapper over the same helper, so these
//! tests pin the behavior the MCP server exhibits without having to
//! spawn it and speak MCP over stdio.
//!
//! **Warn-and-continue** is the pinned contract: a bad archive produces
//! a `Failed` entry in the batch output, the good archives before and
//! after it still install, no cache file lands for the rejection, and
//! the specific `ValidationError` reason survives in the error's
//! `Display` form so the caller's log is actionable.

use std::path::{Path, PathBuf};

use memstead_git_branch::ops::export::export_vault;
use memstead_git_branch::vault_cache::{CACHE_OVERRIDE_ENV, InstallError, TargetVault};
use memstead_git_branch::validator::ValidationError;
use memstead_git_branch::vcs::CommitContext;
use memstead_mcp::read_vaults::{ReadVaultResult, install_read_vaults};
use tempfile::TempDir;

/// Disk-shape install convenience for the existing test fixtures. Wraps
/// `install_read_vaults` with `TargetVault::Disk(project)` and a dummy
/// commit context — the disk arm ignores `ctx` + `commit_message`.
fn install_to_disk_project(
    archives: &[PathBuf],
    project: &Path,
    cwd: &Path,
) -> Vec<ReadVaultResult> {
    install_read_vaults(
        archives,
        TargetVault::Disk(project),
        &CommitContext::internal(),
        "memstead: install (test)",
        cwd,
        &[],
    )
}

/// Build a minimal write-vault directory at `vault_dir` and export it to
/// `archive_path`. The resulting `.mem` is guaranteed to pass
/// `validate_and_normalize_archive` — fixtures shouldn't hand-roll
/// validator-compliant bytes when the exporter can produce them.
fn build_valid_archive(vault_dir: &Path, archive_path: &Path, name: &str) {
    // Configs no
    // longer carry an in-config `name` field. The published archive's
    // identity comes from the disk-path basename via the
    // `published_config_from` fallback chain. Place the vault under
    // `<vault_dir.parent>/<name>/` so the basename matches.
    let vault_dir = vault_dir
        .parent()
        .unwrap_or(vault_dir)
        .join(name);
    std::fs::create_dir_all(vault_dir.join(".memstead")).unwrap();
    std::fs::write(
        vault_dir.join(".memstead/config.json"),
        r#"{"version":"1.0.0","schema":"default@1.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        vault_dir.join("alpha.md"),
        "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-01-15\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nA.\n\n## Purpose\n\nB.\n\n## Specifies\n\nC.\n\n## Constraints\n\nD.\n\n## Rationale\n\nE.\n",
    ).unwrap();

    let config = memstead_schema::load_and_validate(&vault_dir).unwrap();
    // No workspace context — schema resolver falls through to the
    // embedded builtin.
    export_vault(&vault_dir, &config, archive_path, None, None).unwrap();
}

/// Write a minimal writable-vault config directory that the batch can
/// register `readVaults` entries into.
fn write_minimal_vault_config(dir: &Path, _name: &str) {
    std::fs::create_dir_all(dir.join(".memstead")).unwrap();
    std::fs::write(
        dir.join(".memstead/config.json"),
        r#"{"version":"1.0.0","schema":"default@1.0.0"}"#,
    )
    .unwrap();
}

/// Process-global env lock — identical pattern to
/// `memstead_git_branch::vault_cache::tests::ENV_LOCK`. Rust 2024 makes
/// `env::set_var` unsafe precisely because concurrent reads tear; every
/// test in this binary takes the lock before touching the cache env.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct CacheGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Option<String>,
}

impl CacheGuard {
    fn install(cache_dir: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(CACHE_OVERRIDE_ENV).ok();
        // SAFETY: the global mutex above serializes env access for every
        // test in this binary; no other reader runs concurrently.
        unsafe {
            std::env::set_var(CACHE_OVERRIDE_ENV, cache_dir);
        }
        Self { _lock: lock, prev }
    }
}

impl Drop for CacheGuard {
    fn drop(&mut self) {
        // SAFETY: still holding the lock acquired in `install`.
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(CACHE_OVERRIDE_ENV, v),
                None => std::env::remove_var(CACHE_OVERRIDE_ENV),
            }
        }
    }
}

#[test]
fn valid_archive_installs_into_cache_and_registers_in_config() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let project = tmp.path().join("project");
    let src_dir = tmp.path().join("src");
    let archive = tmp.path().join("good.mem");

    std::fs::create_dir_all(&project).unwrap();
    write_minimal_vault_config(&project, "specs");
    build_valid_archive(&src_dir, &archive, "good");

    let _g = CacheGuard::install(&cache);
    let results = install_to_disk_project(std::slice::from_ref(&archive), &project, tmp.path());

    assert_eq!(results.len(), 1);
    let ReadVaultResult::Installed { outcome, .. } = &results[0] else {
        panic!("expected Installed, got {:?}", results[0]);
    };
    assert_eq!(outcome.vault_name, "good");
    assert!(outcome.copied_to_cache);
    assert!(outcome.registered_in_config);

    // Cache is content-addressed: `good-<key>.mem`, no `.tmp` sibling.
    let names: Vec<String> = cache
        .read_dir()
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names.iter().filter(|n| n.starts_with("good-") && n.ends_with(".mem")).count(),
        1,
        "exactly one content-addressed cache file: {names:?}",
    );
    assert!(!names.iter().any(|n| n.ends_with(".tmp")));
}

#[test]
fn corrupt_archive_reports_validation_zip_and_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let project = tmp.path().join("project");
    let bad = tmp.path().join("bad.mem");

    std::fs::create_dir_all(&project).unwrap();
    write_minimal_vault_config(&project, "specs");
    std::fs::write(&bad, b"definitely not a zip").unwrap();

    let _g = CacheGuard::install(&cache);
    let results = install_to_disk_project(std::slice::from_ref(&bad), &project, tmp.path());

    assert_eq!(results.len(), 1);
    let ReadVaultResult::Failed { archive, error } = &results[0] else {
        panic!("expected Failed, got {:?}", results[0]);
    };
    assert_eq!(archive, &bad);

    // The specific variant carries path+reason via Display — pin it here
    // so a future refactor can't swap in a generic "validation failed".
    match error {
        InstallError::Validation(ValidationError::Zip(_)) => {}
        other => panic!("expected InstallError::Validation(Zip), got {other:?}"),
    }
    let rendered = format!("{error}");
    assert!(
        rendered.contains("archive failed strict validation"),
        "error Display must surface the validation wrapper: {rendered}"
    );

    // No cache file landed — validation failed before the write step.
    assert!(cache.read_dir().map(|mut it| it.next().is_none()).unwrap_or(true));
}

#[test]
fn bad_archive_in_batch_does_not_abort_good_ones() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let project = tmp.path().join("project");
    let src_dir_a = tmp.path().join("src_a");
    let src_dir_b = tmp.path().join("src_b");
    let good_a = tmp.path().join("good_a.mem");
    let bad = tmp.path().join("bad.mem");
    let good_b = tmp.path().join("good_b.mem");

    std::fs::create_dir_all(&project).unwrap();
    write_minimal_vault_config(&project, "specs");
    build_valid_archive(&src_dir_a, &good_a, "alpha");
    std::fs::write(&bad, b"not a zip").unwrap();
    build_valid_archive(&src_dir_b, &good_b, "beta");

    let _g = CacheGuard::install(&cache);
    let results = install_to_disk_project(
        &[good_a.clone(), bad.clone(), good_b.clone()],
        &project,
        tmp.path(),
    );

    assert_eq!(results.len(), 3);
    assert!(matches!(results[0], ReadVaultResult::Installed { .. }));
    assert!(matches!(results[1], ReadVaultResult::Failed { .. }));
    assert!(matches!(results[2], ReadVaultResult::Installed { .. }));

    // Both good archives landed in cache under their content-addressed
    // names (`<name>-<key>.mem`); the bad one left no trace.
    let names: Vec<String> = cache
        .read_dir()
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n.starts_with("alpha-") && n.ends_with(".mem")), "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("beta-") && n.ends_with(".mem")), "{names:?}");
    assert_eq!(names.len(), 2, "only the two good archives landed: {names:?}");
}

#[test]
fn relative_archive_paths_resolve_against_cwd() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let project = tmp.path().join("project");
    let src_dir = tmp.path().join("src");
    let archive = tmp.path().join("archives").join("rel.mem");

    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    write_minimal_vault_config(&project, "specs");
    build_valid_archive(&src_dir, &archive, "rel");

    // Caller hands in a relative path; the helper joins against `cwd`.
    let relative: PathBuf = PathBuf::from("archives").join("rel.mem");
    let _g = CacheGuard::install(&cache);
    let results = install_to_disk_project(&[relative], &project, tmp.path());

    assert_eq!(results.len(), 1);
    let ReadVaultResult::Installed { archive: resolved, .. } = &results[0] else {
        panic!("expected Installed, got {:?}", results[0]);
    };
    assert_eq!(resolved, &archive);
}
