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
    ExpansionInfo, Facets, ListResult, Query, ScoreBreakdown, SearchHit, SearchResult, SearchScope,
    SubsectionFacet, SummaryPair, WarningHint,
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
    let filter_type = scoped_type.and_then(|t| resolve_type(t, scope_mem, mem_schemas));
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
            last_modified: entity.metadata.get("last_modified").map(|v| v.to_string()),
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
            let distances = query::reachable_distances(store, related_to, depth, scope.direction);
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
        expand_hits(
            &mut hits,
            store,
            edge_types,
            scope,
            default_schema,
            mem_schemas,
        );
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
        warnings.push(WarningHint::SearchResultsTruncated { kept: keep, budget });
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
    let filter_type = scoped_type.and_then(|t| resolve_type(t, scope_mem, mem_schemas));
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
            last_modified: entity.metadata.get("last_modified").map(|v| v.to_string()),
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

        let tag = if hit.expansion.is_some() {
            "expanded"
        } else {
            "primary"
        };
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
    let mut expanded: HashMap<
        EntityId,
        (
            f32,
            String,
            usize,
            EntityId,
            crate::graph::query::TraversalDirection,
        ),
    > = HashMap::new();

    for primary in hits.iter() {
        let reached =
            query::reachable_via(store, &primary.id, edge_types, depth_limit, scope.direction);
        for reached_via in reached {
            if primary_ids.contains(&reached_via.id) {
                continue;
            }
            let decay = 0.5f32.powi(reached_via.depth as i32);
            let score = primary.score * decay;
            let better = match expanded.get(&reached_via.id) {
                Some((prev_score, _, _, _, _)) => score > *prev_score,
                None => true,
            };
            if better {
                expanded.insert(
                    reached_via.id.clone(),
                    (
                        score,
                        reached_via.via_edge,
                        reached_via.depth,
                        primary.id.clone(),
                        reached_via.direction,
                    ),
                );
            }
        }
    }

    for (id, (score, via_edge, depth, of, via_direction)) in expanded {
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
            last_modified: entity.metadata.get("last_modified").map(|v| v.to_string()),
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
                via_direction,
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
    if entity.title.to_lowercase().contains(&lower_term) {
        return Some(build_snippet(&entity.title, term));
    }
    let mut best: Option<(f32, String)> = None;
    for section_def in &schema.sections {
        if section_def.search_weight == 0.0 {
            continue;
        }
        if let Some(content) = entity.sections.get(section_def.key.as_str())
            && content.to_lowercase().contains(&lower_term)
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
mod tests;
