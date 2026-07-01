//! Strict config parsing for the archive's `.memstead/config.json`.
//!
//! Whitelist projection: `deny_unknown_fields`, required slug-shaped
//! `name`, required semver `version`, required `schema: "<name>@<ver>"`.
//! Schema resolution against the engine's registry happens at load, not
//! here — this module only enforces shape and format integer.

use memstead_schema::{PUBLISHED_VAULT_FORMAT, PublishedVaultConfig};
use regex::Regex;
use std::sync::OnceLock;

use super::ValidationError;

/// The only shape the archive's `.memstead/config.json` is allowed to
/// take. Author-only fields (writeGuidance, mediums, projections,
/// rules, publish, readVaults, vcs, language, community,
/// defaultSchema) are stripped at export. Their presence here means
/// the archive was built before the whitelist export landed, or by
/// hand — surface that specifically so the user knows to re-export
/// rather than debug a raw serde error.
const LEGACY_AUTHOR_FIELDS: &[&str] = &[
    "writeGuidance",
    "mediums",
    "projections",
    "rules",
    "publish",
    "readVaults",
    "vcs",
    "language",
    "community",
    "defaultSchema",
];

/// Parse strict config bytes into a `PublishedVaultConfig`. Runs every
/// check in this module (shape, legacy detection, format, name,
/// version, schema resolution).
pub fn parse_config_bytes(bytes: &[u8]) -> Result<PublishedVaultConfig, ValidationError> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| {
        ValidationError::InvalidConfig {
            reason: format!("malformed JSON: {e}"),
        }
    })?;

    if let Some(obj) = value.as_object() {
        for &field in LEGACY_AUTHOR_FIELDS {
            if obj.contains_key(field) {
                return Err(ValidationError::InvalidConfig {
                    reason: "legacy vault format — re-export via `memstead export`".to_string(),
                });
            }
        }
    } else {
        return Err(ValidationError::InvalidConfig {
            reason: "expected a JSON object".to_string(),
        });
    }

    let config: PublishedVaultConfig =
        serde_json::from_value(value).map_err(|e| ValidationError::InvalidConfig {
            reason: e.to_string(),
        })?;

    check_format(&config)?;
    check_name(&config.name)?;
    check_version(&config.version)?;

    Ok(config)
}

fn check_format(config: &PublishedVaultConfig) -> Result<(), ValidationError> {
    if config.format == PUBLISHED_VAULT_FORMAT {
        return Ok(());
    }
    // `format: 2` archives (top-level `schema/` tree, pre-relocation)
    // are rejected with an actionable re-export hint — mirrors the V1
    // `mdgv.json` branch in `archive::extract_entries`. Any other
    // mismatch falls through to the generic `UnsupportedFormat`.
    if config.format == 2 {
        return Err(ValidationError::InvalidConfig {
            reason: "legacy vault format (format: 2) — re-export via `memstead export`"
                .to_string(),
        });
    }
    Err(ValidationError::UnsupportedFormat {
        got: config.format,
        expected: PUBLISHED_VAULT_FORMAT,
    })
}

fn name_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$").unwrap())
}

fn check_name(name: &str) -> Result<(), ValidationError> {
    if !name_regex().is_match(name) {
        return Err(ValidationError::InvalidName {
            reason: format!(
                "name must match ^[a-z0-9][a-z0-9-]{{0,62}}[a-z0-9]$, got {name:?}"
            ),
        });
    }
    Ok(())
}

/// Guard against a pathological pre-release or build-metadata string
/// that is otherwise free to be arbitrarily long per the semver spec.
const MAX_VERSION_PRE_BUILD: usize = 128;

fn check_version(version: &semver::Version) -> Result<(), ValidationError> {
    let pre_len = version.pre.as_str().len();
    let build_len = version.build.as_str().len();
    if pre_len + build_len > MAX_VERSION_PRE_BUILD {
        return Err(ValidationError::InvalidVersion {
            reason: format!(
                "pre-release + build metadata length {} exceeds cap {}",
                pre_len + build_len,
                MAX_VERSION_PRE_BUILD
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_config() -> serde_json::Value {
        serde_json::json!({
            "format": PUBLISHED_VAULT_FORMAT,
            "name": "example-vault",
            "version": "0.1.0",
            "schema": "default@1.0.0",
        })
    }

    fn parse(value: serde_json::Value) -> Result<PublishedVaultConfig, ValidationError> {
        parse_config_bytes(value.to_string().as_bytes())
    }

    #[test]
    fn accepts_minimal_valid_config() {
        let config = parse(ok_config()).unwrap();
        assert_eq!(config.format, PUBLISHED_VAULT_FORMAT);
        assert_eq!(config.name, "example-vault");
        assert_eq!(config.version.to_string(), "0.1.0");
    }

    #[test]
    fn rejects_malformed_json() {
        let err = parse_config_bytes(b"not json").unwrap_err();
        assert!(matches!(err, ValidationError::InvalidConfig { .. }));
    }

    #[test]
    fn rejects_non_object_root() {
        let err = parse(serde_json::json!([])).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidConfig { reason } if reason.contains("object")));
    }

    #[test]
    fn rejects_legacy_author_fields_with_actionable_message() {
        let mut v = ok_config();
        v["writeGuidance"] = serde_json::json!({});
        let err = parse(v).unwrap_err();
        match err {
            ValidationError::InvalidConfig { reason } => {
                assert!(reason.contains("legacy vault format"), "reason={reason}");
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_top_level_key() {
        let mut v = ok_config();
        v["unexpected"] = serde_json::json!(42);
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidConfig { .. }));
    }

    #[test]
    fn rejects_missing_format() {
        let mut v = ok_config();
        v.as_object_mut().unwrap().remove("format");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidConfig { .. }));
    }

    /// Forge refusal: a publisher cannot smuggle a first-party trust claim
    /// into a vault archive. Trust origin is decided by the consuming
    /// engine (built-in catalogue + writable-mount adoption), never read
    /// from publisher-supplied config, so an archive config that carries an
    /// `origin` (or any first-party-claim) key is an unknown field and the
    /// `deny_unknown_fields` whitelist refuses it with a typed
    /// `InvalidConfig` — no path admits the claim.
    #[test]
    fn rejects_forged_first_party_origin_claim() {
        let mut v = ok_config();
        v["origin"] = serde_json::json!("first-party");
        let err = parse(v).unwrap_err();
        assert!(
            matches!(err, ValidationError::InvalidConfig { .. }),
            "a forged first-party origin claim must be refused with a typed InvalidConfig, got {err:?}"
        );
    }

    #[test]
    fn rejects_wrong_format_version() {
        let mut v = ok_config();
        v["format"] = serde_json::json!(1);
        let err = parse(v).unwrap_err();
        assert!(
            matches!(err, ValidationError::UnsupportedFormat { got: 1, expected: _ }),
            "unexpected err: {err:?}"
        );
    }

    #[test]
    fn rejects_format_2_with_actionable_reexport_hint() {
        let mut v = ok_config();
        v["format"] = serde_json::json!(2);
        let err = parse(v).unwrap_err();
        match err {
            ValidationError::InvalidConfig { reason } => {
                assert!(
                    reason.contains("format: 2") && reason.contains("memstead export"),
                    "reason={reason}"
                );
            }
            other => panic!("expected InvalidConfig with re-export hint, got {other:?}"),
        }
    }

    #[test]
    fn rejects_name_uppercase() {
        let mut v = ok_config();
        v["name"] = serde_json::json!("Upper-Case");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidName { .. }));
    }

    #[test]
    fn rejects_name_with_space() {
        let mut v = ok_config();
        v["name"] = serde_json::json!("has space");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidName { .. }));
    }

    #[test]
    fn rejects_name_leading_hyphen() {
        let mut v = ok_config();
        v["name"] = serde_json::json!("-leading");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidName { .. }));
    }

    #[test]
    fn rejects_name_trailing_hyphen() {
        let mut v = ok_config();
        v["name"] = serde_json::json!("trailing-");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidName { .. }));
    }

    #[test]
    fn rejects_name_too_long() {
        let mut v = ok_config();
        v["name"] = serde_json::json!("a".repeat(65));
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidName { .. }));
    }

    #[test]
    fn rejects_name_with_path_separator() {
        let mut v = ok_config();
        v["name"] = serde_json::json!("scope/name");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidName { .. }));
    }

    #[test]
    fn rejects_name_with_at_prefix() {
        let mut v = ok_config();
        v["name"] = serde_json::json!("@scope");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidName { .. }));
    }

    #[test]
    fn rejects_missing_schema() {
        let mut v = ok_config();
        v.as_object_mut().unwrap().remove("schema");
        let err = parse(v).unwrap_err();
        // Missing `schema` is a serde error (field required) — surfaces as
        // InvalidConfig before anything else runs.
        assert!(matches!(err, ValidationError::InvalidConfig { .. }));
    }

    #[test]
    fn rejects_schema_range_syntax() {
        let mut v = ok_config();
        v["schema"] = serde_json::json!("default@^1.0.0");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidConfig { .. }));
    }

    #[test]
    fn rejects_schema_without_version() {
        let mut v = ok_config();
        v["schema"] = serde_json::json!("default");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidConfig { .. }));
    }

    #[test]
    fn rejects_legacy_types_field() {
        // Legacy `types` is an unknown field under `deny_unknown_fields`.
        let mut v = ok_config();
        v["types"] = serde_json::json!(["spec"]);
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidConfig { .. }));
    }

    #[test]
    fn rejects_invalid_semver() {
        let mut v = ok_config();
        v["version"] = serde_json::json!("not-a-version");
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidConfig { .. }));
    }

    #[test]
    fn rejects_pre_release_plus_build_too_long() {
        let mut v = ok_config();
        let long = "a".repeat(130);
        v["version"] = serde_json::json!(format!("0.1.0-{long}"));
        let err = parse(v).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidVersion { .. }));
    }
}
