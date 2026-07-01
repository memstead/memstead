//! Pro MCP-binary configuration helpers.
//!
//! Post-rebuild, the MCP server's operator-edited settings live in
//! `<workspace>/.memstead/workspace.toml` and are parsed once by
//! `memstead_base::FileWorkspaceStore`. The MCP binary reads them off
//! `Engine::settings()` after boot — there is no parallel TOML parser
//! in this crate.
//!
//! What lives here:
//!
//! - [`DEFAULT_TOKEN_BUDGET`] — fallback chunker size when the
//!   workspace's `[mcp].token_budget` is unset.
//! - [`validate_disabled_tools`] — partitions an operator's
//!   `[mcp].disabled_tools` list against the compile-time tool-name
//!   registry, dropping (and reporting) unknown entries so a misspelled
//!   name never blocks boot.
//! - [`MutationsSection`] re-export — so the existing `McpServer`
//!   constructor's wire shape stays stable while the underlying parse
//!   lives in `memstead_base`.

pub use memstead_base::workspace::MutationsSection;

/// Default token budget when the workspace's `[mcp].token_budget` is
/// unset. Mirrors the pre-rebuild constant that lived alongside the
/// `.mdgv.toml` parser.
pub const DEFAULT_TOKEN_BUDGET: usize = 10_000;

/// Partition `requested` into (effective disabled-set, unknown names)
/// by matching against `known` tool names. Exact string equality —
/// no glob, prefix, or regex semantics. Pure function: the caller
/// surfaces the unknown names (typically `tracing::warn!`) so the unit
/// test asserts on the returned shape rather than log capture.
pub fn validate_disabled_tools(
    requested: &[String],
    known: &[String],
) -> (std::collections::HashSet<String>, Vec<String>) {
    use std::collections::HashSet;
    let known_set: HashSet<&str> = known.iter().map(String::as_str).collect();
    let mut effective: HashSet<String> = HashSet::new();
    let mut unknown: Vec<String> = Vec::new();
    for name in requested {
        if known_set.contains(name.as_str()) {
            effective.insert(name.clone());
        } else {
            unknown.push(name.clone());
        }
    }
    (effective, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_disabled_tools_partitions_known_and_unknown() {
        let known = vec![
            "memstead_entity".to_string(),
            "memstead_search".to_string(),
            "memstead_vault_create".to_string(),
        ];
        let requested = vec![
            "memstead_entity".to_string(),
            "bogus".to_string(),
            "memstead_search".to_string(),
            "also_unknown".to_string(),
        ];
        let (effective, unknown) = validate_disabled_tools(&requested, &known);
        assert_eq!(effective.len(), 2);
        assert!(effective.contains("memstead_entity"));
        assert!(effective.contains("memstead_search"));
        assert_eq!(unknown, vec!["bogus".to_string(), "also_unknown".to_string()]);
    }

    #[test]
    fn validate_disabled_tools_empty_requested_is_empty_pair() {
        let known = vec!["memstead_entity".to_string()];
        let (effective, unknown) = validate_disabled_tools(&[], &known);
        assert!(effective.is_empty());
        assert!(unknown.is_empty());
    }
}
