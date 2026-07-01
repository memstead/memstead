//! Domain-authority publishing — the pure wire protocol shared by the signer
//! (CLI) and the verifier (registry).
//!
//! The signed-payload format and the header/algorithm names are load-bearing:
//! the CLI signs over exactly these bytes and the registry verifies over them.
//! They live here, in the foundational crate both sides already depend on, so a
//! change cannot drift one side out of agreement with the other. This module is
//! pure — no crypto, no I/O — so it carries no extra dependency weight.

/// The fixed path the proof manifest is served at on the publisher's own
/// domain. Distinct from `memstead-serve`'s discovery manifest
/// (`memstead-authority.json`): the two make different claims.
pub const MANIFEST_PATH: &str = "/.well-known/memstead-publishing.json";

/// The only signature algorithm supported at launch. The prefix is part of
/// every key/signature string (`ed25519:<base64>`).
pub const ALG: &str = "ed25519";

/// Domain-separation prefix on the signed payload. Binds a signature to *this*
/// protocol and version so a signature made for some other purpose with the
/// same key can never be replayed as a Memstead publish authorisation. Bump the
/// version suffix if the payload shape ever changes.
pub const SIGNING_DOMAIN: &str = "memstead-domain-publish-v1";

/// Request headers carrying the per-publish signature. The publish handler
/// reads these for a `<domain>:<handle>` scope; the CLI sets them.
pub const HEADER_KEY: &str = "x-memstead-domain-key";
pub const HEADER_SIGNATURE: &str = "x-memstead-domain-signature";
pub const HEADER_TIMESTAMP: &str = "x-memstead-domain-timestamp";

/// The exact bytes a publish signature covers. Binds the signature to *this*
/// upload and target so a captured signature cannot be replayed onto a
/// different archive, scope, name, version, or far-off time.
///
/// The serialisation is a newline-joined, domain-separated concatenation:
///
/// ```text
/// memstead-domain-publish-v1\n
/// <content_sha256>\n      (lowercase hex of the canonical archive bytes)
/// <scope>\n               (canonical, e.g. acme.com:payments)
/// <name>\n
/// <version>\n
/// <timestamp>             (unix seconds, decimal)
/// ```
///
/// Every field is mandatory and both the signer and the verifier MUST produce
/// these bytes identically. None of the fields can contain a newline (hex,
/// scope, name, version, and a decimal integer are all newline-free), so the
/// join is unambiguous.
pub fn signing_payload(
    content_sha256: &str,
    scope: &str,
    name: &str,
    version: &str,
    timestamp: i64,
) -> Vec<u8> {
    format!("{SIGNING_DOMAIN}\n{content_sha256}\n{scope}\n{name}\n{version}\n{timestamp}")
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_is_deterministic_and_binds_every_field() {
        let base = signing_payload("deadbeef", "acme.com:pay", "mem", "1.0.0", 1000);
        assert_eq!(base, signing_payload("deadbeef", "acme.com:pay", "mem", "1.0.0", 1000));
        assert_ne!(base, signing_payload("feedface", "acme.com:pay", "mem", "1.0.0", 1000));
        assert_ne!(base, signing_payload("deadbeef", "other.com:pay", "mem", "1.0.0", 1000));
        assert_ne!(base, signing_payload("deadbeef", "acme.com:pay", "other", "1.0.0", 1000));
        assert_ne!(base, signing_payload("deadbeef", "acme.com:pay", "mem", "2.0.0", 1000));
        assert_ne!(base, signing_payload("deadbeef", "acme.com:pay", "mem", "1.0.0", 1001));
    }

    /// Pin the exact wire bytes. If this vector ever changes, every already-
    /// hosted manifest and both code paths must change together — this test is
    /// the tripwire.
    #[test]
    fn payload_exact_bytes_are_pinned() {
        let p = signing_payload("abc123", "acme.com:pay", "demo", "1.2.3", 1700000000);
        assert_eq!(
            p,
            b"memstead-domain-publish-v1\nabc123\nacme.com:pay\ndemo\n1.2.3\n1700000000".to_vec()
        );
    }
}
