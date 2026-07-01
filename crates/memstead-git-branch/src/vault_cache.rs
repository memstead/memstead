//! Read-vault cache resolution, published-config reads, and the
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
//! works on macOS (`~/Library/Application Support/memstead/vaults`), Linux
//! (`$XDG_DATA_HOME/memstead/vaults` or `~/.local/share/memstead/vaults`), and
//! Windows (`%APPDATA%\memstead\vaults`). For tests, `MEMSTEAD_VAULT_CACHE`
//! overrides the base so temp dirs can stand in without touching the
//! user's real data directory.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use memstead_base::ops::WarningHint;
use memstead_schema::{
    ARCHIVE_CONFIG_PATH, ARCHIVE_EXTENSION, ARCHIVE_SCHEMA_PREFIX, LEGACY_ARCHIVE_EXTENSIONS,
    PublishedVaultConfig, SchemaRef,
    SchemaRegistry,
};
use serde_json::{Map, Value, json};

use crate::entity::loader::LoadError;
use crate::validator::{ValidationError, validate_and_normalize_archive};
use crate::vault_repo_config::{self, VaultRepoWriteError};
use crate::vcs::CommitContext;

/// Where the per-vault `readVaults` registration should land.
///
/// `Disk` mirrors the legacy disk-shaped workspace: `install_read_vault`
/// reads `<vault_dir>/.memstead/config.json`, mutates `readVaults`, and
/// writes the updated bytes back. `VaultRepo` targets the post-cutover
/// vault-repo-backed workspace: the same mutation lands as a tree commit
/// on `vault-repo-git:__MEMSTEAD:vaults/<vault_name>/config.json` instead.
///
/// One enum keeps the validator + cache-copy logic shared across both
/// shapes — the config-registration step is the only branching point.
#[derive(Debug, Clone, Copy)]
pub enum TargetVault<'a> {
    /// Legacy disk-shaped vault. `path` is the directory containing
    /// `.memstead/config.json`.
    Disk(&'a Path),
    /// Post-cutover vault-repo-backed vault. The config blob lives in
    /// `<workspace_root>/vault-repo/.git/` at `__MEMSTEAD:vaults/<vault_name>/config.json`.
    VaultRepo {
        workspace_root: &'a Path,
        vault_name: &'a str,
    },
}

/// Env var that overrides `<data_dir>/memstead/vaults` for tests.
pub const CACHE_OVERRIDE_ENV: &str = "MEMSTEAD_VAULT_CACHE";

/// Resolve the global vault-cache directory.
///
/// Respects `MEMSTEAD_VAULT_CACHE` if set — tests use this to point at a
/// tempdir without touching the real user-data directory. Otherwise
/// returns `<data_dir>/memstead/vaults` on every platform (macOS / Linux /
/// Windows), so the CLI and the Memstead app resolve to the same path
/// without per-platform branching.
///
/// `dirs::data_dir()` is infallible on Tier-1 platforms; `expect` is
/// fine for an engine that only runs on systems with a resolvable home.
pub fn vault_cache_dir() -> PathBuf {
    if let Ok(override_path) = std::env::var(CACHE_OVERRIDE_ENV)
        && !override_path.is_empty()
    {
        return PathBuf::from(override_path);
    }
    dirs::data_dir()
        .expect("platform provides a data directory")
        .join("memstead")
        .join("vaults")
}

/// Read the whitelisted `.memstead/config.json` from a cached archive.
///
/// Does **not** re-run full archive validation — the cache only
/// contains bytes the validator already approved, so entity parse and
/// graph construction can be deferred to the caller. Configs are
/// re-parsed with `parse_config_bytes` so the strict-ingress shape is
/// enforced here as defense-in-depth against a tampered cache file.
pub fn read_published_config(archive_path: &Path) -> Result<PublishedVaultConfig, LoadError> {
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

/// Outcome of an `install_read_vault` call — captured so callers can log
/// what actually happened without re-deriving it from side effects.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// Vault name, taken from the validator's approved config.
    pub vault_name: String,
    /// `true` if canonical bytes were written into the cache on this
    /// call; `false` if the content-addressed cache file already
    /// existed and was left alone.
    pub copied_to_cache: bool,
    /// `true` if a new `readVaults` entry was added to the vault config
    /// on this call; `false` if the name was already declared.
    pub registered_in_config: bool,
    /// Typed non-fatal issues. Today: `LEGACY_ARCHIVE_FORMAT` when the
    /// submitted archive carried the prior `.mstd` extension — the
    /// install succeeded (the cache always receives canonical
    /// `.mem`-extension bytes), the warning tells the operator to
    /// re-export upstream.
    pub warnings: Vec<WarningHint>,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("could not read vault archive: {0}")]
    Archive(#[from] LoadError),
    #[error("io error while installing vault: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error while registering vault: {0}")]
    Config(#[from] memstead_schema::config::ConfigError),
    #[error("archive failed strict validation: {0}")]
    Validation(ValidationError),
    /// Vault-db tree write failed. Carries the underlying gix error
    /// message so callers can surface it without wrapping the variant.
    #[error("vault-repo tree write failed: {0}")]
    VaultRepo(#[from] VaultRepoWriteError),
    /// The archive's
    /// authoritative vault name (carried in its canonical config)
    /// matches a writable mount that already exists in this
    /// workspace. Registering the read-vault would silently shadow
    /// (the engine's boot-time `hydrate_read_vaults` skips read-vault
    /// names that collide with writable mounts), so the install
    /// surface refuses up-front rather than registering a no-op.
    /// An earlier message advised `install to a different
    /// `--vault-name` target` — but `--vault-name` selects the
    /// *host* writable vault to register the read-vault into, not
    /// the read-vault's internal name. The flag cannot rename the
    /// archive. The genuine recovery is to unregister or rename
    /// the writable mount that shadows the archive's internal name.
    #[error(
        "archive's vault name `{archive_name}` already exists as a writable mount in this workspace; \
         unregister or rename the writable mount first (the `--vault` flag selects which writable \
         host vault to register *into* — it does not rename the archive's internal vault)"
    )]
    ShadowsWritable {
        archive_name: String,
        shadows_writable: String,
    },
    // `CacheNameCollision` was retired once the cache became
    // content-addressed (`<name>-<content_key>.mem`): distinct bytes
    // under the same vault name land in distinct files and the collision
    // class it guarded no longer exists. No engine surface can produce it.
}

/// Short content-address for an installed archive: the first 16 hex chars
/// of `sha256(canonical_bytes)`. Used as the cache-file key
/// (`<name>-<key>.mem`) and recorded in the `readVaults` registration so
/// the loader resolves the right file. 64 bits is ample collision
/// resistance for a per-user cache; the same convention (truncated SHA-256
/// hex) the entity content-hash uses.
fn content_cache_key(canonical_bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(canonical_bytes);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Install a sealed vault archive into the global cache and register
/// it in a writable vault's config. Accepts both the current `.mem`
/// format and the prior `.mstd` extension; exercising the legacy path
/// succeeds and lands a `LEGACY_ARCHIVE_FORMAT` warning on the returned
/// [`InstallOutcome`].
///
/// Two independent side effects, both idempotent:
///
/// 1. If the content-addressed cache file does not exist: run the submitted bytes
///    through `validate_and_normalize_archive` and write the
///    validator's `canonical_bytes` via a `.tmp` sibling + atomic
///    rename. A mid-write crash leaves the temp file behind, never a
///    partial cache file. Existing cache files are left untouched —
///    overwrite-on-newer-version is an app-level update flow, not a
///    CLI install semantic. Users who want to force-replace can delete
///    the cache file first.
/// 2. If the target vault's config does not already list this vault
///    under `readVaults`, add an entry with `source: { type: "local" }`.
///    Existing entries are left untouched so re-running install never
///    clobbers a `type: "url"` (etc.) source the user configured by hand.
///
/// The `target` parameter selects where the registration lands:
/// - `TargetVault::Disk(vault_dir)` writes the updated config back to
///   `<vault_dir>/.memstead/config.json` (legacy disk shape).
/// - `TargetVault::VaultRepo { workspace_root, vault_name }` commits the
///   updated `configs/<vault_name>.json` to `vault-repo-git:main` (post-
///   cutover shape).
///
/// `ctx` and `commit_message` are used only by the `VaultRepo` arm —
/// the disk arm rewrites the file via the existing config-update path
/// which has its own (file-mtime-based) provenance trail.
///
/// Returns an `InstallOutcome` describing which effects fired. The
/// authoritative vault name comes from the validator's approved
/// config, not from the submitted filename or caller argument.
pub fn install_read_vault(
    archive_path: &Path,
    target: TargetVault<'_>,
    ctx: &CommitContext<'_>,
    commit_message: &str,
    writable_vault_names: &[&str],
) -> Result<InstallOutcome, InstallError> {
    // 1. Validate + canonicalize. Never install bytes the validator
    //    rejected; never install the caller's original bytes — what
    //    lands in the cache is always the validator's canonical form.
    let bytes = std::fs::read(archive_path)?;
    let validated = validate_and_normalize_archive(&bytes).map_err(InstallError::Validation)?;

    // Legacy-window deprecation signal: the prior `.mstd` file
    // extension still installs, but the outcome carries a typed warning
    // so callers (CLI output, MCP boot log) surface it without
    // re-deriving. (A `.mdgv`-extension/`.mdgv/`-layout archive no
    // longer reaches here — the validator rejects it.)
    let legacy_extension = archive_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            LEGACY_ARCHIVE_EXTENSIONS
                .iter()
                .any(|legacy| e.eq_ignore_ascii_case(legacy))
        });
    let mut warnings: Vec<WarningHint> = Vec::new();
    if legacy_extension {
        warnings.push(WarningHint::LegacyArchiveFormat {
            archive: archive_path.display().to_string(),
        });
    }

    // Refuse up-front
    // when the archive's authoritative name shadows a writable mount
    // in the caller's workspace. The boot-time
    // `hydrate_read_vaults` silently skips a read-vault registration
    // that collides with a writable mount — without this gate the
    // install reports success but the subsequent reload produces no
    // observable effect. The check is opt-in via the caller-supplied
    // `writable_vault_names` slice; passing an empty slice (no
    // workspace context available) skips the gate, preserving the
    // engine-helper's testability in non-workspace contexts.
    if let Some(shadowed) = writable_vault_names
        .iter()
        .find(|n| **n == validated.config.name.as_str())
    {
        return Err(InstallError::ShadowsWritable {
            archive_name: validated.config.name.clone(),
            shadows_writable: (*shadowed).to_string(),
        });
    }

    // 2. Content-addressed atomic-rename write. The cache file is keyed
    //    by `<name>-<content_key>.mem`, where `content_key` is a short
    //    digest of the validator's canonical bytes. `name` passed the
    //    strict slug regex and the key is hex, so the path is provably
    //    safe on every platform.
    //
    //    Content-addressing removes the
    //    name-collision class entirely. Two distinct archives sharing an
    //    internal vault name produce distinct keys → distinct files, so
    //    they coexist in the global cache without one shadowing the other
    //    (the per-registration `cacheKey` resolves each workspace to the
    //    right file). Re-installing byte-identical content resolves to the
    //    same key → the file already exists → idempotent dedup no-op. The
    //    prior `CACHE_NAME_COLLISION` dead end (distinct bytes, same name,
    //    no engine-reachable remedy) can no longer occur.
    let cache_dir = vault_cache_dir();
    std::fs::create_dir_all(&cache_dir)?;
    let cache_key = content_cache_key(&validated.canonical_bytes);
    let dest = cache_dir.join(format!(
        "{}-{}.{ARCHIVE_EXTENSION}",
        validated.config.name, cache_key
    ));
    let copied_to_cache = if dest.exists() {
        // The key IS the content digest, so an existing file at this path
        // is byte-identical by construction — dedup, skip the write.
        false
    } else {
        let tmp = dest.with_extension(format!("{ARCHIVE_EXTENSION}.tmp"));
        std::fs::write(&tmp, &validated.canonical_bytes)?;
        std::fs::rename(&tmp, &dest)?;
        true
    };

    // 3. Config-registration side effect — branches on target shape. The
    //    `cache_key` is recorded in the `readVaults` entry so the loader
    //    resolves the content-addressed file.
    let registered_in_config = match target {
        TargetVault::Disk(vault_dir) => {
            let (mut config, config_path) = memstead_schema::config::load_config(vault_dir)?;
            register_read_vault_in_config(
                &config_path,
                &mut config,
                &validated.config.name,
                &cache_key,
            )?
        }
        TargetVault::VaultRepo {
            workspace_root,
            vault_name,
        } => register_read_vault_in_vault_repo(
            workspace_root,
            vault_name,
            &validated.config.name,
            &cache_key,
            ctx,
            commit_message,
        )?,
    };

    Ok(InstallOutcome {
        vault_name: validated.config.name,
        copied_to_cache,
        registered_in_config,
        warnings,
    })
}

/// Register `read_vault_name` in the workspace vault `vault_name`'s
/// `configs/<vault_name>.json` blob on `vault-repo-git:main`. Read-modify-
/// write: parse the existing blob, insert the `readVaults` entry if
/// missing, serialize, commit on top of `main`. Returns `true` if the
/// entry was added, `false` if it was already declared (no commit lands).
///
/// Race window: non-atomic against concurrent writers on `main`. See
/// `vault_repo_config::commit_config`'s docstring.
fn register_read_vault_in_vault_repo(
    workspace_root: &Path,
    vault_name: &str,
    read_vault_name: &str,
    cache_key: &str,
    ctx: &CommitContext<'_>,
    commit_message: &str,
) -> Result<bool, InstallError> {
    use memstead_schema::config::ConfigError;

    // Read the current blob bytes from the tree, parse as JSON, mutate.
    let config = vault_repo_config::read_config(workspace_root, vault_name)
        .map_err(|e| ConfigError::Other(format!("read configs/{vault_name}.json: {e}")))?;
    let mut value = serde_json::to_value(&config)
        .map_err(|e| ConfigError::Other(format!("re-serialize VaultConfig: {e}")))?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| ConfigError::Other("config root must be a JSON object".into()))?;

    let entry = obj
        .entry("readVaults")
        .or_insert_with(|| Value::Object(Map::new()));
    let map = entry
        .as_object_mut()
        .ok_or_else(|| ConfigError::Other("readVaults must be a JSON object".into()))?;

    if map.contains_key(read_vault_name) {
        return Ok(false);
    }

    map.insert(
        read_vault_name.to_string(),
        json!({ "source": { "type": "local" }, "cacheKey": cache_key }),
    );

    let updated_bytes = serde_json::to_vec_pretty(&value)
        .map_err(|e| ConfigError::Other(format!("serialize updated config: {e}")))?;
    vault_repo_config::commit_config(
        workspace_root,
        vault_name,
        &updated_bytes,
        ctx,
        commit_message,
    )?;
    Ok(true)
}

/// Add a `readVaults` entry for `vault_name` with `source: { type: "local" }`
/// to `config` and persist the change. Returns `true` if the map changed,
/// `false` if the name was already declared (any source) so the config was
/// left untouched and no write happened.
///
/// Kept private because the only valid caller today is `install_read_vault`;
/// hand-editing read vaults from inside the engine would bypass the
/// archive-validation step up front.
fn register_read_vault_in_config(
    config_path: &Path,
    config: &mut Value,
    vault_name: &str,
    cache_key: &str,
) -> Result<bool, memstead_schema::config::ConfigError> {
    let obj = config.as_object_mut().ok_or_else(|| {
        memstead_schema::config::ConfigError::Other("config root must be a JSON object".into())
    })?;

    let entry = obj
        .entry("readVaults")
        .or_insert_with(|| Value::Object(Map::new()));
    let map = entry.as_object_mut().ok_or_else(|| {
        memstead_schema::config::ConfigError::Other("readVaults must be a JSON object".into())
    })?;

    if map.contains_key(vault_name) {
        return Ok(false);
    }

    map.insert(
        vault_name.to_string(),
        json!({ "source": { "type": "local" }, "cacheKey": cache_key }),
    );

    // Route through update_config_field so the commit path (validation +
    // pretty-print + trailing newline) stays in one place. We pass the
    // already-mutated map back in so the writer just serializes it.
    let new_read_vaults = Value::Object(map.clone());
    memstead_schema::config::update_config_field(
        config_path,
        config,
        "readVaults",
        new_read_vaults,
        false,
    )?;
    Ok(true)
}

/// Outcome of `extract_archive_schema_if_needed` — so callers can log
/// the specific reason a no-op happened, or know whether the vault's
/// schema registry needs to be rebuilt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaExtractionOutcome {
    /// The archive's pinned schema is already registered — extraction
    /// skipped. Author-layer schemas always shadow cache entries, so
    /// skipping when the registry already knows the pin preserves the
    /// documented precedence order.
    AlreadyRegistered,
    /// The archive carries no `.memstead/schema/` tree. Loading still works
    /// if the pin happens to be in the registry; otherwise the normal
    /// `resolve_vault_schema` path reports the missing schema with its
    /// actionable error.
    NoEmbeddedSchema,
    /// A cache entry at
    /// `<workspace_root>/.memstead.cache/schemas/<name>-<version>/`
    /// already existed on disk — extraction skipped, but the registry
    /// may still need a rebuild if the caller hadn't picked it up yet.
    CacheAlreadyPopulated,
    /// Fresh extraction wrote files into
    /// `<workspace_root>/.memstead.cache/schemas/<name>-<version>/`. Caller
    /// must rebuild the `SchemaRegistry` to pick it up.
    Extracted { schema: SchemaRef, path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum SchemaExtractionError {
    #[error("could not read vault archive {}: {source}", .archive_path.display())]
    Archive {
        archive_path: PathBuf,
        #[source]
        source: LoadError,
    },
    #[error("archive {} failed strict validation: {source}", .archive_path.display())]
    Validation {
        archive_path: PathBuf,
        #[source]
        source: ValidationError,
    },
    #[error("i/o error extracting schema to {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Extract the schema embedded in `archive_path` into the writable
/// vault's cache, but only when the pinned `(name, version)` is not
/// already registered.
///
/// Idempotent: repeated calls on the same archive are safe. Runs the
/// archive through `validate_and_normalize_archive` — which enforces
/// embedded-schema integrity (the loader-based manifest check + name/
/// version match against `.memstead/config.json`), so a corrupt schema
/// surfaces here as a `Validation` error instead of silently polluting
/// the cache.
///
/// The extraction path is atomic: files are written into a sibling
/// `.tmp` directory and renamed into place only after every byte has
/// landed. A mid-write crash leaves the `.tmp` sibling behind, never
/// a half-populated `<name>-<version>/` that a subsequent
/// `SchemaRegistry::load_for_vault` might try to load.
pub fn extract_archive_schema_if_needed(
    archive_path: &Path,
    workspace_root: &Path,
    registry: &SchemaRegistry,
) -> Result<SchemaExtractionOutcome, SchemaExtractionError> {
    // Cheap prefix pass: read only the archive's published config so we can skip
    // the full validation for archives whose pin is already in the
    // registry (the common case for repeat loads).
    let config = read_published_config(archive_path).map_err(|source| {
        SchemaExtractionError::Archive {
            archive_path: archive_path.to_path_buf(),
            source,
        }
    })?;
    if registry
        .get(&config.schema.name, &config.schema.version)
        .is_some()
    {
        return Ok(SchemaExtractionOutcome::AlreadyRegistered);
    }

    let dest = workspace_root
        .join(".memstead.cache/schemas")
        .join(format!("{}-{}", config.schema.name, config.schema.version));
    if dest.is_dir() {
        // Someone already extracted; the registry just hasn't rebuilt
        // with the cache pass yet. Caller rebuilds.
        return Ok(SchemaExtractionOutcome::CacheAlreadyPopulated);
    }

    // Full validation — loads the archive, validates the embedded schema
    // via `check_embedded_schema`, produces canonical bytes. We only
    // need the schema files, but paying for the full pipeline once on
    // cache-miss is correct: an attacker who drops a tampered archive
    // into the global cache doesn't get to seed the workspace from an
    // unvalidated payload.
    let bytes =
        std::fs::read(archive_path).map_err(|source| SchemaExtractionError::Io {
            path: archive_path.to_path_buf(),
            source,
        })?;
    let validated = validate_and_normalize_archive(&bytes).map_err(|source| {
        SchemaExtractionError::Validation {
            archive_path: archive_path.to_path_buf(),
            source,
        }
    })?;

    if validated.schema_files.is_empty() {
        return Ok(SchemaExtractionOutcome::NoEmbeddedSchema);
    }

    extract_schema_files_atomic(&validated.schema_files, &dest).map_err(|source| {
        SchemaExtractionError::Io {
            path: dest.clone(),
            source,
        }
    })?;

    Ok(SchemaExtractionOutcome::Extracted {
        schema: config.schema,
        path: dest,
    })
}

/// Write `schema_files` to `dest` via a sibling `.tmp` directory that
/// is renamed into place once every file has been written. Rename
/// atomicity varies by FS but every supported target (ext4, HFS+, APFS,
/// NTFS) gives us "dest contains every file or nothing," which is the
/// invariant the load path relies on. The incoming paths always start
/// with `.memstead/schema/` (legacy archives are normalized at extract
/// time) — we strip that prefix so the on-disk layout matches the
/// schema-cache shape `schema.yaml` + `types/<t>.yaml` exactly.
fn extract_schema_files_atomic(
    schema_files: &[crate::validator::archive::SchemaFile],
    dest: &Path,
) -> std::io::Result<()> {
    let parent = dest.parent().ok_or_else(|| {
        std::io::Error::other("schema cache destination has no parent directory")
    })?;
    std::fs::create_dir_all(parent)?;

    // Sibling tmp dir name is dot-prefixed so `list_schema_subdirs`
    // ignores it if a crash between write and rename leaves a straggler.
    // PID + monotonic counter guarantees uniqueness across concurrent
    // extractions of the same pin; the atomic rename serializes the
    // winner and the loser's tmp dir gets best-effort cleanup on the
    // returned error.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = parent.join(format!(
        ".memstead-schema-extract-{}-{}",
        std::process::id(),
        ts,
    ));

    // Wipe any leftover from a previous failed extract with the same
    // PID+time — the path is ours by construction.
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;

    for sf in schema_files {
        let rel = sf
            .archive_path
            .strip_prefix(ARCHIVE_SCHEMA_PREFIX)
            .unwrap_or(sf.archive_path.as_str());
        let file_path = tmp.join(rel);
        if let Some(file_parent) = file_path.parent() {
            std::fs::create_dir_all(file_parent)?;
        }
        std::fs::write(&file_path, sf.content.as_bytes())?;
    }

    match std::fs::rename(&tmp, dest) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Rename lost (dest appeared from a racer, or some other
            // filesystem error). Clean up our tmp so we don't leave a
            // stray `.memstead-schema-extract-*` sibling behind.
            let _ = std::fs::remove_dir_all(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::export::export_vault;
    use tempfile::TempDir;

    /// Write a minimal valid vault directory to `vault_dir` and export it
    /// to `archive_path`. The resulting archive passes
    /// `validate_and_normalize_archive` — the fixture exists precisely so
    /// install tests don't have to hand-build validator-compliant bytes.
    fn build_valid_archive(vault_dir: &Path, archive_path: &Path, name: &str) {
        // Configs no longer carry an in-config `name` field. The
        // archive's identity comes from the disk-path basename via the
        // `published_config_from` fallback chain. Build the vault
        // directory under `<vault_dir.parent>/<name>/` so the basename
        // matches the requested name; tests can pass any throwaway
        // `vault_dir` path and trust the helper to align them.
        let vault_dir = vault_dir
            .parent()
            .unwrap_or(vault_dir)
            .join(name);
        std::fs::create_dir_all(vault_dir.join(".memstead")).unwrap();
        std::fs::write(
            vault_dir.join(".memstead/config.json"),
            r#"{"version":"1.2.0","schema":"default@1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            vault_dir.join("alpha.md"),
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-01-15\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nA.\n\n## Purpose\n\nB.\n\n## Specifies\n\nC.\n\n## Constraints\n\nD.\n\n## Rationale\n\nE.\n",
        ).unwrap();

        let config = memstead_schema::load_and_validate(&vault_dir).unwrap();
        // No workspace context — the schema-source resolver falls through
        // to the embedded builtin.
        export_vault(&vault_dir, &config, archive_path, None, None).unwrap();
    }

    /// Disk-shape install convenience for the existing test fixtures.
    /// Wraps `install_read_vault(archive, TargetVault::Disk(project), ...)`
    /// with a deterministic dummy commit context so the call shape stays
    /// minimal at every test site.
    fn install_to_disk(
        archive: &Path,
        project: &Path,
    ) -> Result<InstallOutcome, InstallError> {
        install_read_vault(
            archive,
            TargetVault::Disk(project),
            &CommitContext::internal(),
            "memstead: install (test)",
            &[],
        )
    }

    /// Build a writable-vault config directory for install tests. Adds the
    /// minimal fields the config writer expects on load.
    fn write_minimal_vault_config(dir: &Path, _name: &str) {
        std::fs::create_dir_all(dir.join(".memstead")).unwrap();
        std::fs::write(
            dir.join(".memstead/config.json"),
            r#"{"version":"1.0.0","schema":"default@1.0.0"}"#,
        )
        .unwrap();
    }

    /// Process-global env lock. All install-helper tests take this before
    /// touching `MEMSTEAD_VAULT_CACHE` so parallel runs inside the same
    /// cargo-test binary don't race on the shared process env. Rust 2024
    /// makes `env::set_var` unsafe precisely because concurrent reads can
    /// tear — the lock is the safety contract.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard for `MEMSTEAD_VAULT_CACHE`: holds the global lock, installs
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
    fn vault_cache_dir_honors_env_override() {
        let custom = std::env::temp_dir().join("memstead-cache-override-test");
        let _g = CacheGuard::install(&custom);
        assert_eq!(vault_cache_dir(), custom);
    }

    #[test]
    fn read_published_config_reads_whitelist_fields() {
        let tmp = TempDir::new().unwrap();
        // Published archive identity comes from the disk-path basename
        // via the `published_config_from` fallback chain (the in-config
        // `name` field is no longer authored).
        let vault_src = tmp.path().join("sample");
        let archive = tmp.path().join("sample.mem");
        build_valid_archive(&vault_src, &archive, "sample");

        let config = read_published_config(&archive).unwrap();
        assert_eq!(config.format, memstead_schema::PUBLISHED_VAULT_FORMAT);
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
        write_minimal_vault_config(&project, "specs");
        build_valid_archive(&src_dir, &src, "aws-patterns");

        let _g = CacheGuard::install(&cache);
        let outcome = install_to_disk(&src, &project).unwrap();

        assert_eq!(outcome.vault_name, "aws-patterns");
        assert!(outcome.copied_to_cache);
        assert!(outcome.registered_in_config);
        assert!(
            outcome.warnings.is_empty(),
            "current-format install must not warn: {:?}",
            outcome.warnings
        );

        // Project config lists the vault with a local source and the
        // content-addressed cache key the loader resolves against.
        let cfg_raw = std::fs::read_to_string(project.join(".memstead/config.json")).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&cfg_raw).unwrap();
        let rv = cfg["readVaults"]["aws-patterns"]["source"]["type"].as_str();
        assert_eq!(rv, Some("local"));
        let key = cfg["readVaults"]["aws-patterns"]["cacheKey"]
            .as_str()
            .expect("registration must record the content cacheKey");

        let cached = cache.join(format!("aws-patterns-{key}.mem"));
        assert!(cached.is_file(), "content-addressed cache file must exist");

        // Cached bytes must equal the validator's canonical form, and the
        // recorded key must be the digest of those bytes.
        let cached_bytes = std::fs::read(&cached).unwrap();
        let revalidated = validate_and_normalize_archive(&cached_bytes).unwrap();
        assert_eq!(revalidated.canonical_bytes, cached_bytes);
        assert_eq!(key, content_cache_key(&cached_bytes), "cacheKey is the content digest");
    }

    #[test]
    fn install_leaves_no_tmp_on_success() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        let src_dir = tmp.path().join("src");
        let src = tmp.path().join("x.mem");
        std::fs::create_dir_all(&project).unwrap();
        write_minimal_vault_config(&project, "specs");
        build_valid_archive(&src_dir, &src, "alpha");

        let _g = CacheGuard::install(&cache);
        install_to_disk(&src, &project).unwrap();

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
        assert!(cache_file.starts_with("alpha-"), "name-keyed prefix: {cache_file}");
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
        write_minimal_vault_config(&project, "specs");
        build_valid_archive(&src_dir, &src, "alpha");

        let _g = CacheGuard::install(&cache);
        let first = install_to_disk(&src, &project).unwrap();
        assert!(first.copied_to_cache);
        assert!(first.registered_in_config);

        // Second run: both side effects report `false`. The cache file
        // survives untouched (existing-file guard fires before the
        // canonical write).
        let second = install_to_disk(&src, &project).unwrap();
        assert!(!second.copied_to_cache);
        assert!(!second.registered_in_config);
    }

    #[test]
    fn install_preserves_existing_non_local_source() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        let src_dir = tmp.path().join("src");
        let src = tmp.path().join("x.mem");
        std::fs::create_dir_all(project.join(".memstead")).unwrap();
        std::fs::write(
            project.join(".memstead/config.json"),
            r#"{
                "version":"1.0.0",
                "schema":"default@1.0.0",
                "readVaults": {
                    "alpha": {"source":{"type":"url","url":"https://example.com/x.mem"}}
                }
            }"#,
        )
        .unwrap();
        build_valid_archive(&src_dir, &src, "alpha");

        let _g = CacheGuard::install(&cache);
        let outcome = install_to_disk(&src, &project).unwrap();
        assert!(outcome.copied_to_cache);
        assert!(
            !outcome.registered_in_config,
            "existing entry must not be overwritten"
        );

        let cfg_raw = std::fs::read_to_string(project.join(".memstead/config.json")).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&cfg_raw).unwrap();
        assert_eq!(
            cfg["readVaults"]["alpha"]["source"]["type"].as_str(),
            Some("url")
        );
    }

    /// Two byte-distinct archives that share an internal vault name both
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
        write_minimal_vault_config(&project, "specs");
        build_valid_archive(&src_a_dir, &src_a, "alpha");

        let _g = CacheGuard::install(&cache);
        let first = install_to_disk(&src_a, &project).unwrap();
        assert!(first.copied_to_cache);
        let key_a = std::fs::read_to_string(project.join(".memstead/config.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|c| c["readVaults"]["alpha"]["cacheKey"].as_str().map(String::from))
            .expect("first install records a cacheKey");

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
        crate::ops::export::export_vault(&src_b_dir.join("alpha"), &cfg_b, &src_b, None, None)
            .unwrap();
        assert_ne!(
            std::fs::read(&src_a).unwrap(),
            std::fs::read(&src_b).unwrap(),
            "fixture must produce two distinct archives sharing the name `alpha`"
        );

        // Second install (different bytes, same name): SUCCEEDS — no
        // collision, no dead end. A second project registers it.
        let project_b = tmp.path().join("project-b");
        std::fs::create_dir_all(&project_b).unwrap();
        write_minimal_vault_config(&project_b, "specs");
        let second = install_read_vault(
            &src_b,
            TargetVault::Disk(&project_b),
            &CommitContext::internal(),
            "memstead: install (test)",
            &[],
        )
        .unwrap();
        assert!(second.copied_to_cache, "distinct bytes must install, not collide");
        let key_b = std::fs::read_to_string(project_b.join(".memstead/config.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|c| c["readVaults"]["alpha"]["cacheKey"].as_str().map(String::from))
            .expect("second install records a cacheKey");

        // Distinct content ⇒ distinct keys ⇒ both cache files coexist.
        assert_ne!(key_a, key_b, "distinct archives must get distinct content keys");
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
        write_minimal_vault_config(&project, "specs");
        build_valid_archive(&src_dir, &src, "alpha");

        let _g = CacheGuard::install(&cache);
        let first = install_to_disk(&src, &project).unwrap();
        assert!(first.copied_to_cache);

        // Re-install with the SAME archive bytes — canonical(input)
        // matches the cache file → idempotent success.
        let second = install_to_disk(&src, &project).unwrap();
        assert!(
            !second.copied_to_cache,
            "idempotent re-install must report copied_to_cache: false"
        );
        assert!(
            !second.registered_in_config,
            "idempotent re-install must not re-register"
        );
    }

    /// Rewrite a current-layout archive into the pre-rename legacy
    /// layout: every `.memstead/` member moves to `.mdgv/`. Test-only —
    /// production writers never emit the legacy spelling.
    fn repack_as_legacy(src: &Path, dest: &Path) {
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
                Some(rest) => format!(".mdgv/{rest}"),
                None => name,
            };
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            writer.start_file(name, opts).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    /// The legacy `.mdgv` extension + `.mdgv/` in-zip layout is no
    /// longer tolerated: installing a genuine pre-rename archive now
    /// fails at validation — its `.mdgv/` members fall outside the
    /// `.memstead/` whitelist. (Closes the archive half of the `.mdgv`
    /// migration window.)
    #[test]
    fn install_legacy_mdgv_archive_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        let src_dir = tmp.path().join("src");
        let modern = tmp.path().join("modern.mem");
        std::fs::create_dir_all(&project).unwrap();
        write_minimal_vault_config(&project, "specs");
        build_valid_archive(&src_dir, &modern, "legacy-vault");

        let legacy = tmp.path().join("legacy-vault.mdgv");
        repack_as_legacy(&modern, &legacy);

        let _g = CacheGuard::install(&cache);
        let err = install_to_disk(&legacy, &project)
            .expect_err("a `.mdgv/`-layout archive must no longer install");
        assert!(matches!(err, InstallError::Validation(_)), "got {err:?}");
    }

    /// The prior-canonical `.mstd` extension is still read-tolerated: a
    /// `.mstd` archive (current `.memstead/` members) installs and
    /// carries the `LEGACY_ARCHIVE_FORMAT` warning whose remedy names
    /// re-export to `.mem`. This is the AC2 surface for the `.mstd → .mem`
    /// rename and the sole surviving tolerated legacy after the `.mdgv`
    /// window closed.
    #[test]
    fn install_mstd_extension_warns_and_names_mem_reexport() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        let src_dir = tmp.path().join("src");
        let src = tmp.path().join("legacy-mstd.mstd");
        std::fs::create_dir_all(&project).unwrap();
        write_minimal_vault_config(&project, "specs");
        build_valid_archive(&src_dir, &src, "legacy-mstd");

        let _g = CacheGuard::install(&cache);
        let outcome = install_to_disk(&src, &project).unwrap();
        match outcome.warnings.as_slice() {
            [w @ WarningHint::LegacyArchiveFormat { .. }] => {
                let msg = format!("{w}");
                assert!(
                    msg.contains(".mstd"),
                    "warning should name the `.mstd` extension: {msg}"
                );
                assert!(
                    msg.contains("`.mem` archive"),
                    "remedy must name re-export to `.mem`: {msg}"
                );
            }
            other => panic!("expected exactly one LegacyArchiveFormat warning, got {other:?}"),
        }
    }

    /// Refusal complement: a canonical `.mem` archive (current extension,
    /// current layout) installs with NO `LEGACY_ARCHIVE_FORMAT` warning —
    /// the deprecation signal fires only for the tolerated-prior spellings.
    #[test]
    fn install_canonical_mem_extension_has_no_legacy_warning() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        let src_dir = tmp.path().join("src");
        let src = tmp.path().join("canonical.mem");
        std::fs::create_dir_all(&project).unwrap();
        write_minimal_vault_config(&project, "specs");
        build_valid_archive(&src_dir, &src, "canonical");

        let _g = CacheGuard::install(&cache);
        let outcome = install_to_disk(&src, &project).unwrap();
        assert!(
            !outcome
                .warnings
                .iter()
                .any(|w| w.code() == "LEGACY_ARCHIVE_FORMAT"),
            "a `.mem` archive must not carry the legacy warning: {:?}",
            outcome.warnings
        );
    }

    #[test]
    fn install_rejects_non_archive_bytes() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        write_minimal_vault_config(&project, "specs");
        let src = tmp.path().join("bad.mem");
        std::fs::write(&src, b"not a zip").unwrap();

        let _g = CacheGuard::install(&cache);
        let err = install_to_disk(&src, &project).unwrap_err();
        assert!(matches!(err, InstallError::Validation(_)));
        // Validation failed up front → neither cache file nor temp
        // sibling was written.
        assert!(!cache.join("bad.mem").exists());
        assert!(!cache.join("bad.mem.tmp").exists());
    }
}
