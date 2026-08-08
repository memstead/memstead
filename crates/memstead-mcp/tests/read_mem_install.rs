#![cfg(feature = "mem-repo")]
//! Integration tests for the `--read-mem` install batch.
//!
//! Drives [`memstead_mcp::read_mems::install_read_mems`] directly — the
//! binary entry point is a thin wrapper over the same helper, so these
//! tests pin the behavior the MCP server exhibits without having to
//! spawn it and speak MCP over stdio. Each accepted archive lands in
//! the global cache and registers as a **workspace-level read-only
//! mount** on the engine — no host-mem config is involved.
//!
//! **Warn-and-continue** is the pinned contract: a bad archive produces
//! a `Failed` entry in the batch output, the good archives before and
//! after it still install, no cache file lands for the rejection, and
//! the specific validation reason survives in the error string so the
//! caller's log is actionable.

use std::path::{Path, PathBuf};

use memstead_git_branch::mem_cache::CACHE_OVERRIDE_ENV;
use memstead_git_branch::ops::export::export_mem;
use memstead_mcp::read_mems::{ReadMemResult, install_read_mems};
use tempfile::TempDir;

/// Batch-install convenience: a fresh empty engine (no mounts, no
/// workspace root) receives the registrations; the pair comes back so
/// tests can assert both the batch outcomes and the engine's mount
/// state.
fn install_batch(archives: &[PathBuf], cwd: &Path) -> (memstead_base::Engine, Vec<ReadMemResult>) {
    let mut engine = memstead_base::Engine::from_mounts(Vec::new()).unwrap();
    let results = install_read_mems(&mut engine, archives, cwd);
    (engine, results)
}

/// Build a minimal write-mem directory at `mem_dir` and export it to
/// `archive_path`. The resulting `.mem` is guaranteed to pass
/// `validate_and_normalize_archive` — fixtures shouldn't hand-roll
/// validator-compliant bytes when the exporter can produce them.
fn build_valid_archive(mem_dir: &Path, archive_path: &Path, name: &str) {
    // Configs no
    // longer carry an in-config `name` field. The published archive's
    // identity comes from the disk-path basename via the
    // `published_config_from` fallback chain. Place the mem under
    // `<mem_dir.parent>/<name>/` so the basename matches.
    let mem_dir = mem_dir.parent().unwrap_or(mem_dir).join(name);
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead/config.json"),
        r#"{"version":"1.0.0","schema":"default@1.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        mem_dir.join("alpha.md"),
        "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-01-15\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nA.\n\n## Purpose\n\nB.\n\n## Specifies\n\nC.\n\n## Constraints\n\nD.\n\n## Rationale\n\nE.\n",
    ).unwrap();

    let config = memstead_schema::load_and_validate(&mem_dir).unwrap();
    // No workspace context — schema resolver falls through to the
    // embedded builtin.
    export_mem(&mem_dir, &config, archive_path, None, None).unwrap();
}

/// Process-global env lock — identical pattern to
/// `memstead_git_branch::mem_cache::tests::ENV_LOCK`. Rust 2024 makes
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
fn valid_archive_installs_into_cache_and_registers_a_read_only_mount() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let src_dir = tmp.path().join("src");
    let archive = tmp.path().join("good.mem");

    build_valid_archive(&src_dir, &archive, "good");

    let _g = CacheGuard::install(&cache);
    let (engine, results) = install_batch(std::slice::from_ref(&archive), tmp.path());

    assert_eq!(results.len(), 1);
    let ReadMemResult::Installed { outcome, mount, .. } = &results[0] else {
        panic!("expected Installed, got {:?}", results[0]);
    };
    assert_eq!(outcome.mem_name, "good");
    assert!(outcome.copied_to_cache);
    assert_eq!(
        *mount,
        memstead_git_branch::mem_cache::MountRegistration::Registered
    );

    // The engine carries the workspace-level read-only mount.
    let mounted = engine.mount("good").expect("mount registered");
    assert_eq!(mounted.capability, memstead_base::MountCapability::ReadOnly);
    assert!(matches!(
        &mounted.storage,
        memstead_base::MountStorage::Archive { path } if *path == outcome.cache_path
    ));
    assert!(!engine.mem_router().is_writable("good"));
    assert!(engine.mem_router().is_visible("good"));

    // Cache is content-addressed: `good-<key>.mem`, no `.tmp` sibling.
    let names: Vec<String> = cache
        .read_dir()
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names
            .iter()
            .filter(|n| n.starts_with("good-") && n.ends_with(".mem"))
            .count(),
        1,
        "exactly one content-addressed cache file: {names:?}",
    );
    assert!(!names.iter().any(|n| n.ends_with(".tmp")));
}

#[test]
fn corrupt_archive_reports_validation_and_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let bad = tmp.path().join("bad.mem");

    std::fs::write(&bad, b"definitely not a zip").unwrap();

    let _g = CacheGuard::install(&cache);
    let (engine, results) = install_batch(std::slice::from_ref(&bad), tmp.path());

    assert_eq!(results.len(), 1);
    let ReadMemResult::Failed { archive, error } = &results[0] else {
        panic!("expected Failed, got {:?}", results[0]);
    };
    assert_eq!(archive, &bad);
    assert!(
        error.contains("archive failed strict validation"),
        "error must surface the validation wrapper: {error}"
    );

    // No mount registered, no cache file landed.
    assert!(engine.mount("bad").is_none());
    assert!(
        cache
            .read_dir()
            .map(|mut it| it.next().is_none())
            .unwrap_or(true)
    );
}

#[test]
fn bad_archive_in_batch_does_not_abort_good_ones() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let src_dir_a = tmp.path().join("src_a");
    let src_dir_b = tmp.path().join("src_b");
    let good_a = tmp.path().join("good_a.mem");
    let bad = tmp.path().join("bad.mem");
    let good_b = tmp.path().join("good_b.mem");

    build_valid_archive(&src_dir_a, &good_a, "alpha");
    std::fs::write(&bad, b"not a zip").unwrap();
    build_valid_archive(&src_dir_b, &good_b, "beta");

    let _g = CacheGuard::install(&cache);
    let (engine, results) =
        install_batch(&[good_a.clone(), bad.clone(), good_b.clone()], tmp.path());

    assert_eq!(results.len(), 3);
    assert!(matches!(results[0], ReadMemResult::Installed { .. }));
    assert!(matches!(results[1], ReadMemResult::Failed { .. }));
    assert!(matches!(results[2], ReadMemResult::Installed { .. }));

    // Both good archives are mounted read-only; the bad one is absent.
    assert!(engine.mount("alpha").is_some());
    assert!(engine.mount("beta").is_some());
    assert!(engine.mount("bad").is_none());

    // Both good archives landed in cache under their content-addressed
    // names (`<name>-<key>.mem`); the bad one left no trace.
    let names: Vec<String> = cache
        .read_dir()
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("alpha-") && n.ends_with(".mem")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("beta-") && n.ends_with(".mem")),
        "{names:?}"
    );
    assert_eq!(
        names.len(),
        2,
        "only the two good archives landed: {names:?}"
    );
}

#[test]
fn relative_archive_paths_resolve_against_cwd() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let src_dir = tmp.path().join("src");
    let archive = tmp.path().join("archives").join("rel.mem");

    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    build_valid_archive(&src_dir, &archive, "rel");

    // Caller hands in a relative path; the helper joins against `cwd`.
    let relative: PathBuf = PathBuf::from("archives").join("rel.mem");
    let _g = CacheGuard::install(&cache);
    let (_engine, results) = install_batch(&[relative], tmp.path());

    assert_eq!(results.len(), 1);
    let ReadMemResult::Installed {
        archive: resolved, ..
    } = &results[0]
    else {
        panic!("expected Installed, got {:?}", results[0]);
    };
    assert_eq!(resolved, &archive);
}
