//! Persistent credential store for the `memstead` CLI.
//!
//! Layout: `<config_dir>/memstead/credentials` — TOML, keyed on registry
//! host so the same CLI can talk to staging + production without
//! juggling files. Hostnames are lowercased so `Memstead.io` and
//! `memstead.io` resolve to the same entry.
//!
//! Example on disk:
//! ```toml
//! [registries."memstead.io"]
//! token = "gho_..."
//! user_login = "you"
//! scopes = ["read:user"]
//! obtained_at = "2026-04-16T08:12:34Z"
//! ```
//!
//! File mode is locked to `0600` on Unix. Windows inherits default
//! user ACL — tightening that is out of scope for Session E.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// One credentials block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub token: String,
    pub user_login: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// RFC 3339 UTC timestamp the token was obtained.
    pub obtained_at: String,
}

impl Entry {
    pub fn new(token: String, user_login: String, scopes: Vec<String>) -> Self {
        let obtained_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
        Self {
            token,
            user_login,
            scopes,
            obtained_at,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    registries: BTreeMap<String, Entry>,
}

/// Absolute path to the credentials file. Caller is free to dereference
/// this even when the file itself is missing; the read functions
/// handle ENOENT.
pub fn credentials_path() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .context("no config directory resolvable on this platform (set $XDG_CONFIG_HOME)")?;
    Ok(base.join("memstead").join("credentials"))
}

/// Test hook: point the credentials file at a caller-specified path.
/// Honoured via the `MEMSTEAD_CREDENTIALS_FILE` env var — chosen over a
/// separate function parameter so every subcommand that calls into
/// auth picks it up uniformly.
fn resolved_path() -> Result<PathBuf> {
    if let Ok(override_path) = std::env::var("MEMSTEAD_CREDENTIALS_FILE")
        && !override_path.is_empty()
    {
        return Ok(PathBuf::from(override_path));
    }
    credentials_path()
}

fn load_store() -> Result<Store> {
    let path = resolved_path()?;
    match std::fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s)
            .with_context(|| format!("parsing credentials file at {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => {
            Err(e).with_context(|| format!("reading credentials file at {}", path.display()))
        }
    }
}

fn save_store(store: &Store) -> Result<()> {
    let path = resolved_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating credentials dir at {}", parent.display()))?;
    }
    let body = toml::to_string(store)
        .context("serializing credentials TOML")?;
    std::fs::write(&path, body)
        .with_context(|| format!("writing credentials file at {}", path.display()))?;
    tighten_permissions(&path)?;
    Ok(())
}

#[cfg(unix)]
fn tighten_permissions(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("setting mode 0600 on {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn tighten_permissions(_: &std::path::Path) -> Result<()> {
    Ok(())
}

/// Retrieve the credentials entry for a registry host, or `None` if
/// nothing is stored. Missing file is treated as "no credentials".
pub fn load_for(host: &str) -> Result<Option<Entry>> {
    let store = load_store()?;
    Ok(store.registries.get(&host.to_ascii_lowercase()).cloned())
}

/// Persist a credentials entry for a registry host. Overwrites any
/// existing entry for that host.
pub fn save_for(host: &str, entry: Entry) -> Result<()> {
    let mut store = load_store()?;
    store
        .registries
        .insert(host.to_ascii_lowercase(), entry);
    save_store(&store)
}

/// Remove a credentials entry. Returns true if something was removed.
/// Non-existent host is a silent no-op (returns false).
pub fn remove_for(host: &str) -> Result<bool> {
    let mut store = load_store()?;
    let removed = store.registries.remove(&host.to_ascii_lowercase()).is_some();
    if removed {
        save_store(&store)?;
    }
    Ok(removed)
}
