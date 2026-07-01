//! `memstead domain …` — manage the keys that authorise publishing under a
//! `<domain>:<handle>` scope.
//!
//! Domain publishing needs no Memstead account: a publisher proves control of a
//! domain by hosting a small signed-key manifest at
//! `https://<domain>/.well-known/memstead-publishing.json` and signing each
//! publish with the matching private key. This command produces both halves:
//!
//! - **`keygen`** generates a signing keypair, stores the private key locally,
//!   and prints the manifest JSON to host.
//! - **`manifest`** re-prints the manifest for an existing key (e.g. to change
//!   the abuse contacts) without rotating the key.
//!
//! `memstead publish --scope <domain>:<handle>` then signs transparently using
//! the stored key.

use clap::{Parser, Subcommand};
use serde_json::json;

use crate::auth::domain_key;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;
use crate::CliError;

#[derive(Subcommand, Debug)]
pub enum DomainAction {
    /// Generate a signing keypair for a domain and print the manifest to host.
    Keygen(KeygenArgs),
    /// Re-print the `.well-known` manifest for a domain's existing key.
    Manifest(ManifestArgs),
}

#[derive(Parser, Debug)]
pub struct KeygenArgs {
    /// The domain you control, e.g. `acme.com`. Vaults publish under
    /// `<domain>:<handle>`.
    #[arg(long, value_name = "DOMAIN")]
    pub domain: String,

    /// Abuse / ownership contact (email or URI). Repeatable; at least one is
    /// required — a takedown notice must be able to reach you.
    #[arg(long = "contact", value_name = "EMAIL_OR_URI", required = true)]
    pub contacts: Vec<String>,

    /// Replace an existing key for this domain (rotation). The hosted manifest
    /// must then be updated to the new public key.
    #[arg(long)]
    pub force: bool,
}

#[derive(Parser, Debug)]
pub struct ManifestArgs {
    /// The domain whose stored key to render a manifest for.
    #[arg(long, value_name = "DOMAIN")]
    pub domain: String,

    /// Abuse / ownership contact (email or URI). Repeatable; at least one is
    /// required.
    #[arg(long = "contact", value_name = "EMAIL_OR_URI", required = true)]
    pub contacts: Vec<String>,
}

pub fn run(ctx: &CliContext, action: DomainAction) -> anyhow::Result<()> {
    match action {
        DomainAction::Keygen(args) => keygen(ctx, args),
        DomainAction::Manifest(args) => manifest(ctx, args),
    }
}

fn keygen(ctx: &CliContext, args: KeygenArgs) -> anyhow::Result<()> {
    let domain = normalize_domain(&args.domain)?;
    let public_key = domain_key::generate(&domain, args.force).map_err(|e| {
        CliError::new(ExitKind::Generic, "DOMAIN_KEYGEN_FAILED", e.to_string())
    })?;
    emit_manifest(ctx, &domain, &public_key, &args.contacts, true)
}

fn manifest(ctx: &CliContext, args: ManifestArgs) -> anyhow::Result<()> {
    let domain = normalize_domain(&args.domain)?;
    let signing = domain_key::load(&domain)
        .map_err(|e| CliError::new(ExitKind::NotFound, "DOMAIN_KEY_NOT_FOUND", e.to_string()))?;
    let public_key = domain_key::public_key_string(&signing);
    emit_manifest(ctx, &domain, &public_key, &args.contacts, false)
}

/// Print the manifest to host plus where to host it. `generated` toggles the
/// "new key" framing vs the re-print framing.
fn emit_manifest(
    ctx: &CliContext,
    domain: &str,
    public_key: &str,
    contacts: &[String],
    generated: bool,
) -> anyhow::Result<()> {
    let manifest = domain_key::manifest_json(
        std::slice::from_ref(&public_key.to_string()),
        contacts,
    );
    let url = format!("https://{domain}/.well-known/memstead-publishing.json");
    if ctx.json {
        print_json(&json!({
            "domain": domain,
            "public_key": public_key,
            "manifest_url": url,
            "manifest": manifest,
            "generated": generated,
        }))?;
    } else {
        let pretty = serde_json::to_string_pretty(&manifest).unwrap_or_default();
        let lead = if generated {
            format!("# Domain signing key for `{domain}`\n\nA new keypair was generated and the private key stored locally.")
        } else {
            format!("# Manifest for `{domain}`")
        };
        print_markdown(&format!(
            "{lead}\n\n\
             Host this exact file at:\n\n    {url}\n\n\
             ```json\n{pretty}\n```\n\n\
             Then publish with `memstead publish --scope {domain}:<handle>` — \
             the CLI signs each publish with the stored key. Remove the key from \
             the manifest (or take the manifest down) to revoke."
        ));
    }
    Ok(())
}

/// Lowercase + validate the domain as a publishable scope domain (dot-separated
/// labels, no scheme, no path, no `:`). Mirrors the registry's domain grammar
/// closely enough to fail fast on obvious mistakes.
fn normalize_domain(raw: &str) -> anyhow::Result<String> {
    let d = raw.trim().to_ascii_lowercase();
    let looks_like_domain = !d.is_empty()
        && d.len() <= 253
        && d.contains('.')
        && !d.contains('/')
        && !d.contains(':')
        && d.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    if !looks_like_domain {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_DOMAIN",
            format!("{raw:?} is not a valid domain (expected e.g. `acme.com`, no scheme or path)"),
        )
        .into());
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_validates_domains() {
        assert_eq!(normalize_domain("Acme.COM").unwrap(), "acme.com");
        assert_eq!(normalize_domain(" sub.acme.co.uk ").unwrap(), "sub.acme.co.uk");
        for bad in ["nodot", "has space.com", "https://acme.com", "acme.com:demo", "acme.com/x"] {
            assert!(normalize_domain(bad).is_err(), "{bad} should be rejected");
        }
    }
}
