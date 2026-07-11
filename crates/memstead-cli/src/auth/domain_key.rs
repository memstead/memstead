//! Local store for the ed25519 keys that authorise domain-scoped publishing.
//!
//! A domain publisher proves control by hosting a `.well-known` manifest that
//! lists public keys, and signing each publish with the matching private key
//! (see `memstead_base::domain_authority_wire`). This module owns the **private**
//! half: one key per domain, stored under `~/.config/memstead/domain-keys/`, and
//! the helpers to generate, load, sign with, and render the manifest for it.
//!
//! The key file holds the base64 of the 32-byte ed25519 seed, mode 0600. The
//! directory can be overridden with `MEMSTEAD_DOMAIN_KEYS_DIR` (used by tests).

use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer, SigningKey};
use memstead_base::domain_authority_wire::ALG;
use serde_json::json;

/// Directory holding per-domain key files. Honours `MEMSTEAD_DOMAIN_KEYS_DIR`
/// (test hook), else `~/.config/memstead/domain-keys/`.
pub fn keys_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("MEMSTEAD_DOMAIN_KEYS_DIR")
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    let base = dirs::config_dir()
        .context("no config directory resolvable on this platform (set $XDG_CONFIG_HOME)")?;
    Ok(base.join("memstead").join("domain-keys"))
}

/// Path to the key file for `domain`. The domain is a validated scope label
/// (lowercase, dot-separated, no slashes), so it is a safe single path segment.
fn key_path(domain: &str) -> Result<PathBuf> {
    Ok(keys_dir()?.join(format!("{domain}.key")))
}

/// Is there already a key stored for `domain`?
pub fn exists(domain: &str) -> Result<bool> {
    Ok(key_path(domain)?.exists())
}

/// Generate a fresh keypair for `domain` and persist the private key. Refuses
/// to overwrite an existing key unless `force` (rotation is deliberate — a lost
/// old key cannot sign, so clobbering silently would strand published mems).
/// Returns the new key's `ed25519:<base64>` public-key string.
pub fn generate(domain: &str, force: bool) -> Result<String> {
    if exists(domain)? && !force {
        anyhow::bail!(
            "a signing key already exists for {domain}; pass --force to replace it \
             (this rotates the key — update the hosted manifest to the new public key)"
        );
    }
    use getrandom::{SysRng, rand_core::UnwrapErr};
    let mut rng = UnwrapErr(SysRng);
    let signing = SigningKey::generate(&mut rng);
    save(domain, &signing)?;
    Ok(public_key_string(&signing))
}

/// Persist `signing`'s seed (base64) to the key file, mode 0600.
fn save(domain: &str, signing: &SigningKey) -> Result<()> {
    let path = key_path(domain)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating domain-keys dir at {}", parent.display()))?;
    }
    let body = BASE64.encode(signing.to_bytes());
    std::fs::write(&path, body)
        .with_context(|| format!("writing domain key at {}", path.display()))?;
    tighten_permissions(&path)?;
    Ok(())
}

/// Load the signing key for `domain`. Errors actionably if none is stored.
pub fn load(domain: &str) -> Result<SigningKey> {
    let path = key_path(domain)?;
    let body = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "no signing key for {domain} — run `memstead domain keygen --domain {domain} \
                 --contact <email>` first, then host the printed manifest"
            );
        }
        Err(e) => {
            return Err(e).with_context(|| format!("reading domain key at {}", path.display()));
        }
    };
    let bytes = BASE64
        .decode(body.trim())
        .with_context(|| format!("decoding domain key at {}", path.display()))?;
    let seed: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("domain key at {} is not a 32-byte seed", path.display()))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// `ed25519:<base64>` public-key string for a signing key — the form listed in
/// the manifest and presented on a publish.
pub fn public_key_string(signing: &SigningKey) -> String {
    format!(
        "{ALG}:{}",
        BASE64.encode(signing.verifying_key().to_bytes())
    )
}

/// Sign `payload`, returning the `ed25519:<base64>` signature string.
pub fn sign(signing: &SigningKey, payload: &[u8]) -> String {
    format!("{ALG}:{}", BASE64.encode(signing.sign(payload).to_bytes()))
}

/// The proof manifest to host at `https://<domain>/.well-known/memstead-publishing.json`.
pub fn manifest_json(public_keys: &[String], contacts: &[String]) -> serde_json::Value {
    json!({
        "memstead_publishing": true,
        "publish_keys": public_keys,
        "contacts": contacts,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use memstead_base::domain_authority_wire::signing_payload;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // `MEMSTEAD_DOMAIN_KEYS_DIR` is process-global; serialize the env-dependent
    // tests so parallel runs don't clobber each other's override.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_keys_dir<T>(f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        // SAFETY: the lock serializes env access across this module's tests.
        unsafe { std::env::set_var("MEMSTEAD_DOMAIN_KEYS_DIR", tmp.path()) };
        let out = f();
        unsafe { std::env::remove_var("MEMSTEAD_DOMAIN_KEYS_DIR") };
        out
    }

    #[test]
    fn generate_then_load_roundtrips_and_signs_verifiably() {
        with_keys_dir(|| {
            let pk = generate("acme.com", false).unwrap();
            assert!(pk.starts_with("ed25519:"));
            // The stored key reproduces the same public key.
            let sk = load("acme.com").unwrap();
            assert_eq!(public_key_string(&sk), pk);
            // A signature it makes verifies under the published public key.
            let payload = signing_payload("hash", "acme.com:demo", "v", "1.0.0", 1000);
            let sig = sign(&sk, &payload);
            assert!(sig.starts_with("ed25519:"));
        });
    }

    #[test]
    fn generate_refuses_to_clobber_without_force() {
        with_keys_dir(|| {
            let pk1 = generate("acme.com", false).unwrap();
            assert!(
                generate("acme.com", false).is_err(),
                "must not clobber silently"
            );
            // Force rotates to a new key.
            let pk2 = generate("acme.com", true).unwrap();
            assert_ne!(pk1, pk2, "force must produce a new key");
        });
    }

    #[test]
    fn load_missing_key_is_actionable() {
        with_keys_dir(|| {
            let err = load("nope.com").unwrap_err().to_string();
            assert!(err.contains("keygen"), "error must point to keygen: {err}");
        });
    }

    #[test]
    fn manifest_has_marker_keys_and_contacts() {
        let m = manifest_json(
            &["ed25519:AAAA".to_string()],
            &["mailto:abuse@acme.com".to_string()],
        );
        assert_eq!(m["memstead_publishing"], true);
        assert_eq!(m["publish_keys"][0], "ed25519:AAAA");
        assert_eq!(m["contacts"][0], "mailto:abuse@acme.com");
    }
}
