//! Search arguments, results and facets.

use super::*;

// ---------------------------------------------------------------------------
// Search types
// ---------------------------------------------------------------------------

/// Flat query shape for full-text search. Four optional fields, all
/// combined with implicit AND across fields.
///
/// Within `any`: at least one term must match (OR semantics). Entities
/// matching more terms rank higher automatically — no explicit `and`.
/// Within `not`: none of the listed terms may appear. `phrase` requires
/// exact adjacency (case- and diacritic-folded). `field` narrows the match
/// region for all three to a single indexed field; `None` = match anywhere
/// indexed.
///
/// Empty/unset everywhere ⇒ no text predicate; `search` behaves as a
/// metadata-only filter (subsumes the former `list` semantics).
///
/// No stemming, wildcards, or regex — the caller expands morphology and
/// synonyms by enumerating variants in `any`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Query {
    /// Terms where at least one must match (OR semantics).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any: Vec<String>,
    /// Terms that must not match (exclusion).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not: Vec<String>,
    /// Exact phrase that must appear (case- and diacritic-folded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phrase: Option<String>,
    /// Restrict `any` / `not` / `phrase` to a single field (title or section
    /// key). `None` = match anywhere indexed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl Query {
    /// True if no text predicate is set — caller falls back to the
    /// metadata-only filter path.
    pub fn is_empty(&self) -> bool {
        self.any.is_empty() && self.not.is_empty() && self.phrase.is_none()
    }
}

/// Scope filters for search and list operations.
#[derive(Debug, Clone, Default)]
pub struct SearchScope {
    /// Structured flat query. All text matching flows through this field;
    /// see [`Query`] for semantics. `None` (or an empty query) makes
    /// `search` behave as a metadata-only filter.
    pub query: Option<Query>,
    pub mem: Option<String>,
    pub entity_type: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// Equality filters on metadata fields: `{ "level": "M0" }`.
    pub filters: HashMap<String, String>,
    /// Range filters: `{ "min_coverage": "0.5", "max_coverage": "1.0" }`.
    pub range_filters: HashMap<String, String>,
    /// Only entities with this edge type (incoming or outgoing).
    pub edge_type: Option<String>,
    /// Only entities reachable from this entity within `depth` hops.
    pub related_to: Option<EntityId>,
    pub depth: Option<usize>,
    /// Relationship types to follow from primary hits to pull in graph-proximal
    /// neighbours.
    pub expand_via: Option<Vec<String>>,
    /// Maximum hops to traverse via `expand_via` (default: 1 when `expand_via`
    /// is set).
    pub expand_depth: Option<usize>,
    /// Traversal direction for `related_to` AND `expand_via`, applied at
    /// EVERY hop (depth > 1 is a pure transitive closure in the chosen
    /// direction, never a mixed walk). Defaults to `both` — the
    /// historical undirected behaviour, so a query omitting the
    /// selector returns exactly what it always returned.
    pub direction: crate::graph::query::TraversalDirection,
    /// Filter by stub status. `None` = no filter (returns both stubs and real
    /// entities); `Some(true)` = only stubs; `Some(false)` = only real entities.
    pub stub: Option<bool>,
    /// Token budget bounding the returned hit payload (search path only).
    /// `None` uses the engine default. A page whose hits exceed the budget is
    /// greedily trimmed (at least one hit always returns) with a
    /// `SEARCH_RESULTS_TRUNCATED` warning; `total` still reflects the full
    /// match count so the agent can page with `offset`.
    pub token_budget: Option<usize>,
}

/// Per-hit score components surfaced so agents can understand ranking.
///
/// Note: this is illustrative feedback, not a numerically authoritative
/// decomposition — tantivy's `Explanation` for `BoostQuery` over
/// `BooleanQuery` does not always sum cleanly. Agents should treat these
/// as proportions, not exact sums.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ScoreBreakdown {
    pub bm25: f32,
    pub title_boost: f32,
    pub field_weights: HashMap<String, f32>,
    /// `Some(f32)` on expanded hits only, carrying the depth-based decay
    /// factor (`0.5.powi(depth)`). `None` on primary hits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion_decay: Option<f32>,
}

/// One snippet-level match recorded per (term, field). `heading_path` is
/// `Some` when the match falls under an H3–H6 sub-heading; elements are
/// ordered outermost → innermost.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TermMatch {
    pub field: String,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_path: Option<Vec<String>>,
}

/// Metadata attached to hits reached via graph expansion. The
/// primary hit that seeded the expansion is identified by `of`; `via_edge`
/// is the exact `rel_type` string; `depth` counts hops from the seed.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ExpansionInfo {
    pub of: EntityId,
    pub via_edge: String,
    pub depth: usize,
    /// The direction the first-reaching edge was traversed in (`out` =
    /// away from the seed, `in` = at the seed) — keeps a `both` result
    /// interpretable. Additive: clients that ignore it decode unchanged.
    pub via_direction: crate::graph::query::TraversalDirection,
}

/// One sub-section-level facet entry. `path` is ordered outermost →
/// innermost, prefixed with the H2 section key (e.g. `["specifies",
/// "Response Shapes", "Markdown Output"]`). Structured vector (not a
/// delimiter-joined string) so headings containing punctuation don't break
/// the key.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SubsectionFacet {
    pub path: Vec<String>,
    pub count: usize,
}

/// Fixed set of facet dimensions computed over the unpaginated hit set.
/// Tier 1 freezes the dimensions; extend later only if empirical use
/// demands it. Zero-count entries are excluded to keep the payload small.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct Facets {
    pub by_type: HashMap<String, usize>,
    pub by_mem: HashMap<String, usize>,
    pub by_level: HashMap<String, usize>,
    pub by_status: HashMap<String, usize>,
    pub by_confidence: HashMap<String, usize>,
    pub by_subsection: Vec<SubsectionFacet>,
    /// `"primary"` / `"expanded"` — counts of primary vs. graph-expanded
    /// hits. Always present; `expanded` is `0` when no expansion ran.
    pub by_expansion: HashMap<String, usize>,
}

/// A search result hit.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub id: EntityId,
    pub title: String,
    pub mem: String,
    pub entity_type: String,
    pub stub: bool,
    pub score: f32,
    pub tokens: usize,
    /// The entity's `last_modified` stamp (RFC-3339 date) — list/roster
    /// consumers (the app's Liste, agents asking "what moved lately")
    /// sort on it without per-entity reads. `None` for stubs and hits
    /// built outside the engine ops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    pub snippet: Option<String>,
    /// Lead/key section bodies for the hit. The `search` op leaves this
    /// **empty** — search finds entities, `memstead_entity` reads their
    /// bodies; carrying every required section per hit overflowed the MCP
    /// transport cap. The `list` op still populates it (its human-facing
    /// roster consumers read the lead section as a one-line summary).
    /// Empty maps are omitted from the serialized envelope.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sections: HashMap<String, String>,
    /// Score component breakdown — populated when the call supplied a
    /// text predicate; `None` on the metadata-only path. Always on the
    /// wire (as `null` when absent): the envelope the `memstead_search`
    /// description promises is one stable shape, so a consumer reads
    /// the key and branches on its value, never on its presence.
    #[serde(default)]
    pub score_breakdown: Option<ScoreBreakdown>,
    /// Per-term match details keyed by query term — populated when the
    /// call supplied a text predicate; `None` on the metadata-only path.
    /// Always on the wire, as `score_breakdown` is.
    #[serde(default)]
    pub matched_terms: Option<HashMap<String, Vec<TermMatch>>>,
    /// Expansion metadata — populated on hits reached via graph
    /// expansion; `None` on primary hits. Always on the wire, as
    /// `score_breakdown` is.
    #[serde(default)]
    pub expansion: Option<ExpansionInfo>,
    /// Lead-section summary resolved against the hit's *own* mem schema
    /// at search time (see [`SummaryPair`]). The renderer cannot resolve
    /// it correctly on its own — the global `type_by_name` only sees the
    /// `default` schema, so a `software`-schema hit (`requirement` →
    /// `Statement`, `actor` → `Role`) would miss its anchor section and
    /// render `—`. `#[serde(skip)]` keeps `SearchHit`'s wire shape
    /// unchanged; the value surfaces on the envelope's `summary_heading` /
    /// `summary_value`. `None` only on hits built outside the engine
    /// search op (FFI/bridge and test fixtures), where the renderer falls
    /// back to the default-schema lookup.
    #[serde(skip)]
    pub summary: Option<SummaryPair>,
}

/// Lead-section `(heading, value)` for a search/list hit, resolved
/// against the hit's own mem schema at search time. Carried in-memory
/// from the search op to the renderers; see [`SearchHit::summary`].
#[derive(Debug, Clone)]
pub struct SummaryPair {
    pub heading: String,
    pub value: String,
}

/// Search result with metadata.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub total: usize,
    pub returned: usize,
    pub offset: usize,
    /// Sum of estimated tokens across all matching entities (pre-pagination).
    /// Lets agents judge read cost before paging.
    pub total_tokens: usize,
    pub hits: Vec<SearchHit>,
    /// Faceted counts over the unpaginated hit set. Stable closed
    /// struct; zero-count entries are excluded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facets: Option<Facets>,
    /// Non-fatal issues surfaced to the caller. Structured
    /// `WarningHint` shape (`{code, details, message}`) — same wire
    /// envelope every other tool's warnings already use. Agents
    /// branch on `code`; the message field carries the existing
    /// remediation prose.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
}

/// List result with token totals.
#[derive(Debug, Clone, Serialize)]
pub struct ListResult {
    pub total: usize,
    pub returned: usize,
    pub offset: usize,
    pub total_tokens: usize,
    pub hits: Vec<SearchHit>,
    /// Non-fatal issues surfaced to the caller — same structured
    /// shape as `SearchResult.warnings`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
}
