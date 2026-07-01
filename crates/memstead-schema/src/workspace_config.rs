//! Cross-vault-link policy value parsing for `.memstead/workspace.toml`.
//!
//! [`CrossLinkValue`] is the shared shape behind `[cross_vault_links]`
//! and `[[vault_management.create]].default_cross_links` — an operator
//! writes either `"*"` (wildcard) or a list of vault names. Each engine
//! crate that loads workspace policy (`memstead-base`,
//! `memstead-engine`, `memstead-mcp`, `memstead-cli`) calls
//! [`CrossLinkValue::parse_toml`] when lifting those tables; the value
//! parser lives here so every crate validates the shape identically.

use crate::config::ConfigError;

/// One entry in `[cross_vault_links]` (and the matching shape that
/// `[[vault_management.create]].default_cross_links` uses). The operator
/// writes either a list of vault names or the literal string `"*"`; mixed
/// lists containing `"*"` are rejected at parse with `CONFIG_ERROR`.
///
/// Empty lists (`[]`) are valid and behave identically to omission of
/// the key — kept as an explicit shape so an operator can encode "this
/// vault is intentionally locked down" without relying on absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossLinkValue {
    /// Wildcard — any current writable target is admitted.
    Wildcard,
    /// Explicit allowlist of vault names. May be empty (default-deny
    /// for that vault).
    List(Vec<String>),
}

impl CrossLinkValue {
    /// Parse a TOML value into a `CrossLinkValue`, rejecting mixed lists
    /// containing `"*"` and any non-string-list shape. Used by both
    /// `[cross_vault_links]` and `[[vault_management.create]].default_cross_links`.
    pub fn parse_toml(
        location: &str,
        value: &toml::Value,
    ) -> Result<Self, ConfigError> {
        match value {
            toml::Value::String(s) if s == "*" => Ok(Self::Wildcard),
            toml::Value::String(other) => Err(ConfigError::Other(format!(
                "{location}: expected `\"*\"` or a list of vault names, got string {other:?}"
            ))),
            toml::Value::Array(items) => {
                let mut names: Vec<String> = Vec::with_capacity(items.len());
                let mut has_wildcard = false;
                for (idx, item) in items.iter().enumerate() {
                    match item {
                        toml::Value::String(s) if s == "*" => {
                            has_wildcard = true;
                        }
                        toml::Value::String(s) if s.is_empty() => {
                            return Err(ConfigError::Other(format!(
                                "{location}[{idx}]: vault name must not be empty"
                            )));
                        }
                        toml::Value::String(s) => names.push(s.clone()),
                        _ => {
                            return Err(ConfigError::Other(format!(
                                "{location}[{idx}]: expected string vault name, got {item}"
                            )));
                        }
                    }
                }
                if has_wildcard && !names.is_empty() {
                    return Err(ConfigError::Other(format!(
                        "{location}: `\"*\"` wildcard must be the sole entry — \
                         remove the named entries or drop the wildcard"
                    )));
                }
                if has_wildcard {
                    Ok(Self::Wildcard)
                } else {
                    Ok(Self::List(names))
                }
            }
            other => Err(ConfigError::Other(format!(
                "{location}: expected `\"*\"` or a list of vault names, got {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(s: &str) -> toml::Value {
        toml::from_str::<toml::Value>(&format!("x = {s}"))
            .unwrap()
            .get("x")
            .unwrap()
            .clone()
    }

    #[test]
    fn wildcard_string_parses() {
        assert_eq!(
            CrossLinkValue::parse_toml("[loc]", &val("\"*\"")).unwrap(),
            CrossLinkValue::Wildcard
        );
    }

    #[test]
    fn name_list_parses() {
        assert_eq!(
            CrossLinkValue::parse_toml("[loc]", &val("[\"a\", \"b\"]")).unwrap(),
            CrossLinkValue::List(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn empty_list_is_default_deny() {
        assert_eq!(
            CrossLinkValue::parse_toml("[loc]", &val("[]")).unwrap(),
            CrossLinkValue::List(vec![])
        );
    }

    #[test]
    fn wildcard_in_a_list_is_a_lone_wildcard() {
        assert_eq!(
            CrossLinkValue::parse_toml("[loc]", &val("[\"*\"]")).unwrap(),
            CrossLinkValue::Wildcard
        );
    }

    #[test]
    fn mixed_wildcard_and_names_is_rejected() {
        let err = CrossLinkValue::parse_toml("[loc]", &val("[\"*\", \"a\"]")).unwrap_err();
        assert!(format!("{err}").contains("sole entry"));
    }

    #[test]
    fn empty_vault_name_is_rejected() {
        let err = CrossLinkValue::parse_toml("[loc]", &val("[\"\"]")).unwrap_err();
        assert!(format!("{err}").contains("must not be empty"));
    }

    #[test]
    fn non_string_list_entry_is_rejected() {
        let err = CrossLinkValue::parse_toml("[loc]", &val("[1]")).unwrap_err();
        assert!(format!("{err}").contains("expected string vault name"));
    }

    #[test]
    fn bare_integer_is_rejected() {
        let err = CrossLinkValue::parse_toml("[loc]", &val("42")).unwrap_err();
        assert!(format!("{err}").contains("expected"));
    }
}
