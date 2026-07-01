//! Full-text search across entities with BM25 scoring via tantivy.
//!
//! `SearchScope.query` is the sole text-predicate entry point.
//! Empty or absent `query` ⇒ metadata-only scan (the `list` semantics
//! path). Metadata, topology, and pagination filters still run
//! in-memory against the store after the tantivy hit set is collected.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use memstead_schema::{Filterable, Schema, Serialization, TypeDefinition, type_by_name};

use super::{
    ExpansionInfo, Facets, ListResult, Query, ScoreBreakdown, SearchHit, SearchResult,
    SearchScope, SubsectionFacet, SummaryPair, WarningHint,
};
use crate::entity::EntityId;
use crate::entity::generator::generate_markdown;
use crate::graph::query;
use crate::search_index::{
    MemIndex, compute_matched_terms, compute_score_breakdown, query as search_query,
};
use crate::store::Store;

/// Hard ceiling on how many hits to pull back from tantivy per mem. The
/// in-memory post-filter trims this down; the ceiling exists so misconfigured
/// callers (e.g. an unbounded offset) can't degrade into a full-corpus scan
/// per mem. 10k matches the "typical mem" perf budget.
const MAX_HITS_PER_MEM: usize = 10_000;

/// Resolve a hit's lead-section summary against its *own* mem schema —
/// the renderer can't do this correctly (its `type_by_name` only sees the
/// `default` schema), so the search op computes it here where the per-mem
/// `schema` is in hand and stores it on the hit. Delegates to the shared
/// [`crate::render::lead_section_pair`] so the lead-section rule has one home.
fn hit_summary<'a>(
    schema: &TypeDefinition,
    get_section: impl Fn(&str) -> Option<&'a str>,
) -> SummaryPair {
    let (heading, value) = crate::render::lead_section_pair(schema, get_section);
    SummaryPair { heading, value }
}

/// Estimate token count for an entity (rough: markdown length / 4).
fn estimate_tokens(entity: &crate::entity::Entity, schema: &TypeDefinition) -> usize {
    let md = generate_markdown(entity, schema);
    md.len() / 4
}

/// #54: a `related_to` neighbourhood larger than this is ranked by proximity
/// and bounded to its nearest members so a hub can't flood the caller. Sized
/// generously — a normal (non-hub) neighbourhood stays whole (the refusal AC).
const RELATED_TO_NEIGHBOURHOOD_CAP: usize = 100;

/// Default token budget bounding a single search page's hit payload. Sized
/// to leave headroom under the MCP transport cap once both response channels
/// (structured envelope + rendered markdown, each derived from the same
/// hits) and the facets/frontmatter overhead are counted. Agents override via
/// `token_budget`; a page that overflows it is greedily trimmed with a
/// `SEARCH_RESULTS_TRUNCATED` warning.
const DEFAULT_SEARCH_TOKEN_BUDGET: usize = 12_000;

/// Rough serialized-token cost of one search hit (chars / 4) — the same
/// heuristic the rest of the engine uses for token estimates. Drives the
/// budget greedy-fill; `summary` is `#[serde(skip)]` so it doesn't serialize
/// here, which slightly under-counts, but the markdown channel carries the
/// summary instead, so the budget headroom absorbs it.
fn hit_response_tokens(hit: &SearchHit) -> usize {
    serde_json::to_string(hit).map(|s| s.len()).unwrap_or(0) / 4
}

/// Search entities with text matching and filtering.
///
/// Evaluates `scope.query` against the per-mem tantivy indexes when any
/// text predicate is set; otherwise degrades to a metadata-only scan of the
/// store (the `list` semantics path).
pub fn search(
    store: &Store,
    scope: &SearchScope,
    default_schema: &TypeDefinition,
    search_indexes: &HashMap<String, MemIndex>,
    mem_schemas: &HashMap<String, Arc<Schema>>,
) -> SearchResult {
    let mut warnings: Vec<WarningHint> = Vec::new();
    let scoped_type = scope.entity_type.as_deref();
    let scope_mem = scope.mem.as_deref();
    let filter_type = scoped_type
        .and_then(|t| resolve_type(t, scope_mem, mem_schemas));
    let filter_schema: &TypeDefinition = filter_type.as_deref().unwrap_or(default_schema);
    collect_equality_filter_warnings(
        &scope.filters,
        filter_schema,
        scoped_type,
        scope_mem,
        mem_schemas,
        &mut warnings,
    );
    collect_range_filter_warnings(
        &scope.range_filters,
        filter_schema,
        scoped_type,
        scope_mem,
        mem_schemas,
        &mut warnings,
    );
    collect_stub_type_exclusion_warning(scope, &mut warnings);

    // `scope.query` is the sole text-predicate entry point. An absent or
    // empty query falls through to the metadata-only scan below.
    let effective_query: Option<&Query> = scope.query.as_ref().filter(|q| !q.is_empty());
    let query_has_text = effective_query.is_some();

    // Execute the tantivy query across the selected mems — at most one
    // when `scope.mem` is Some, otherwise every indexed mem. Keep the
    // highest score per entity (a cross-mem dedup is irrelevant today but
    // cheap insurance).
    let mut scored_ids: HashMap<EntityId, f32> = HashMap::new();
    if query_has_text {
        let query = effective_query.unwrap();
        let target_mems = resolve_target_mems(search_indexes, scope.mem.as_deref());
        if let Some(name) = scope.mem.as_ref()
            && target_mems.is_empty()
        {
            warnings.push(WarningHint::SearchMemIndexUnavailable {
                mem: name.clone(),
                reason: "missing_index",
                error: None,
            });
        }
        for mem_name in &target_mems {
            let Some(idx) = search_indexes.get(mem_name.as_str()) else {
                continue;
            };
            let schema = mem_schemas.get(mem_name.as_str());
            match search_query::execute_on_mem(idx, schema, query, MAX_HITS_PER_MEM) {
                Ok(hits) => {
                    for (id, score) in hits {
                        let slot = scored_ids.entry(id).or_insert(f32::MIN);
                        if score > *slot {
                            *slot = score;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        mem = mem_name.as_str(),
                        error = %e,
                        "tantivy query failed; mem contributes no hits"
                    );
                    warnings.push(WarningHint::SearchMemIndexUnavailable {
                        mem: mem_name.to_string(),
                        reason: "query_failed",
                        error: Some(e.to_string()),
                    });
                }
            }
        }
        if scored_ids.is_empty() {
            return SearchResult {
                total: 0,
                returned: 0,
                offset: scope.offset.unwrap_or(0),
                total_tokens: 0,
                hits: Vec::new(),
                // Empty-but-present facets keeps the response shape stable
                // even when there are no hits — agents can always branch on
                // the keys without null checks.
                facets: Some(Facets::default()),
                warnings,
            };
        }
    }

    let query_term = first_positive_term(effective_query);

    let mut hits: Vec<SearchHit> = Vec::new();
    for entity in store.all_entities() {
        match scope.stub {
            Some(true) if !entity.stub => continue,
            Some(false) if entity.stub => continue,
            _ => {}
        }

        if let Some(ref mem) = scope.mem
            && entity.mem != *mem
        {
            continue;
        }

        if query_has_text && !scored_ids.contains_key(&entity.id) {
            continue;
        }

        if let Some(ref type_name) = scope.entity_type
            && entity.entity_type != *type_name
        {
            continue;
        }

        let resolved = resolve_type(&entity.entity_type, Some(entity.mem.as_str()), mem_schemas);
        let schema: &TypeDefinition = resolved.as_deref().unwrap_or(default_schema);

        if !apply_equality_filters(
            entity,
            &scope.filters,
            schema,
            scope.mem.as_deref(),
            mem_schemas,
        ) {
            continue;
        }
        if !apply_range_filters(
            entity,
            &scope.range_filters,
            schema,
            scope.mem.as_deref(),
            mem_schemas,
        ) {
            continue;
        }

        if let Some(ref edge_type) = scope.edge_type {
            let has_out = store
                .outgoing(&entity.id)
                .iter()
                .any(|e| e.rel_type == *edge_type);
            let has_in = store
                .incoming(&entity.id)
                .iter()
                .any(|e| e.rel_type == *edge_type);
            if !has_out && !has_in {
                continue;
            }
        }

        let score = scored_ids.get(&entity.id).copied().unwrap_or(0.0);
        let snippet = query_term
            .as_ref()
            .and_then(|term| snippet_for(entity, term, schema));

        let tokens = estimate_tokens(entity, schema);

        // Full section bodies are deliberately NOT carried on search hits:
        // search finds entities, `memstead_entity` reads them in full.
        // Shipping every required section per hit pushed a page of
        // content-rich matches past the MCP transport token cap; the
        // lead-section summary, `snippet`, and `matched_terms` carry enough
        // signal to triage a hit, and the body is one `memstead_entity` call
        // away. (`list` still ships sections — its human-facing roster
        // consumers read them.)
        let summary = Some(hit_summary(schema, |k| {
            entity.sections.get(k).map(String::as_str)
        }));

        // Populate matched_terms + score_breakdown only when the
        // caller actually supplied a text predicate. The metadata-only path
        // keeps both as `None` so empty queries don't carry pointless feedback.
        let (matched_terms, score_breakdown) = if let Some(q) = effective_query {
            let mt = compute_matched_terms(entity, q);
            let sb = compute_score_breakdown(schema, score, &mt);
            (mt, Some(sb))
        } else {
            (None, None)
        };

        hits.push(SearchHit {
            id: entity.id.clone(),
            title: entity.title.clone(),
            mem: entity.mem.clone(),
            entity_type: entity.entity_type.clone(),
            stub: entity.stub,
            score,
            tokens,
            snippet,
            summary,
            sections: HashMap::new(),
            score_breakdown,
            matched_terms,
            expansion: None,
        });
    }

    // #54: a `related_to` neighbourhood is ranked by proximity (nearer
    // first) and bounded, not a flat alphabetical flood. Compute hop-
    // distances (membership = the reachable set, unchanged) and the anchor's
    // directly-typed neighbours; the sort and cap below consume them.
    let neighbourhood: Option<(HashMap<EntityId, usize>, HashSet<EntityId>)> =
        if let Some(ref related_to) = scope.related_to {
            let depth = scope.depth.unwrap_or(1);
            let distances = query::reachable_distances(store, related_to, depth);
            hits.retain(|h| distances.contains_key(&h.id));
            let typed_direct: HashSet<EntityId> = store
                .outgoing(related_to)
                .iter()
                .filter(|e| e.source != crate::store::EdgeSource::BodyLink)
                .map(|e| e.target.clone())
                .chain(
                    store
                        .incoming(related_to)
                        .iter()
                        .filter(|e| e.source != crate::store::EdgeSource::BodyLink)
                        .map(|e| e.from.clone()),
                )
                .collect();
            Some((distances, typed_direct))
        } else {
            None
        };

    // Graph expansion. After the primary hit set is computed,
    // optionally pull in neighbours reachable via the requested edge types.
    // Non-query filters (mem, entity_type, filters, range_filters) also
    // apply to expanded candidates — a violating neighbour is dropped. The
    // `related_to`, `edge_type`, and text predicates deliberately do NOT
    // apply: expansion is a graph-proximity surface on top of
    // the primary hit set, not a second text query.
    if let Some(ref edge_types) = scope.expand_via
        && !edge_types.is_empty()
    {
        expand_hits(&mut hits, store, edge_types, scope, default_schema, mem_schemas);
    }

    // Sort: a `related_to` neighbourhood ranks by proximity — nearer hops
    // first, then a typed (dependency) link to the anchor before a
    // co-mention at the same distance — otherwise by tantivy score. Title
    // asc is the stable tiebreak throughout.
    if let Some((distances, typed_direct)) = neighbourhood.as_ref() {
        hits.sort_by(|a, b| {
            let da = distances.get(&a.id).copied().unwrap_or(usize::MAX);
            let db = distances.get(&b.id).copied().unwrap_or(usize::MAX);
            da.cmp(&db)
                .then_with(|| {
                    typed_direct
                        .contains(&b.id)
                        .cmp(&typed_direct.contains(&a.id))
                })
                .then_with(|| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.title.cmp(&b.title))
        });
    } else {
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.title.cmp(&b.title))
        });
    }

    // #54: bound a hub neighbourhood to its nearest N (after proximity
    // ranking) so it can't flood the caller; a neighbourhood at/under the
    // cap is unchanged (refusal AC). The warning surfaces the truncation.
    if neighbourhood.is_some() && hits.len() > RELATED_TO_NEIGHBOURHOOD_CAP {
        warnings.push(WarningHint::NeighbourhoodCapped {
            kept: RELATED_TO_NEIGHBOURHOOD_CAP,
            total: hits.len(),
        });
        hits.truncate(RELATED_TO_NEIGHBOURHOOD_CAP);
    }

    let total = hits.len();
    let total_tokens: usize = hits.iter().map(|h| h.tokens).sum();
    // Facets are computed over the unpaginated hit set. Pagination
    // is for display, facets are for navigation — counting only the page
    // window would mislead the agent.
    let facets = compute_facets(&hits, store);
    let offset = scope.offset.unwrap_or(0);
    let limit = scope.limit.unwrap_or(50).min(200);

    let mut paginated: Vec<SearchHit> = hits.into_iter().skip(offset).take(limit).collect();

    // Token-budget guard: a page of content-rich hits can still overflow the
    // MCP transport cap even after `limit`. Greedily keep hits while the
    // running serialized cost stays under the budget; always keep at least
    // one (a single oversized hit must still come back). `total` stays the
    // full match count — the agent pages with `offset` or raises
    // `token_budget`. Bounding here (not in the markdown renderer) keeps both
    // response channels in lockstep, since both derive from `hits`.
    let budget = scope.token_budget.unwrap_or(DEFAULT_SEARCH_TOKEN_BUDGET);
    let pre_budget = paginated.len();
    let mut running = 0usize;
    let mut keep = 0usize;
    for hit in &paginated {
        let cost = hit_response_tokens(hit);
        if keep > 0 && running + cost > budget {
            break;
        }
        running += cost;
        keep += 1;
    }
    if keep < pre_budget {
        paginated.truncate(keep);
        warnings.push(WarningHint::SearchResultsTruncated {
            kept: keep,
            budget,
        });
    }
    let returned = paginated.len();

    SearchResult {
        total,
        returned,
        offset,
        total_tokens,
        hits: paginated,
        facets: Some(facets),
        warnings,
    }
}

/// List entities with filtering (no text matching, returns all matching entities).
pub fn list(
    store: &Store,
    scope: &SearchScope,
    default_schema: &TypeDefinition,
    mem_schemas: &HashMap<String, Arc<Schema>>,
) -> ListResult {
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut total_tokens = 0;
    let mut warnings: Vec<WarningHint> = Vec::new();
    let scoped_type = scope.entity_type.as_deref();
    let scope_mem = scope.mem.as_deref();
    let filter_type = scoped_type
        .and_then(|t| resolve_type(t, scope_mem, mem_schemas));
    let filter_schema: &TypeDefinition = filter_type.as_deref().unwrap_or(default_schema);
    collect_equality_filter_warnings(
        &scope.filters,
        filter_schema,
        scoped_type,
        scope_mem,
        mem_schemas,
        &mut warnings,
    );
    collect_range_filter_warnings(
        &scope.range_filters,
        filter_schema,
        scoped_type,
        scope_mem,
        mem_schemas,
        &mut warnings,
    );
    collect_stub_type_exclusion_warning(scope, &mut warnings);

    for entity in store.all_entities() {
        match scope.stub {
            Some(true) if !entity.stub => continue,
            Some(false) if entity.stub => continue,
            _ => {}
        }

        if let Some(ref mem) = scope.mem
            && entity.mem != *mem
        {
            continue;
        }

        if let Some(ref type_name) = scope.entity_type
            && entity.entity_type != *type_name
        {
            continue;
        }

        let resolved = resolve_type(&entity.entity_type, Some(entity.mem.as_str()), mem_schemas);
        let schema: &TypeDefinition = resolved.as_deref().unwrap_or(default_schema);

        if !apply_equality_filters(
            entity,
            &scope.filters,
            schema,
            scope.mem.as_deref(),
            mem_schemas,
        ) {
            continue;
        }
        if !apply_range_filters(
            entity,
            &scope.range_filters,
            schema,
            scope.mem.as_deref(),
            mem_schemas,
        ) {
            continue;
        }

        if let Some(ref edge_type) = scope.edge_type {
            let has_out = store
                .outgoing(&entity.id)
                .iter()
                .any(|e| e.rel_type == *edge_type);
            let has_in = store
                .incoming(&entity.id)
                .iter()
                .any(|e| e.rel_type == *edge_type);
            if !has_out && !has_in {
                continue;
            }
        }

        let tokens = estimate_tokens(entity, schema);
        total_tokens += tokens;

        let mut result_sections = HashMap::new();
        for section_def in schema.sections.iter().filter(|s| s.required) {
            if let Some(content) = entity.sections.get(section_def.key.as_str()) {
                result_sections.insert(section_def.key.clone(), content.clone());
            }
        }

        // Resolve the summary before moving `result_sections` into the hit —
        // the closure borrows it, so the borrow must end first.
        let summary = Some(hit_summary(schema, |k| {
            result_sections.get(k).map(String::as_str)
        }));

        hits.push(SearchHit {
            id: entity.id.clone(),
            title: entity.title.clone(),
            mem: entity.mem.clone(),
            entity_type: entity.entity_type.clone(),
            stub: entity.stub,
            score: 0.0,
            tokens,
            snippet: None,
            summary,
            sections: result_sections,
            score_breakdown: None,
            matched_terms: None,
            expansion: None,
        });
    }

    hits.sort_by(|a, b| a.title.cmp(&b.title));

    let total = hits.len();
    let offset = scope.offset.unwrap_or(0);
    let limit = scope.limit.unwrap_or(50).min(200);
    let paginated: Vec<SearchHit> = hits.into_iter().skip(offset).take(limit).collect();
    let returned = paginated.len();

    ListResult {
        total,
        returned,
        offset,
        total_tokens,
        hits: paginated,
        warnings,
    }
}

// ---------------------------------------------------------------------------
// Facets
// ---------------------------------------------------------------------------

/// Compute facet counts over the unpaginated hit set. Zero-count entries are
/// excluded to keep the payload small — agents branch on presence, not on
/// counts. `by_expansion` tags each hit `primary` or `expanded`.
///
/// `by_level` / `by_status` / `by_confidence` are the fixed Tier 1
/// `Filterable::Equality` dimensions. We look them up by literal metadata
/// key — the three closed fields on `Facets` match the three conventional
/// names used across the built-in schemas. If a schema renames them (e.g.
/// `verification_status` on assertions), that value lands in neither
/// `by_status` nor a dynamic dim — Tier 1 freezes the facet set; extending
/// is a Tier 2 concern.
fn compute_facets(hits: &[SearchHit], store: &Store) -> Facets {
    let mut by_type: HashMap<String, usize> = HashMap::new();
    let mut by_mem: HashMap<String, usize> = HashMap::new();
    let mut by_level: HashMap<String, usize> = HashMap::new();
    let mut by_status: HashMap<String, usize> = HashMap::new();
    let mut by_confidence: HashMap<String, usize> = HashMap::new();
    let mut subsection_counts: HashMap<Vec<String>, usize> = HashMap::new();
    let mut by_expansion: HashMap<String, usize> = HashMap::new();

    for hit in hits {
        // Stubs carry `entity_type: ""` by construction (store_builder::make_stub).
        // Skip them here so the facet doesn't expose a meaningless empty-string
        // bucket — an `entity_type` is semantically undefined for a stub.
        // Agents that need stub counts already have `stub=true|false` filter +
        // `memstead_health.stubs`.
        if !hit.entity_type.is_empty() {
            *by_type.entry(hit.entity_type.clone()).or_insert(0) += 1;
        }
        *by_mem.entry(hit.mem.clone()).or_insert(0) += 1;

        if let Some(entity) = store.get(&hit.id) {
            if let Some(v) = entity.metadata.get("level") {
                *by_level.entry(v.to_frontmatter_string()).or_insert(0) += 1;
            }
            if let Some(v) = entity.metadata.get("status") {
                *by_status.entry(v.to_frontmatter_string()).or_insert(0) += 1;
            }
            if let Some(v) = entity.metadata.get("confidence") {
                *by_confidence.entry(v.to_frontmatter_string()).or_insert(0) += 1;
            }
        }

        let tag = if hit.expansion.is_some() { "expanded" } else { "primary" };
        *by_expansion.entry(tag.into()).or_insert(0) += 1;

        if let Some(matched) = &hit.matched_terms {
            for term_matches in matched.values() {
                for tm in term_matches {
                    let Some(heading_path) = &tm.heading_path else {
                        continue;
                    };
                    if heading_path.is_empty() {
                        continue;
                    }
                    let mut path = Vec::with_capacity(heading_path.len() + 1);
                    path.push(tm.field.clone());
                    path.extend(heading_path.iter().cloned());
                    *subsection_counts.entry(path).or_insert(0) += 1;
                }
            }
        }
    }

    // Deterministic order: count desc, then path asc. Makes the wire shape
    // stable across runs for snapshot tests + readable for agents.
    let mut by_subsection: Vec<SubsectionFacet> = subsection_counts
        .into_iter()
        .map(|(path, count)| SubsectionFacet { path, count })
        .collect();
    by_subsection.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.path.cmp(&b.path)));

    Facets {
        by_type,
        by_mem,
        by_level,
        by_status,
        by_confidence,
        by_subsection,
        by_expansion,
    }
}

// ---------------------------------------------------------------------------
// Graph expansion
// ---------------------------------------------------------------------------

/// Append expanded hits to the primary set. For each primary seed, walk
/// `edge_types` bidirectionally up to `expand_depth` hops (default 1) and
/// add neighbours with `expansion: Some(ExpansionInfo)`. Score decays by
/// `0.5^depth`. Non-query filters (`mem`, `entity_type`, `filters`,
/// `range_filters`) are enforced on every candidate; violating neighbours
/// are dropped. Duplicates across multiple seeds are resolved by keeping
/// the highest-score candidate.
///
/// Re-sorting is the caller's job (happens once after expansion so primary
/// and expanded hits interleave by score).
fn expand_hits(
    hits: &mut Vec<SearchHit>,
    store: &Store,
    edge_types: &[String],
    scope: &SearchScope,
    default_schema: &TypeDefinition,
    mem_schemas: &HashMap<String, Arc<Schema>>,
) {
    let depth_limit = scope.expand_depth.unwrap_or(1);
    if depth_limit == 0 {
        return;
    }
    let primary_ids: HashSet<EntityId> = hits.iter().map(|h| h.id.clone()).collect();

    // Dedup across seeds: if a neighbour is reached from two primaries,
    // keep the candidate with the highest score so agents see the shortest
    // / highest-ranking path.
    let mut expanded: HashMap<EntityId, (f32, String, usize, EntityId)> = HashMap::new();

    for primary in hits.iter() {
        let reached = query::reachable_via(store, &primary.id, edge_types, depth_limit);
        for (neighbor_id, via_edge, depth) in reached {
            if primary_ids.contains(&neighbor_id) {
                continue;
            }
            let decay = 0.5f32.powi(depth as i32);
            let score = primary.score * decay;
            let better = match expanded.get(&neighbor_id) {
                Some((prev_score, _, _, _)) => score > *prev_score,
                None => true,
            };
            if better {
                expanded.insert(
                    neighbor_id,
                    (score, via_edge, depth, primary.id.clone()),
                );
            }
        }
    }

    for (id, (score, via_edge, depth, of)) in expanded {
        let Some(entity) = store.get(&id) else {
            continue;
        };
        match scope.stub {
            Some(true) if !entity.stub => continue,
            Some(false) if entity.stub => continue,
            _ => {}
        }
        if let Some(ref mem) = scope.mem
            && entity.mem != *mem
        {
            continue;
        }
        if let Some(ref type_name) = scope.entity_type
            && entity.entity_type != *type_name
        {
            continue;
        }

        let resolved = resolve_type(&entity.entity_type, Some(entity.mem.as_str()), mem_schemas);
        let schema: &TypeDefinition = resolved.as_deref().unwrap_or(default_schema);

        if !apply_equality_filters(
            entity,
            &scope.filters,
            schema,
            scope.mem.as_deref(),
            mem_schemas,
        ) {
            continue;
        }
        if !apply_range_filters(
            entity,
            &scope.range_filters,
            schema,
            scope.mem.as_deref(),
            mem_schemas,
        ) {
            continue;
        }

        let tokens = estimate_tokens(entity, schema);
        // Expanded hits follow the same no-section-bodies rule as primary
        // search hits — see the note at the primary push site.
        let summary = Some(hit_summary(schema, |k| {
            entity.sections.get(k).map(String::as_str)
        }));

        let decay = 0.5f32.powi(depth as i32);
        let score_breakdown = ScoreBreakdown {
            bm25: 0.0,
            title_boost: 0.0,
            field_weights: HashMap::new(),
            expansion_decay: Some(decay),
        };

        hits.push(SearchHit {
            id: id.clone(),
            title: entity.title.clone(),
            mem: entity.mem.clone(),
            entity_type: entity.entity_type.clone(),
            stub: entity.stub,
            score,
            tokens,
            snippet: None,
            summary,
            sections: HashMap::new(),
            score_breakdown: Some(score_breakdown),
            matched_terms: None,
            expansion: Some(ExpansionInfo {
                of,
                via_edge,
                depth,
            }),
        });
    }
}

// ---------------------------------------------------------------------------
// Query derivation helpers
// ---------------------------------------------------------------------------

/// First positive term across `any` → `phrase`. Drives the single-snippet
/// surface alongside the per-term snippets in [`compute_matched_terms`].
fn first_positive_term(query: Option<&Query>) -> Option<String> {
    let q = query?;
    if let Some(t) = q.any.first() {
        return Some(t.clone());
    }
    q.phrase.clone()
}

/// Pick which mems to query. `None` = every indexed mem; `Some(name)`
/// narrows to that mem (or empty when the name isn't indexed).
fn resolve_target_mems<'a>(
    search_indexes: &'a HashMap<String, MemIndex>,
    requested: Option<&str>,
) -> Vec<&'a String> {
    match requested {
        Some(name) => search_indexes
            .keys()
            .filter(|k| k.as_str() == name)
            .collect(),
        None => search_indexes.keys().collect(),
    }
}

/// Build a one-line snippet for a hit by finding the first case-insensitive
/// substring match of `term` in the title or a weighted section. The
/// per-term snippets with heading-path attribution live in
/// [`compute_matched_terms`].
fn snippet_for(
    entity: &crate::entity::Entity,
    term: &str,
    schema: &TypeDefinition,
) -> Option<String> {
    let lower_term = term.to_lowercase();
    if entity
        .title
        .to_lowercase()
        .contains(&lower_term)
    {
        return Some(build_snippet(&entity.title, term));
    }
    let mut best: Option<(f32, String)> = None;
    for section_def in &schema.sections {
        if section_def.search_weight == 0.0 {
            continue;
        }
        if let Some(content) = entity.sections.get(section_def.key.as_str())
            && content
                .to_lowercase()
                .contains(&lower_term)
        {
            let snippet = build_snippet(content, term);
            let pick = match &best {
                Some((w, _)) if *w >= section_def.search_weight => continue,
                _ => (section_def.search_weight, snippet),
            };
            best = Some(pick);
        }
    }
    best.map(|(_, s)| s)
}

/// Build a snippet showing context around the match.
pub(crate) fn build_snippet(content: &str, query: &str) -> String {
    let lower = content.to_lowercase();
    let lower_query = query.to_lowercase();
    let pos = match lower.find(&lower_query) {
        Some(p) => p,
        None => return content.chars().take(100).collect(),
    };

    let context = 50;
    let start = content[..pos]
        .char_indices()
        .rev()
        .nth(context)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end_of_match = pos + query.len();
    let end = content[end_of_match..]
        .char_indices()
        .nth(context)
        .map(|(i, _)| end_of_match + i)
        .unwrap_or(content.len());

    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < content.len() { "..." } else { "" };
    let before = &content[start..pos];
    let matched = &content[pos..end_of_match];
    let after = &content[end_of_match..end];

    format!("{prefix}{before}**{matched}**{after}{suffix}")
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

fn apply_equality_filters(
    entity: &crate::entity::Entity,
    filters: &HashMap<String, String>,
    schema: &TypeDefinition,
    scope_mem: Option<&str>,
    mem_schemas: &HashMap<String, Arc<Schema>>,
) -> bool {
    // Two distinct branches decide whether an entity survives a filter
    // key it can't equality-match — and they are NOT the same outcome:
    //
    // - **Field absent from this entity's type but equality-filterable on
    //   some other reachable type** → exclude (`return false`). This is
    //   the deliberate strict type-narrowing: `filters={level:"M0"}`
    //   excludes memos/stubs that have no `level` field, so the result
    //   doesn't lie about what matched.
    // - **Field declared on this entity's type but `Filterable::None`, OR
    //   absent here but declared only as non-filterable workspace-wide**
    //   → pass through (`continue`). A non-filterable field can't
    //   discriminate, so filtering on it is a no-op: the entity survives
    //   and the result set equals the same search without the filter. The
    //   `FIELD_NOT_FILTERABLE` warning still fires from
    //   `collect_equality_filter_warnings`. The narrowing decision is keyed
    //   on *filterability*, not mere declaration — a non-filterable field
    //   never type-narrows in either the scoped or unscoped case.
    //
    // A key not declared by ANY reachable schema also passes through
    // (the warning channel flags it `UNKNOWN_FILTER_KEY`) so a single
    // typo doesn't collapse the result set.
    for (key, filter_value) in filters {
        let Some(field_def) = schema.metadata_field(key) else {
            if classify_filter_field(key, scope_mem, mem_schemas, false)
                == FieldFilterability::Filterable
            {
                return false;
            }
            continue;
        };
        if !matches!(
            field_def.filterable,
            Filterable::Equality | Filterable::Range
        ) {
            // Declared but non-filterable — truly ignore (pass through).
            continue;
        }
        let is_csv = field_def.serialization == Serialization::CsvArray;

        match entity.metadata.get(key) {
            Some(val) => {
                let val_str = val.to_frontmatter_string();
                if is_csv {
                    let items: Vec<&str> = val_str
                        .split(',')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !items.iter().any(|item| *item == filter_value) {
                        return false;
                    }
                } else if val_str != *filter_value {
                    return false;
                }
            }
            None => return false,
        }
    }
    true
}

/// Workspace-wide verdict on a filter key, keyed on *filterability* rather
/// than mere declaration. Both the application path (`apply_*_filters`) and
/// the warning path (`collect_*_filter_warnings`) consult this single
/// helper so they cannot disagree about what the filter did — the
/// warning-matches-result contract.
#[derive(PartialEq, Eq, Clone, Copy)]
enum FieldFilterability {
    /// No reachable schema (within `scope_mem` if set) declares the key.
    Unknown,
    /// Declared on ≥1 type, but no declaring type marks it filterable in
    /// the requested mode → the filter is ignored, result = unfiltered.
    DeclaredNotFilterable,
    /// Filterable (in the requested mode) on ≥1 declaring type → the
    /// filter narrows and value-matches.
    Filterable,
}

/// Classify `key`'s filterability across the reachable schemas, ignoring
/// any single reference type. `scope_mem = Some(v)` narrows to that
/// mem's pinned schema (mirrors [`find_filter_declaring_types`] so the
/// application and warning paths see the same reachable set); `None` scans
/// every schema. `range = true` counts only `Filterable::Range`; `false`
/// (equality) counts `Equality | Range`.
///
/// This replaces the old declaration-only `filter_declared_anywhere`
/// boolean: a key declared only as non-filterable must be *ignored* (result
/// = unfiltered), not type-narrowed, in both the scoped and unscoped cases.
/// The deliberate narrowing on a *filterable* field absent from an
/// entity's type is preserved via the `Filterable` verdict.
fn classify_filter_field(
    key: &str,
    scope_mem: Option<&str>,
    mem_schemas: &HashMap<String, Arc<Schema>>,
    range: bool,
) -> FieldFilterability {
    let counts = |f: Filterable| {
        if range {
            f == Filterable::Range
        } else {
            matches!(f, Filterable::Equality | Filterable::Range)
        }
    };
    let mut declared = false;
    let mut filterable = false;
    let mut scan = |schema: &Schema| {
        for t in schema.types.values() {
            if let Some(fd) = t.metadata_field(key) {
                declared = true;
                if counts(fd.filterable) {
                    filterable = true;
                }
            }
        }
    };
    match scope_mem {
        Some(v) => {
            if let Some(s) = mem_schemas.get(v) {
                scan(s);
            }
        }
        None => {
            for s in mem_schemas.values() {
                scan(s);
            }
        }
    }
    if filterable {
        FieldFilterability::Filterable
    } else if declared {
        FieldFilterability::DeclaredNotFilterable
    } else {
        FieldFilterability::Unknown
    }
}

fn apply_range_filters(
    entity: &crate::entity::Entity,
    filters: &HashMap<String, String>,
    schema: &TypeDefinition,
    scope_mem: Option<&str>,
    mem_schemas: &HashMap<String, Arc<Schema>>,
) -> bool {
    // Same two-branch posture as `apply_equality_filters`:
    // - Field absent from this type but range-filterable on another
    //   reachable type → exclude (narrowing).
    // - Field declared on this type but NOT `Filterable::Range` (so `None`
    //   or `Equality`), OR absent here but declared only as
    //   non-range-filterable workspace-wide → pass through: a
    //   non-range-filterable field can't range-discriminate, so the range
    //   filter is a no-op and the result set equals the same search without
    //   it. The `FIELD_NOT_RANGE_FILTERABLE` warning still fires. The
    //   narrowing decision is keyed on range-filterability, not mere
    //   declaration.
    // Malformed keys (no `min_`/`max_`/`_before`/`_after`) and
    // workspace-wide-unknown fields pass through too.
    for (key, filter_value) in filters {
        let Some((field_name, op)) = parse_range_key(key) else {
            continue;
        };
        let Some(field_def) = schema.metadata_field(field_name) else {
            if classify_filter_field(field_name, scope_mem, mem_schemas, true)
                == FieldFilterability::Filterable
            {
                return false;
            }
            continue;
        };
        if field_def.filterable != Filterable::Range {
            // Declared but not range-filterable — truly ignore.
            continue;
        }

        let Some(val) = entity.metadata.get(field_name) else {
            return false;
        };
        let matched = match op {
            RangeOp::Min => compare_numeric(val, filter_value, |ev, fv| ev >= fv),
            RangeOp::Max => compare_numeric(val, filter_value, |ev, fv| ev <= fv),
            RangeOp::Before => val.to_frontmatter_string() <= *filter_value,
            RangeOp::After => val.to_frontmatter_string() >= *filter_value,
        };
        if !matched {
            return false;
        }
    }
    true
}

#[derive(Copy, Clone)]
enum RangeOp {
    Min,
    Max,
    Before,
    After,
}

fn parse_range_key(key: &str) -> Option<(&str, RangeOp)> {
    if let Some(field) = key.strip_prefix("min_") {
        Some((field, RangeOp::Min))
    } else if let Some(field) = key.strip_prefix("max_") {
        Some((field, RangeOp::Max))
    } else if let Some(field) = key.strip_suffix("_before") {
        Some((field, RangeOp::Before))
    } else {
        key.strip_suffix("_after")
            .map(|field| (field, RangeOp::After))
    }
}

/// Emit `STUB_FILTER_EXCLUDES_ALL` when both `stub=true` and `entity_type`
/// are set. Stubs carry `entity_type: ""` (see store_builder::make_stub),
/// so the combined filter excludes every stub by construction. Surfacing
/// the impossibility as a typed warning prevents an agent from reading an
/// empty hit set as "no such stub exists" when in fact no stub could ever
/// satisfy the filter.
fn collect_stub_type_exclusion_warning(scope: &SearchScope, warnings: &mut Vec<WarningHint>) {
    if scope.stub == Some(true)
        && let Some(entity_type) = scope.entity_type.as_deref()
    {
        warnings.push(WarningHint::StubFilterExcludesAll {
            entity_type: entity_type.to_string(),
        });
    }
}

fn collect_equality_filter_warnings(
    filters: &HashMap<String, String>,
    schema: &TypeDefinition,
    scoped_type: Option<&str>,
    scope_mem: Option<&str>,
    mem_schemas: &HashMap<String, Arc<Schema>>,
    warnings: &mut Vec<WarningHint>,
) {
    for (key, value) in filters {
        match schema.metadata_field(key) {
            None => {
                // Field not on the reference type (the scoped type, or the
                // engine fallback type in the unscoped case). Classify it
                // workspace-wide so the warning matches what the application
                // path did: a field declared only as non-filterable is
                // ignored (result = unfiltered) and must report
                // `FIELD_NOT_FILTERABLE`, not an "applied-with-narrowing"
                // code — the fallback type's accident of declaration does
                // not decide the outcome.
                match classify_filter_field(key, scope_mem, mem_schemas, false) {
                    FieldFilterability::DeclaredNotFilterable => {
                        warnings.push(WarningHint::FieldNotFilterable { field: key.clone() });
                    }
                    _ => {
                        let others = find_filter_declaring_types(key, scope_mem, mem_schemas);
                        warnings.push(WarningHint::UnknownFilterKey {
                            key: key.clone(),
                            scoped_type: scoped_type.map(|s| s.to_string()),
                            declared_on_other_types: others,
                        });
                    }
                }
            }
            Some(field_def) if field_def.filterable == Filterable::None => {
                warnings.push(WarningHint::FieldNotFilterable { field: key.clone() });
            }
            Some(field_def) => {
                // Filterable field. A comma-bearing value on a csv-array
                // field can never equal a single member (members are split
                // on comma), so the filter silently matches nothing —
                // surface the shape mismatch and the single-member form
                // (CLI F8). The filter still applies as written.
                if field_def.serialization == Serialization::CsvArray && value.contains(',') {
                    warnings.push(WarningHint::FilterValueMultiMember {
                        key: key.clone(),
                        value: value.clone(),
                    });
                }
                // #52: a value the field's `enum_values` allow-list rejects
                // can never match, so a 0-hit result would otherwise be
                // indistinguishable from a true no-match. Check per-member
                // for csv-array fields (each member is matched singly).
                if let Some(allowed) = field_def.enum_values.as_ref() {
                    let members: Vec<&str> = if field_def.serialization == Serialization::CsvArray {
                        value.split(',').map(str::trim).collect()
                    } else {
                        vec![value.as_str()]
                    };
                    for member in members {
                        if !member.is_empty() && !allowed.iter().any(|a| a == member) {
                            warnings.push(WarningHint::FilterValueNotInEnum {
                                key: key.clone(),
                                value: member.to_string(),
                                allowed: allowed.clone(),
                            });
                        }
                    }
                }
            }
        }
    }
}

fn collect_range_filter_warnings(
    filters: &HashMap<String, String>,
    schema: &TypeDefinition,
    scoped_type: Option<&str>,
    scope_mem: Option<&str>,
    mem_schemas: &HashMap<String, Arc<Schema>>,
    warnings: &mut Vec<WarningHint>,
) {
    for key in filters.keys() {
        let Some((field_name, _)) = parse_range_key(key) else {
            warnings.push(WarningHint::RangeFilterKeyMalformed { key: key.clone() });
            continue;
        };
        match schema.metadata_field(field_name) {
            None => {
                // Classify workspace-wide (range mode) so the warning
                // matches the application path: a field declared only as
                // non-range-filterable is ignored (result = unfiltered) and
                // reports `FIELD_NOT_RANGE_FILTERABLE`, not an
                // applied-with-narrowing code.
                match classify_filter_field(field_name, scope_mem, mem_schemas, true) {
                    FieldFilterability::DeclaredNotFilterable => {
                        warnings.push(WarningHint::FieldNotRangeFilterable {
                            field: field_name.to_string(),
                        });
                    }
                    _ => {
                        let others =
                            find_filter_declaring_types(field_name, scope_mem, mem_schemas);
                        warnings.push(WarningHint::UnknownRangeFilterField {
                            field: field_name.to_string(),
                            key: key.clone(),
                            scoped_type: scoped_type.map(|s| s.to_string()),
                            declared_on_other_types: others,
                        });
                    }
                }
            }
            Some(field_def) if field_def.filterable != Filterable::Range => {
                warnings.push(WarningHint::FieldNotRangeFilterable {
                    field: field_name.to_string(),
                });
            }
            Some(_) => {}
        }
    }
}

/// Resolve `entity_type` to a TypeDefinition by consulting the per-mem
/// schema map first (narrowed to `preferred_mem`'s schema if provided
/// and the type is declared there), then any reachable schema in the
/// map, then the builtin default. Used by both filter dispatch (where
/// the entity's mem drives resolution) and warning collection (where
/// the scope's mem narrows the reachable set).
fn resolve_type(
    entity_type: &str,
    preferred_mem: Option<&str>,
    mem_schemas: &HashMap<String, Arc<Schema>>,
) -> Option<Arc<TypeDefinition>> {
    if let Some(v) = preferred_mem
        && let Some(s) = mem_schemas.get(v)
        && let Some(t) = s.get_type(entity_type)
    {
        return Some(t);
    }
    for s in mem_schemas.values() {
        if let Some(t) = s.get_type(entity_type) {
            return Some(t);
        }
    }
    type_by_name(entity_type)
}

/// Locate every reachable type that declares `key` as a metadata
/// field, regardless of its `filterable` kind. `scope_mem = Some(v)`
/// narrows the search to that mem's pinned schema; `None` scans every
/// schema in `mem_schemas`. Empty return ⇒ no reachable schema
/// declares the filter at all — caller distinguishes the
/// "filter-on-other-type(s)" message from the "no-declaration-anywhere"
/// message based on the list length.
///
/// Multi-type result: when a filter (e.g. `status`) is declared on
/// several types with disjoint enum values, naming only the first
/// match sends the agent toward the wrong type — surface every
/// declaring type so the agent picks the right `--type` scope.
fn find_filter_declaring_types(
    key: &str,
    scope_mem: Option<&str>,
    mem_schemas: &HashMap<String, Arc<Schema>>,
) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut scan = |schema: &Schema| {
        for t in schema.types.values() {
            if t.metadata_field(key).is_some() && !found.contains(&t.name) {
                found.push(t.name.clone());
            }
        }
    };
    match scope_mem {
        Some(v) => {
            if let Some(s) = mem_schemas.get(v) {
                scan(s);
            }
        }
        None => {
            for s in mem_schemas.values() {
                scan(s);
            }
        }
    }
    found.sort();
    found
}

fn compare_numeric(
    val: &crate::entity::MetadataValue,
    filter_str: &str,
    cmp: impl Fn(f64, f64) -> bool,
) -> bool {
    let entity_num = match val {
        crate::entity::MetadataValue::Integer(n) => *n as f64,
        crate::entity::MetadataValue::Float(f) => *f,
        crate::entity::MetadataValue::String(s) => match s.parse::<f64>() {
            Ok(n) => n,
            Err(_) => return false,
        },
        _ => return false,
    };
    let filter_num = match filter_str.parse::<f64>() {
        Ok(n) => n,
        Err(_) => return false,
    };
    cmp(entity_num, filter_num)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{Entity, EntityId, MetadataValue};
    use crate::search_index::MemIndex;
    use crate::store::Store;
    use indexmap::IndexMap;
    use memstead_schema::{Schema, type_by_name};

    fn make_entity(name: &str, mem: &str) -> Entity {
        let mut metadata = IndexMap::new();
        metadata.insert("level".into(), MetadataValue::String("M0".into()));
        metadata.insert("type".into(), MetadataValue::String("spec".into()));
        metadata.insert("tags".into(), MetadataValue::String("backend, api".into()));

        let mut sections = IndexMap::new();
        sections.insert("identity".into(), format!("Identity of {name}."));
        sections.insert("purpose".into(), format!("Purpose of {name}."));

        Entity {
            id: EntityId::new(mem, name),
            title: name.to_string(),
            entity_type: "spec".into(),
            mem: mem.into(),
            file_path: format!("{name}.md"),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: "abc123".into(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    /// Build per-mem tantivy indexes from a store's contents. Used by the
    /// unit tests since the search path now goes through tantivy.
    fn build_test_indexes(
        store: &Store,
    ) -> (HashMap<String, MemIndex>, HashMap<String, Arc<Schema>>) {
        let schema = Schema::builtin_default();
        let mut indexes = HashMap::new();
        let mut schemas = HashMap::new();
        let mems: HashSet<String> = store
            .all_entities()
            .filter(|e| !e.stub)
            .map(|e| e.mem.clone())
            .collect();
        for mem in mems {
            let mut idx = MemIndex::build_in_ram(mem.clone(), Some(&schema)).unwrap();
            for e in store.all_entities().filter(|e| e.mem == mem) {
                idx.index_entity(e).unwrap();
            }
            idx.commit().unwrap();
            indexes.insert(mem.clone(), idx);
            schemas.insert(mem, schema.clone());
        }
        (indexes, schemas)
    }

    fn run_search(store: &Store, scope: &SearchScope) -> SearchResult {
        let (indexes, schemas) = build_test_indexes(store);
        let schema = type_by_name("spec").unwrap();
        search(store, scope, &schema, &indexes, &schemas)
    }

    #[test]
    fn search_by_title() {
        let mut store = Store::new();
        let e1 = make_entity("graph-engine", "specs");
        let e2 = make_entity("mcp-server", "specs");
        store.upsert(e1.id.clone(), e1);
        store.upsert(e2.id.clone(), e2);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["graph".into()],
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].id.name(), "graph-engine");
    }

    #[test]
    fn search_by_section_content() {
        let mut store = Store::new();
        let mut e = make_entity("test-entity", "specs");
        e.sections.insert(
            "identity".into(),
            "Uses the graph database for queries.".into(),
        );
        store.upsert(e.id.clone(), e);

        let scope = SearchScope {
            query: Some(Query {
                phrase: Some("graph database".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
    }

    #[test]
    fn search_with_mem_filter() {
        let mut store = Store::new();
        store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
        store.upsert(EntityId::new("memos", "b"), make_entity("b", "memos"));

        let scope = SearchScope {
            mem: Some("specs".into()),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].mem, "specs");
    }

    #[test]
    fn search_with_equality_filter() {
        let mut store = Store::new();
        let mut e1 = make_entity("m0-entity", "specs");
        e1.metadata
            .insert("level".into(), MetadataValue::String("M0".into()));
        let mut e2 = make_entity("m1-entity", "specs");
        e2.metadata
            .insert("level".into(), MetadataValue::String("M1".into()));
        store.upsert(e1.id.clone(), e1);
        store.upsert(e2.id.clone(), e2);

        let scope = SearchScope {
            filters: HashMap::from([("level".into(), "M0".into())]),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].id.name(), "m0-entity");
        assert!(result.warnings.is_empty(), "no warnings for valid filter");
    }

    #[test]
    fn search_unknown_filter_key_warns_and_keeps_hits() {
        let mut store = Store::new();
        let e1 = make_entity("m0-entity", "specs");
        let e2 = make_entity("m1-entity", "specs");
        store.upsert(e1.id.clone(), e1);
        store.upsert(e2.id.clone(), e2);

        let scope = SearchScope {
            filters: HashMap::from([("stauts".into(), "active".into())]),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(
            result.total, 2,
            "unknown filter should be skipped, not reject all entities"
        );
        assert_eq!(result.warnings.len(), 1);
        assert!(
            result.warnings[0].to_string().contains("stauts") && result.warnings[0].to_string().contains("unknown"),
            "warning mentions unknown key: {:?}",
            result.warnings
        );
    }

    /// F7: a search scoped to entity_type=T with an unknown filter
    /// key must name `T` in the warning, not the schema's default
    /// type. Pre-fix the warning generator used the resolved
    /// `filter_schema.name` (the default type when `T` doesn't
    /// resolve), which read as if the search had been scoped to that
    /// unrelated type and cost an agent round-trip while they
    /// figured out the mismatch.
    #[test]
    fn search_unknown_filter_key_names_scoped_entity_type() {
        let mut store = Store::new();
        let e = make_entity("only", "specs");
        store.upsert(e.id.clone(), e);

        let scope = SearchScope {
            entity_type: Some("contract".into()),
            filters: HashMap::from([("confidence".into(), "verified".into())]),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
        let warning = result.warnings[0].to_string();
        assert!(
            warning.contains("'contract'"),
            "warning must name the agent's scoped type: {warning}",
        );
        assert!(
            !warning.contains("'spec'"),
            "warning must not name an unrelated default type: {warning}",
        );
    }

    /// F7: when the caller did NOT scope the search to any
    /// entity_type, the warning must omit the "for type 'X'" clause
    /// rather than name the schema's default type — the user didn't
    /// ask about any specific type, so naming one in the warning is
    /// misleading.
    #[test]
    fn search_unknown_filter_key_omits_type_when_no_scope() {
        let mut store = Store::new();
        let e = make_entity("only", "specs");
        store.upsert(e.id.clone(), e);

        let scope = SearchScope {
            filters: HashMap::from([("confidence".into(), "verified".into())]),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
        let warning = result.warnings[0].to_string();
        assert!(
            warning.contains("confidence"),
            "warning must name the unknown key: {warning}",
        );
        assert!(
            !warning.contains("for type"),
            "warning must omit the type-name clause when caller didn't scope: {warning}",
        );
    }

    /// F7 (range sibling): the range-filter warning has the same
    /// scoped-type contract as its equality cousin.
    #[test]
    fn search_unknown_range_filter_names_scoped_entity_type() {
        let mut store = Store::new();
        let e = make_entity("only", "specs");
        store.upsert(e.id.clone(), e);

        let scope = SearchScope {
            entity_type: Some("contract".into()),
            range_filters: HashMap::from([("min_priority".into(), "0".into())]),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
        let warning = result.warnings[0].to_string();
        assert!(
            warning.contains("'contract'"),
            "range warning must name the agent's scoped type: {warning}",
        );
        assert!(
            !warning.contains("'spec'"),
            "range warning must not name an unrelated default type: {warning}",
        );
    }

    /// Strict semantics — a filter on `level` (declared by `spec`)
    /// excludes entities whose type doesn't declare the field. A
    /// non-narrowing variant would pass all entities through and the
    /// result would lie about what matched.
    #[test]
    fn search_equality_filter_excludes_types_without_declared_field() {
        let mut store = Store::new();
        // Spec entity with the filter field set — must match.
        let mut spec_match = make_entity("level-m0", "specs");
        spec_match
            .metadata
            .insert("level".into(), MetadataValue::String("M0".into()));
        // Spec entity with the field set to a different value — must
        // be excluded by the value check.
        let mut spec_other = make_entity("level-m1", "specs");
        spec_other
            .metadata
            .insert("level".into(), MetadataValue::String("M1".into()));
        // Memo-typed entity that doesn't declare `level`. It's excluded
        // because the workspace-wide schema knows `level`.
        let mut memo = make_entity("memo-no-level", "specs");
        memo.entity_type = "memo".into();
        store.upsert(spec_match.id.clone(), spec_match.clone());
        store.upsert(spec_other.id.clone(), spec_other);
        store.upsert(memo.id.clone(), memo);

        let scope = SearchScope {
            filters: HashMap::from([("level".into(), "M0".into())]),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(
            result.total, 1,
            "strict filter must keep only the matching spec entity; got {:?}",
            result.hits.iter().map(|h| h.id.to_string()).collect::<Vec<_>>(),
        );
        assert_eq!(result.hits[0].id, spec_match.id);
    }

    /// A workspace-wide-unknown filter key continues to warn and pass
    /// through (no result collapse on a single typo). Companion to the
    /// type-aware exclusion test
    /// above — exercises the unknown-key fallback gate inside
    /// `classify_filter_field` (the `Unknown` verdict passes through).
    #[test]
    fn search_workspace_wide_unknown_filter_passes_through() {
        let mut store = Store::new();
        let e = make_entity("only", "specs");
        store.upsert(e.id.clone(), e);

        let scope = SearchScope {
            filters: HashMap::from([("definitely-not-a-real-field".into(), "x".into())]),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(
            result.total, 1,
            "unknown-anywhere filter key must not collapse the result set; got {:?}",
            result.hits.iter().map(|h| h.id.to_string()).collect::<Vec<_>>(),
        );
        assert!(
            result.warnings.iter().any(|w| w.to_string().contains("definitely-not-a-real-field")),
            "unknown-key warning must still surface: {:?}",
            result.warnings,
        );
    }

    #[test]
    fn search_non_filterable_field_ignored_returns_unfiltered() {
        // MCP F2: a filter on a field declared but marked
        // `Filterable::None` (here, the
        // universal `type` base field) is truly ignored — the result
        // set equals the same search without the filter, NOT an empty
        // set. Pre-fix this branch `return false`d and emptied the set
        // under a "filter ignored" banner; the warning's word and the
        // behaviour disagreed. The `FIELD_NOT_FILTERABLE` warning still
        // fires so the agent knows the filter had no effect.
        let mut store = Store::new();
        store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
        store.upsert(EntityId::new("specs", "b"), make_entity("b", "specs"));

        let scope = SearchScope {
            filters: HashMap::from([("type".into(), "totally-different".into())]),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(
            result.total, 2,
            "non-filterable field filter must be ignored — result equals the unfiltered search, not emptied",
        );
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(
            result.warnings[0].code(),
            "FIELD_NOT_FILTERABLE",
            "non-filterable field must still warn so the agent knows the filter had no effect: {:?}",
            result.warnings,
        );
    }

    /// A search scoped to a type with a filter on a field that type
    /// declares but marks
    /// non-filterable returns the SAME hits as the same search without
    /// the filter — "ignored" means unfiltered, not emptied — plus a
    /// `FIELD_NOT_FILTERABLE` warning. The code/effect is coherent in
    /// the scoped shape just as in the unscoped shape above.
    #[test]
    fn search_scoped_non_filterable_field_matches_unfiltered() {
        let mut store = Store::new();
        store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
        store.upsert(EntityId::new("specs", "b"), make_entity("b", "specs"));

        let baseline = run_search(
            &store,
            &SearchScope {
                entity_type: Some("spec".into()),
                ..Default::default()
            },
        );
        let filtered = run_search(
            &store,
            &SearchScope {
                entity_type: Some("spec".into()),
                filters: HashMap::from([("type".into(), "irrelevant".into())]),
                ..Default::default()
            },
        );
        assert_eq!(
            filtered.total, baseline.total,
            "non-filterable filter must leave the scoped result set identical to the unfiltered search",
        );
        assert_eq!(filtered.total, 2);
        assert!(
            filtered.warnings.iter().any(|w| w.code() == "FIELD_NOT_FILTERABLE"),
            "scoped non-filterable filter must warn FIELD_NOT_FILTERABLE: {:?}",
            filtered.warnings,
        );
    }

    /// An unscoped filter on a field that IS filterable on some type
    /// (here `maturity` on
    /// `concept`) narrows the result to the declaring type and carries
    /// `FILTER_TYPE_SCOPED` — a code distinct from the truly-unknown-key
    /// code, so a consumer branching on `code` alone learns the filter
    /// took effect.
    #[test]
    fn search_unscoped_filterable_field_narrows_with_distinct_code() {
        let mut store = Store::new();
        // Two concept entities, one matching the filter value.
        let mut c_match = make_entity("c-emerging", "specs");
        c_match.entity_type = "concept".into();
        c_match.metadata.insert("maturity".into(), MetadataValue::String("emerging".into()));
        let mut c_other = make_entity("c-stable", "specs");
        c_other.entity_type = "concept".into();
        c_other.metadata.insert("maturity".into(), MetadataValue::String("stable".into()));
        // A spec entity that doesn't declare `maturity` — narrowed away.
        let spec = make_entity("s", "specs");
        store.upsert(c_match.id.clone(), c_match.clone());
        store.upsert(c_other.id.clone(), c_other);
        store.upsert(spec.id.clone(), spec);

        let result = run_search(
            &store,
            &SearchScope {
                filters: HashMap::from([("maturity".into(), "emerging".into())]),
                ..Default::default()
            },
        );
        assert_eq!(result.total, 1, "only the matching concept survives the narrowing");
        assert_eq!(result.hits[0].id, c_match.id);
        assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
        assert_eq!(
            result.warnings[0].code(),
            "FILTER_TYPE_SCOPED",
            "applied-with-narrowing must carry a code distinct from UNKNOWN_FILTER_KEY: {:?}",
            result.warnings,
        );
    }

    /// MCP F3: an UNSCOPED filter on a field that is declared only as
    /// **non-filterable** (`source_quality`
    /// on `assertion`, `Filterable::None`) is ignored, not type-narrowed —
    /// the result equals the same search without the filter (the spec
    /// entities are retained, not silently dropped to the declaring type) —
    /// and the warning reports `FIELD_NOT_FILTERABLE`, not the
    /// `FILTER_TYPE_SCOPED` "applied-with-narrowing" code it carried pre-fix
    /// (which lied: no value predicate ever ran). Filterability, not the
    /// fallback type's accident of declaration, decides the outcome.
    #[test]
    fn search_unscoped_non_filterable_field_ignored_not_narrowed() {
        let mut store = Store::new();
        let mut assertion = make_entity("a-claim", "specs");
        assertion.entity_type = "assertion".into();
        assertion
            .metadata
            .insert("source_quality".into(), MetadataValue::String("experimental".into()));
        store.upsert(assertion.id.clone(), assertion);
        store.upsert(EntityId::new("specs", "s1"), make_entity("s1", "specs"));
        store.upsert(EntityId::new("specs", "s2"), make_entity("s2", "specs"));

        let baseline = run_search(&store, &SearchScope::default());

        // Both a wrong value and the assertion's real value must return the
        // same set as the unfiltered baseline — the filter is ignored, the
        // value is never matched. This is the discriminator that separates
        // "ignored" from "narrowed".
        for value in ["WRONG-VALUE", "experimental"] {
            let result = run_search(
                &store,
                &SearchScope {
                    filters: HashMap::from([("source_quality".into(), value.into())]),
                    ..Default::default()
                },
            );
            assert_eq!(
                result.total, baseline.total,
                "non-filterable filter (value={value}) must return the unfiltered set, not narrow to the declaring type",
            );
            assert_eq!(result.total, 3);
            assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
            assert_eq!(
                result.warnings[0].code(),
                "FIELD_NOT_FILTERABLE",
                "unscoped non-filterable field must report FIELD_NOT_FILTERABLE, not FILTER_TYPE_SCOPED: {:?}",
                result.warnings,
            );
        }
    }

    /// MCP F4 (range): an UNSCOPED range filter on a field no type
    /// declares as range-filterable does
    /// not drop the types that lack the field. `level` is `Filterable::
    /// Equality` on `spec`; a `min_level` range filter is ignored, so a
    /// `memo` entity (which doesn't declare `level`) is retained rather than
    /// silently narrowed away — the warning's "ignored" word now matches the
    /// result set.
    #[test]
    fn search_unscoped_non_range_filterable_field_not_dropped() {
        let mut store = Store::new();
        store.upsert(EntityId::new("specs", "s1"), make_entity("s1", "specs"));
        let mut memo = make_entity("m1", "specs");
        memo.entity_type = "memo".into();
        memo.metadata.shift_remove("level");
        store.upsert(memo.id.clone(), memo);

        let result = run_search(
            &store,
            &SearchScope {
                range_filters: HashMap::from([("min_level".into(), "M0".into())]),
                ..Default::default()
            },
        );
        assert_eq!(
            result.total, 2,
            "non-range-filterable range filter must not drop the memo lacking the field; got {:?}",
            result.hits.iter().map(|h| h.id.to_string()).collect::<Vec<_>>(),
        );
        assert!(
            result.warnings.iter().any(|w| w.code() == "FIELD_NOT_RANGE_FILTERABLE"),
            "must warn FIELD_NOT_RANGE_FILTERABLE: {:?}",
            result.warnings,
        );
    }

    /// Range warning, fallback-type independence: an unscoped range
    /// filter on a field the engine fallback type does NOT declare but
    /// another type declares as
    /// equality-only (`maturity` on `concept`) reports
    /// `FIELD_NOT_RANGE_FILTERABLE` — keyed on workspace-wide
    /// range-filterability, not on whether the fallback type happens to
    /// declare it (pre-fix it emitted `RANGE_FILTER_TYPE_SCOPED`).
    #[test]
    fn search_unscoped_range_on_equality_only_other_type_field() {
        let mut store = Store::new();
        let mut concept = make_entity("c1", "specs");
        concept.entity_type = "concept".into();
        concept.metadata.insert("maturity".into(), MetadataValue::String("stable".into()));
        store.upsert(concept.id.clone(), concept);
        store.upsert(EntityId::new("specs", "s1"), make_entity("s1", "specs"));

        let result = run_search(
            &store,
            &SearchScope {
                range_filters: HashMap::from([("min_maturity".into(), "stable".into())]),
                ..Default::default()
            },
        );
        assert_eq!(
            result.total, 2,
            "non-range-filterable field range filter must leave the set unfiltered",
        );
        assert!(
            result.warnings.iter().any(|w| w.code() == "FIELD_NOT_RANGE_FILTERABLE"),
            "must warn FIELD_NOT_RANGE_FILTERABLE (not RANGE_FILTER_TYPE_SCOPED): {:?}",
            result.warnings,
        );
    }

    /// A truly-unknown filter key (no reachable schema declares it) runs
    /// the query unfiltered
    /// and carries `UNKNOWN_FILTER_KEY` — the only "ignored" code whose
    /// result set equals the unfiltered search via an unknown key.
    #[test]
    fn search_truly_unknown_key_ignored_with_unknown_code() {
        let mut store = Store::new();
        store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
        store.upsert(EntityId::new("specs", "b"), make_entity("b", "specs"));

        let result = run_search(
            &store,
            &SearchScope {
                filters: HashMap::from([("boguskey".into(), "x".into())]),
                ..Default::default()
            },
        );
        assert_eq!(result.total, 2, "truly-unknown key must leave the result set unfiltered");
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(
            result.warnings[0].code(),
            "UNKNOWN_FILTER_KEY",
            "a key no schema declares must carry UNKNOWN_FILTER_KEY: {:?}",
            result.warnings,
        );
    }

    /// Range complement: a range filter on a field declared but not
    /// range-filterable
    /// (`level` is `filterable: equality`) is truly ignored — the
    /// result equals the same search without it — and carries
    /// `FIELD_NOT_RANGE_FILTERABLE`, not a silent empty.
    #[test]
    fn search_non_range_filterable_field_ignored_returns_unfiltered() {
        let mut store = Store::new();
        store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
        store.upsert(EntityId::new("specs", "b"), make_entity("b", "specs"));

        let result = run_search(
            &store,
            &SearchScope {
                entity_type: Some("spec".into()),
                range_filters: HashMap::from([("min_level".into(), "M0".into())]),
                ..Default::default()
            },
        );
        assert_eq!(
            result.total, 2,
            "non-range-filterable field range filter must be ignored, not empty the set",
        );
        assert!(
            result.warnings.iter().any(|w| w.code() == "FIELD_NOT_RANGE_FILTERABLE"),
            "must warn FIELD_NOT_RANGE_FILTERABLE: {:?}",
            result.warnings,
        );
    }

    #[test]
    fn search_range_filter_unknown_field_warns() {
        let mut store = Store::new();
        let e = make_entity("only", "specs");
        store.upsert(e.id.clone(), e);

        let scope = SearchScope {
            range_filters: HashMap::from([("min_nonexistent".into(), "0".into())]),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(
            result.total, 1,
            "unknown range field should be skipped, not reject all entities"
        );
        assert_eq!(result.warnings.len(), 1);
        assert!(
            result.warnings[0].to_string().contains("nonexistent"),
            "warning mentions unknown range field: {:?}",
            result.warnings
        );
    }

    #[test]
    fn list_unknown_filter_key_warns() {
        let mut store = Store::new();
        store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));

        let schema = type_by_name("spec").unwrap();
        let scope = SearchScope {
            filters: HashMap::from([("nope".into(), "x".into())]),
            ..Default::default()
        };

        let schemas: HashMap<String, Arc<Schema>> = HashMap::new();
        let result = list(&store, &scope, &schema, &schemas);
        assert_eq!(result.total, 1);
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn token_budget_trims_overflowing_page_and_warns() {
        let mut store = Store::new();
        for i in 0..20 {
            let mut e = make_entity(&format!("entity-{i:02}"), "specs");
            e.sections.insert("identity".into(), "graph ".repeat(50));
            store.upsert(e.id.clone(), e);
        }

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["graph".into()],
                ..Default::default()
            }),
            // Tiny budget: a single hit already exceeds it, so the page must
            // trim to exactly one and warn.
            token_budget: Some(20),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(result.total, 20, "total reflects the full match count");
        assert!(result.returned >= 1, "at least one hit always returns");
        assert!(result.returned < 20, "the page was trimmed by the budget");
        assert_eq!(result.hits.len(), result.returned);
        let trunc = result
            .warnings
            .iter()
            .find(|w| w.code() == "SEARCH_RESULTS_TRUNCATED")
            .expect("budget trim emits SEARCH_RESULTS_TRUNCATED");
        assert!(trunc.message().contains("budget"));
    }

    #[test]
    fn ample_budget_returns_all_hits_without_warning() {
        let mut store = Store::new();
        for i in 0..5 {
            let mut e = make_entity(&format!("entity-{i}"), "specs");
            e.sections.insert("identity".into(), "graph".into());
            store.upsert(e.id.clone(), e);
        }
        let scope = SearchScope {
            query: Some(Query {
                any: vec!["graph".into()],
                ..Default::default()
            }),
            token_budget: Some(1_000_000),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.returned, 5);
        assert!(
            result
                .warnings
                .iter()
                .all(|w| w.code() != "SEARCH_RESULTS_TRUNCATED"),
            "an ample budget does not trim"
        );
    }

    #[test]
    fn search_hits_carry_no_section_bodies() {
        let mut store = Store::new();
        store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
        let scope = SearchScope {
            query: Some(Query {
                any: vec!["Identity".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        assert!(
            result.hits[0].sections.is_empty(),
            "search hits ship no section bodies — read them with memstead_entity"
        );
        // The lead-section summary is still resolved from the entity.
        assert!(result.hits[0].summary.is_some(), "summary still resolved");
    }

    #[test]
    fn list_hits_still_carry_section_bodies() {
        let mut store = Store::new();
        store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
        let schema = type_by_name("spec").unwrap();
        let schemas: HashMap<String, Arc<Schema>> = HashMap::new();
        let result = list(&store, &SearchScope::default(), &schema, &schemas);
        assert_eq!(result.total, 1);
        assert!(
            !result.hits[0].sections.is_empty(),
            "list hits keep section bodies for human-facing roster consumers"
        );
    }

    #[test]
    fn search_csv_array_filter() {
        let mut store = Store::new();
        let mut e = make_entity("tagged", "specs");
        e.metadata.insert(
            "tags".into(),
            MetadataValue::String("backend, api, rust".into()),
        );
        store.upsert(e.id.clone(), e);

        let scope = SearchScope {
            filters: HashMap::from([("tags".into(), "api".into())]),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
    }

    #[test]
    fn search_pagination() {
        let mut store = Store::new();
        for i in 0..10 {
            let e = make_entity(&format!("entity-{i:02}"), "specs");
            store.upsert(e.id.clone(), e);
        }

        let scope = SearchScope {
            limit: Some(3),
            offset: Some(2),
            ..Default::default()
        };

        let result = run_search(&store, &scope);
        assert_eq!(result.total, 10);
        assert_eq!(result.returned, 3);
        assert_eq!(result.offset, 2);
    }

    #[test]
    fn list_entities() {
        let mut store = Store::new();
        store.upsert(EntityId::new("specs", "a"), make_entity("a", "specs"));
        store.upsert(EntityId::new("specs", "b"), make_entity("b", "specs"));

        let schema = type_by_name("spec").unwrap();
        let scope = SearchScope::default();

        let schemas: HashMap<String, Arc<Schema>> = HashMap::new();
        let result = list(&store, &scope, &schema, &schemas);
        assert_eq!(result.total, 2);
        assert!(result.total_tokens > 0);
    }

    #[test]
    fn build_snippet_basic() {
        let content = "The graph engine processes queries efficiently.";
        let snippet = build_snippet(content, "engine");
        assert!(snippet.contains("**engine**"));
    }

    // ---- Structured-query semantics ----

    #[test]
    fn query_any_or_semantics() {
        let mut store = Store::new();
        let mut a = make_entity("a", "specs");
        a.sections.insert("identity".into(), "authentication flow".into());
        let mut b = make_entity("b", "specs");
        b.sections.insert("identity".into(), "login pipeline".into());
        let mut c = make_entity("c", "specs");
        c.sections.insert("identity".into(), "unrelated subject".into());
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);
        store.upsert(c.id.clone(), c);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["authentication".into(), "login".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let names: Vec<_> = result
            .hits
            .iter()
            .map(|h| h.id.name().to_string())
            .collect();
        assert_eq!(result.total, 2, "union of any terms: {names:?}");
        assert!(names.contains(&"a".to_string()));
        assert!(names.contains(&"b".to_string()));
    }

    #[test]
    fn query_not_excludes() {
        let mut store = Store::new();
        let mut a = make_entity("a", "specs");
        a.sections.insert("identity".into(), "uses authentication".into());
        let mut b = make_entity("b", "specs");
        b.sections
            .insert("identity".into(), "uses authentication mock".into());
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["authentication".into()],
                not: vec!["mock".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].id.name(), "a");
    }

    #[test]
    fn query_phrase_match() {
        let mut store = Store::new();
        let mut a = make_entity("a", "specs");
        a.sections
            .insert("identity".into(), "the client side agent runs locally".into());
        let mut b = make_entity("b", "specs");
        b.sections.insert(
            "identity".into(),
            "the client invokes the side channel for the agent".into(),
        );
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);

        let scope = SearchScope {
            query: Some(Query {
                phrase: Some("client side agent".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].id.name(), "a");
    }

    #[test]
    fn query_field_restricted() {
        let mut store = Store::new();
        let mut a = make_entity("a", "specs");
        a.sections.insert("identity".into(), "foo content".into());
        let mut b = make_entity("b", "specs");
        b.sections.insert("purpose".into(), "foo content".into());
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["foo".into()],
                field: Some("identity".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].id.name(), "a");
    }

    #[test]
    fn query_empty_is_metadata_filter() {
        let mut store = Store::new();
        let mut memo_entity = make_entity("m", "specs");
        memo_entity.entity_type = "memo".into();
        store.upsert(memo_entity.id.clone(), memo_entity);
        store.upsert(EntityId::new("specs", "s1"), make_entity("s1", "specs"));
        store.upsert(EntityId::new("specs", "s2"), make_entity("s2", "specs"));

        let scope = SearchScope {
            query: Some(Query::default()),
            entity_type: Some("spec".into()),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 2, "empty query ⇒ metadata filter over entity_type");
    }

    #[test]
    fn query_diacritic_folding() {
        let mut store = Store::new();
        let mut a = make_entity("a", "specs");
        a.sections
            .insert("identity".into(), "schöne Häuser".into());
        store.upsert(a.id.clone(), a);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["hauser".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
    }

    #[test]
    fn query_spans_all_mems_when_mem_none() {
        let mut store = Store::new();
        let mut a = make_entity("a", "specs");
        a.sections.insert("identity".into(), "foo".into());
        let mut b = make_entity("b", "memos");
        b.sections.insert("identity".into(), "foo".into());
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["foo".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 2);
    }

    #[test]
    fn query_targets_single_mem_when_named() {
        let mut store = Store::new();
        let mut a = make_entity("a", "specs");
        a.sections.insert("identity".into(), "foo".into());
        let mut b = make_entity("b", "memos");
        b.sections.insert("identity".into(), "foo".into());
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["foo".into()],
                ..Default::default()
            }),
            mem: Some("memos".into()),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].mem, "memos");
    }

    // ---- matched_terms + score_breakdown + heading_path ----

    use crate::entity::HeadingSpan;

    #[test]
    fn matched_terms_populated_for_any() {
        let mut store = Store::new();
        let mut a = make_entity("a", "specs");
        a.sections
            .insert("identity".into(), "auth flow uses oidc sessions".into());
        store.upsert(a.id.clone(), a);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["auth".into(), "oidc".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        let hit = &result.hits[0];
        let mt = hit.matched_terms.as_ref().expect("matched_terms populated");
        assert!(mt.contains_key("auth"), "auth keyed: {mt:?}");
        assert!(mt.contains_key("oidc"), "oidc keyed: {mt:?}");
    }

    #[test]
    fn matched_terms_per_field() {
        let mut store = Store::new();
        let mut a = make_entity("graph-engine", "specs");
        a.sections
            .insert("identity".into(), "graph-engine uses graph primitives".into());
        store.upsert(a.id.clone(), a);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["graph".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let hit = &result.hits[0];
        let mt = hit.matched_terms.as_ref().unwrap();
        let fields: Vec<&str> = mt["graph"].iter().map(|tm| tm.field.as_str()).collect();
        assert!(fields.contains(&"title"), "title field: {fields:?}");
        assert!(fields.contains(&"identity"), "identity field: {fields:?}");
    }

    #[test]
    fn matched_terms_excludes_not_terms() {
        let mut store = Store::new();
        let mut a = make_entity("a", "specs");
        a.sections
            .insert("identity".into(), "uses authentication".into());
        store.upsert(a.id.clone(), a);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["authentication".into()],
                not: vec!["mock".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let hit = &result.hits[0];
        let mt = hit.matched_terms.as_ref().unwrap();
        assert!(mt.contains_key("authentication"));
        assert!(
            !mt.contains_key("mock"),
            "negative predicate must not populate matched_terms: {mt:?}"
        );
    }

    #[test]
    fn score_breakdown_sums_to_score() {
        let mut store = Store::new();
        let mut a = make_entity("a", "specs");
        a.sections
            .insert("identity".into(), "graph engine core".into());
        store.upsert(a.id.clone(), a);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["graph".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let hit = &result.hits[0];
        let br = hit.score_breakdown.as_ref().expect("breakdown populated");
        let sum: f32 = br.bm25 + br.title_boost + br.field_weights.values().sum::<f32>();
        assert!(
            (sum - hit.score).abs() < 0.01,
            "components should sum to score: sum={sum} score={}",
            hit.score
        );
    }

    #[test]
    fn phrase_snippet_contains_full_phrase() {
        let mut store = Store::new();
        let mut a = make_entity("a", "specs");
        a.sections.insert(
            "identity".into(),
            "the client side agent runs locally".into(),
        );
        store.upsert(a.id.clone(), a);

        let scope = SearchScope {
            query: Some(Query {
                phrase: Some("client side agent".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let hit = &result.hits[0];
        let mt = hit.matched_terms.as_ref().unwrap();
        let matches = mt
            .get("client side agent")
            .expect("phrase term keyed in matched_terms");
        let identity_snippet = matches
            .iter()
            .find(|tm| tm.field == "identity")
            .expect("phrase matched in identity");
        assert!(
            identity_snippet.snippet.contains("client side agent"),
            "snippet must contain full phrase: {}",
            identity_snippet.snippet
        );
    }

    fn entity_with_heading_spans(
        name: &str,
        section_key: &str,
        content: &str,
        spans: Vec<HeadingSpan>,
    ) -> Entity {
        let mut e = make_entity(name, "specs");
        e.sections
            .insert(section_key.to_string(), content.to_string());
        e.heading_spans
            .insert(section_key.to_string(), spans);
        e
    }

    #[test]
    fn heading_path_none_when_match_above_first_subheading() {
        // H3 starts at offset 20 in the section content; match "anchor" is at offset 4 (before).
        let content = "the anchor word here\n### Later Heading\nmore text";
        let h3_offset = content.find("### Later Heading").unwrap();
        let spans = vec![HeadingSpan {
            level: 3,
            title: "Later Heading".into(),
            start_offset: h3_offset,
            end_offset: content.len(),
        }];
        let mut store = Store::new();
        store.upsert(
            EntityId::new("specs", "a"),
            entity_with_heading_spans("a", "identity", content, spans),
        );

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["anchor".into()],
                field: Some("identity".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let mt = result.hits[0].matched_terms.as_ref().unwrap();
        let tm = &mt["anchor"][0];
        assert!(
            tm.heading_path.is_none(),
            "match above first subheading ⇒ no heading_path: {:?}",
            tm.heading_path
        );
    }

    #[test]
    fn heading_path_single_level() {
        // Match under one H3.
        let content = "### Response Shapes\nhandles unique keyword here\n";
        let spans = vec![HeadingSpan {
            level: 3,
            title: "Response Shapes".into(),
            start_offset: 0,
            end_offset: content.len(),
        }];
        let mut store = Store::new();
        store.upsert(
            EntityId::new("specs", "a"),
            entity_with_heading_spans("a", "identity", content, spans),
        );

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["unique".into()],
                field: Some("identity".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let mt = result.hits[0].matched_terms.as_ref().unwrap();
        let tm = &mt["unique"][0];
        assert_eq!(
            tm.heading_path,
            Some(vec!["Response Shapes".into()]),
            "single-level path under one H3"
        );
    }

    #[test]
    fn heading_path_nested_h3_h4() {
        // Section content:
        //   ### Response Shapes
        //   some text
        //   #### Markdown Output
        //   match distinct-keyword here
        let mut content = String::new();
        content.push_str("### Response Shapes\n");
        content.push_str("some text\n");
        let h4_start = content.len();
        content.push_str("#### Markdown Output\n");
        let payload_start = content.len();
        content.push_str("distinct-keyword is below\n");
        let spans = vec![
            HeadingSpan {
                level: 3,
                title: "Response Shapes".into(),
                start_offset: 0,
                end_offset: content.len(),
            },
            HeadingSpan {
                level: 4,
                title: "Markdown Output".into(),
                start_offset: h4_start,
                end_offset: content.len(),
            },
        ];
        let _ = payload_start;
        let mut store = Store::new();
        store.upsert(
            EntityId::new("specs", "a"),
            entity_with_heading_spans("a", "identity", &content, spans),
        );

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["distinct-keyword".into()],
                field: Some("identity".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let mt = result.hits[0].matched_terms.as_ref().unwrap();
        let tm = &mt["distinct-keyword"][0];
        assert_eq!(
            tm.heading_path,
            Some(vec!["Response Shapes".into(), "Markdown Output".into()]),
            "nested path: outermost (H3) first, innermost (H4) last"
        );
    }

    #[test]
    fn heading_path_survives_level_skip() {
        // H2 → H4 directly (no H3). Only the H4 span exists.
        let content = "#### Direct Subsection\nrare-match word here\n";
        let spans = vec![HeadingSpan {
            level: 4,
            title: "Direct Subsection".into(),
            start_offset: 0,
            end_offset: content.len(),
        }];
        let mut store = Store::new();
        store.upsert(
            EntityId::new("specs", "a"),
            entity_with_heading_spans("a", "identity", content, spans),
        );

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["rare-match".into()],
                field: Some("identity".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let mt = result.hits[0].matched_terms.as_ref().unwrap();
        let tm = &mt["rare-match"][0];
        assert_eq!(
            tm.heading_path,
            Some(vec!["Direct Subsection".into()]),
            "flat H4 span produces single-element path; no virtual H3 inserted"
        );
    }

    #[test]
    fn heading_path_distinguishes_duplicate_siblings() {
        // Two `### Foo` under the same section; match in second one → path
        // carries "Foo" from the second span (same title, distinguished by
        // offset containment).
        let mut content = String::new();
        content.push_str("### Foo\nfirst body\n");
        let second_start = content.len();
        content.push_str("### Foo\nsecond body carries sentinel-word here\n");
        let spans = vec![
            HeadingSpan {
                level: 3,
                title: "Foo".into(),
                start_offset: 0,
                end_offset: second_start,
            },
            HeadingSpan {
                level: 3,
                title: "Foo".into(),
                start_offset: second_start,
                end_offset: content.len(),
            },
        ];
        let mut store = Store::new();
        store.upsert(
            EntityId::new("specs", "a"),
            entity_with_heading_spans("a", "identity", &content, spans),
        );

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["sentinel-word".into()],
                field: Some("identity".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let mt = result.hits[0].matched_terms.as_ref().unwrap();
        let tm = &mt["sentinel-word"][0];
        assert_eq!(
            tm.heading_path,
            Some(vec!["Foo".into()]),
            "second `### Foo` span contains the match (offset-based)"
        );
    }

    // ---- Facets ----

    #[test]
    fn facets_count_over_full_result_not_page() {
        // 12 matching entities; page limit 5. Facets must reflect all 12.
        let mut store = Store::new();
        for i in 0..12 {
            let mut e = make_entity(&format!("e-{i:02}"), "specs");
            e.sections
                .insert("identity".into(), "shared-keyword here".into());
            store.upsert(e.id.clone(), e);
        }

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["shared-keyword".into()],
                ..Default::default()
            }),
            limit: Some(5),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 12);
        assert_eq!(result.returned, 5);
        let facets = result.facets.as_ref().expect("facets present");
        let by_type_sum: usize = facets.by_type.values().sum();
        assert_eq!(
            by_type_sum, 12,
            "by_type must cover the full unpaginated set, not just the page"
        );
        let by_mem_sum: usize = facets.by_mem.values().sum();
        assert_eq!(by_mem_sum, 12);
    }

    #[test]
    fn facets_by_type_and_mem_exact() {
        let mut store = Store::new();
        // 3 specs in 'specs', 2 memos in 'memos'.
        for i in 0..3 {
            let mut e = make_entity(&format!("s-{i}"), "specs");
            e.sections
                .insert("identity".into(), "shared anchor".into());
            store.upsert(e.id.clone(), e);
        }
        for i in 0..2 {
            let mut e = make_entity(&format!("m-{i}"), "memos");
            e.entity_type = "memo".into();
            e.sections
                .insert("identity".into(), "shared anchor".into());
            store.upsert(e.id.clone(), e);
        }

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["anchor".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let facets = result.facets.as_ref().unwrap();
        assert_eq!(facets.by_type.get("spec").copied(), Some(3));
        assert_eq!(facets.by_type.get("memo").copied(), Some(2));
        assert_eq!(facets.by_mem.get("specs").copied(), Some(3));
        assert_eq!(facets.by_mem.get("memos").copied(), Some(2));
        // Without graph expansion every hit is primary; no `expanded`
        // dim is populated.
        assert_eq!(facets.by_expansion.get("primary").copied(), Some(5));
        assert!(!facets.by_expansion.contains_key("expanded"));
    }

    #[test]
    fn facets_empty_when_no_hits() {
        let mut store = Store::new();
        let e = make_entity("lonely", "specs");
        store.upsert(e.id.clone(), e);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["never-occurs-keyword".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 0);
        let facets = result
            .facets
            .as_ref()
            .expect("facets is Some(Facets::default()) even when hit set is empty");
        assert!(facets.by_type.is_empty());
        assert!(facets.by_mem.is_empty());
        assert!(facets.by_level.is_empty());
        assert!(facets.by_subsection.is_empty());
        assert!(facets.by_expansion.is_empty());
    }

    #[test]
    fn facets_by_subsection_exact() {
        // Two hits both matching under two distinct sub-sections.
        let content_a = "### Response Shapes\nentity-a unique-anchor here\n";
        let spans_a = vec![HeadingSpan {
            level: 3,
            title: "Response Shapes".into(),
            start_offset: 0,
            end_offset: content_a.len(),
        }];
        let content_b = "### Tool Surface\nentity-b unique-anchor here\n";
        let spans_b = vec![HeadingSpan {
            level: 3,
            title: "Tool Surface".into(),
            start_offset: 0,
            end_offset: content_b.len(),
        }];
        let mut store = Store::new();
        store.upsert(
            EntityId::new("specs", "a"),
            entity_with_heading_spans("a", "identity", content_a, spans_a),
        );
        store.upsert(
            EntityId::new("specs", "b"),
            entity_with_heading_spans("b", "identity", content_b, spans_b),
        );

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["unique-anchor".into()],
                field: Some("identity".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let facets = result.facets.as_ref().unwrap();
        assert_eq!(facets.by_subsection.len(), 2);
        let paths: std::collections::HashSet<Vec<String>> = facets
            .by_subsection
            .iter()
            .map(|e| e.path.clone())
            .collect();
        assert!(paths.contains(&vec!["identity".into(), "Response Shapes".into()]));
        assert!(paths.contains(&vec!["identity".into(), "Tool Surface".into()]));
        for entry in &facets.by_subsection {
            assert_eq!(entry.count, 1);
        }
    }

    #[test]
    fn facets_by_subsection_excludes_h2_only_matches() {
        // Match falls inside an H2 section that has no H3–H6 spans. No
        // `by_subsection` entry should appear for it.
        let mut store = Store::new();
        let mut e = make_entity("a", "specs");
        e.sections
            .insert("identity".into(), "only-here unique-keyword lives".into());
        store.upsert(e.id.clone(), e);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["unique-keyword".into()],
                field: Some("identity".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        let facets = result.facets.as_ref().unwrap();
        assert!(
            facets.by_subsection.is_empty(),
            "H2-only match must not contribute to by_subsection: {:?}",
            facets.by_subsection
        );
    }

    #[test]
    fn facets_by_subsection_survives_punctuation_in_heading() {
        // A heading containing a slash must not be split by a delimiter.
        let content = "### Client/Server split\nword punctuation-anchor exists\n";
        let spans = vec![HeadingSpan {
            level: 3,
            title: "Client/Server split".into(),
            start_offset: 0,
            end_offset: content.len(),
        }];
        let mut store = Store::new();
        store.upsert(
            EntityId::new("specs", "a"),
            entity_with_heading_spans("a", "identity", content, spans),
        );

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["punctuation-anchor".into()],
                field: Some("identity".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let facets = result.facets.as_ref().unwrap();
        assert_eq!(facets.by_subsection.len(), 1);
        let entry = &facets.by_subsection[0];
        assert_eq!(entry.count, 1);
        assert_eq!(
            entry.path,
            vec!["identity".to_string(), "Client/Server split".to_string()],
            "punctuation in heading must remain a single path element"
        );
    }

    #[test]
    fn facets_by_level_counts_when_present() {
        let mut store = Store::new();
        let mut e1 = make_entity("a", "specs");
        e1.metadata
            .insert("level".into(), MetadataValue::String("M0".into()));
        e1.sections.insert("identity".into(), "shared anchor".into());
        let mut e2 = make_entity("b", "specs");
        e2.metadata
            .insert("level".into(), MetadataValue::String("M1".into()));
        e2.sections.insert("identity".into(), "shared anchor".into());
        let mut e3 = make_entity("c", "specs");
        e3.metadata
            .insert("level".into(), MetadataValue::String("M1".into()));
        e3.sections.insert("identity".into(), "shared anchor".into());
        store.upsert(e1.id.clone(), e1);
        store.upsert(e2.id.clone(), e2);
        store.upsert(e3.id.clone(), e3);

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["anchor".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let facets = result.facets.as_ref().unwrap();
        assert_eq!(facets.by_level.get("M0").copied(), Some(1));
        assert_eq!(facets.by_level.get("M1").copied(), Some(2));
    }

    // ---- Graph expansion via expand_via ----

    use crate::store::{Edge, EdgeSource};

    fn add_edge(store: &mut Store, from: EntityId, to: EntityId, rel: &str) {
        store.add_edge(
            from,
            Edge {
                rel_type: rel.into(),
                target: to,
                source: EdgeSource::Explicit,
            },
        );
    }

    /// An auto-emitted mention edge (`EdgeSource::BodyLink`) — a co-mention,
    /// not a typed dependency.
    fn add_body_edge(store: &mut Store, from: EntityId, to: EntityId) {
        store.add_edge(
            from,
            Edge {
                rel_type: "REFERENCES".into(),
                target: to,
                source: EdgeSource::BodyLink,
            },
        );
    }

    /// #54: a `related_to` neighbourhood ranks by proximity — nearer hops
    /// first, and a typed (dependency) link to the anchor before a
    /// co-mention at the same hop. A small neighbourhood keeps full
    /// membership (only ordering changes — the refusal AC).
    #[test]
    fn related_to_ranks_by_proximity_then_typed() {
        let mut store = Store::new();
        for n in ["hub", "dep1", "men1", "far1"] {
            let e = make_entity(n, "specs");
            store.upsert(e.id.clone(), e);
        }
        let hub = EntityId::new("specs", "hub");
        // hub —USES→ dep1 (typed, dist 1); hub —REFERENCES(mention)→ men1
        // (dist 1); dep1 —USES→ far1 (dist 2 from hub).
        add_edge(&mut store, hub.clone(), EntityId::new("specs", "dep1"), "USES");
        add_body_edge(&mut store, hub.clone(), EntityId::new("specs", "men1"));
        add_edge(
            &mut store,
            EntityId::new("specs", "dep1"),
            EntityId::new("specs", "far1"),
            "USES",
        );

        let scope = SearchScope {
            related_to: Some(hub.clone()),
            depth: Some(2),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        // Membership unchanged: hub(0) + dep1,men1(1) + far1(2) — all 4.
        let order: Vec<&str> = result.hits.iter().map(|h| h.id.name()).collect();
        assert_eq!(result.total, 4, "small neighbourhood keeps full membership: {order:?}");
        let pos = |n: &str| order.iter().position(|x| *x == n).unwrap();
        assert!(pos("dep1") < pos("far1"), "nearer before farther: {order:?}");
        assert!(pos("men1") < pos("far1"), "nearer before farther: {order:?}");
        assert!(
            pos("dep1") < pos("men1"),
            "typed link before co-mention at the same hop: {order:?}"
        );
    }

    /// #54: a hub neighbourhood larger than the cap is bounded to its
    /// nearest N with a `NEIGHBOURHOOD_CAPPED` warning.
    #[test]
    fn related_to_hub_is_capped_with_warning() {
        let mut store = Store::new();
        let hub = EntityId::new("specs", "hub");
        store.upsert(hub.clone(), make_entity("hub", "specs"));
        for i in 0..150 {
            let n = format!("n{i:03}");
            let id = EntityId::new("specs", &n);
            store.upsert(id.clone(), make_entity(&n, "specs"));
            add_edge(&mut store, hub.clone(), id, "USES");
        }
        let scope = SearchScope {
            related_to: Some(hub.clone()),
            depth: Some(1),
            limit: Some(200),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(
            result.total, RELATED_TO_NEIGHBOURHOOD_CAP,
            "hub neighbourhood bounded to the cap"
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code() == "NEIGHBOURHOOD_CAPPED"),
            "capping must surface a warning; got {:?}",
            result.warnings.iter().map(|w| w.code()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn expand_via_pulls_in_direct_neighbours() {
        let mut store = Store::new();
        let mut primary = make_entity("primary", "specs");
        primary
            .sections
            .insert("identity".into(), "auth flow".into());
        let n1 = make_entity("n1", "specs");
        let n2 = make_entity("n2", "specs");
        let primary_id = primary.id.clone();
        store.upsert(primary_id.clone(), primary);
        store.upsert(n1.id.clone(), n1);
        store.upsert(n2.id.clone(), n2);
        add_edge(&mut store, primary_id.clone(), EntityId::new("specs", "n1"), "REFERENCES");
        add_edge(&mut store, primary_id.clone(), EntityId::new("specs", "n2"), "REFERENCES");

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["auth".into()],
                ..Default::default()
            }),
            expand_via: Some(vec!["REFERENCES".into()]),
            expand_depth: Some(1),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 3, "primary + 2 expanded");

        let expanded: Vec<&SearchHit> = result
            .hits
            .iter()
            .filter(|h| h.expansion.is_some())
            .collect();
        assert_eq!(expanded.len(), 2);
        for h in expanded {
            let exp = h.expansion.as_ref().unwrap();
            assert_eq!(exp.of, primary_id);
            assert_eq!(exp.via_edge, "REFERENCES");
            assert_eq!(exp.depth, 1);
            // Facet side check lands below — here, confirm the wire contract:
            // expanded hits carry a decayed score_breakdown, no matched_terms.
            let bd = h.score_breakdown.as_ref().unwrap();
            assert_eq!(bd.expansion_decay, Some(0.5));
            assert!(h.matched_terms.is_none());
        }
        // Facet by_expansion now carries both keys.
        let facets = result.facets.as_ref().unwrap();
        assert_eq!(facets.by_expansion.get("primary").copied(), Some(1));
        assert_eq!(facets.by_expansion.get("expanded").copied(), Some(2));
    }

    #[test]
    fn expand_via_respects_filter() {
        // Primary is a spec; neighbour is a memo. entity_type filter drops it.
        let mut store = Store::new();
        let mut primary = make_entity("primary", "specs");
        primary
            .sections
            .insert("identity".into(), "auth flow".into());
        let mut neighbor = make_entity("neighbor", "specs");
        neighbor.entity_type = "memo".into();
        let primary_id = primary.id.clone();
        store.upsert(primary_id.clone(), primary);
        store.upsert(neighbor.id.clone(), neighbor);
        add_edge(
            &mut store,
            primary_id,
            EntityId::new("specs", "neighbor"),
            "REFERENCES",
        );

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["auth".into()],
                ..Default::default()
            }),
            entity_type: Some("spec".into()),
            expand_via: Some(vec!["REFERENCES".into()]),
            expand_depth: Some(1),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1, "only the primary — memo neighbour dropped by entity_type");
        assert!(result.hits[0].expansion.is_none());
    }

    #[test]
    fn expand_via_respects_depth() {
        // primary --R--> a --R--> b
        let mut store = Store::new();
        let mut primary = make_entity("primary", "specs");
        primary.sections.insert("identity".into(), "anchor".into());
        let a = make_entity("a", "specs");
        let b = make_entity("b", "specs");
        let primary_id = primary.id.clone();
        store.upsert(primary_id.clone(), primary);
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);
        add_edge(
            &mut store,
            primary_id.clone(),
            EntityId::new("specs", "a"),
            "REFERENCES",
        );
        add_edge(
            &mut store,
            EntityId::new("specs", "a"),
            EntityId::new("specs", "b"),
            "REFERENCES",
        );

        let make_scope = |depth: usize| SearchScope {
            query: Some(Query {
                any: vec!["anchor".into()],
                ..Default::default()
            }),
            expand_via: Some(vec!["REFERENCES".into()]),
            expand_depth: Some(depth),
            ..Default::default()
        };
        let r1 = run_search(&store, &make_scope(1));
        assert_eq!(r1.total, 2, "depth 1: primary + a");

        let r2 = run_search(&store, &make_scope(2));
        assert_eq!(r2.total, 3, "depth 2: primary + a + b");
        let b_hit = r2.hits.iter().find(|h| h.id.name() == "b").unwrap();
        assert_eq!(b_hit.expansion.as_ref().unwrap().depth, 2);
    }

    #[test]
    fn expand_via_empty_edge_types_skips() {
        let mut store = Store::new();
        let mut primary = make_entity("primary", "specs");
        primary.sections.insert("identity".into(), "anchor".into());
        let n = make_entity("n", "specs");
        let primary_id = primary.id.clone();
        store.upsert(primary_id.clone(), primary);
        store.upsert(n.id.clone(), n);
        add_edge(
            &mut store,
            primary_id,
            EntityId::new("specs", "n"),
            "REFERENCES",
        );

        let scope_empty = SearchScope {
            query: Some(Query {
                any: vec!["anchor".into()],
                ..Default::default()
            }),
            expand_via: Some(Vec::new()),
            ..Default::default()
        };
        let scope_none = SearchScope {
            query: Some(Query {
                any: vec!["anchor".into()],
                ..Default::default()
            }),
            expand_via: None,
            ..Default::default()
        };
        let r_empty = run_search(&store, &scope_empty);
        let r_none = run_search(&store, &scope_none);
        assert_eq!(r_empty.total, 1);
        assert_eq!(r_empty.total, r_none.total);
    }

    #[test]
    fn expand_via_score_decay() {
        // primary --R--> a --R--> b, depth 2
        let mut store = Store::new();
        let mut primary = make_entity("primary", "specs");
        primary.sections.insert("identity".into(), "keyword".into());
        let a = make_entity("a", "specs");
        let b = make_entity("b", "specs");
        let primary_id = primary.id.clone();
        store.upsert(primary_id.clone(), primary);
        store.upsert(a.id.clone(), a);
        store.upsert(b.id.clone(), b);
        add_edge(
            &mut store,
            primary_id.clone(),
            EntityId::new("specs", "a"),
            "REFERENCES",
        );
        add_edge(
            &mut store,
            EntityId::new("specs", "a"),
            EntityId::new("specs", "b"),
            "REFERENCES",
        );

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["keyword".into()],
                ..Default::default()
            }),
            expand_via: Some(vec!["REFERENCES".into()]),
            expand_depth: Some(2),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let primary_hit = result
            .hits
            .iter()
            .find(|h| h.id == primary_id)
            .expect("primary present");
        let primary_score = primary_hit.score;
        assert!(primary_score > 0.0, "primary must have BM25 score");

        let a_hit = result
            .hits
            .iter()
            .find(|h| h.id.name() == "a")
            .unwrap();
        let b_hit = result
            .hits
            .iter()
            .find(|h| h.id.name() == "b")
            .unwrap();
        assert!((a_hit.score - primary_score * 0.5).abs() < 0.0001);
        assert!((b_hit.score - primary_score * 0.25).abs() < 0.0001);
        assert_eq!(a_hit.score_breakdown.as_ref().unwrap().expansion_decay, Some(0.5));
        assert_eq!(b_hit.score_breakdown.as_ref().unwrap().expansion_decay, Some(0.25));
    }

    #[test]
    fn expand_via_via_edge_label_correct() {
        let mut store = Store::new();
        let mut primary = make_entity("primary", "specs");
        primary.sections.insert("identity".into(), "keyword".into());
        let realizes_n = make_entity("realizes-target", "specs");
        let references_n = make_entity("references-target", "specs");
        let primary_id = primary.id.clone();
        store.upsert(primary_id.clone(), primary);
        store.upsert(realizes_n.id.clone(), realizes_n);
        store.upsert(references_n.id.clone(), references_n);
        add_edge(
            &mut store,
            primary_id.clone(),
            EntityId::new("specs", "realizes-target"),
            "REALIZES",
        );
        add_edge(
            &mut store,
            primary_id,
            EntityId::new("specs", "references-target"),
            "REFERENCES",
        );

        let scope = SearchScope {
            query: Some(Query {
                any: vec!["keyword".into()],
                ..Default::default()
            }),
            expand_via: Some(vec!["REALIZES".into(), "REFERENCES".into()]),
            expand_depth: Some(1),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        let rt = result
            .hits
            .iter()
            .find(|h| h.id.name() == "realizes-target")
            .unwrap();
        assert_eq!(rt.expansion.as_ref().unwrap().via_edge, "REALIZES");
        let rf = result
            .hits
            .iter()
            .find(|h| h.id.name() == "references-target")
            .unwrap();
        assert_eq!(rf.expansion.as_ref().unwrap().via_edge, "REFERENCES");
    }

    fn make_stub_entity(name: &str, mem: &str) -> Entity {
        let mut e = make_entity(name, mem);
        e.stub = true;
        e
    }

    #[test]
    fn search_filter_stub_none_returns_both() {
        let mut store = Store::new();
        let real = make_entity("real-a", "specs");
        let stub = make_stub_entity("stub-b", "specs");
        store.upsert(real.id.clone(), real);
        store.upsert(stub.id.clone(), stub);

        let scope = SearchScope::default();
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 2, "default returns both stubs and reals");

        let stub_hit = result
            .hits
            .iter()
            .find(|h| h.id.name() == "stub-b")
            .expect("stub must appear in default results");
        assert!(stub_hit.stub, "hit.stub reflects entity.stub (regression guard)");
        let real_hit = result
            .hits
            .iter()
            .find(|h| h.id.name() == "real-a")
            .expect("real must appear");
        assert!(!real_hit.stub);
    }

    #[test]
    fn search_filter_stub_true_returns_only_stubs() {
        let mut store = Store::new();
        let real = make_entity("real-a", "specs");
        let stub = make_stub_entity("stub-b", "specs");
        store.upsert(real.id.clone(), real);
        store.upsert(stub.id.clone(), stub);

        let scope = SearchScope {
            stub: Some(true),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].id.name(), "stub-b");
        assert!(result.hits[0].stub);
    }

    #[test]
    fn search_filter_stub_false_excludes_stubs() {
        let mut store = Store::new();
        let real = make_entity("real-a", "specs");
        let stub = make_stub_entity("stub-b", "specs");
        store.upsert(real.id.clone(), real);
        store.upsert(stub.id.clone(), stub);

        let scope = SearchScope {
            stub: Some(false),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].id.name(), "real-a");
        assert!(!result.hits[0].stub);
    }

    #[test]
    fn search_filter_stub_intersects_entity_type() {
        let mut store = Store::new();
        let real_spec = make_entity("real-spec", "specs");
        let stub_spec = make_stub_entity("stub-spec", "specs");
        let mut stub_memo = make_stub_entity("stub-memo", "specs");
        stub_memo.entity_type = "memo".into();
        store.upsert(real_spec.id.clone(), real_spec);
        store.upsert(stub_spec.id.clone(), stub_spec);
        store.upsert(stub_memo.id.clone(), stub_memo);

        let scope = SearchScope {
            stub: Some(true),
            entity_type: Some("spec".into()),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].id.name(), "stub-spec");
        assert!(result.hits[0].stub);
    }

    #[test]
    fn facets_by_type_omits_empty_bucket_for_stubs() {
        // Production stubs carry `entity_type: ""` (crud::make_stub). When a
        // mixed hit-set reaches compute_facets, the empty string must not
        // surface as its own `by_type` bucket — the type is semantically
        // undefined for a stub. Agents read stub counts from the `stub`
        // filter or memstead_health.stubs, not from the type facet.
        let mut store = Store::new();
        let real = make_entity("real-a", "specs");
        let mut stub = make_stub_entity("stub-b", "specs");
        stub.entity_type = String::new(); // match production make_stub
        store.upsert(real.id.clone(), real);
        store.upsert(stub.id.clone(), stub);

        let result = run_search(&store, &SearchScope::default());
        assert_eq!(result.total, 2, "both entities are in the hit set");
        let facets = result.facets.as_ref().expect("facets present");
        assert_eq!(facets.by_type.get("spec").copied(), Some(1));
        assert!(
            !facets.by_type.contains_key(""),
            "by_type must not expose an empty-string bucket for stubs: {:?}",
            facets.by_type
        );
    }

    /// A hit's summary is resolved against its *own* mem schema at
    /// search time,
    /// not the global `default` schema. A `software`-schema `requirement`
    /// projects its `Statement` anchor — pre-fix `type_by_name` missed it
    /// (requirement isn't a `default`-schema type) and rendered `—`.
    #[test]
    fn search_summary_uses_per_mem_schema_anchor_section() {
        use memstead_schema::SchemaRegistry;

        let software = SchemaRegistry::builtin()
            .resolve_by_name("software")
            .unwrap()
            .expect("software builtin present");
        let req_type = software.get_type("requirement").expect("requirement type");

        let mut metadata = IndexMap::new();
        metadata.insert("type".into(), MetadataValue::String("requirement".into()));
        let mut sections = IndexMap::new();
        sections.insert(
            "statement".into(),
            "The system shall encrypt tokens at rest.".into(),
        );
        let entity = Entity {
            id: EntityId::new("reqs", "encrypt-tokens"),
            title: "Encrypt tokens".into(),
            entity_type: "requirement".into(),
            mem: "reqs".into(),
            file_path: "encrypt-tokens.md".into(),
            metadata,
            sections,
            relationships: Vec::new(),
            content_hash: "h".into(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        };
        let mut store = Store::new();
        store.upsert(entity.id.clone(), entity);

        // Index + per-mem schema map keyed to the *software* schema, so the
        // search op resolves `requirement` against it (not the default schema).
        let mut idx = MemIndex::build_in_ram("reqs".into(), Some(&software)).unwrap();
        for e in store.all_entities() {
            idx.index_entity(e).unwrap();
        }
        idx.commit().unwrap();
        let mut indexes = HashMap::new();
        indexes.insert("reqs".to_string(), idx);
        let mut schemas: HashMap<String, Arc<Schema>> = HashMap::new();
        schemas.insert("reqs".to_string(), software.clone());

        // Metadata-only scan returns the requirement.
        let result = search(&store, &SearchScope::default(), &req_type, &indexes, &schemas);
        assert_eq!(result.total, 1);
        let summary = result.hits[0]
            .summary
            .as_ref()
            .expect("summary computed at search time");
        assert_eq!(summary.heading, "Statement");
        assert!(
            summary.value.contains("encrypt tokens at rest"),
            "got: {}",
            summary.value
        );

        // The envelope projects the anchor section, not the `—` fallback.
        let envelope = crate::render::build_search_envelope(&result, 0);
        assert_eq!(envelope.hits[0].summary_heading, "Statement");
        assert!(envelope.hits[0].summary_value.contains("encrypt tokens at rest"));
    }

    /// The engine-stamped `created_date` is range-filterable, so the
    /// canonical
    /// "entities created since X" query works and returns only entities
    /// past the bound — pre-fix it warned `FIELD_NOT_RANGE_FILTERABLE`.
    #[test]
    fn range_filter_on_created_date_works() {
        let mut store = Store::new();
        let mut old = make_entity("old", "specs");
        old.metadata
            .insert("created_date".into(), MetadataValue::String("2020-01-01".into()));
        let mut recent = make_entity("recent", "specs");
        recent
            .metadata
            .insert("created_date".into(), MetadataValue::String("2026-06-01".into()));
        store.upsert(old.id.clone(), old);
        store.upsert(recent.id.clone(), recent);

        let scope = SearchScope {
            range_filters: HashMap::from([("created_date_after".into(), "2025-01-01".into())]),
            ..Default::default()
        };
        let result = run_search(&store, &scope);
        assert_eq!(result.total, 1, "only the entity created after the bound matches");
        assert_eq!(result.hits[0].id.name(), "recent");
        assert!(
            result.warnings.is_empty(),
            "created_date is range-filterable — no FIELD_NOT_RANGE_FILTERABLE warning; got {:?}",
            result.warnings
        );
    }

    /// Build a one-type schema whose `tags` field is a csv-array,
    /// equality-filterable metadata field — the shape CLI F8 is about.
    fn csv_tag_schema() -> std::sync::Arc<Schema> {
        let manifest = "name: tagtest\nversion: 0.1.0\ndescription: t\nwhen_to_use: t\n\
types:\n  - thing\nrelationships:\n  mode: open\n  definitions:\n    \
- name: PART_OF\n      description: parent\n      default_weight: 3.0\n    \
- name: _default\n      description: fallback\n      default_weight: 1.0\n\
community:\n  resolution: 1.0\n  seed: 42\n";
        let type_yaml = "name: thing\ndescription: t\nwhen_to_use: t\nsections:\n  \
- key: body\n    heading: Body\n    required: true\n    catch_all: true\n    \
search_weight: 1.0\n    write_rules: []\nmetadata_fields:\n  - key: labels\n    \
description: csv labels\n    field_type: string\n    serialization: csv_array\n    \
filterable: equality\n  - key: priority\n    description: prio\n    field_type: string\n    \
enum_values: [low, mid, high]\n    filterable: equality\ntitle_weight: 1.0\ntext_fields:\n  - body\n\
hierarchy_relationship: PART_OF\npropagating_relationships: []\n\
updatable_fields: [title, body, labels]\nhealth_required_fields: []\n\
staleness_threshold_days: 90\nwrite_rules: []\n";
        std::sync::Arc::new(
            memstead_schema::load_schema_from_memory(
                manifest,
                &[("thing".to_string(), type_yaml.to_string())],
            )
            .expect("csv-tag test schema must load"),
        )
    }

    fn codes_for(filters: &[(&str, &str)]) -> Vec<&'static str> {
        let schema = csv_tag_schema();
        let type_def = schema.get_type("thing").expect("thing type present");
        let type_def = type_def.as_ref();
        let mem_schemas: HashMap<String, Arc<Schema>> = HashMap::new();
        let filters: HashMap<String, String> = filters
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mut warnings = Vec::new();
        super::collect_equality_filter_warnings(
            &filters,
            type_def,
            None,
            None,
            &mem_schemas,
            &mut warnings,
        );
        warnings.iter().map(|w| w.code()).collect()
    }

    /// CLI F8 positive: a comma-bearing value on a csv-array field warns
    /// `FILTER_VALUE_MULTI_MEMBER` — the silent zero gets a recoverable
    /// signal naming the single-member form.
    #[test]
    fn csv_filter_comma_value_warns_multi_member() {
        let codes = codes_for(&[("labels", "dedup,retry")]);
        assert!(
            codes.contains(&"FILTER_VALUE_MULTI_MEMBER"),
            "comma-bearing csv value must warn; got: {codes:?}",
        );
    }

    /// CLI F8 complement: a single-member value is the supported shape —
    /// no multi-member warning.
    #[test]
    fn csv_filter_single_member_does_not_warn() {
        let codes = codes_for(&[("labels", "dedup")]);
        assert!(
            !codes.contains(&"FILTER_VALUE_MULTI_MEMBER"),
            "single-member csv value must not warn; got: {codes:?}",
        );
    }

    /// CLI F8 complement: a genuinely-unknown key still warns
    /// `UNKNOWN_FILTER_KEY` (the new advisory is additive, not a
    /// replacement).
    #[test]
    fn unknown_filter_key_still_warns_unknown() {
        let codes = codes_for(&[("nonexistent", "x")]);
        assert!(
            codes.contains(&"UNKNOWN_FILTER_KEY"),
            "unknown key must still warn UNKNOWN_FILTER_KEY; got: {codes:?}",
        );
        assert!(!codes.contains(&"FILTER_VALUE_MULTI_MEMBER"));
    }

    /// #52: filtering a valid enum-constrained field with a value outside
    /// `enum_values` warns `INVALID_ENUM_VALUE`, so a 0-hit result isn't
    /// mistaken for a true no-match.
    #[test]
    fn enum_filter_invalid_value_warns() {
        let codes = codes_for(&[("priority", "urgent")]);
        assert!(
            codes.contains(&"INVALID_ENUM_VALUE"),
            "out-of-enum filter value must warn INVALID_ENUM_VALUE; got: {codes:?}",
        );
    }

    /// #52 refusal: a valid enum value filters normally — no false warning.
    #[test]
    fn enum_filter_valid_value_does_not_warn() {
        let codes = codes_for(&[("priority", "high")]);
        assert!(
            !codes.contains(&"INVALID_ENUM_VALUE"),
            "a valid enum value must not warn; got: {codes:?}",
        );
    }

    /// #52 complement: an unknown field key keeps `UNKNOWN_FILTER_KEY` (the
    /// enum check runs only on declared fields), not the enum warning.
    #[test]
    fn enum_check_does_not_fire_on_unknown_key() {
        let codes = codes_for(&[("nonexistent", "urgent")]);
        assert!(codes.contains(&"UNKNOWN_FILTER_KEY"));
        assert!(!codes.contains(&"INVALID_ENUM_VALUE"));
    }
}
