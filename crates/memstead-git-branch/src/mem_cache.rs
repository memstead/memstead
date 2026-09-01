//! Read-mem cache resolution, published-config reads, and the
//! install-to-cache side effect.
//!
//! Every sealed-archive byte entering the cache goes through
//! `validate_and_normalize_archive` — the install path reads the
//! submitted archive, hands the bytes to the validator, and writes the
//! validator's `canonical_bytes` via a temp-plus-atomic-rename so no
//! partial archive ever lands on disk. Steady-state loads (through
//! `read_published_config` or the entity loader) trust the cached
//! bytes: they were canonical at write time and re-validation on every
//! load would just pay for the same work twice.
//!
//! The cache base path resolves via `dirs::data_dir()` so the same path
//! works on macOS (`~/Library/Application Support/memstead/mems`), Linux
//! (`$XDG_DATA_HOME/memstead/mems` or `~/.local/share/memstead/mems`), and
//! Windows (`%APPDATA%\memstead\mems`). For tests, `MEMSTEAD_MEM_CACHE`
//! overrides the base so temp dirs can stand in without touching the
//! user's real data directory.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use memstead_base::ops::WarningHint;
use memstead_schema::{ARCHIVE_CONFIG_PATH, ARCHIVE_EXTENSION, PublishedMemConfig};

use crate::entity::loader::LoadError;
use crate::mem_repo_config::MemRepoWriteError;
use crate::validator::{ValidationError, validate_and_normalize_archive};

/// Env var that overrides `<data_dir>/memstead/mems` for tests.
pub const CACHE_OVERRIDE_ENV: &str = "MEMSTEAD_MEM_CACHE";

/// Resolve the global mem-cache directory.
///
/// Respects `MEMSTEAD_MEM_CACHE` if set — tests use this to point at a
/// tempdir without touching the real user-data directory. Otherwise
/// returns `<data_dir>/memstead/mems` on every platform (macOS / Linux /
/// Windows), so the CLI and the Memstead app resolve to the same path
/// without per-platform branching.
///
/// `dirs::data_dir()` is infallible on Tier-1 platforms; `expect` is
/// fine for an engine that only runs on systems with a resolvable home.
pub fn mem_cache_dir() -> PathBuf {
    if let Ok(override_path) = std::env::var(CACHE_OVERRIDE_ENV)
        && !override_path.is_empty()
    {
        return PathBuf::from(override_path);
    }
    dirs::data_dir()
        .expect("platform provides a data directory")
        .join("memstead")
        .join("mems")
}

/// Read the whitelisted `.memstead/config.json` from a cached archive.
///
/// Does **not** re-run full archive validation — the cache only
/// contains bytes the validator already approved, so entity parse and
/// graph construction can be deferred to the caller. Configs are
/// re-parsed with `parse_config_bytes` so the strict-ingress shape is
/// enforced here as defense-in-depth against a tampered cache file.
pub fn read_published_config(archive_path: &Path) -> Result<PublishedMemConfig, LoadError> {
    if !archive_path.is_file() {
        return Err(LoadError::ArchiveNotFound(
            archive_path.display().to_string(),
        ));
    }
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    // Take the mutable entry borrow only if the config member is
    // present (`by_name` holds `&mut archive`).
    let config_name = ARCHIVE_CONFIG_PATH;
    if archive.index_for_name(config_name).is_none() {
        return Err(LoadError::InvalidArchive(format!(
            "missing {ARCHIVE_CONFIG_PATH} in {}",
            archive_path.display()
        )));
    }
    let mut entry = archive.by_name(config_name).map_err(|e| {
        LoadError::InvalidArchive(format!(
            "reading {config_name} in {}: {e}",
            archive_path.display()
        ))
    })?;

    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)?;

    crate::validator::config::parse_config_bytes(&bytes).map_err(|e| {
        LoadError::InvalidArchive(format!(
            "invalid {ARCHIVE_CONFIG_PATH} in {}: {e}",
            archive_path.display()
        ))
    })
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("could not read mem archive: {0}")]
    Archive(#[from] LoadError),
    #[error("io error while installing mem: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error while registering mem: {0}")]
    Config(#[from] memstead_schema::config::ConfigError),
    #[error("archive failed strict validation: {0}")]
    Validation(ValidationError),
    /// Mem-db tree write failed. Carries the underlying gix error
    /// message so callers can surface it without wrapping the variant.
    #[error("mem-repo tree write failed: {0}")]
    MemRepo(#[from] MemRepoWriteError),
    /// The archive's authoritative mem name (carried in its canonical
    /// config) matches a writable mount that already exists in this
    /// workspace. A read-only mount cannot share a writable mount's
    /// name (the archive's internal name is its sole identity and
    /// nothing can rename it at install time), so the install surface
    /// refuses up-front. The genuine recovery is to rename or
    /// unregister the writable mount that shadows the archive's
    /// internal name.
    #[error(
        "archive's mem name `{archive_name}` already exists as a writable mount in this workspace; \
         rename the writable mount (`memstead mem rename`) or unregister it first — the archive's \
         internal name is its sole identity and cannot be changed at install time"
    )]
    ShadowsWritable {
        archive_name: String,
        shadows_writable: String,
    },
    // `CacheNameCollision` was retired once the cache became
    // content-addressed (`<name>-<content_key>.mem`): distinct bytes
    // under the same mem name land in distinct files and the collision
    // class it guarded no longer exists. No engine surface can produce it.
}

/// Short content-address for an installed archive: the first 16 hex chars
/// of `sha256(canonical_bytes)`. Used as the cache-file key
/// (`<name>-<key>.mem`) and carried on the archive mount so the loader
/// resolves the right file. 64 bits is ample collision resistance for a
/// per-user cache; the same convention (truncated SHA-256 hex) the
/// entity content-hash uses.
fn content_cache_key(canonical_bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(canonical_bytes);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Outcome of [`install_to_cache`] — the cache-side half of an
/// install, with everything the caller needs to register the archive
/// as a workspace-level read-only mount.
#[derive(Debug, Clone)]
pub struct CacheInstallOutcome {
    /// Mem name, taken from the validator's approved config — the
    /// archive's sole identity.
    pub mem_name: String,
    /// The archive's schema pin, from its bundled config.
    pub schema: memstead_schema::SchemaRef,
    /// Content-addressed cache file the mount's `Archive` storage
    /// points at.
    pub cache_path: PathBuf,
    /// The content digest half of the cache filename.
    pub cache_key: String,
    /// `true` if canonical bytes were written on this call; `false`
    /// on the idempotent dedup path.
    pub copied_to_cache: bool,
    /// The archive's embedded schema package, package-relative
    /// (`schema.yaml`, `types/<t>.yaml`, `schema-format.json`) and
    /// byte-verbatim. Carried out of the validator because the mount
    /// that follows can only be registered once this schema resolves,
    /// and for a third party's vocabulary the archive is the only
    /// place it exists. Empty when the archive embeds no schema tree.
    pub schema_files: Vec<(String, Vec<u8>)>,
    /// Typed non-fatal issues surfaced by the install.
    pub warnings: Vec<WarningHint>,
}

/// Validate an archive and land it in the global content-addressed
/// cache — the cache-side half of `memstead install`, with **no
/// config or mount side effects** (the caller registers the returned
/// archive as a workspace-level read-only mount). Shares the
/// validator, the shadow-name gate, and the content-addressed
/// atomic-rename write with the historical combined path.
pub fn install_to_cache(
    archive_path: &Path,
    writable_mem_names: &[&str],
) -> Result<CacheInstallOutcome, InstallError> {
    let bytes = std::fs::read(archive_path)?;
    let validated = validate_and_normalize_archive(&bytes).map_err(InstallError::Validation)?;

    if let Some(shadowed) = writable_mem_names
        .iter()
        .find(|n| **n == validated.config.name.as_str())
    {
        return Err(InstallError::ShadowsWritable {
            archive_name: validated.config.name.clone(),
            shadows_writable: (*shadowed).to_string(),
        });
    }

    let cache_dir = mem_cache_dir();
    std::fs::create_dir_all(&cache_dir)?;
    let cache_key = content_cache_key(&validated.canonical_bytes);
    let dest = cache_dir.join(format!(
        "{}-{}.{ARCHIVE_EXTENSION}",
        validated.config.name, cache_key
    ));
    let copied_to_cache = if dest.exists() {
        false
    } else {
        let tmp = dest.with_extension(format!("{ARCHIVE_EXTENSION}.tmp"));
        std::fs::write(&tmp, &validated.canonical_bytes)?;
        std::fs::rename(&tmp, &dest)?;
        true
    };

    Ok(CacheInstallOutcome {
        mem_name: validated.config.name,
        schema: validated.config.schema.clone(),
        cache_path: dest,
        cache_key,
        copied_to_cache,
        schema_files: memstead_base::validator::archive::to_package_files(&validated.schema_files),
        warnings: Vec::new(),
    })
}

/// What happened on the mount-registration side of an install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountRegistration {
    /// Fresh registration.
    Registered,
    /// Same name, same content-addressed cache file — nothing to do.
    AlreadyRegistered,
    /// Same name, new content — the mount was re-pointed at the new
    /// cache file.
    Refreshed,
}

/// Register a cached archive (the outcome of [`install_to_cache`]) as
/// a workspace-level read-only mount on the live engine — the shared
/// back half of `memstead install` and the MCP server's `--read-mem`
/// boot flag. Idempotent per content: an existing read-only mount
/// under the same name is a no-op when it already points at this
/// cache file, and an in-place refresh (unregister + re-register)
/// when the content changed. The caller persists the mount state
/// (`engine.persist_state()`) after a `Registered` / `Refreshed`
/// outcome.
///
/// Staging comes first and for a reason: a mem published under a
/// third party's vocabulary carries that vocabulary inside the
/// archive and nowhere else, so the pin the mount resolves against
/// only exists once the embedded package has been written into the
/// workspace's local schema storage. Staging is idempotent and
/// refuses before any mount side effect, so a broken embedded schema
/// leaves neither a staged package nor a registered mount behind.
pub fn register_cached_archive(
    engine: &mut memstead_base::Engine,
    outcome: &CacheInstallOutcome,
    by_tool: &'static str,
) -> Result<MountRegistration, memstead_base::EngineError> {
    engine.stage_sealed_schema(&outcome.mem_name, &outcome.schema, &outcome.schema_files)?;

    let registration = match engine.mount(&outcome.mem_name) {
        Some(existing) if existing.capability == memstead_base::MountCapability::ReadOnly => {
            match &existing.storage {
                memstead_base::MountStorage::Archive { path } if *path == outcome.cache_path => {
                    return Ok(MountRegistration::AlreadyRegistered);
                }
                _ => {
                    engine.unregister_read_mount(&outcome.mem_name)?;
                    MountRegistration::Refreshed
                }
            }
        }
        // A writable mount of the same name is the caller's shadow
        // gate's business (install_to_cache refuses it up-front).
        _ => MountRegistration::Registered,
    };

    let mount = memstead_base::Mount {
        mem: outcome.mem_name.clone(),
        schema: Some(outcome.schema.clone()),
        storage: memstead_base::MountStorage::Archive {
            path: outcome.cache_path.clone(),
        },
        capability: memstead_base::MountCapability::ReadOnly,
        lifecycle: memstead_base::MountLifecycle::Eager,
        cross_linkable: false,
        migration_target: None,
    };
    let backend: Box<dyn memstead_base::MemBackend> = Box::new(
        memstead_base::storage::ArchiveBackend::new(outcome.cache_path.clone()),
    );
    let origin = memstead_base::MemOrigin::RuntimeCreated {
        at: std::time::SystemTime::now(),
        by_tool,
    };
    engine.register_read_mount(mount, backend, origin)?;
    Ok(registration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::export::export_mem;
    use tempfile::TempDir;

    /// Write a minimal valid mem directory to `mem_dir` and export it
    /// to `archive_path`. The resulting archive passes
    /// `validate_and_normalize_archive` — the fixture exists precisely so
    /// install tests don't have to hand-build validator-compliant bytes.
    fn build_valid_archive(mem_dir: &Path, archive_path: &Path, name: &str) {
        // Configs no longer carry an in-config `name` field. The
        // archive's identity comes from the disk-path basename via the
        // `published_config_from` fallback chain. Build the mem
        // directory under `<mem_dir.parent>/<name>/` so the basename
        // matches the requested name; tests can pass any throwaway
        // `mem_dir` path and trust the helper to align them.
        let mem_dir = mem_dir.parent().unwrap_or(mem_dir).join(name);
        std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
        std::fs::write(
            mem_dir.join(".memstead/config.json"),
            r#"{"version":"1.2.0","schema":"default@1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            mem_dir.join("alpha.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-01-15\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nA.\n\n## Purpose\n\nB.\n\n## Specifies\n\nC.\n\n## Constraints\n\nD.\n\n## Rationale\n\nE.\n",
        ).unwrap();

        let config = memstead_schema::load_and_validate(&mem_dir).unwrap();
        // No workspace context — the schema-source resolver falls through
        // to the embedded builtin.
        export_mem(&mem_dir, &config, archive_path, None, None, None).unwrap();
    }

    /// Cache-side install convenience for the test fixtures — no
    /// shadow set, no mount registration (the engine-side half has its
    /// own tests).
    fn cache_install(archive: &Path) -> Result<CacheInstallOutcome, InstallError> {
        install_to_cache(archive, &[])
    }

    /// Build a writable-mem config directory for install tests. Adds the
    /// minimal fields the config writer expects on load.
    fn write_minimal_mem_config(dir: &Path, _name: &str) {
        std::fs::create_dir_all(dir.join(".memstead")).unwrap();
        std::fs::write(
            dir.join(".memstead/config.json"),
            r#"{"version":"1.0.0","schema":"default@1.0.0"}"#,
        )
        .unwrap();
    }

    /// Process-global env lock. All install-helper tests take this before
    /// touching `MEMSTEAD_MEM_CACHE` so parallel runs inside the same
    /// cargo-test binary don't race on the shared process env. Rust 2024
    /// makes `env::set_var` unsafe precisely because concurrent reads can
    /// tear — the lock is the safety contract.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard for `MEMSTEAD_MEM_CACHE`: holds the global lock, installs
    /// the override, restores the previous value on drop.
    struct CacheGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev: Option<String>,
    }
    impl CacheGuard {
        fn install(cache_dir: &Path) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var(CACHE_OVERRIDE_ENV).ok();
            // SAFETY: the global mutex above serializes env access for
            // every test in this module; no other reader runs concurrently.
            unsafe {
                std::env::set_var(CACHE_OVERRIDE_ENV, cache_dir);
            }
            Self { _lock: lock, prev }
        }
    }
    impl Drop for CacheGuard {
        fn drop(&mut self) {
            // SAFETY: we still hold the lock acquired in `install`.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var(CACHE_OVERRIDE_ENV, v),
                    None => std::env::remove_var(CACHE_OVERRIDE_ENV),
                }
            }
        }
    }

    #[test]
    fn mem_cache_dir_honors_env_override() {
        let custom = std::env::temp_dir().join("memstead-cache-override-test");
        let _g = CacheGuard::install(&custom);
        assert_eq!(mem_cache_dir(), custom);
    }

    #[test]
    fn read_published_config_reads_whitelist_fields() {
        let tmp = TempDir::new().unwrap();
        // Published archive identity comes from the disk-path basename
        // via the `published_config_from` fallback chain (the in-config
        // `name` field is no longer authored).
        let mem_src = tmp.path().join("sample");
        let archive = tmp.path().join("sample.mem");
        build_valid_archive(&mem_src, &archive, "sample");

        let config = read_published_config(&archive).unwrap();
        assert_eq!(config.format, memstead_schema::PUBLISHED_MEM_FORMAT);
        assert_eq!(config.name, "sample");
        assert_eq!(config.version.to_string(), "1.2.0");
    }

    #[test]
    fn read_published_config_missing_file_is_archive_not_found() {
        let err = read_published_config(&PathBuf::from("/nonexistent/nope.mem")).unwrap_err();
        assert!(matches!(err, LoadError::ArchiveNotFound(_)));
    }

    #[test]
    fn read_published_config_corrupt_archive_is_zip_error() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("corrupt.mem");
        std::fs::write(&archive, b"definitely not a zip").unwrap();
        let err = read_published_config(&archive).unwrap_err();
        assert!(matches!(err, LoadError::Zip(_)));
    }

    #[test]
    fn install_validates_and_canonicalizes() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        let src_dir = tmp.path().join("src");
        let src = tmp.path().join("aws-patterns.mem");

        std::fs::create_dir_all(&project).unwrap();
        write_minimal_mem_config(&project, "specs");
        build_valid_archive(&src_dir, &src, "aws-patterns");

        let _g = CacheGuard::install(&cache);
        let outcome = cache_install(&src).unwrap();

        assert_eq!(outcome.mem_name, "aws-patterns");
        assert!(outcome.copied_to_cache);
        assert!(
            outcome.warnings.is_empty(),
            "current-format install must not warn: {:?}",
            outcome.warnings
        );

        // The outcome carries the content-addressed cache reference the
        // mount registration points at.
        let key = outcome.cache_key.as_str();
        let cached = cache.join(format!("aws-patterns-{key}.mem"));
        assert_eq!(outcome.cache_path, cached);
        assert!(cached.is_file(), "content-addressed cache file must exist");

        // Cached bytes must equal the validator's canonical form, and the
        // recorded key must be the digest of those bytes.
        let cached_bytes = std::fs::read(&cached).unwrap();
        let revalidated = validate_and_normalize_archive(&cached_bytes).unwrap();
        assert_eq!(revalidated.canonical_bytes, cached_bytes);
        assert_eq!(
            key,
            content_cache_key(&cached_bytes),
            "cacheKey is the content digest"
        );
    }

    #[test]
    fn install_leaves_no_tmp_on_success() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        let src_dir = tmp.path().join("src");
        let src = tmp.path().join("x.mem");
        std::fs::create_dir_all(&project).unwrap();
        write_minimal_mem_config(&project, "specs");
        build_valid_archive(&src_dir, &src, "alpha");

        let _g = CacheGuard::install(&cache);
        cache_install(&src).unwrap();

        // The temp-then-rename path must leave the content-addressed
        // `<name>-<key>.mem` on disk and never the `.tmp` sibling. The
        // filename is derived from the validator's approved `config.name`
        // ("alpha") plus the content key, not from the submitted filename.
        let entries: Vec<_> = std::fs::read_dir(&cache)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries.iter().filter(|n| n.ends_with(".mem")).count(),
            1,
            "exactly one cache file, no .tmp sibling: {entries:?}",
        );
        let cache_file = entries.iter().find(|n| n.ends_with(".mem")).unwrap();
        assert!(
            cache_file.starts_with("alpha-"),
            "name-keyed prefix: {cache_file}"
        );
        assert!(!entries.iter().any(|n| n.ends_with(".tmp")));
    }

    #[test]
    fn install_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        let src_dir = tmp.path().join("src");
        let src = tmp.path().join("x.mem");
        std::fs::create_dir_all(&project).unwrap();
        write_minimal_mem_config(&project, "specs");
        build_valid_archive(&src_dir, &src, "alpha");

        let _g = CacheGuard::install(&cache);
        let first = cache_install(&src).unwrap();
        assert!(first.copied_to_cache);

        // Second run: the cache side effect reports `false`. The cache
        // file survives untouched (existing-file guard fires before
        // the canonical write) and the content key is stable.
        let second = cache_install(&src).unwrap();
        assert!(!second.copied_to_cache);
        assert_eq!(first.cache_key, second.cache_key);
    }

    /// Two byte-distinct archives that share an internal mem name both
    /// install successfully into distinct content-addressed cache files —
    /// neither blocks nor silently shadows the other, and the registration
    /// records each archive's own `cacheKey`. This replaces the prior
    /// `CACHE_NAME_COLLISION` refusal, which was a dead end requiring
    /// manual cache-file deletion.
    #[test]
    fn install_distinct_archives_same_name_coexist_via_content_address() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        let src_a_dir = tmp.path().join("src-a");
        let src_a = tmp.path().join("a.mem");
        std::fs::create_dir_all(&project).unwrap();
        write_minimal_mem_config(&project, "specs");
        build_valid_archive(&src_a_dir, &src_a, "alpha");

        let _g = CacheGuard::install(&cache);
        let first = cache_install(&src_a).unwrap();
        assert!(first.copied_to_cache);
        let key_a = first.cache_key.clone();

        // Build a *different* archive that lands at the same canonical
        // name (`alpha`) with distinct content.
        let src_b_dir = tmp.path().join("src-b");
        std::fs::create_dir_all(src_b_dir.join("alpha/.memstead")).unwrap();
        std::fs::write(
            src_b_dir.join("alpha/.memstead/config.json"),
            r#"{"version":"1.2.0","schema":"default@1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            src_b_dir.join("alpha/beta.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-01-15\nlevel: M0\n---\n# Beta\n\n## Identity\n\nA different content.\n\n## Purpose\n\nB different content.\n\n## Specifies\n\nC different content.\n\n## Constraints\n\nD different content.\n\n## Rationale\n\nE different content.\n",
        ).unwrap();
        let src_b = tmp.path().join("b.mem");
        let cfg_b = memstead_schema::load_and_validate(&src_b_dir.join("alpha")).unwrap();
        crate::ops::export::export_mem(&src_b_dir.join("alpha"), &cfg_b, &src_b, None, None, None)
            .unwrap();
        assert_ne!(
            std::fs::read(&src_a).unwrap(),
            std::fs::read(&src_b).unwrap(),
            "fixture must produce two distinct archives sharing the name `alpha`"
        );

        // Second install (different bytes, same name): SUCCEEDS — no
        // collision, no dead end.
        let second = cache_install(&src_b).unwrap();
        assert!(
            second.copied_to_cache,
            "distinct bytes must install, not collide"
        );
        let key_b = second.cache_key.clone();

        // Distinct content ⇒ distinct keys ⇒ both cache files coexist.
        assert_ne!(
            key_a, key_b,
            "distinct archives must get distinct content keys"
        );
        assert!(cache.join(format!("alpha-{key_a}.mem")).is_file());
        assert!(cache.join(format!("alpha-{key_b}.mem")).is_file());
    }

    /// A re-install with byte-identical input is the idempotent success
    /// path — no write, no commit, no churn, and
    /// `copied_to_cache: false`. The pre-fix idempotency contract is
    /// preserved; what's gone is the silent third state where
    /// `copied_to_cache: false` admitted unrelated bytes.
    #[test]
    fn install_idempotent_path_returns_false_without_refusal() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        let src_dir = tmp.path().join("src");
        let src = tmp.path().join("x.mem");
        std::fs::create_dir_all(&project).unwrap();
        write_minimal_mem_config(&project, "specs");
        build_valid_archive(&src_dir, &src, "alpha");

        let _g = CacheGuard::install(&cache);
        let first = cache_install(&src).unwrap();
        assert!(first.copied_to_cache);

        // Re-install with the SAME archive bytes — canonical(input)
        // matches the cache file → idempotent success.
        let second = cache_install(&src).unwrap();
        assert!(
            !second.copied_to_cache,
            "idempotent re-install must report copied_to_cache: false"
        );
    }

    /// Rewrite a current-layout archive so its meta members live under a
    /// non-whitelisted dir (`.other/` instead of `.memstead/`). Test-only.
    fn repack_with_foreign_meta_dir(src: &Path, dest: &Path) {
        use std::io::{Read as _, Write as _};
        let file = std::fs::File::open(src).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let out = std::fs::File::create(dest).unwrap();
        let mut writer = zip::ZipWriter::new(out);
        let opts = zip::write::SimpleFileOptions::default();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let name = entry.name().to_string();
            let name = match name.strip_prefix(".memstead/") {
                Some(rest) => format!(".other/{rest}"),
                None => name,
            };
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            writer.start_file(name, opts).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    /// Only the `.memstead/` meta layout is tolerated: an archive whose
    /// meta members live under any other dir fails at validation — its
    /// members fall outside the `.memstead/` whitelist.
    #[test]
    fn install_foreign_meta_layout_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        let src_dir = tmp.path().join("src");
        let modern = tmp.path().join("modern.mem");
        std::fs::create_dir_all(&project).unwrap();
        write_minimal_mem_config(&project, "specs");
        build_valid_archive(&src_dir, &modern, "foreign-mem");

        let foreign = tmp.path().join("foreign-mem.mem");
        repack_with_foreign_meta_dir(&modern, &foreign);

        let _g = CacheGuard::install(&cache);
        let err =
            cache_install(&foreign).expect_err("a foreign meta-layout archive must not install");
        assert!(matches!(err, InstallError::Validation(_)), "got {err:?}");
    }

    #[test]
    fn install_rejects_non_archive_bytes() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        write_minimal_mem_config(&project, "specs");
        let src = tmp.path().join("bad.mem");
        std::fs::write(&src, b"not a zip").unwrap();

        let _g = CacheGuard::install(&cache);
        let err = cache_install(&src).unwrap_err();
        assert!(matches!(err, InstallError::Validation(_)));
        // Validation failed up front → neither cache file nor temp
        // sibling was written.
        assert!(!cache.join("bad.mem").exists());
        assert!(!cache.join("bad.mem.tmp").exists());
    }
}
