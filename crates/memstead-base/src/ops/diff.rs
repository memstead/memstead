//! Two-ref structural diff types.
//!
//! `Diff` is the engine-level response shape for `Engine::diff(ref_a,
//! ref_b, config)`. Consumers (LLM replay skills, PR-review UIs,
//! pre-merge previews, snapshot comparisons, cross-mem reflection
//! tools) all consume this one struct rather than re-walking git trees
//! themselves.
//!
//! Wire format is deterministic and stable so external tooling
//! (memstead-mcp, memstead-cli, future Webhooks) can deserialise into the same
//! types it serialises.
//!
//! The rename-chain + ripple fields are motivated by the LLM-replay
//! flow that consumes these diffs.

use serde::{Deserialize, Serialize};

use crate::entity::EntityId;

/// Per-entity diff entry. Variants mirror the change kinds an
/// entity-level diff can produce; `InvalidEntity` is the soft-failure
/// path for entities that fail to parse on either side (consumers
/// decide how to handle each case — `memstead_diff` does not refuse the
/// whole call just because one entity is malformed).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EntityDiff {
    /// Entity exists on the `ref_b` side and not on `ref_a`. No
    /// rename was detected — this is a fresh entity.
    Added {
        id: EntityId,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity_type: Option<String>,
        /// Full markdown body on the `ref_b` side. `None` when the
        /// caller passed `include_content: false`.
        #[serde(skip_serializing_if = "Option::is_none")]
        content_after: Option<String>,
        /// Entities on either side that link inbound to this id.
        /// Empty when ripple is disabled or no inbound links exist.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ripple: Vec<IncomingRipple>,
    },
    /// Entity exists on both sides; bodies differ.
    Modified {
        id: EntityId,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_before: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_after: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ripple: Vec<IncomingRipple>,
    },
    /// Entity exists on `ref_a` but not on `ref_b` — and no rename
    /// was detected to a surviving entity.
    Deleted {
        id: EntityId,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_before: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ripple: Vec<IncomingRipple>,
    },
    /// Entity was renamed from `from_id` (present on `ref_a`) to
    /// `to_id` (present on `ref_b`). The two ids are reported with
    /// the rename chain — the sequence of intermediate ids when the
    /// rename passed through multiple commits between `ref_a` and
    /// `ref_b`.
    Renamed {
        from_id: EntityId,
        to_id: EntityId,
        /// Intermediate ids the rename passed through, oldest to
        /// newest. Empty for a direct one-step rename. Pulled from
        /// the Tier-D agent-notes trail so a single `Renamed` entry
        /// covers the full chain rather than emitting N intermediate
        /// pairs.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rename_chain: Vec<EntityId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_before: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_after: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ripple: Vec<IncomingRipple>,
    },
    /// Entity failed to parse on at least one side. The caller sees
    /// the id (best-effort) and the parse-error message; the
    /// surviving side's content can still ride along when available.
    InvalidEntity {
        id: EntityId,
        /// `"ref_a"` or `"ref_b"` — whichever side tripped the
        /// parser. `"both"` when both sides fail.
        side: String,
        /// Human-readable parse error message. Stable enough for
        /// consumers to grep / regex against — variant-specific
        /// payloads stay on the structured channel.
        error: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_before: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_after: Option<String>,
    },
}

/// One entry in an entity's incoming-wikilink ripple list. The
/// referrer entity is on either `ref_a` or `ref_b` (`side` discriminates);
/// consumers building a "what would break" preview consult both sides.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IncomingRipple {
    /// The entity that holds the inbound wiki-link.
    pub from_id: EntityId,
    /// Which side of the diff this referrer lives on: `"ref_a"` or
    /// `"ref_b"`. Pre- and post-state referrers can both appear in
    /// the same list — consumers branching on `side` know which one
    /// would still hold the link after a hypothetical merge.
    pub side: String,
    /// Section key where the inbound wiki-link surfaces. `None` when
    /// the relation was derived from `## Relationships` rather than
    /// a body link.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

/// Caller-supplied diff configuration. Defaults yield a useful diff
/// without requiring callers to opt in to every feature.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffConfig {
    /// Rename-similarity threshold in `[0.1, 1.0]` (floor
    /// `RENAME_SIMILARITY_MIN`). Lower values match more aggressively.
    /// Out-of-range values refuse with `INVALID_INPUT` (mirrors
    /// `memstead_changes_since`).
    pub rename_similarity: f32,
    /// When `true` (default), each entry carries the entity's full
    /// markdown body on both sides. When `false`, only the metadata
    /// (id, title, type, status) survives — smaller payload, useful
    /// for audit counts.
    pub include_content: bool,
    /// When `true` (default), each entry carries the set of inbound
    /// wiki-links — what would break if a downstream consumer
    /// applied or skipped this change. When `false`, the ripple
    /// field stays empty.
    pub include_ripple: bool,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            rename_similarity: crate::ops::RENAME_SIMILARITY_DEFAULT,
            include_content: true,
            include_ripple: true,
        }
    }
}

/// Top-level diff response. Echoes the two refs the caller passed in
/// (verbatim), reports the SHAs they resolved to, surfaces the
/// configuration the operation used, and lists every per-entity
/// entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diff {
    /// The first ref the caller passed in, verbatim.
    pub ref_a: String,
    /// The second ref the caller passed in, verbatim.
    pub ref_b: String,
    /// SHA that `ref_a` resolved to. Stable cursor for follow-up
    /// calls — consumers re-issue the diff with these SHAs to get
    /// identical output regardless of branch tip movement.
    pub resolved_a_sha: String,
    /// SHA that `ref_b` resolved to.
    pub resolved_b_sha: String,
    /// Configuration in effect for this diff.
    pub config: DiffConfig,
    /// Per-entity diff entries. Ordering is implementation-defined
    /// (today: stable by primary entity id) — consumers that need a
    /// specific order sort client-side.
    pub entries: Vec<EntityDiff>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_config_default_matches_rename_similarity_default() {
        let cfg = DiffConfig::default();
        assert_eq!(cfg.rename_similarity, crate::ops::RENAME_SIMILARITY_DEFAULT);
        assert!(cfg.include_content);
        assert!(cfg.include_ripple);
    }

    #[test]
    fn entity_diff_added_serialises_with_status_tag() {
        let entry = EntityDiff::Added {
            id: EntityId::new("specs", "alpha"),
            title: Some("Alpha".to_string()),
            entity_type: Some("spec".to_string()),
            content_after: Some("# Alpha\n".to_string()),
            ripple: Vec::new(),
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["status"], "added");
        assert_eq!(json["id"], "specs--alpha");
        assert_eq!(json["title"], "Alpha");
        assert_eq!(json["content_after"], "# Alpha\n");
        // Empty ripple list is omitted via skip_serializing_if so the
        // wire shape stays compact for the common case.
        assert!(json.get("ripple").is_none());
    }

    #[test]
    fn entity_diff_renamed_carries_optional_rename_chain() {
        let entry = EntityDiff::Renamed {
            from_id: EntityId::new("specs", "old"),
            to_id: EntityId::new("specs", "new"),
            rename_chain: vec![EntityId::new("specs", "interim")],
            title: None,
            entity_type: None,
            content_before: None,
            content_after: None,
            ripple: Vec::new(),
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["status"], "renamed");
        assert_eq!(json["from_id"], "specs--old");
        assert_eq!(json["to_id"], "specs--new");
        assert_eq!(
            json["rename_chain"],
            serde_json::json!(["specs--interim"])
        );
    }

    #[test]
    fn entity_diff_invalid_entity_carries_side_and_error() {
        let entry = EntityDiff::InvalidEntity {
            id: EntityId::new("specs", "broken"),
            side: "ref_a".to_string(),
            error: "missing frontmatter".to_string(),
            content_before: Some("not yaml".to_string()),
            content_after: None,
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["status"], "invalid_entity");
        assert_eq!(json["side"], "ref_a");
        assert_eq!(json["error"], "missing frontmatter");
    }

    #[test]
    fn diff_top_level_round_trips_through_serde() {
        let diff = Diff {
            ref_a: "main".to_string(),
            ref_b: "feature".to_string(),
            resolved_a_sha: "a".repeat(40),
            resolved_b_sha: "b".repeat(40),
            config: DiffConfig::default(),
            entries: vec![EntityDiff::Added {
                id: EntityId::new("v", "x"),
                title: None,
                entity_type: None,
                content_after: None,
                ripple: Vec::new(),
            }],
        };
        let json = serde_json::to_string(&diff).unwrap();
        let parsed: Diff = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, diff);
    }
}
