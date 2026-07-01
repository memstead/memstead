//! Wire-format types for the bridge HTTP / SSE surface.
//!
//! Mirrors the browser-sync JSON shapes one-to-one: the commit
//! envelope and the SSE event.
//!
//! These types are the durable contract between server
//! implementations (this crate's embedders, future Node / Python
//! bindings) and client code. Field order matches the spec; field
//! names are the canonical wire identifiers — do not rename or
//! reshape without bumping the wire-format version.
//!
//! `CommitEnvelope` and `EntityChange` live in
//! [`memstead_base::ops::commit_envelope`] (the engine owns the value
//! type so wasm clients can build against `memstead-base` alone) and
//! are re-exported here for backward compatibility with existing
//! `memstead_bridge::wire::…` import sites.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub use memstead_base::ops::{CommitEnvelope, EntityChange};

/// Per-hit shape returned by `/search`.
///
/// Owned wire mirror of [`memstead_base::ops::SearchHit`] — the JSON
/// encoding is byte-identical because every field name (and the
/// `skip_serializing_if`-driven optional surface) mirrors the
/// engine's `SearchHit` derive one-to-one. This MCP-shape conformance
/// (the bridge's `/search` and MCP's `memstead_search` emit identical
/// per-hit JSON) is what `search_hit_serializes_identical_to_engine_hit`
/// pins.
///
/// Heavyweight per-hit annotations (`score_breakdown`,
/// `matched_terms`, `expansion`) ride through as
/// [`serde_json::Value`] so the bridge does not have to vendor the
/// internal type definitions (`ScoreBreakdown`, `TermMatch`,
/// `ExpansionInfo`). The JSON pass-through keeps the wire shape
/// stable while keeping the crate-boundary thin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    /// Entity id — `<vault>--<slug>`. Matches the wire form
    /// downstream consumers (MCP, CLI) use.
    pub id: String,
    pub title: String,
    pub vault: String,
    pub entity_type: String,
    pub stub: bool,
    pub score: f32,
    pub tokens: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sections: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_breakdown: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_terms: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion: Option<serde_json::Value>,
}

impl SearchHit {
    /// Project an engine-side [`memstead_base::ops::SearchHit`] into the
    /// owned wire shape. Round-trips through `serde_json::Value` for
    /// the heavyweight optional fields so the bridge stays
    /// crate-boundary-thin.
    pub fn from_engine(hit: &memstead_base::ops::SearchHit) -> Self {
        Self {
            id: hit.id.0.clone(),
            title: hit.title.clone(),
            vault: hit.vault.clone(),
            entity_type: hit.entity_type.clone(),
            stub: hit.stub,
            score: hit.score,
            tokens: hit.tokens,
            snippet: hit.snippet.clone(),
            sections: hit.sections.clone(),
            score_breakdown: hit.score_breakdown.as_ref().and_then(|v| serde_json::to_value(v).ok()),
            matched_terms: hit.matched_terms.as_ref().and_then(|v| serde_json::to_value(v).ok()),
            expansion: hit.expansion.as_ref().and_then(|v| serde_json::to_value(v).ok()),
        }
    }
}

/// `GET /search` query parameters. Deserialised from the URL query
/// string via axum's `Query` extractor. Only `q` is required;
/// every other field is optional and falls back to a server-side
/// default.
///
/// Whitespace inside `q` is split into separate query terms that
/// flow into [`memstead_base::ops::Query::any`] — the engine's BM25
/// ranking promotes entities matching more terms (no explicit
/// boolean `AND` needed).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SearchQuery {
    /// Text predicate. Whitespace-separated tokens become an
    /// `any`-array on the engine's structured query. Required —
    /// empty `q` refuses with `INVALID_SEARCH_QUERY`.
    pub q: String,
    /// Optional `type` filter (`spec`, `memo`, …). Matches the
    /// MCP `memstead_search` `entity_type` param.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    /// Optional page size. Default + max are server-config
    /// driven ([`crate::BuildConfig::search_default_limit`] /
    /// [`crate::BuildConfig::search_max_limit`]). Out-of-range
    /// values refuse with `INVALID_SEARCH_QUERY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Optional 0-based offset for pagination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

/// Response body for `GET /search`. The outer shape stays small —
/// `vault` / `query` echo the request for client-side correlation,
/// `total_matched` / `truncated` carry pagination state, and
/// `hits[]` ships each match as a [`SearchHit`]. The hit shape is
/// the same one [`memstead_base::ops::SearchHit`] uses, so MCP's
/// `memstead_search` and the bridge's `/search` deliver byte-identical
/// per-hit JSON for the same vault state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// Vault the search ran against — echoed from the request path.
    pub vault: String,
    /// Echo of the original `q` parameter so a client that scoped
    /// the response into a UI element can label it without keeping
    /// its own bookkeeping.
    pub query: String,
    /// Hits in score order, newest BM25 first. Empty when no
    /// entity matches.
    pub hits: Vec<SearchHit>,
    /// Total number of matches before pagination — clients use
    /// this to size pagination controls without re-querying.
    pub total_matched: usize,
    /// `true` when the engine produced more hits than the
    /// effective limit returned in this response.
    pub truncated: bool,
    /// Non-fatal warnings the engine surfaced (e.g. unknown
    /// filter keys). Mirrors the MCP envelope's `warnings` slot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// SSE-side `vault_changed` event payload. Pushed when a vault's
/// HEAD advances; the client reacts by fetching the corresponding
/// commit envelope range via `/commits`.
///
/// Mirrors the [`memstead_base::engine::VaultChangedEvent`] core type — the
/// engine's broadcast surface is the natural producer. Re-declared
/// here so consumers depend only on the bridge crate's wire types
/// without pulling in the engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultChangedEvent {
    pub vault: String,
    pub head: String,
    pub previous: String,
    pub n_commits: u32,
}

impl From<memstead_base::engine::VaultChangedEvent> for VaultChangedEvent {
    fn from(e: memstead_base::engine::VaultChangedEvent) -> Self {
        Self {
            vault: e.vault,
            head: e.head,
            previous: e.previous,
            n_commits: e.n_commits,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn commit_envelope_re_export_round_trips_via_serde() {
        // Pin that the re-export from memstead-base preserves the
        // tagged wire shape — this is the cross-crate contract that
        // bridge embedders and wasm clients both depend on.
        let env = CommitEnvelope {
            sha: "c4f2a8".to_string(),
            parent: String::new(),
            vault: "engine".to_string(),
            timestamp: "2026-05-18T14:23:01Z".to_string(),
            trailers: BTreeMap::new(),
            changes: vec![EntityChange::Renamed {
                from: "old.md".to_string(),
                to: "new.md".to_string(),
                content: "body".to_string(),
            }],
        };
        let json = serde_json::to_value(&env).unwrap();
        assert_eq!(json["sha"], "c4f2a8");
        // `parent` is empty — `skip_serializing_if` keeps it out of
        // the wire form.
        assert!(json.get("parent").is_none());
        assert_eq!(json["changes"][0]["op"], "renamed");
        assert_eq!(json["changes"][0]["from"], "old.md");
        let parsed: CommitEnvelope = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn search_query_parses_url_query_string() {
        // axum's `Query` extractor goes via `serde_urlencoded` under
        // the hood; pin the shape the request side actually decodes.
        let q: SearchQuery =
            serde_urlencoded::from_str("q=hello+world&type=memo&limit=5&offset=10").unwrap();
        assert_eq!(q.q, "hello world");
        assert_eq!(q.entity_type.as_deref(), Some("memo"));
        assert_eq!(q.limit, Some(5));
        assert_eq!(q.offset, Some(10));
    }

    #[test]
    fn search_result_round_trips_via_serde() {
        let hit = SearchHit {
            id: "specs--alpha".to_string(),
            title: "Alpha".to_string(),
            vault: "specs".to_string(),
            entity_type: "spec".to_string(),
            stub: false,
            score: 1.5,
            tokens: 42,
            snippet: Some("...alpha matched...".to_string()),
            sections: {
                let mut m = HashMap::new();
                m.insert("identity".to_string(), "Alpha".to_string());
                m
            },
            score_breakdown: None,
            matched_terms: None,
            expansion: None,
        };
        let result = SearchResult {
            vault: "specs".to_string(),
            query: "alpha".to_string(),
            hits: vec![hit],
            total_matched: 1,
            truncated: false,
            warnings: vec![],
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["vault"], "specs");
        assert_eq!(json["query"], "alpha");
        assert_eq!(json["total_matched"], 1);
        assert_eq!(json["truncated"], false);
        assert_eq!(json["hits"][0]["id"], "specs--alpha");
        assert_eq!(json["hits"][0]["title"], "Alpha");
        assert_eq!(json["hits"][0]["entity_type"], "spec");
        // `warnings` is empty → skipped on the wire by the
        // `skip_serializing_if` discipline.
        assert!(json.get("warnings").is_none());
        // Round-trip back into a SearchResult and confirm equality —
        // the contract the AC requires.
        let parsed: SearchResult = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, result);
    }

    #[test]
    fn search_hit_serializes_identical_to_engine_hit() {
        // MCP-shape conformance: the bridge's `SearchHit` JSON equals
        // the engine's `SearchHit` JSON for the same data. Constructing
        // one of each by hand and comparing the serialized JSON pins
        // the byte-for-byte
        // equivalence — a future field-rename on the engine that
        // forgets to mirror here surfaces as a snapshot diff.
        let engine_hit = memstead_base::ops::SearchHit {
            id: memstead_base::EntityId::new("specs", "alpha"),
            title: "Alpha".to_string(),
            vault: "specs".to_string(),
            entity_type: "spec".to_string(),
            stub: false,
            score: 1.5,
            tokens: 42,
            snippet: Some("snip".to_string()),
            sections: {
                let mut m = HashMap::new();
                m.insert("identity".to_string(), "Alpha".to_string());
                m
            },
            score_breakdown: None,
            matched_terms: None,
            expansion: None,
            summary: None,
        };
        let bridge_hit = SearchHit::from_engine(&engine_hit);
        let engine_json = serde_json::to_value(&engine_hit).unwrap();
        let bridge_json = serde_json::to_value(&bridge_hit).unwrap();
        assert_eq!(
            bridge_json, engine_json,
            "bridge SearchHit must serialize identically to engine SearchHit"
        );
    }

    #[test]
    fn vault_changed_event_round_trips_via_serde() {
        let e = VaultChangedEvent {
            vault: "specs".to_string(),
            head: "abc".to_string(),
            previous: "def".to_string(),
            n_commits: 2,
        };
        let json = serde_json::to_string(&e).unwrap();
        let parsed: VaultChangedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, e);
    }

    #[test]
    fn vault_changed_event_lifts_from_engine_type() {
        let core = memstead_base::engine::VaultChangedEvent {
            vault: "v".to_string(),
            head: "h".to_string(),
            previous: "p".to_string(),
            n_commits: 1,
        };
        let wire: VaultChangedEvent = core.into();
        assert_eq!(wire.vault, "v");
        assert_eq!(wire.head, "h");
        assert_eq!(wire.previous, "p");
        assert_eq!(wire.n_commits, 1);
    }
}
