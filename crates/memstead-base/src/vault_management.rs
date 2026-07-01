//! Compiled lifecycle-policy rule sets — first-match-wins glob lookup
//! over the `[[vault_management.create]]` /
//! `[[vault_management.delete]]` arrays surfaced in `workspace.toml`.
//!
//! The data carriers ([`crate::workspace::CreateRuleSetting`],
//! [`crate::workspace::DeleteRuleSetting`]) live with the workspace
//! types in [`crate::workspace`]; this module owns the *compiled*
//! matcher view that handlers call to decide whether a candidate
//! vault path matches an operator-allowed rule.
//!
//! Each rule's pattern is a gitignore-style glob: `*` does not cross
//! `/`; `**` matches zero-or-more path segments. Matching is
//! case-sensitive, no normalization. The matcher targets the composed
//! candidate `<path>/<name>` (the vault's full hierarchical branch
//! path on the vault-repo) so rules can scope to a directory under
//! the registry tree without separate path-glob plumbing. Flat-layout
//! vaults pass their leaf name as the candidate.
//!
//! **Boundary note.** The lifecycle orchestrators (`create_vault`,
//! `delete_vault`, their params/responses, the shared `NOTE_MAX_LEN`
//! cap, the `validate_vault_path` helper) live in
//! [`memstead_engine::vault_management`]. The matcher primitives stay
//! here because the basis engine's
//! [`crate::Engine::cross_vault_link_allowed`] synthesises a
//! [`CreateRuleSet`] on multi-folder workspaces — they are a basis
//! policy primitive shared by both flavors.

use std::path::Path;

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use thiserror::Error;

use crate::workspace::{CreateRuleSetting, DeleteRuleSetting};

/// Construction-time error for the matcher constructors. The
/// `ParseEntry` variant names the offending entry string so callers
/// can surface an actionable `INVALID_INPUT` envelope without
/// re-deriving which entry blew up.
#[derive(Debug, Error)]
pub enum MatcherSetError {
    #[error("invalid glob pattern {entry:?}: {source}")]
    ParseEntry {
        entry: String,
        #[source]
        source: globset::Error,
    },
    #[error("glob set build failed: {0}")]
    Build(#[source] globset::Error),
}

/// Compiled set of allowlist globs with gitignore semantics.
///
/// The `patterns` vector preserves the original strings (used by
/// `memstead_health` and error envelopes). The `set` is the compiled form
/// used for `matches`. Empty-input construction is valid and produces
/// a matcher that rejects every candidate — the natural default when
/// `[vault_management]` is absent.
#[derive(Debug, Clone)]
pub struct MatcherSet {
    patterns: Vec<String>,
    set: GlobSet,
}

impl MatcherSet {
    /// Compile a list of glob strings. Empty input is valid. A
    /// malformed entry returns `ParseEntry` naming the entry; `Build`
    /// covers residual post-assembly failures.
    pub fn new<I, S>(entries: I) -> Result<Self, MatcherSetError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut builder = GlobSetBuilder::new();
        let mut patterns: Vec<String> = Vec::new();
        for entry in entries {
            let entry_str = entry.as_ref();
            let glob: Glob = GlobBuilder::new(entry_str)
                .literal_separator(true)
                .build()
                .map_err(|source| MatcherSetError::ParseEntry {
                    entry: entry_str.to_string(),
                    source,
                })?;
            builder.add(glob);
            patterns.push(entry_str.to_string());
        }
        let set = builder.build().map_err(MatcherSetError::Build)?;
        Ok(Self { patterns, set })
    }

    /// Test whether `candidate` matches any compiled glob. The
    /// caller passes a path-like string; the matcher compares as-is —
    /// no normalization, no canonicalization, no cross-boundary
    /// widening.
    pub fn matches(&self, candidate: &Path) -> bool {
        self.set.is_match(candidate)
    }

    /// Original glob strings in input order. Consumed by health
    /// surfaces and `VAULT_PATH_NOT_ALLOWED` envelopes.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// `true` when no globs were compiled. Lifecycle handlers
    /// short-circuit with a `reason: "no_allowlist_configured"` detail
    /// instead of the generic "no match" message.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

impl Default for MatcherSet {
    fn default() -> Self {
        Self::new::<_, &str>(std::iter::empty::<&str>())
            .expect("empty MatcherSet is always valid")
    }
}

/// Compiled `[[vault_management.create]]` rule set. Each rule is a
/// pre-built `globset::Glob` plus the raw [`CreateRuleSetting`]. The
/// underlying `GlobSet` carries the same globs in declaration order
/// so [`Self::first_match`] resolves to the first-listed rule whose
/// glob matches.
#[derive(Debug, Clone)]
pub struct CreateRuleSet {
    rules: Vec<CreateRuleSetting>,
    set: GlobSet,
}

/// Compiled `[[vault_management.delete]]` rule set. Same first-match
/// semantics as [`CreateRuleSet`], minus the schema dimension.
#[derive(Debug, Clone)]
pub struct DeleteRuleSet {
    rules: Vec<DeleteRuleSetting>,
    set: GlobSet,
}

impl CreateRuleSet {
    /// Compile each rule's glob with `literal_separator(true)`. A
    /// malformed pattern surfaces as
    /// [`MatcherSetError::ParseEntry`] naming the offending entry so
    /// `Engine::init` can produce an `INVALID_INPUT` envelope without
    /// half-constructing.
    pub fn new(rules: Vec<CreateRuleSetting>) -> Result<Self, MatcherSetError> {
        let mut builder = GlobSetBuilder::new();
        for r in &rules {
            let glob = GlobBuilder::new(&r.pattern)
                .literal_separator(true)
                .build()
                .map_err(|source| MatcherSetError::ParseEntry {
                    entry: r.pattern.clone(),
                    source,
                })?;
            builder.add(glob);
        }
        let set = builder.build().map_err(MatcherSetError::Build)?;
        Ok(Self { rules, set })
    }

    /// First rule whose glob matches `candidate`, in declaration
    /// order. `None` when no rule matches; callers check
    /// [`Self::is_empty`] separately to distinguish "no rules
    /// configured" from "rules configured but none match".
    pub fn first_match(&self, candidate: &Path) -> Option<&CreateRuleSetting> {
        let matched = self.set.matches(candidate);
        matched.into_iter().next().map(|i| &self.rules[i])
    }

    /// Raw rule list in declaration order. Consumed by `memstead_overview`
    /// when surfacing the lifecycle-namespaces section.
    pub fn rules(&self) -> &[CreateRuleSetting] {
        &self.rules
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Convenience: pattern strings in declaration order. Useful for
    /// tests and diagnostic surfaces.
    pub fn patterns(&self) -> Vec<String> {
        self.rules.iter().map(|r| r.pattern.clone()).collect()
    }

    /// Convenience: any-rule match against `candidate`.
    pub fn matches(&self, candidate: &Path) -> bool {
        self.first_match(candidate).is_some()
    }
}

impl DeleteRuleSet {
    pub fn new(rules: Vec<DeleteRuleSetting>) -> Result<Self, MatcherSetError> {
        let mut builder = GlobSetBuilder::new();
        for r in &rules {
            let glob = GlobBuilder::new(&r.pattern)
                .literal_separator(true)
                .build()
                .map_err(|source| MatcherSetError::ParseEntry {
                    entry: r.pattern.clone(),
                    source,
                })?;
            builder.add(glob);
        }
        let set = builder.build().map_err(MatcherSetError::Build)?;
        Ok(Self { rules, set })
    }

    pub fn first_match(&self, candidate: &Path) -> Option<&DeleteRuleSetting> {
        let matched = self.set.matches(candidate);
        matched.into_iter().next().map(|i| &self.rules[i])
    }

    pub fn rules(&self) -> &[DeleteRuleSetting] {
        &self.rules
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn patterns(&self) -> Vec<String> {
        self.rules.iter().map(|r| r.pattern.clone()).collect()
    }

    pub fn matches(&self, candidate: &Path) -> bool {
        self.first_match(candidate).is_some()
    }
}

impl Default for CreateRuleSet {
    fn default() -> Self {
        Self::new(Vec::new()).expect("empty rule set always compiles")
    }
}

impl Default for DeleteRuleSet {
    fn default() -> Self {
        Self::new(Vec::new()).expect("empty rule set always compiles")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn cr(pattern: &str, schemas: &[&str]) -> CreateRuleSetting {
        CreateRuleSetting {
            pattern: pattern.to_string(),
            schemas: schemas.iter().map(|s| s.to_string()).collect(),
            default_cross_links: None,
        }
    }

    fn dr(pattern: &str) -> DeleteRuleSetting {
        DeleteRuleSetting {
            pattern: pattern.to_string(),
        }
    }

    // ---- MatcherSet primitives -------------------------------------

    #[test]
    fn empty_matcher_rejects_everything() {
        let m = MatcherSet::new::<_, &str>(std::iter::empty::<&str>()).unwrap();
        assert!(m.is_empty());
        assert_eq!(m.patterns().len(), 0);
        assert!(!m.matches(Path::new("anything")));
    }

    #[test]
    fn single_segment_star_does_not_cross_slash() {
        let m = MatcherSet::new(["memstead/*"]).unwrap();
        assert!(m.matches(Path::new("memstead/engine")));
        assert!(!m.matches(Path::new("memstead/engine/nested")));
        assert!(!m.matches(Path::new("other/engine")));
    }

    #[test]
    fn double_star_matches_any_segments() {
        let m = MatcherSet::new(["memstead/**"]).unwrap();
        assert!(m.matches(Path::new("memstead/engine")));
        assert!(m.matches(Path::new("memstead/engine/nested")));
        assert!(!m.matches(Path::new("other/engine")));
    }

    #[test]
    fn malformed_entry_returns_parse_entry_error() {
        let err = MatcherSet::new(["[unclosed"]).unwrap_err();
        match err {
            MatcherSetError::ParseEntry { entry, .. } => assert_eq!(entry, "[unclosed"),
            MatcherSetError::Build(_) => panic!("expected ParseEntry, got Build"),
        }
    }

    #[test]
    fn case_sensitive_by_default() {
        let m = MatcherSet::new(["MEMSTEAD/*"]).unwrap();
        assert!(m.matches(Path::new("MEMSTEAD/foo")));
        assert!(!m.matches(Path::new("memstead/foo")));
    }

    // ---- CreateRuleSet ---------------------------------------------

    #[test]
    fn empty_rule_set_matches_nothing() {
        let cs = CreateRuleSet::new(vec![]).unwrap();
        assert!(cs.is_empty());
        assert!(cs.first_match(Path::new("anything")).is_none());
    }

    #[test]
    fn first_match_resolves_in_declaration_order() {
        // `planning/plan-*` is more specific than `planning/**` and
        // appears first; both globs match `planning/plan-foo` but the
        // resolver returns the first-listed.
        let cs = CreateRuleSet::new(vec![
            cr("planning/plan-*", &["default@1.0.0"]),
            cr("planning/**", &["*"]),
        ])
        .unwrap();
        let m = cs.first_match(Path::new("planning/plan-foo")).unwrap();
        assert_eq!(m.pattern, "planning/plan-*");
    }

    #[test]
    fn second_rule_picked_when_first_does_not_match() {
        let cs = CreateRuleSet::new(vec![
            cr("planning/plan-*", &["default@1.0.0"]),
            cr("exec-*", &["default@1.0.0"]),
        ])
        .unwrap();
        let m = cs.first_match(Path::new("exec-foo")).unwrap();
        assert_eq!(m.pattern, "exec-*");
    }

    #[test]
    fn flat_candidate_matches_flat_pattern() {
        let cs = CreateRuleSet::new(vec![cr("exec-*", &["default@1.0.0"])]).unwrap();
        assert!(cs.first_match(Path::new("exec-foo")).is_some());
        // `exec-*` does not match `nested/exec-foo` (literal_separator).
        assert!(cs.first_match(Path::new("nested/exec-foo")).is_none());
    }

    #[test]
    fn hierarchical_candidate_requires_path_prefix() {
        let cs = CreateRuleSet::new(vec![cr(
            "planning/plan-*",
            &["default@1.0.0"],
        )])
        .unwrap();
        assert!(cs.first_match(Path::new("planning/plan-q4")).is_some());
        // Same leaf without the path prefix does not match.
        assert!(cs.first_match(Path::new("plan-q4")).is_none());
        // Same leaf under a different path does not match.
        assert!(cs.first_match(Path::new("other/plan-q4")).is_none());
    }

    #[test]
    fn create_rule_set_malformed_pattern_returns_parse_entry_error() {
        let err = CreateRuleSet::new(vec![cr("[unclosed", &["*"])]).unwrap_err();
        match err {
            MatcherSetError::ParseEntry { entry, .. } => {
                assert_eq!(entry, "[unclosed")
            }
            _ => panic!("expected ParseEntry, got {err:?}"),
        }
    }

    #[test]
    fn create_rule_set_carries_default_cross_links_in_matched_rule() {
        // Regression: lifting must keep default_cross_links accessible
        // through first_match.
        use memstead_schema::workspace_config::CrossLinkValue;
        let rule = CreateRuleSetting {
            pattern: "exec-*".to_string(),
            schemas: vec!["default".to_string()],
            default_cross_links: Some(CrossLinkValue::Wildcard),
        };
        let cs = CreateRuleSet::new(vec![rule]).unwrap();
        let m = cs.first_match(Path::new("exec-foo")).unwrap();
        assert_eq!(m.default_cross_links, Some(CrossLinkValue::Wildcard));
    }

    // ---- DeleteRuleSet ---------------------------------------------

    #[test]
    fn delete_rule_set_resolves_by_pattern_only() {
        let ds = DeleteRuleSet::new(vec![
            dr("planning/plan-*"),
            dr("exec-*"),
        ])
        .unwrap();
        assert!(ds.first_match(Path::new("planning/plan-foo")).is_some());
        assert!(ds.first_match(Path::new("exec-bar")).is_some());
        assert!(ds.first_match(Path::new("engine")).is_none());
    }

    #[test]
    fn delete_rule_set_default_is_empty() {
        let ds = DeleteRuleSet::default();
        assert!(ds.is_empty());
        assert!(!ds.matches(Path::new("anything")));
    }

    #[test]
    fn create_rule_set_default_is_empty() {
        let cs = CreateRuleSet::default();
        assert!(cs.is_empty());
        assert!(!cs.matches(Path::new("anything")));
    }
}
