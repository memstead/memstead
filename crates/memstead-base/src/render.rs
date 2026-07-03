//! Markdown rendering of Engine result types.
//!
//! Shared by `memstead-mcp` (wraps output in MCP `CallToolResult`) and
//! `memstead-cli` (prints directly to stdout).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use memstead_schema::{
    FieldType, Filterable, ManualAuthoring, PerEdgeDescription, RelationshipMode, Schema,
    Serialization, TypeDefinition, all_types, type_by_name,
};
use serde::Serialize;

use crate::chunking::estimate_tokens;
use crate::graph::community::generate_auto_summary;
use crate::ops::Direction;
use crate::ops::{ExpansionInfo, Facets, ScoreBreakdown, SubsectionFacet, TermMatch};
use crate::store::Store;
use crate::{
    ContextResult, Edge, Entity, InEdge, ListResult, LouvainOutput, SearchHit, SearchResult,
};

// ---------------------------------------------------------------------------
// Entity rendering
// ---------------------------------------------------------------------------

/// Render a single entity as markdown with frontmatter metadata.
pub fn render_entity_markdown(entity: &Entity, sections_filter: Option<&[String]>) -> String {
    let body_text = render_entity_body(entity, sections_filter);

    // Frontmatter — _tokens reflects the rendered output, not the full entity.
    let mut lines = Vec::new();
    lines.push("---".to_string());
    lines.push(format!("_hash: {}", entity.content_hash));
    // Typed stub provenance — only emitted when the entity carries
    // a `stub_kind` (real entities are absent from this surface).
    // Agents reading a stub three calls after the mutation that
    // produced it recover the diagnostic context that the
    // mutation-time warning carried.
    if let Some(kind) = &entity.stub_kind {
        match kind {
            crate::entity::StubKind::ForwardReference => {
                lines.push("_stub_kind: forward_reference".to_string());
            }
            crate::entity::StubKind::LoadTime => {
                lines.push("_stub_kind: load_time".to_string());
            }
            crate::entity::StubKind::Residual {
                since_commit,
                readonly_referrers,
            } => {
                lines.push("_stub_kind: residual".to_string());
                if !since_commit.is_empty() {
                    lines.push(format!("_stub_since_commit: {since_commit}"));
                }
                if !readonly_referrers.is_empty() {
                    let refs: Vec<String> =
                        readonly_referrers.iter().map(|r| r.to_string()).collect();
                    lines.push(format!("_stub_readonly_referrers: [{}]", refs.join(", ")));
                }
            }
        }
    }
    let tokens = estimate_tokens(&body_text);
    lines.push(format!("_tokens: {tokens}"));

    // When sections are filtered and some were excluded, show full entity size
    // so agents know how much they're missing.
    let is_filtered = sections_filter.is_some_and(|f| {
        let all_keys: Vec<&String> = entity.sections.keys().collect();
        f.len() < all_keys.len() || !all_keys.iter().all(|k| f.iter().any(|fk| fk == *k))
    });
    if is_filtered {
        let full_body = render_entity_body(entity, None);
        let full_tokens = estimate_tokens(&full_body);
        lines.push(format!("_tokens_unfiltered_body: {full_tokens}"));
    }

    // Emit entity metadata
    for (key, value) in &entity.metadata {
        lines.push(format!("{key}: {value}"));
    }
    lines.push("---".to_string());
    lines.push(String::new());

    lines.push(body_text);
    lines.join("\n")
}

/// Token estimate for an entity's rendered body (title + sections +
/// relationships, filter applied) — the exact number `render_entity_markdown`
/// embeds as its frontmatter `_tokens`. Use this when building a structured
/// envelope so the envelope's `_tokens` and the markdown channel's frontmatter
/// `_tokens` describe the *same* thing for a given `_hash`: the rendered body,
/// not the full markdown document (which would additionally count frontmatter).
pub fn rendered_body_tokens(entity: &Entity, sections_filter: Option<&[String]>) -> usize {
    estimate_tokens(&render_entity_body(entity, sections_filter))
}

/// Build the body (title + sections + relationships) for an entity, optionally filtered.
///
/// Section iteration order follows `entity.sections` — an `IndexMap`, so
/// insertion order is the authoritative render order. The parser inserts keys
/// in the schema's declared order, which is what ships to clients. Do not
/// migrate `entity.sections` back to `HashMap`.
fn render_entity_body(entity: &Entity, sections_filter: Option<&[String]>) -> String {
    let mut body = Vec::new();

    body.push(format!("# {}", entity.title));
    body.push(String::new());

    // Look up the entity's TypeDefinition across every built-in schema
    // so non-default schemas (e.g. `ingest.inconsistency`) get their
    // declared headings rendered exactly as the on-disk markdown
    // emitted them. Falls back to key→heading derivation when no
    // built-in schema declares this type — preserves the prior shape
    // for custom workspace schemas not yet bridged through the
    // renderer.
    let type_def = lookup_builtin_type(&entity.entity_type);

    for (key, content) in &entity.sections {
        if let Some(filter) = sections_filter
            && !filter.iter().any(|f| f == key)
        {
            continue;
        }
        let heading = section_heading_for(type_def.as_deref(), key);
        body.push(format!("## {heading}"));
        body.push(String::new());
        body.push(content.trim().to_string());
        body.push(String::new());
    }

    if !entity.relationships.is_empty()
        && sections_filter.is_none_or(|f| f.iter().any(|s| s == "relationships"))
    {
        body.push("## Relationships".to_string());
        body.push(String::new());
        for rel in &entity.relationships {
            // Mirror the on-disk renderer (`entity::generator`):
            // canonical em-dash delimiter when the relation carries a
            // per-edge description, simple form otherwise.
            match rel
                .description
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(text) => body.push(format!(
                    "- **{}**: [[{}]] \u{2014} {text}",
                    rel.rel_type, rel.target
                )),
                None => body.push(format!("- **{}**: [[{}]]", rel.rel_type, rel.target)),
            }
        }
        body.push(String::new());
    }

    body.join("\n")
}

/// Render a `## Relations` section as markdown — typed edges grouped by
/// direction. Appended to `memstead_entity` output when `include_relations: true`.
/// A JSON-shaped version is available via `render_relations_json` for the
/// `memstead-cli relations --json` consumer.
pub fn render_relations_markdown(
    entity_id: &str,
    outgoing: &[Edge],
    incoming: &[InEdge],
) -> String {
    let mut lines = Vec::new();
    lines.push(String::new());
    lines.push("## Relations".to_string());
    lines.push(String::new());

    if outgoing.is_empty() && incoming.is_empty() {
        lines.push(format!("(no relations for {entity_id})"));
        lines.push(String::new());
        return lines.join("\n");
    }

    if !outgoing.is_empty() {
        lines.push("### Outgoing".to_string());
        for e in outgoing {
            lines.push(format!("- **{}** → [[{}]]", e.rel_type, e.target));
        }
        lines.push(String::new());
    }

    if !incoming.is_empty() {
        lines.push("### Incoming".to_string());
        for e in incoming {
            lines.push(format!("- [[{}]] → **{}** → (this)", e.from, e.rel_type));
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

/// Render outgoing/incoming relations as a JSON envelope. Consumed by
/// `memstead-cli relations --json`; no MCP path uses it.
pub fn render_relations_json(
    entity_id: &str,
    outgoing: &[Edge],
    incoming: &[InEdge],
) -> serde_json::Value {
    let out: Vec<serde_json::Value> = outgoing
        .iter()
        .map(|e| {
            serde_json::json!({
                "type": e.rel_type,
                "target": e.target.to_string(),
                "source": format!("{:?}", e.source).to_lowercase(),
            })
        })
        .collect();

    let inc: Vec<serde_json::Value> = incoming
        .iter()
        .map(|e| {
            serde_json::json!({
                "type": e.rel_type,
                "from": e.from.to_string(),
                "source": format!("{:?}", e.source).to_lowercase(),
            })
        })
        .collect();

    serde_json::json!({
        "entity": entity_id,
        "outgoing": out,
        "incoming": inc,
    })
}

// ---------------------------------------------------------------------------
// Search / List rendering
// ---------------------------------------------------------------------------

/// Render search results as markdown.
pub fn render_search_markdown(result: &SearchResult, offset: usize) -> String {
    let mut lines = Vec::new();

    lines.push("---".to_string());
    lines.push(format!("_total: {}", result.total));
    lines.push(format!("_returned: {}", result.returned));
    lines.push(format!("_offset: {offset}"));
    lines.push(format!("_total_tokens: {}", result.total_tokens));
    lines.push("---".to_string());
    lines.push(String::new());

    if !result.warnings.is_empty() {
        // Render each search warning with its typed code as the lead — same
        // shape mutation-tool `## Warnings` blocks already use — so an
        // agent reading the markdown sees the code without decoding
        // the structured channel.
        lines.push("## Filter warnings".to_string());
        for w in &result.warnings {
            lines.push(format!("- **{}**: {}", w.code(), w.message()));
        }
        lines.push(String::new());
    }

    if let Some(facets) = &result.facets
        && let Some(block) = render_facets_block(facets)
    {
        lines.push(block);
    }

    for hit in &result.hits {
        lines.push(format!(
            "### {} — {} (_score: {:.1}, _tokens: {})",
            hit.id, hit.title, hit.score, hit.tokens,
        ));
        lines.push(hit_summary_line(hit));
        if let Some(line) = render_matched_terms_line(hit.matched_terms.as_ref()) {
            lines.push(line);
        }
        if let Some(line) = render_score_breakdown_line(hit.score_breakdown.as_ref()) {
            lines.push(line);
        }
        if let Some(line) = render_heading_paths_line(hit.matched_terms.as_ref()) {
            lines.push(line);
        }
        if let Some(line) = render_expansion_line(hit.expansion.as_ref()) {
            lines.push(line);
        }
        if let Some(snippet) = &hit.snippet {
            lines.push(format!("> ...{snippet}..."));
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

/// Render the `## Facets` block for a `SearchResult`. Returns `None` when
/// every facet bucket is empty — callers elide the section entirely in
/// that case. Buckets with mixed presence each ship independently.
///
/// Ordering: keys inside a bucket sort by count desc, then key asc so the
/// output is deterministic for tests. `by_subsection` uses its native
/// stored order (already sorted by count desc in `ops::search`).
fn render_facets_block(facets: &Facets) -> Option<String> {
    let blocks: Vec<(&str, String)> = [
        ("by_type", &facets.by_type),
        ("by_mem", &facets.by_mem),
        ("by_level", &facets.by_level),
        ("by_status", &facets.by_status),
        ("by_confidence", &facets.by_confidence),
        ("by_expansion", &facets.by_expansion),
    ]
    .into_iter()
    .filter_map(|(name, bucket)| format_facet_bucket(bucket).map(|s| (name, s)))
    .collect();

    if blocks.is_empty() && facets.by_subsection.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str("## Facets\n");
    for (name, body) in blocks {
        out.push_str(&format!("- **{name}:** {body}\n"));
    }
    if !facets.by_subsection.is_empty() {
        out.push_str("- **by_subsection:**\n");
        for entry in &facets.by_subsection {
            out.push_str(&format!("  - {}\n", format_subsection_facet(entry)));
        }
    }
    Some(out)
}

fn format_facet_bucket(bucket: &HashMap<String, usize>) -> Option<String> {
    if bucket.is_empty() {
        return None;
    }
    let mut entries: Vec<(&String, &usize)> = bucket.iter().collect();
    entries.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    Some(
        entries
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn format_subsection_facet(entry: &SubsectionFacet) -> String {
    let path = entry.path.join(" › ");
    format!("`{path}`: {}", entry.count)
}

/// Render the `**Matched terms:**` line for one hit. `matched_terms`
/// groups `TermMatch`es per query term; output is one `term (field×N, ...)`
/// group per term, joined with `, `. Terms and fields both sort
/// alphabetically for deterministic output.
fn render_matched_terms_line(matched: Option<&HashMap<String, Vec<TermMatch>>>) -> Option<String> {
    let matched = matched?;
    if matched.is_empty() {
        return None;
    }
    let mut terms: Vec<(&String, &Vec<TermMatch>)> = matched.iter().collect();
    terms.sort_by(|a, b| a.0.cmp(b.0));
    let groups: Vec<String> = terms
        .iter()
        .map(|(term, tms)| {
            let mut field_counts: HashMap<&str, usize> = HashMap::new();
            for tm in tms.iter() {
                *field_counts.entry(tm.field.as_str()).or_insert(0) += 1;
            }
            let mut fields: Vec<(&&str, &usize)> = field_counts.iter().collect();
            fields.sort_by(|a, b| a.0.cmp(b.0));
            let inner: Vec<String> = fields.iter().map(|(f, n)| format!("{f}×{n}")).collect();
            format!("`{term}` ({})", inner.join(", "))
        })
        .collect();
    Some(format!("**Matched terms:** {}", groups.join(", ")))
}

/// Render the `**Score:**` line from a `ScoreBreakdown`. Fields render as
/// `bm25 X.X + title X.X + <field> X.X [+ expansion_decay ×X.X]`. Zero-
/// valued components still ship — the breakdown is informational, and the
/// composition "title 0.0" is itself a fact worth surfacing.
fn render_score_breakdown_line(breakdown: Option<&ScoreBreakdown>) -> Option<String> {
    let b = breakdown?;
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("bm25 {:.1}", b.bm25));
    parts.push(format!("title {:.1}", b.title_boost));
    let mut fields: Vec<(&String, &f32)> = b.field_weights.iter().collect();
    fields.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in fields {
        parts.push(format!("{k} {v:.1}"));
    }
    if let Some(decay) = b.expansion_decay {
        parts.push(format!("expansion_decay ×{decay:.1}"));
    }
    Some(format!("**Score:** {}", parts.join(" + ")))
}

/// Render the `**Heading path:**` line for one hit. Collects distinct
/// non-empty `heading_path`s across the hit's `TermMatch`es. Single path
/// renders inline (`A › B`), multiple paths render as `A › B; C › D`.
fn render_heading_paths_line(matched: Option<&HashMap<String, Vec<TermMatch>>>) -> Option<String> {
    let matched = matched?;
    let mut paths: Vec<Vec<String>> = Vec::new();
    let mut term_keys: Vec<&String> = matched.keys().collect();
    term_keys.sort();
    for term in term_keys {
        for tm in &matched[term] {
            if let Some(path) = &tm.heading_path
                && !path.is_empty()
                && !paths.iter().any(|p| p == path)
            {
                paths.push(path.clone());
            }
        }
    }
    if paths.is_empty() {
        return None;
    }
    let formatted: Vec<String> = paths.iter().map(|p| p.join(" › ")).collect();
    Some(format!("**Heading path:** {}", formatted.join("; ")))
}

/// Render the `**Expansion:**` line for one hit — `from <id> via <edge>
/// (depth N)`.
fn render_expansion_line(expansion: Option<&ExpansionInfo>) -> Option<String> {
    let e = expansion?;
    Some(format!(
        "**Expansion:** from `{}` via `{}` (depth {})",
        e.of, e.via_edge, e.depth,
    ))
}

/// Render list results as markdown.
pub fn render_list_markdown(result: &ListResult) -> String {
    let mut lines = Vec::new();

    lines.push("---".to_string());
    lines.push(format!("_total: {}", result.total));
    lines.push(format!("_returned: {}", result.returned));
    lines.push(format!("_offset: {}", result.offset));
    lines.push(format!("_total_tokens: {}", result.total_tokens));
    lines.push("---".to_string());
    lines.push(String::new());

    if !result.warnings.is_empty() {
        lines.push("## Filter warnings".to_string());
        for w in &result.warnings {
            lines.push(format!("- **{}**: {}", w.code(), w.message()));
        }
        lines.push(String::new());
    }

    for hit in &result.hits {
        let meta = hit
            .sections
            .get("level")
            .map(|l| format!("{l}, "))
            .unwrap_or_default();
        lines.push(format!(
            "### {} — {} ({meta}_tokens: {})",
            hit.id, hit.title, hit.tokens,
        ));
        lines.push(hit_summary_line(hit));
        lines.push(String::new());
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Context / Overview rendering
// ---------------------------------------------------------------------------

/// Render a `## Community Context` section — cluster id + neighbor list —
/// appended to `memstead_entity` output when `include_context: true`. No
/// frontmatter; the entity body owns that.
pub fn render_community_context_section(result: &ContextResult, cluster_id: &str) -> String {
    let mut lines = Vec::new();
    lines.push(String::new());
    lines.push("## Community Context".to_string());
    lines.push(String::new());
    lines.push(format!("**Cluster {cluster_id}**"));
    lines.push(String::new());

    if !result.neighbors.is_empty() {
        lines.push("### Neighbors".to_string());
        for n in &result.neighbors {
            let dir = match n.direction {
                Direction::Outgoing => "→",
                Direction::Incoming => "←",
            };
            lines.push(format!(
                "- {} —{}— **{}** ({})",
                result.entity_id, dir, n.id, n.relationship,
            ));
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

/// Render context (community cluster) as markdown.
pub fn render_context_markdown(result: &ContextResult, cluster_id: &str) -> String {
    let mut lines = Vec::new();

    lines.push("---".to_string());
    lines.push(format!("_cluster_id: {cluster_id}"));
    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(format!("## Cluster {cluster_id}"));
    lines.push(String::new());

    // Neighbors grouped by direction
    lines.push("### Neighbors".to_string());
    for n in &result.neighbors {
        let dir = match n.direction {
            Direction::Outgoing => "→",
            Direction::Incoming => "←",
        };
        lines.push(format!(
            "- {} —{}— **{}** ({})",
            result.entity_id, dir, n.id, n.relationship,
        ));
    }
    lines.push(String::new());

    lines.join("\n")
}

/// Render overview (all clusters) as markdown. `store` provides entity titles
/// for the on-the-fly auto-summary (title-join) — there is no stored summary.
pub fn render_overview_markdown(output: &LouvainOutput, store: &Store) -> String {
    let mut lines = Vec::new();

    let entity_count: usize = output.clusters.values().map(|c| c.entities.len()).sum();

    lines.push("---".to_string());
    lines.push(format!("_cluster_count: {}", output.count));
    lines.push(format!("_entity_count: {entity_count}"));
    // Use compact formatting to match JS: "0" instead of "0.0000"
    let mod_str = if output.modularity == 0.0 {
        "0".to_string()
    } else {
        format!("{:.4}", output.modularity)
    };
    lines.push(format!("_modularity: {mod_str}"));
    lines.push("---".to_string());
    lines.push(String::new());

    // Sort clusters by ID for deterministic output
    let mut cluster_ids: Vec<&String> = output.clusters.keys().collect();
    cluster_ids.sort();

    for cluster_id in cluster_ids {
        let info = &output.clusters[cluster_id];
        let summary = generate_auto_summary(store, &info.entities);

        lines.push(format!(
            "## Cluster {cluster_id} ({} entities)",
            info.entities.len(),
        ));
        if !summary.is_empty() {
            lines.push(summary);
        }
        for entity_id in &info.entities {
            lines.push(format!("- {entity_id}"));
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// JSON envelopes for search / list — consumed by `memstead-cli` only
// ---------------------------------------------------------------------------
//
// These wrap the core `SearchResult` / `ListResult` with precomputed
// `summary_heading` / `summary_value` per hit — the same values the
// markdown renderer emits — so the CLI's `--json` output doesn't
// reimplement schema lead-section lookup. The MCP side carries no JSON
// sidecar; these envelopes remain on the `memstead-cli search --json` /
// `memstead-cli list --json` path.
//
// Snake-case field names are intentional: they match on-disk YAML and the
// core `SearchHit` struct. Do not add `rename_all = "camelCase"`.

/// Envelope wrapping a `SearchHit` with precomputed summary fields.
#[derive(Serialize)]
pub struct SearchHitEnvelope<'a> {
    #[serde(flatten)]
    pub hit: &'a SearchHit,
    pub summary_heading: String,
    pub summary_value: String,
}

/// Envelope for a full `SearchResult`:
/// `_-prefixed` engine-emitted counters at the top level, `facets`
/// as a structured object (not a markdown blob), and the full per-hit
/// shape (score, score_breakdown, matched_terms, expansion) inherited
/// verbatim from `SearchHit` so the structured envelope is the
/// branching surface — agents reading `structured_content` don't have
/// to parse the text channel's rendered prose to recover scores or
/// score components. CLI `--json` and MCP `structured_content` share
/// this shape.
#[derive(Serialize)]
pub struct SearchResultEnvelope<'a> {
    #[serde(rename = "_total")]
    pub total: usize,
    #[serde(rename = "_returned")]
    pub returned: usize,
    #[serde(rename = "_offset")]
    pub offset: usize,
    /// Sum of estimated tokens across all matching entities (pre-pagination).
    /// Mirrors `ListResultEnvelope.total_tokens` so the field has consistent
    /// meaning across both surfaces — migration cost for agents is zero.
    #[serde(rename = "_total_tokens")]
    pub total_tokens: usize,
    pub hits: Vec<SearchHitEnvelope<'a>>,
    /// Faceted counts over the unpaginated hit set. Skipped on the
    /// wire when the engine produced no facets (rare; the unified
    /// engine always populates an empty `Facets::default()` for
    /// shape stability).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<&'a Facets>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: &'a Vec<crate::ops::WarningHint>,
}

/// Envelope for a full `ListResult`. The engine-meta counters carry the
/// same `_`-prefixed wire keys as [`SearchResultEnvelope`] (and as both
/// surfaces' markdown form) so an agent moving between `memstead list --json`
/// and `memstead search --json` parses one envelope-meta convention. The
/// `_` prefix reads as "engine-meta, not entity content".
#[derive(Serialize)]
pub struct ListResultEnvelope<'a> {
    #[serde(rename = "_total")]
    pub total: usize,
    #[serde(rename = "_returned")]
    pub returned: usize,
    #[serde(rename = "_offset")]
    pub offset: usize,
    #[serde(rename = "_total_tokens")]
    pub total_tokens: usize,
    pub hits: Vec<SearchHitEnvelope<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: &'a Vec<crate::ops::WarningHint>,
}

/// Build the structured `memstead_entity` envelope. Identity fields
/// (`_hash`, `id`, `mem`, `type`, `_stub_kind`) come from the parsed
/// `Entity` and live at the top level. Every schema-declared frontmatter
/// key surfaces under a nested `metadata: {...}` map — its single home.
/// Read a metadata
/// value as `envelope.metadata.<key>`; generic consumers iterate the map
/// without per-type branching. The prior shape additionally hoisted
/// `level`/`stability`/`created_date`/`last_modified` to the top level,
/// serialising those fields twice; that hoist is gone. The read-only
/// identity triple (`mem`/`id`/`type`) is excluded from the nested map
/// — it appears only top-level — and underscore-prefixed internal keys
/// (`_hash`, `_tokens*`, `_mem_schema`, `_stub_*`) live in dedicated
/// top-level slots and never appear inside the nested map. `sections` and
/// `relationships` round-trip the engine's internal IndexMap / Vec
/// shapes verbatim. `_tokens` is computed from the rendered body
/// (filter and opt-in inserts applied) so agents can pre-size before
/// a follow-up `token_budget`-bounded read. `_mem_schema` rides
/// when the workspace pinned a schema for the mem.
///
/// Per-section filtering applies — when `sections_filter` is
/// `Some`, the structured `sections` map carries only the requested
/// keys (matching the markdown projection). The unfiltered-base
/// token cost surfaces as `_tokens_unfiltered_body` so agents can
/// predict the cost of dropping the filter. The name avoids implying a
/// monotonic relationship (`_tokens_unfiltered_body ≥ _tokens`) that the
/// opt-in (`include_relations` / `include_context`) path can invert:
/// opt-in inserts contribute to `_tokens` but not to this baseline. Stub
/// entities ship every key with empty `sections` / `relationships`
/// arrays.
///
/// The structured envelope is the contract for `memstead_entity`:
/// agents read `_hash`, sections, and relations from typed fields
/// rather than string-scraping the markdown frontmatter.
pub fn build_entity_envelope(
    entity: &Entity,
    rendered_body_tokens: usize,
    full_tokens: Option<usize>,
    sections_filter: Option<&[String]>,
    schema_anchor: Option<&str>,
    outgoing_edges: &[crate::store::Edge],
) -> serde_json::Value {
    let mut envelope = serde_json::Map::new();
    envelope.insert(
        "_hash".to_string(),
        serde_json::Value::String(entity.content_hash.clone()),
    );
    envelope.insert(
        "id".to_string(),
        serde_json::Value::String(entity.id.to_string()),
    );
    envelope.insert(
        "mem".to_string(),
        serde_json::Value::String(entity.mem.clone()),
    );
    envelope.insert(
        "type".to_string(),
        serde_json::Value::String(entity.entity_type.clone()),
    );

    // Metadata has exactly one home on the envelope — the nested
    // `metadata` map. Scalars like `level`/`stability`/`created_date`/
    // `last_modified` are NOT hoisted to the top level; agents read
    // `envelope.metadata.<key>`. The nested map is authoritative because
    // it carries every schema-declared frontmatter key (including
    // type-specific fields a top-level hoist never covered).
    //
    // Identity keys stay top-level and are excluded here so they too
    // appear exactly once: `_hash`, `id`, `mem`, `type` are the
    // entity's structural identity (inserted above), not free-form
    // metadata. `mem`/`id`/`type` is the engine's read-only key triple
    // (`READ_ONLY_METADATA_KEYS`); `_`-prefixed internal keys live in
    // dedicated top-level slots (`_tokens*`, `_mem_schema`, `_stub_*`).
    // Stub entities surface an empty `metadata: {}` so consumers don't
    // branch on its presence.
    let mut metadata = serde_json::Map::new();
    for (key, value) in &entity.metadata {
        if key.starts_with('_')
            || crate::runtime_validator::READ_ONLY_METADATA_KEYS.contains(&key.as_str())
        {
            continue;
        }
        metadata.insert(
            key.clone(),
            serde_json::Value::String(value.to_frontmatter_string()),
        );
    }
    envelope.insert("metadata".to_string(), serde_json::Value::Object(metadata));

    envelope.insert(
        "_tokens".to_string(),
        serde_json::Value::Number(serde_json::Number::from(rendered_body_tokens)),
    );
    if let Some(t) = full_tokens {
        // This measures the unfiltered base body cost without
        // `include_relations` / `include_context` opt-in inserts.
        // `_tokens` may exceed `_tokens_unfiltered_body` when opt-ins
        // are active (the opt-in inserts contribute to `_tokens` but not
        // to this baseline) — the field name avoids implying a monotonic
        // relationship the opt-in path can invert.
        envelope.insert(
            "_tokens_unfiltered_body".to_string(),
            serde_json::Value::Number(serde_json::Number::from(t)),
        );
    }
    if let Some(s) = schema_anchor {
        envelope.insert(
            "_mem_schema".to_string(),
            serde_json::Value::String(s.to_string()),
        );
    }

    if let Some(kind) = &entity.stub_kind {
        envelope.insert(
            "_stub_kind".to_string(),
            serde_json::to_value(kind).unwrap_or(serde_json::Value::Null),
        );
    }

    let mut sections = serde_json::Map::new();
    for (key, content) in &entity.sections {
        if let Some(filter) = sections_filter
            && !filter.iter().any(|f| f == key)
        {
            continue;
        }
        sections.insert(key.clone(), serde_json::Value::String(content.clone()));
    }
    envelope.insert("sections".to_string(), serde_json::Value::Object(sections));

    // Resolve each relationship's `source` label against the store's
    // outgoing-edge index. A hardcoded `"explicit"` would disagree
    // with the stub-adoption
    // response's `incoming[].source` for alias-synthesised
    // REFERENCES edges (and was actively misleading because
    // REFERENCES carries `manual_authoring: forbidden` — no edge of
    // that rel-type can be authored explicitly). The store's
    // `EdgeSource` is the single source of truth; the markdown
    // round-trip (which doesn't encode source) is no longer
    // consulted for this field.
    let resolve_source = |rel: &crate::entity::Relationship| -> &'static str {
        outgoing_edges
            .iter()
            .find(|e| e.rel_type == rel.rel_type && e.target == rel.target)
            .map(|e| match e.source {
                crate::store::EdgeSource::BodyLink => "body_link",
                crate::store::EdgeSource::Hierarchy => "hierarchy",
                crate::store::EdgeSource::Explicit => "explicit",
            })
            .unwrap_or("explicit")
    };
    let relationships = entity
        .relationships
        .iter()
        .map(|rel| {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "rel_type".to_string(),
                serde_json::Value::String(rel.rel_type.clone()),
            );
            obj.insert(
                "target".to_string(),
                serde_json::Value::String(rel.target.to_string()),
            );
            obj.insert(
                "source".to_string(),
                serde_json::Value::String(resolve_source(rel).to_string()),
            );
            if let Some(desc) = rel
                .description
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                obj.insert(
                    "description".to_string(),
                    serde_json::Value::String(desc.to_string()),
                );
            }
            serde_json::Value::Object(obj)
        })
        .collect();
    envelope.insert(
        "relationships".to_string(),
        serde_json::Value::Array(relationships),
    );

    serde_json::Value::Object(envelope)
}

/// Build a `SearchResultEnvelope` borrowing from `result`.
pub fn build_search_envelope<'a>(
    result: &'a SearchResult,
    offset: usize,
) -> SearchResultEnvelope<'a> {
    SearchResultEnvelope {
        total: result.total,
        returned: result.returned,
        offset,
        total_tokens: result.total_tokens,
        hits: result.hits.iter().map(build_hit_envelope).collect(),
        facets: result.facets.as_ref(),
        warnings: &result.warnings,
    }
}

/// Build a `ListResultEnvelope` borrowing from `result`.
pub fn build_list_envelope(result: &ListResult) -> ListResultEnvelope<'_> {
    ListResultEnvelope {
        total: result.total,
        returned: result.returned,
        offset: result.offset,
        total_tokens: result.total_tokens,
        hits: result.hits.iter().map(build_hit_envelope).collect(),
        warnings: &result.warnings,
    }
}

fn build_hit_envelope(hit: &SearchHit) -> SearchHitEnvelope<'_> {
    let (heading, value) = hit_summary_pair(hit);
    SearchHitEnvelope {
        hit,
        summary_heading: heading,
        summary_value: value,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the one-line summary for a search/list hit.
///
/// Resolves the hit's schema and uses its lead section (first required, or
/// first section if none are required) as the label. Never panics — unknown
/// schemas or schemas with no sections fall back to `**Summary**: —`.
fn hit_summary_line(hit: &SearchHit) -> String {
    let (heading, value) = hit_summary_pair(hit);
    format!("**{heading}**: {value}")
}

/// Resolve `(heading, value)` for a hit's summary line — the single source of
/// truth for lead-section lookup. Used by both markdown rendering and the
/// structured-content envelope.
///
/// Prefers the engine-precomputed [`SearchHit::summary`] (resolved against the
/// hit's own mem schema at search time). Falls back to the global
/// `type_by_name` lookup only for hits built outside the search op (FFI/bridge
/// and test fixtures) — that fallback sees only the `default` schema, which is
/// why the engine resolves the pair where the per-mem schema is in hand.
fn hit_summary_pair(hit: &SearchHit) -> (String, String) {
    if let Some(summary) = &hit.summary {
        return (summary.heading.clone(), summary.value.clone());
    }
    summary_pair(type_by_name(&hit.entity_type).as_deref(), &hit.sections)
}

/// Resolve `(heading, value)` given a schema and the hit's section map.
fn summary_pair(
    schema: Option<&TypeDefinition>,
    sections: &HashMap<String, String>,
) -> (String, String) {
    match schema {
        Some(schema) => lead_section_pair(schema, |k| sections.get(k).map(String::as_str)),
        None => ("Summary".to_string(), "—".to_string()),
    }
}

/// The lead-section `(heading, value)` for a hit given its resolved schema:
/// the first required section (or the first section when none are required),
/// with its value pulled from `sections`. Returns `("Summary", "—")` when the
/// type declares no sections, and an honest `"—"` value when the lead section
/// is absent/empty in this hit. The single source of truth shared by the
/// render-time fallback ([`summary_pair`]) and the search op, which calls it
/// with each hit's correctly-resolved per-mem schema.
pub(crate) fn lead_section_pair<'a>(
    schema: &TypeDefinition,
    get_section: impl Fn(&str) -> Option<&'a str>,
) -> (String, String) {
    let Some(section) = schema
        .required_sections()
        .next()
        .or(schema.sections.first())
    else {
        return ("Summary".to_string(), "—".to_string());
    };
    let value = get_section(section.key.as_str()).unwrap_or("—");
    (section.heading.clone(), value.to_string())
}

/// Convert a section key to a display heading via the simple
/// derivation: first char uppercased, underscores → spaces. Used as
/// a fallback when no schema-declared heading is available.
fn section_key_to_heading(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => {
            let first: String = c.to_uppercase().collect();
            let rest: String = chars.map(|c| if c == '_' { ' ' } else { c }).collect();
            format!("{first}{rest}")
        }
    }
}

/// Resolve the heading for `key` from the type's declared sections;
/// fall back to the key-derivation when the type is unknown or the
/// key is not declared (e.g. the `relationships` virtual surface, or
/// catch-all extra keys). The schema-declared heading is the on-disk
/// truth — the renderer must echo it so rendered text matches the
/// markdown file content.
fn section_heading_for(type_def: Option<&TypeDefinition>, key: &str) -> String {
    type_def
        .and_then(|t| t.sections.iter().find(|s| s.key == key))
        .map(|s| s.heading.clone())
        .unwrap_or_else(|| section_key_to_heading(key))
}

/// Search every built-in schema for `name`, returning the first match.
/// Caches the loaded schema list via `OnceLock` so subsequent renders
/// pay only the HashMap lookup cost.
///
/// Distinct from `memstead_schema::type_by_name`, which is limited to the
/// `default` schema — that helper exists for legacy short-name lookups
/// and is left unchanged here. Custom workspace schemas (not embedded
/// in the binary) still fall through to the key-derivation path.
fn lookup_builtin_type(name: &str) -> Option<Arc<TypeDefinition>> {
    static CACHE: OnceLock<Vec<Arc<Schema>>> = OnceLock::new();
    let schemas =
        CACHE.get_or_init(|| memstead_schema::builtins::load_builtin_schemas().unwrap_or_default());
    for s in schemas {
        if let Some(t) = s.get_type(name) {
            return Some(t);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Schema introspection rendering
// ---------------------------------------------------------------------------

/// Render the full schema catalog as markdown — built-in default types.
pub fn render_type_catalog_markdown() -> String {
    render_type_catalog_lines(all_types())
}

/// Render the type catalog for an arbitrary loaded [`Schema`].
/// Same shape as [`render_type_catalog_markdown`]; iterates the
/// schema's own types in name order so multi-mem workspaces can
/// describe the schema pinned by the writable mem, not the engine's
/// hard-coded built-in.
pub fn render_type_catalog_markdown_for(schema: &Schema) -> String {
    let mut types: Vec<Arc<TypeDefinition>> = schema.types.values().cloned().collect();
    types.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    render_type_catalog_lines(types)
}

fn render_type_catalog_lines(types: Vec<Arc<TypeDefinition>>) -> String {
    let mut lines = vec![
        "# Available types".to_string(),
        String::new(),
        "Run `memstead type <name>` (or call `memstead_schema` with a type name) to see its metadata fields, sections, relationship types, and writing guidance."
            .to_string(),
        String::new(),
    ];
    for schema in types {
        let required_sections = schema.required_sections().count();
        let total_sections = schema.sections.len();
        let metadata_count = schema.metadata_fields.len();
        lines.push(format!(
            "- **{}** — {} sections ({} required), {} metadata fields, staleness {}d",
            schema.name.as_str(),
            total_sections,
            required_sections,
            metadata_count,
            schema.staleness_threshold_days,
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}

/// Render a single type's definition as agent-friendly markdown.
pub fn render_type_info_markdown(schema: &TypeDefinition) -> String {
    let mut lines = Vec::new();
    lines.push(format!("# Type: {}", schema.name.as_str()));
    lines.push(String::new());
    lines.push(format!(
        "Staleness threshold: {} days. Hierarchy: `{}`.",
        schema.staleness_threshold_days, schema.hierarchy_relationship,
    ));
    lines.push(String::new());

    // Metadata fields
    lines.push("## Metadata fields".to_string());
    for field in &schema.metadata_fields {
        lines.push(format!("- {}", describe_metadata_field(field)));
    }
    lines.push(String::new());

    // Sections
    lines.push("## Sections".to_string());
    for section in &schema.sections {
        let req = if section.required {
            "required"
        } else {
            "optional"
        };
        let catch_all = if section.catch_all { ", catch-all" } else { "" };
        lines.push(format!(
            "- **{}** ({req}{catch_all}, search_weight: {:.1})",
            section.key, section.search_weight,
        ));
        for rule in &section.write_rules {
            lines.push(format!("  - Write rule: {rule}"));
        }
    }
    lines.push(String::new());

    // Relationship types
    lines.push("## Relationship types (with edge weights)".to_string());
    for (rel_type, weight) in &schema.edge_weights {
        if rel_type == "_default" {
            continue;
        }
        let mut flags: Vec<&str> = Vec::new();
        if rel_type == &schema.hierarchy_relationship {
            flags.push("hierarchy");
        }
        if schema
            .propagating_relationships
            .iter()
            .any(|r| r == rel_type)
        {
            flags.push("propagating");
        }
        let flag_str = if flags.is_empty() {
            String::new()
        } else {
            format!(" ({})", flags.join(", "))
        };
        lines.push(format!("- **{rel_type}**: {weight}{flag_str}"));
    }
    // Default weight
    if let Some((_, default_weight)) = schema.edge_weights.iter().find(|(n, _)| *n == "_default") {
        lines.push(format!(
            "- _default_ (any other relationship type): {default_weight}"
        ));
    }
    lines.push(String::new());

    // Writing guidance (schema-level)
    if !schema.write_rules.is_empty() {
        lines.push("## Writing guidance".to_string());
        for rule in &schema.write_rules {
            lines.push(format!("- {rule}"));
        }
        lines.push(String::new());
    }

    // System context
    let system_msg = schema.system_message_str();
    if !system_msg.is_empty() {
        lines.push("## System context".to_string());
        lines.push(system_msg.to_string());
        lines.push(String::new());
    }

    lines.join("\n")
}

/// Render a [`PerEdgeDescription`] to its wire literal — bit-identical to
/// what the schema YAML accepts so consumers can echo the value back
/// without case fiddling. `forbidden` (the default) is emitted explicitly
/// rather than omitted so a schema without an explicit declaration still
/// surfaces the resolved posture on the wire.
pub fn per_edge_description_str(p: PerEdgeDescription) -> &'static str {
    match p {
        PerEdgeDescription::Forbidden => "forbidden",
        PerEdgeDescription::Optional => "optional",
        PerEdgeDescription::Required => "required",
    }
}

/// Stable wire string for the `manual_authoring` posture.
pub fn manual_authoring_str(p: ManualAuthoring) -> &'static str {
    match p {
        ManualAuthoring::Allow => "allow",
        ManualAuthoring::Warn => "warn",
        ManualAuthoring::Forbidden => "forbidden",
    }
}

/// Verbosity selector for [`build_schema_payload`].
///
/// `Full` is the complete payload — every description, `when_to_use`,
/// write-rule, and writing-guidance string. `Lite` drops that long-form
/// prose and returns a structural skeleton: entity-type names with their
/// section keys and metadata-field shapes, relationship names with their
/// allowed endpoints. The skeleton keeps every *flag* an agent needs to
/// author a legal write — the alias-model pointer, required-section and
/// required-field markers, endpoint constraints, the manual-authoring
/// posture, the `acyclic` flag, and the per-edge-description posture — so
/// a lite caller can plan a write without round-tripping to full and
/// without walking into a write-time refusal. Full and lite emit the two
/// heavy arrays under *distinct keys* (`types` / `relationships` vs.
/// `types_summary` / `relationships_summary`), so a consumer decodes by
/// key presence rather than by branching on the request shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SchemaVerbosity {
    #[default]
    Full,
    Lite,
}

impl SchemaVerbosity {
    /// Parse the wire token (`"full"` / `"lite"`). Returns `None` for an
    /// unrecognized token so the calling surface can raise a typed error
    /// naming the bad value rather than silently defaulting. An absent
    /// parameter maps to `Full` at the call site, not here.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "full" => Some(Self::Full),
            "lite" => Some(Self::Lite),
            _ => None,
        }
    }

    /// The wire token for this verbosity.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Lite => "lite",
        }
    }
}

/// Trust origin of a schema (or the mem that pins it), decided at
/// adopt/write time and reported — never re-derived — on the read path.
///
/// `FirstParty` is an engine built-in or a schema authored/explicitly
/// trusted in this workspace. Its prose-instruction fields
/// (`system_context`, `write_rules`, `writing_guidance`, `when_to_use`,
/// prose `description`, `default_writing_guidance`) guide *authoring* in
/// this workspace and are served in full.
///
/// `ThirdParty` is a schema that arrived from outside this workspace
/// (registry-installed or adopted from a foreign folder/clone) and has
/// not been explicitly trusted. Memstead's value proposition pulls a
/// mem's schema directly into a consuming agent's context, where the
/// schema's free-text fields are framed *as instructions* ("System
/// context", "Writing guidance"). A third-party schema is therefore
/// served structural-only: [`build_schema_payload`] forces the
/// [`SchemaVerbosity::Lite`] skeleton regardless of the requested
/// verbosity, omitting every prose-instruction field. This is lossless
/// for the legitimate use case — the omitted fields only guide writing,
/// and a write never targets a foreign mem.
///
/// The class is unforgeable by a publisher: it is decided by *how* the
/// schema entered the workspace, not by any content the schema carries.
/// An unknown/ambiguous origin classifies `ThirdParty` — the safe
/// default (a stranger's prose is never served as first-party
/// instructions on the strength of a missing label).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OriginClass {
    /// Engine built-in, or authored/explicitly trusted in this workspace.
    FirstParty,
    /// Arrived from outside this workspace and not explicitly trusted.
    /// The safe default for an unlabelled/ambiguous origin.
    #[default]
    ThirdParty,
}

impl OriginClass {
    /// The wire token for this origin (`"first-party"` / `"third-party"`),
    /// emitted on every schema read so a consuming host can quarantine
    /// non-first-party content.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::FirstParty => "first-party",
            Self::ThirdParty => "third-party",
        }
    }

    /// Whether this origin must have its schema served structural-only
    /// (prose-instruction fields omitted) on the read path.
    pub fn is_third_party(self) -> bool {
        matches!(self, Self::ThirdParty)
    }
}

/// Build the transport-neutral, rmcp-free JSON payload for a schema read
/// (`memstead_schema`). Shared by the MCP server, the HTTP surface, and
/// the filesystem-mem MCP flavour so every surface emits identical
/// schema-read bytes from one source. `used_by` lists the writable mems
/// whose pinned schema resolves to this one; `verbosity` toggles the full
/// payload versus the lightweight skeleton (see [`SchemaVerbosity`]).
///
/// `origin` ([`OriginClass`]) is reported on the wire as `origin` and
/// governs de-framing: a [`OriginClass::ThirdParty`] schema is served
/// structural-only — the requested `verbosity` is overridden to
/// [`SchemaVerbosity::Lite`] so none of its prose-instruction fields
/// (`system_context`, `write_rules`, `writing_guidance`, `when_to_use`,
/// prose `description`, `default_writing_guidance`) reach a consuming
/// agent as instructions. A `full`-verbosity request on a third-party
/// schema therefore still omits them — the override is one-directional.
pub fn build_schema_payload(
    schema: &Arc<Schema>,
    used_by: Vec<String>,
    verbosity: SchemaVerbosity,
    origin: OriginClass,
) -> serde_json::Value {
    let manifest = &schema.manifest;
    // De-frame third-party schemas: their prose-instruction fields only
    // guide authoring (which never targets a foreign mem), so omitting
    // them is lossless — and serving them would place a stranger's
    // free-text in the consuming agent's instruction context. The Lite
    // skeleton keeps every structural flag an agent needs to understand
    // and query the mem. The override is one-directional: a `full`
    // request cannot re-admit the prose for a third-party schema.
    let verbosity = if origin.is_third_party() {
        SchemaVerbosity::Lite
    } else {
        verbosity
    };

    // `_default` is the schema's internal weight-fallback knob — it
    // sets the edge weight every `_default`-less rel-type inherits and
    // is *not* a usable rel-type on `memstead_relate` (the relate path
    // rejects it with `INVALID_REL_TYPE`). Surfacing it in the agent-
    // facing vocabulary cost one round-trip per
    // session as agents tried it and learned the asymmetry by trial,
    // so it is suppressed here: the schema response advertises only
    // the rel-types `memstead_relate` actually accepts. Schemas that
    // declare `_default` for weight purposes are unaffected — the
    // engine still consults it for `edge_weight` fallback.
    let relationships: Vec<serde_json::Value> = manifest
        .relationships
        .definitions
        .iter()
        .filter(|d| d.name != "_default")
        .map(|d| {
            // Surface the `acyclic` flag so agents can predict cycle-check
            // refusal from introspection without trial-and-error.
            // Combined with each type's `propagating_relationships`
            // list (below), the schema response fully describes the
            // self-loop / long-cycle gates.
            //
            // Surface the `manual_authoring` posture so agents see at
            // introspection time which rel-types refuse explicit
            // `memstead_relate` (forbidden), warn softly (warn), or
            // admit explicit authoring (allow, default).
            //
            // Surface the source/target type pinning declared on the
            // schema's `RelationshipDefinition` so agents can pre-filter
            // rel-types for their `(from_type, to_type)` pair from
            // introspection instead of trial-and-error against
            // `INVALID_REL_SHAPE`. Field names mirror the
            // `INVALID_REL_SHAPE` `details.allowed_source_types` /
            // `details.allowed_target_types` payload so the agent
            // learns the contract once. Empty arrays = "any type
            // admitted" (no pinning).
            serde_json::json!({
                "name": d.name,
                "description": d.description,
                "when_to_use": d.when_to_use,
                "default_weight": d.default_weight,
                "acyclic": d.acyclic,
                "per_edge_description": per_edge_description_str(d.per_edge_description),
                "manual_authoring": manual_authoring_str(d.manual_authoring),
                "allowed_sources": d.source_types,
                "allowed_targets": d.target_types,
            })
        })
        .collect();

    // Outbound cross-mem vocabulary, one entry per target schema.
    // Same shape as the YAML — `{ to_schema, definitions: [...] }` —
    // so consumers can decode the section symmetrically with the
    // intra-mem `relationships` array. `_default` filtering mirrors
    // the intra-mem block; the rest of the per-definition shape is
    // identical so a single decoder handles both.
    let cross_mem_relationships: Vec<serde_json::Value> = manifest
        .cross_mem_relationships
        .iter()
        .map(|entry| {
            let definitions: Vec<serde_json::Value> = entry
                .definitions
                .iter()
                .filter(|d| d.name != "_default")
                .map(|d| {
                    serde_json::json!({
                        "name": d.name,
                        "description": d.description,
                        "when_to_use": d.when_to_use,
                        "default_weight": d.default_weight,
                        "source_types": d.source_types,
                        "target_types": d.target_types,
                        "per_edge_description": per_edge_description_str(d.per_edge_description),
                    })
                })
                .collect();
            serde_json::json!({
                "to_schema": entry.to_schema,
                "definitions": definitions,
            })
        })
        .collect();

    // Iterate type names in manifest-declared order so the output is
    // deterministic and matches the schema author's intent.
    let types_full: Vec<serde_json::Value> = manifest
        .types
        .iter()
        .filter_map(|name| schema.types.get(name.as_str()).map(|td| (name, td)))
        .map(|(_, td)| {
            let sections: Vec<serde_json::Value> = td
                .sections
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "key": s.key,
                        "heading": s.heading,
                        "required": s.required,
                        "write_rules": s.write_rules,
                    })
                })
                .collect();

            let fields: Vec<serde_json::Value> = td
                .metadata_fields
                .iter()
                .map(|f| {
                    let mut obj = serde_json::json!({
                        "name": f.key,
                        "description": f.description,
                        "required": !f.optional,
                    });
                    if let Some(enum_values) = &f.enum_values {
                        obj.as_object_mut()
                            .unwrap()
                            .insert("enum".into(), serde_json::json!(enum_values));
                    }
                    // Surface schema-declared `default_value` so agents
                    // see what the create path fills in when a required
                    // field is omitted. Without this, the engine appears
                    // to silently default — `priority: mid` on a
                    // `coverage_gap` would land with no schema-side
                    // explanation of where the value came from.
                    if let Some(default) = &f.default_value {
                        obj.as_object_mut()
                            .unwrap()
                            .insert("default".into(), serde_json::json!(default));
                    }
                    // Surface the `filterable` posture so an agent constructs
                    // valid `filters` / `range_filters` from the schema body
                    // in one shot. Always present: `"equality"` accepts
                    // `filters`, `"range"` accepts `range_filters`, `null`
                    // means not filterable.
                    obj.as_object_mut().unwrap().insert(
                        "filterable".into(),
                        match f.filterable.as_wire_str() {
                            Some(s) => serde_json::json!(s),
                            None => serde_json::Value::Null,
                        },
                    );
                    obj
                })
                .collect();

            // Expose the per-type `propagating_relationships` list so agents
            // can predict self-loop refusal. The engine refuses
            // `memstead_relate type=R from=X(type=T) to=X` whenever R
            // appears here, independent of R's `acyclic` flag.
            serde_json::json!({
                "name": td.name,
                "description": td.description,
                "when_to_use": td.when_to_use,
                "sections": sections,
                "fields": fields,
                "writing_guidance": td.write_rules,
                "system_context": td.system_message_str(),
                "staleness_threshold_days": td.staleness_threshold_days,
                "propagating_relationships": td.propagating_relationships,
            })
        })
        .collect();

    let mode = match manifest.relationships.mode {
        RelationshipMode::Strict => "strict",
        RelationshipMode::Open => "open",
    };

    let full = verbosity == SchemaVerbosity::Full;

    // Scalar fields present in BOTH modes. `ref` names the schema even
    // in the lite skeleton; `relationship_mode`, `community`, and
    // `used_by` are bounded and cheap.
    let mut payload = serde_json::json!({
        "ref": format!("{}@{}", manifest.name, schema.version),
        "relationship_mode": mode,
        "community": {
            "resolution": manifest.community.resolution,
            "seed": manifest.community.seed,
        },
        "used_by": used_by,
        // Machine-readable trust origin, present in both modes. A
        // consuming host reads this to decide whether to treat the
        // schema as workspace instructions (`first-party`) or quarantine
        // it as untrusted (`third-party`). Additive — a client that
        // ignores it still decodes the rest of the payload unchanged.
        "origin": origin.as_wire(),
    });
    let obj = payload.as_object_mut().unwrap();

    // Schema-level prose — FULL mode only. An agent that asked for the
    // lite skeleton is orienting on structure; the human-readable
    // `description` / `when_to_use` is exactly the weight the lite cut
    // exists to drop. The schema `ref` still identifies the schema.
    if full {
        obj.insert(
            "description".into(),
            serde_json::Value::String(manifest.description.clone()),
        );
        obj.insert(
            "when_to_use".into(),
            serde_json::Value::String(manifest.when_to_use.clone()),
        );
    }

    // Schema-level `alias_target_rel_type` pointer — names the rel-type
    // that body wiki-links `[[target]]` auto-emit through the
    // alias-synthesis pass. Present in BOTH modes: it governs whether an
    // unbacked wiki-link bakes an edge or refuses with
    // `WIKILINK_WITHOUT_RELATION`, so dropping it from lite would leave a
    // caller one round-trip from a write-time refusal. Schemas omitting
    // the field render with the key absent so existing agents don't see
    // a noisy `null`.
    if let Some(target) = &manifest.alias_target_rel_type {
        obj.insert(
            "alias_target_rel_type".into(),
            serde_json::Value::String(target.clone()),
        );
    }

    // Surface `default_writing_guidance` at the top level so plugin-side
    // resolvers can concatenate the schema-generic prose with per-mem
    // additions without parsing schema YAML themselves. FULL mode only —
    // it is guidance prose. Field-by-field omission — a schema with
    // neither `avoid` nor `goal` declared emits no key at all (both
    // `Option<String>` inside an `Option<DefaultWritingGuidance>`).
    if full && let Some(dwg) = &manifest.default_writing_guidance {
        let mut block = serde_json::Map::new();
        if let Some(avoid) = &dwg.avoid {
            block.insert("avoid".into(), serde_json::Value::String(avoid.clone()));
        }
        if let Some(goal) = &dwg.goal {
            block.insert("goal".into(), serde_json::Value::String(goal.clone()));
        }
        if !block.is_empty() {
            obj.insert(
                "default_writing_guidance".into(),
                serde_json::Value::Object(block),
            );
        }
    }

    if full {
        obj.insert(
            "relationships".into(),
            serde_json::Value::Array(relationships),
        );
        // Only surface the cross-mem block when the schema declares
        // outbound entries — keeps the response minimal for schemas
        // that don't speak cross-mem vocabulary.
        if !cross_mem_relationships.is_empty() {
            obj.insert(
                "cross_mem_relationships".into(),
                serde_json::Value::Array(cross_mem_relationships),
            );
        }
        obj.insert("types".into(), serde_json::Value::Array(types_full));
    } else {
        // Lite relationship form: name + endpoint constraints
        // (`allowed_sources`/`allowed_targets`) + manual-authoring
        // posture + `acyclic` + per-edge-description posture — every flag
        // that governs a relate-path refusal (`INVALID_REL_SHAPE`,
        // `RELATION_MANUAL_AUTHORING_FORBIDDEN`, cycle check,
        // `MISSING_REQUIRED_DESCRIPTION`) — with the description /
        // when_to_use / weight prose dropped. The ~42 rel-types carry the
        // bulk of the bytes, so this is the load-bearing half of the cut.
        // Projected from the rich array so each field value has one source.
        let relationships_summary: Vec<serde_json::Value> = relationships
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r["name"],
                    "allowed_sources": r["allowed_sources"],
                    "allowed_targets": r["allowed_targets"],
                    "manual_authoring": r["manual_authoring"],
                    "acyclic": r["acyclic"],
                    "per_edge_description": r["per_edge_description"],
                })
            })
            .collect();
        obj.insert(
            "relationships_summary".into(),
            serde_json::Value::Array(relationships_summary),
        );

        // Lite cross-mem form mirrors the intra-mem lite shape:
        // name + endpoint pinning, prose dropped. Same emit-when-non-empty
        // rule as full mode.
        if !cross_mem_relationships.is_empty() {
            let cross_summary: Vec<serde_json::Value> = cross_mem_relationships
                .iter()
                .map(|e| {
                    let definitions: Vec<serde_json::Value> = e["definitions"]
                        .as_array()
                        .map(|defs| {
                            defs.iter()
                                .map(|d| {
                                    serde_json::json!({
                                        "name": d["name"],
                                        "source_types": d["source_types"],
                                        "target_types": d["target_types"],
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    serde_json::json!({
                        "to_schema": e["to_schema"],
                        "definitions": definitions,
                    })
                })
                .collect();
            obj.insert(
                "cross_mem_relationships_summary".into(),
                serde_json::Value::Array(cross_summary),
            );
        }

        // Lite entity-type form: name + section keys (each with its
        // `required` marker) + metadata-field shapes (name, required,
        // `enum`, `default`) + `propagating_relationships` — the
        // structural minimum to author a legal write — with the
        // type/section prose (descriptions, write_rules, writing_guidance,
        // system_context) dropped. `propagating_relationships` rides along
        // because it governs the self-loop relate refusal (relate R X→X
        // when R propagates on type T), one of the refusals the lite view
        // must let an agent avoid. Projected from the rich array.
        let types_summary: Vec<serde_json::Value> = types_full
            .iter()
            .map(|t| {
                let sections: Vec<serde_json::Value> = t["sections"]
                    .as_array()
                    .map(|secs| {
                        secs.iter()
                            .map(|s| {
                                serde_json::json!({
                                    "key": s["key"],
                                    "required": s["required"],
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let fields: Vec<serde_json::Value> = t["fields"]
                    .as_array()
                    .map(|fs| {
                        fs.iter()
                            .map(|f| {
                                let mut o = serde_json::Map::new();
                                o.insert("name".into(), f["name"].clone());
                                o.insert("required".into(), f["required"].clone());
                                if let Some(e) = f.get("enum") {
                                    o.insert("enum".into(), e.clone());
                                }
                                if let Some(d) = f.get("default") {
                                    o.insert("default".into(), d.clone());
                                }
                                serde_json::Value::Object(o)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                serde_json::json!({
                    "name": t["name"],
                    "sections": sections,
                    "fields": fields,
                    "propagating_relationships": t["propagating_relationships"],
                })
            })
            .collect();
        obj.insert(
            "types_summary".into(),
            serde_json::Value::Array(types_summary),
        );
    }

    payload
}

/// Format a metadata field definition as a single bullet line.
fn describe_metadata_field(field: &memstead_schema::MetadataFieldDef) -> String {
    let type_str = match field.field_type {
        FieldType::String => "String",
        FieldType::Number => "Number",
        FieldType::Date => "Date",
        FieldType::Boolean => "Boolean",
    };

    let mut flags: Vec<&str> = Vec::new();
    if field.optional {
        flags.push("optional");
    } else {
        flags.push("required");
    }
    if field.init_timestamp {
        flags.push("auto-init");
    }
    if field.auto_timestamp {
        flags.push("auto-update");
    }
    match field.serialization {
        Serialization::CsvArray => flags.push("csv array"),
        Serialization::OmitWhenFalsy => flags.push("omit when falsy"),
        Serialization::Default => {}
    }

    let mut extras: Vec<String> = Vec::new();
    if let Some(values) = &field.enum_values {
        extras.push(format!("enum: {}", values.join(", ")));
    }
    if let Some(default) = &field.default_value {
        extras.push(format!("default: {default}"));
    }
    let filterable_str = match field.filterable {
        Filterable::None => None,
        Filterable::Equality => Some("filterable: equality"),
        Filterable::Range => Some("filterable: range"),
    };
    if let Some(f) = filterable_str {
        extras.push(f.to_string());
    }

    let extras_str = if extras.is_empty() {
        String::new()
    } else {
        format!(" — {}", extras.join(" — "))
    };

    format!(
        "**{key}**: {type_str} ({flags}){extras_str}",
        key = field.key,
        flags = flags.join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entity, EntityId, ListResult, SearchResult};
    use indexmap::IndexMap;
    use std::collections::HashMap;

    fn make_hit(id: &str, title: &str, entity_type: &str, sections: &[(&str, &str)]) -> SearchHit {
        SearchHit {
            id: EntityId(id.to_string()),
            title: title.to_string(),
            mem: id.split("--").next().unwrap_or("").to_string(),
            entity_type: entity_type.to_string(),
            stub: false,
            score: 1.0,
            tokens: 10,
            snippet: None,
            sections: sections
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            score_breakdown: None,
            matched_terms: None,
            expansion: None,
            // Test fixtures exercise the render-time fallback (default-schema
            // lookup); the engine-precomputed path is set in the search op.
            summary: None,
        }
    }

    fn search_result(hits: Vec<SearchHit>) -> SearchResult {
        let returned = hits.len();
        let total_tokens = hits.iter().map(|h| h.tokens).sum();
        SearchResult {
            total: returned,
            returned,
            offset: 0,
            total_tokens,
            hits,
            facets: None,
            warnings: vec![],
        }
    }

    fn list_result(hits: Vec<SearchHit>) -> ListResult {
        let returned = hits.len();
        ListResult {
            total: returned,
            returned,
            offset: 0,
            total_tokens: hits.iter().map(|h| h.tokens).sum(),
            hits,
            warnings: vec![],
        }
    }

    fn test_entity() -> Entity {
        Entity {
            id: EntityId("specs--test-entity".to_string()),
            title: "Test Entity".to_string(),
            entity_type: "spec".to_string(),
            mem: "specs".to_string(),
            file_path: "test-entity.md".to_string(),
            metadata: IndexMap::new(),
            sections: IndexMap::from([
                ("identity".to_string(), "A test entity for unit tests.".to_string()),
                ("purpose".to_string(), "Validates render logic.".to_string()),
                ("specifies".to_string(), "Long section content that adds significant token weight to the full entity estimate.".to_string()),
            ]),
            relationships: vec![],
            content_hash: "abc123".to_string(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn section_key_to_heading_basic() {
        assert_eq!(section_key_to_heading("identity"), "Identity");
        assert_eq!(section_key_to_heading("current_state"), "Current state");
    }

    #[test]
    fn render_uses_schema_declared_heading_for_non_trivial_casing() {
        // The `ingest.inconsistency` schema declares `claim_a` with
        // heading "Claim A" — the simple key-derivation would produce
        // "Claim a", which would disagree with the on-disk markdown
        // emitted by the generator. The renderer must echo the
        // schema's declared heading verbatim.
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("claim_a".to_string(), "Body A.".to_string());
        sections.insert("claim_b".to_string(), "Body B.".to_string());

        let entity = Entity {
            id: EntityId("ingest--example".to_string()),
            title: "Example".to_string(),
            entity_type: "inconsistency".to_string(),
            mem: "ingest".to_string(),
            file_path: "example.md".to_string(),
            metadata: IndexMap::new(),
            sections,
            relationships: vec![],
            content_hash: "h".to_string(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        };

        let md = render_entity_markdown(&entity, None);
        assert!(
            md.contains("## Claim A"),
            "expected schema-declared `## Claim A` heading; got:\n{md}"
        );
        assert!(
            md.contains("## Claim B"),
            "expected schema-declared `## Claim B` heading; got:\n{md}"
        );
        // The naive derivation would have produced lower-case `a`/`b`.
        assert!(
            !md.contains("## Claim a"),
            "renderer must not fall back to key-derivation when the \
             schema declares a heading; got:\n{md}"
        );
    }

    #[test]
    fn render_falls_back_to_key_derivation_for_unknown_types() {
        // When the entity_type is not in any built-in schema (custom
        // workspace schemas, legacy entities), the renderer falls back
        // to the simple key→heading derivation.
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("identity".to_string(), "body".to_string());

        let entity = Entity {
            id: EntityId("custom--example".to_string()),
            title: "Example".to_string(),
            entity_type: "not-a-builtin-type".to_string(),
            mem: "custom".to_string(),
            file_path: "example.md".to_string(),
            metadata: IndexMap::new(),
            sections,
            relationships: vec![],
            content_hash: "h".to_string(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        };

        let md = render_entity_markdown(&entity, None);
        assert!(
            md.contains("## Identity"),
            "fallback derivation must produce `## Identity`; got:\n{md}"
        );
    }

    // Regression lock for deterministic section order. The invariant:
    // render_entity_body walks `entity.sections` in IndexMap insertion order,
    // so whatever order the parser/caller inserts is what ships. The parser
    // inserts in schema-declared order; this test deliberately inserts in
    // REVERSE schema order to prove the renderer honors insertion order
    // (not the schema's declared order directly).
    #[test]
    fn render_entity_sections_follow_indexmap_insertion_order() {
        let mut sections: IndexMap<String, String> = IndexMap::new();
        sections.insert("specifies".to_string(), "S content.".to_string());
        sections.insert("purpose".to_string(), "P content.".to_string());
        sections.insert("identity".to_string(), "I content.".to_string());

        let entity = Entity {
            id: EntityId("specs--order-test".to_string()),
            title: "Order Test".to_string(),
            entity_type: "spec".to_string(),
            mem: "specs".to_string(),
            file_path: "order-test.md".to_string(),
            metadata: IndexMap::new(),
            sections,
            relationships: vec![],
            content_hash: "abc123".to_string(),
            stub: false,
            stub_kind: None,
            heading_spans: std::collections::HashMap::new(),
        };

        let md = render_entity_markdown(&entity, None);
        let specifies_pos = md.find("## Specifies").expect("## Specifies must appear");
        let purpose_pos = md.find("## Purpose").expect("## Purpose must appear");
        let identity_pos = md.find("## Identity").expect("## Identity must appear");

        assert!(
            specifies_pos < purpose_pos,
            "Specifies (inserted first) must render before Purpose; got:\n{md}"
        );
        assert!(
            purpose_pos < identity_pos,
            "Purpose (inserted second) must render before Identity; got:\n{md}"
        );
    }

    /// `_tokens_unfiltered_body` rides only when a section filter
    /// narrows the rendered output; it carries the unfiltered-base
    /// cost so agents can predict the cost of dropping the filter. The
    /// name avoids a monotonic-relationship implication
    /// that the opt-in path could invert.
    #[test]
    fn tokens_reflect_filtered_output() {
        let entity = test_entity();

        // Full render — no filter
        let full = render_entity_markdown(&entity, None);
        assert!(full.contains("_tokens:"), "should have _tokens");
        assert!(
            !full.contains("_tokens_unfiltered_body:"),
            "should NOT have _tokens_unfiltered_body when unfiltered"
        );
        assert!(
            !full.contains("_tokens_full:"),
            "old _tokens_full name must not survive — rename is one-way"
        );

        // Filtered render — request only "identity"
        let filtered = render_entity_markdown(&entity, Some(&["identity".to_string()]));
        assert!(filtered.contains("_tokens:"), "should have _tokens");
        assert!(
            filtered.contains("_tokens_unfiltered_body:"),
            "should have _tokens_unfiltered_body when filtered"
        );
        assert!(
            !filtered.contains("_tokens_full:"),
            "old _tokens_full name must not survive — rename is one-way"
        );

        // Extract token values
        let full_tokens: usize = full
            .lines()
            .find(|l| l.starts_with("_tokens:"))
            .unwrap()
            .trim_start_matches("_tokens: ")
            .parse()
            .unwrap();
        let filtered_tokens: usize = filtered
            .lines()
            .find(|l| l.starts_with("_tokens:"))
            .unwrap()
            .trim_start_matches("_tokens: ")
            .parse()
            .unwrap();
        let tokens_unfiltered_body: usize = filtered
            .lines()
            .find(|l| l.starts_with("_tokens_unfiltered_body:"))
            .unwrap()
            .trim_start_matches("_tokens_unfiltered_body: ")
            .parse()
            .unwrap();

        assert!(
            filtered_tokens < full_tokens,
            "filtered _tokens ({filtered_tokens}) should be less than full _tokens ({full_tokens})"
        );
        assert!(
            tokens_unfiltered_body >= full_tokens,
            "_tokens_unfiltered_body ({tokens_unfiltered_body}) should be >= full render _tokens ({full_tokens})"
        );
    }

    // -----------------------------------------------------------------------
    // Summary line — search rendering
    // -----------------------------------------------------------------------

    #[test]
    fn render_search_uses_first_required_section_for_spec() {
        let hit = make_hit(
            "specs--demo",
            "Demo Spec",
            "spec",
            &[
                ("identity", "A demo spec."),
                ("purpose", "Verifies rendering."),
            ],
        );
        let out = render_search_markdown(&search_result(vec![hit]), 0);
        assert!(
            out.contains("**Identity**: A demo spec."),
            "expected Identity line for spec hit, got:\n{out}"
        );
    }

    #[test]
    fn render_search_uses_first_required_section_for_memo() {
        let hit = make_hit(
            "memos--d1",
            "Memo One",
            "memo",
            &[("claim", "Some claim."), ("context", "Some context.")],
        );
        let out = render_search_markdown(&search_result(vec![hit]), 0);
        assert!(
            out.contains("**Claim**: Some claim."),
            "expected Claim line for memo hit, got:\n{out}"
        );
        assert!(
            !out.contains("**Identity**"),
            "memo hit must not render Identity label"
        );
        assert!(
            !out.contains("**Purpose**"),
            "memo hit must not render Purpose label"
        );
    }

    #[test]
    fn render_search_uses_first_required_section_for_concept() {
        let hit = make_hit(
            "concepts--thing",
            "Thing",
            "concept",
            &[("definition", "A thing."), ("explanation", "Details.")],
        );
        let out = render_search_markdown(&search_result(vec![hit]), 0);
        assert!(
            out.contains("**Definition**: A thing."),
            "expected Definition line for concept hit, got:\n{out}"
        );
    }

    #[test]
    fn render_search_missing_summary_section_shows_dash() {
        // Memo hit with no "claim" section — renderer falls back to em-dash.
        let hit = make_hit("memos--empty", "Empty Memo", "memo", &[]);
        let out = render_search_markdown(&search_result(vec![hit]), 0);
        assert!(
            out.contains("**Claim**: —"),
            "expected Claim dash fallback, got:\n{out}"
        );
    }

    #[test]
    fn render_search_mixes_schemas_in_one_result() {
        let spec_hit = make_hit(
            "specs--s1",
            "Spec One",
            "spec",
            &[("identity", "Spec body.")],
        );
        let memo_hit = make_hit("memos--m1", "Memo One", "memo", &[("claim", "Memo claim.")]);
        let out = render_search_markdown(&search_result(vec![spec_hit, memo_hit]), 0);
        assert!(
            out.contains("**Identity**: Spec body."),
            "spec hit should still render Identity, got:\n{out}"
        );
        assert!(
            out.contains("**Claim**: Memo claim."),
            "memo hit should render Claim in the same output, got:\n{out}"
        );
    }

    #[test]
    fn render_search_unknown_schema_shows_summary_dash() {
        let hit = make_hit("bogus--x", "Bogus", "bogus", &[]);
        let out = render_search_markdown(&search_result(vec![hit]), 0);
        assert!(
            out.contains("**Summary**: —"),
            "unknown schema should render Summary dash, got:\n{out}"
        );
    }

    #[test]
    fn summary_pair_falls_back_when_schema_has_no_required_sections() {
        use memstead_schema::{SectionDef, TypeDefinition};

        let schema = TypeDefinition {
            name: "spec".to_string(),
            description: "test".to_string(),
            when_to_use: "test".to_string(),
            boundaries: vec![],
            examples: vec![],
            system_message: None,
            sections: vec![SectionDef {
                key: "note".to_string(),
                heading: "Note".to_string(),
                required: false,
                search_weight: 1.0,
                catch_all: false,
                write_rules: vec![],
                description: None,
            }],
            metadata_fields: vec![],
            title_weight: 1.0,
            text_fields: vec![],
            hierarchy_relationship: "PART_OF".to_string(),
            edge_weight_overrides: indexmap::IndexMap::new(),
            edge_weights: indexmap::IndexMap::new(),
            propagating_relationships: vec![],
            updatable_fields: vec![],
            health_required_fields: vec![],
            staleness_threshold_days: 90,
            write_rules: vec![],
            required_outgoing: vec![],
        };

        let mut sections = HashMap::new();
        sections.insert("note".to_string(), "a note".to_string());
        assert_eq!(
            summary_pair(Some(&schema), &sections),
            ("Note".to_string(), "a note".to_string()),
        );

        assert_eq!(
            summary_pair(Some(&schema), &HashMap::new()),
            ("Note".to_string(), "—".to_string()),
        );
    }

    // -----------------------------------------------------------------------
    // Summary line — list rendering (symmetric)
    // -----------------------------------------------------------------------

    #[test]
    fn render_list_uses_first_required_section_for_spec() {
        let hit = make_hit(
            "specs--demo",
            "Demo Spec",
            "spec",
            &[
                ("identity", "A demo spec."),
                ("purpose", "Verifies rendering."),
            ],
        );
        let out = render_list_markdown(&list_result(vec![hit]));
        assert!(
            out.contains("**Identity**: A demo spec."),
            "expected Identity line for spec hit, got:\n{out}"
        );
    }

    #[test]
    fn render_list_uses_first_required_section_for_memo() {
        let hit = make_hit("memos--d1", "Memo One", "memo", &[("claim", "Some claim.")]);
        let out = render_list_markdown(&list_result(vec![hit]));
        assert!(
            out.contains("**Claim**: Some claim."),
            "expected Claim line for memo hit, got:\n{out}"
        );
        assert!(
            !out.contains("**Identity**"),
            "memo hit must not render Identity label in list output"
        );
    }

    #[test]
    fn render_list_uses_first_required_section_for_concept() {
        let hit = make_hit(
            "concepts--thing",
            "Thing",
            "concept",
            &[("definition", "A thing.")],
        );
        let out = render_list_markdown(&list_result(vec![hit]));
        assert!(
            out.contains("**Definition**: A thing."),
            "expected Definition line for concept hit, got:\n{out}"
        );
    }

    #[test]
    fn render_list_missing_summary_section_shows_dash() {
        let hit = make_hit("memos--empty", "Empty Memo", "memo", &[]);
        let out = render_list_markdown(&list_result(vec![hit]));
        assert!(
            out.contains("**Claim**: —"),
            "expected Claim dash fallback in list output, got:\n{out}"
        );
    }

    #[test]
    fn render_list_mixes_schemas_in_one_result() {
        let spec_hit = make_hit(
            "specs--s1",
            "Spec One",
            "spec",
            &[("identity", "Spec body.")],
        );
        let memo_hit = make_hit("memos--m1", "Memo One", "memo", &[("claim", "Memo claim.")]);
        let out = render_list_markdown(&list_result(vec![spec_hit, memo_hit]));
        assert!(
            out.contains("**Identity**: Spec body."),
            "spec hit should still render Identity in list output, got:\n{out}"
        );
        assert!(
            out.contains("**Claim**: Memo claim."),
            "memo hit should render Claim in list output, got:\n{out}"
        );
    }

    #[test]
    fn render_list_unknown_schema_shows_summary_dash() {
        let hit = make_hit("bogus--x", "Bogus", "bogus", &[]);
        let out = render_list_markdown(&list_result(vec![hit]));
        assert!(
            out.contains("**Summary**: —"),
            "unknown schema should render Summary dash in list output, got:\n{out}"
        );
    }

    // -----------------------------------------------------------------------
    // summary_pair — structured-content source of truth
    // -----------------------------------------------------------------------

    #[test]
    fn summary_pair_for_spec_returns_identity() {
        let schema = type_by_name("spec");
        let mut sections = HashMap::new();
        sections.insert("identity".to_string(), "A demo spec.".to_string());
        assert_eq!(
            summary_pair(schema.as_deref(), &sections),
            ("Identity".to_string(), "A demo spec.".to_string()),
        );
    }

    #[test]
    fn summary_pair_for_memo_returns_claim() {
        let schema = type_by_name("memo");
        let mut sections = HashMap::new();
        sections.insert("claim".to_string(), "Memos matter.".to_string());
        assert_eq!(
            summary_pair(schema.as_deref(), &sections),
            ("Claim".to_string(), "Memos matter.".to_string()),
        );
    }

    #[test]
    fn summary_pair_missing_section_returns_dash() {
        let schema = type_by_name("memo");
        assert_eq!(
            summary_pair(schema.as_deref(), &HashMap::new()),
            ("Claim".to_string(), "—".to_string()),
        );
    }

    #[test]
    fn summary_pair_unknown_schema_returns_summary_dash() {
        assert_eq!(
            summary_pair(None, &HashMap::new()),
            ("Summary".to_string(), "—".to_string()),
        );
    }

    // -----------------------------------------------------------------------
    // Envelope serialization — structured-content sidecar
    // -----------------------------------------------------------------------

    #[test]
    fn envelope_serializes_summary_fields() {
        let hit = make_hit(
            "memos--d1",
            "Memo One",
            "memo",
            &[("claim", "Memos matter.")],
        );
        let result = search_result(vec![hit]);
        let envelope = build_search_envelope(&result, 0);
        let value = serde_json::to_value(&envelope).expect("envelope must serialize");

        // The top-level counters use the `_-prefixed` engine-emitted
        // shape so the wire signals "engine-authored metadata, not
        // user data".
        assert_eq!(value["_total"], 1);
        assert_eq!(value["_returned"], 1);
        assert_eq!(value["_offset"], 0);
        // Warnings field is omitted when empty (skip_serializing_if).
        assert!(
            value.get("warnings").is_none(),
            "empty warnings must be elided, got: {value}"
        );

        let hit0 = &value["hits"][0];
        assert_eq!(hit0["summary_heading"], "Claim");
        assert_eq!(hit0["summary_value"], "Memos matter.");
        // Flattened SearchHit fields present.
        assert_eq!(hit0["id"], "memos--d1");
        assert_eq!(hit0["title"], "Memo One");
        assert_eq!(hit0["entity_type"], "memo");
        assert_eq!(hit0["mem"], "memos");
        assert_eq!(hit0["stub"], false);
        assert_eq!(hit0["tokens"], 10);
        assert!(hit0["sections"].is_object());
    }

    #[test]
    fn envelope_roundtrips_through_structured_content() {
        // Mixed-schema result: one spec hit, one memo hit. Both summary pairs
        // must match what summary_pair produces for each schema.
        let spec_hit = make_hit(
            "specs--s1",
            "Spec One",
            "spec",
            &[("identity", "Spec body.")],
        );
        let memo_hit = make_hit("memos--m1", "Memo One", "memo", &[("claim", "Memo claim.")]);
        let result = search_result(vec![spec_hit, memo_hit]);
        let envelope = build_search_envelope(&result, 0);
        let value = serde_json::to_value(&envelope).expect("envelope must serialize");

        let hits = value["hits"].as_array().expect("hits must be array");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0]["summary_heading"], "Identity");
        assert_eq!(hits[0]["summary_value"], "Spec body.");
        assert_eq!(hits[1]["summary_heading"], "Claim");
        assert_eq!(hits[1]["summary_value"], "Memo claim.");
    }

    #[test]
    fn list_envelope_includes_total_tokens() {
        let hit = make_hit(
            "concepts--c1",
            "Thing",
            "concept",
            &[("definition", "A thing.")],
        );
        let result = list_result(vec![hit]);
        let envelope = build_list_envelope(&result);
        let value = serde_json::to_value(&envelope).expect("envelope must serialize");

        // `_`-prefixed engine-meta keys, matching the search envelope.
        assert_eq!(value["_total"], 1);
        assert_eq!(value["_total_tokens"], 10);
        assert!(value.get("total").is_none(), "unprefixed keys retired");
        assert_eq!(value["hits"][0]["summary_heading"], "Definition");
        assert_eq!(value["hits"][0]["summary_value"], "A thing.");
    }

    #[test]
    fn envelope_emits_warnings_when_present() {
        let mut result = search_result(vec![]);
        // Search warnings ship as typed `WarningHint` entries (same
        // `{code, details, message}` envelope every other tool uses).
        result.warnings = vec![crate::ops::WarningHint::FieldNotFilterable {
            field: "foo".to_string(),
        }];
        let envelope = build_search_envelope(&result, 0);
        let value = serde_json::to_value(&envelope).expect("envelope must serialize");
        assert_eq!(value["warnings"][0]["code"], "FIELD_NOT_FILTERABLE");
        assert_eq!(value["warnings"][0]["details"]["field"], "foo");
        assert!(
            value["warnings"][0]["message"]
                .as_str()
                .is_some_and(|m| m.contains("not filterable"))
        );
    }

    // -----------------------------------------------------------------------
    // Per-hit and per-result fields that must appear in the Markdown body.
    // -----------------------------------------------------------------------

    fn tm(field: &str, snippet: &str, heading_path: Option<&[&str]>) -> TermMatch {
        TermMatch {
            field: field.to_string(),
            snippet: snippet.to_string(),
            heading_path: heading_path.map(|p| p.iter().map(|s| s.to_string()).collect()),
        }
    }

    fn sample_facets() -> Facets {
        use crate::ops::SubsectionFacet;
        Facets {
            by_type: HashMap::from([
                ("spec".to_string(), 7),
                ("memo".to_string(), 3),
                ("decision".to_string(), 2),
            ]),
            by_mem: HashMap::from([("specs".to_string(), 10), ("memos".to_string(), 2)]),
            by_level: HashMap::from([("high".to_string(), 4)]),
            by_status: HashMap::from([("active".to_string(), 6)]),
            by_confidence: HashMap::from([("medium".to_string(), 3)]),
            by_subsection: vec![
                SubsectionFacet {
                    path: vec!["specifies".to_string(), "Response Shapes".to_string()],
                    count: 4,
                },
                SubsectionFacet {
                    path: vec!["purpose".to_string(), "Rationale".to_string()],
                    count: 2,
                },
            ],
            by_expansion: HashMap::from([("primary".to_string(), 8), ("expanded".to_string(), 4)]),
        }
    }

    #[test]
    fn render_search_emits_matched_terms_line() {
        let mut hit = make_hit(
            "specs--e1",
            "Entity One",
            "spec",
            &[("identity", "Body text.")],
        );
        hit.matched_terms = Some(HashMap::from([
            (
                "entity".to_string(),
                vec![
                    tm("title", "...entity...", None),
                    tm("purpose", "...entity...", None),
                    tm("purpose", "...entity two...", None),
                ],
            ),
            ("one".to_string(), vec![tm("title", "...one...", None)]),
        ]));
        let out = render_search_markdown(&search_result(vec![hit]), 0);
        assert!(
            out.contains("**Matched terms:**"),
            "missing Matched terms line; got:\n{out}"
        );
        assert!(
            out.contains("`entity` (purpose×2, title×1)"),
            "entity term grouping wrong; got:\n{out}"
        );
        assert!(
            out.contains("`one` (title×1)"),
            "one term grouping wrong; got:\n{out}"
        );
    }

    #[test]
    fn render_search_emits_score_breakdown_line() {
        let mut hit = make_hit("specs--e1", "Entity", "spec", &[("identity", "b")]);
        hit.score_breakdown = Some(ScoreBreakdown {
            bm25: 2.5,
            title_boost: 2.0,
            field_weights: HashMap::from([("body".to_string(), 0.8), ("purpose".to_string(), 0.3)]),
            expansion_decay: Some(0.5),
        });
        let out = render_search_markdown(&search_result(vec![hit]), 0);
        assert!(
            out.contains(
                "**Score:** bm25 2.5 + title 2.0 + body 0.8 + purpose 0.3 + expansion_decay ×0.5"
            ),
            "score breakdown line wrong; got:\n{out}"
        );
    }

    #[test]
    fn render_search_omits_expansion_decay_when_none() {
        let mut hit = make_hit("specs--e1", "Entity", "spec", &[("identity", "b")]);
        hit.score_breakdown = Some(ScoreBreakdown {
            bm25: 1.5,
            title_boost: 1.0,
            field_weights: HashMap::new(),
            expansion_decay: None,
        });
        let out = render_search_markdown(&search_result(vec![hit]), 0);
        assert!(
            out.contains("**Score:** bm25 1.5 + title 1.0"),
            "base score wrong; got:\n{out}"
        );
        assert!(
            !out.contains("expansion_decay"),
            "expansion_decay must be absent when None; got:\n{out}"
        );
    }

    #[test]
    fn render_search_emits_heading_path_line() {
        let mut hit = make_hit("specs--e1", "Entity", "spec", &[("identity", "b")]);
        hit.matched_terms = Some(HashMap::from([(
            "x".to_string(),
            vec![
                tm("purpose", "...x...", Some(&["Purpose", "Rationale"])),
                tm("purpose", "...x...", Some(&["Purpose", "Rationale"])), // duplicate, dedupe
                tm("specifies", "...x...", Some(&["Specifies", "Responses"])),
            ],
        )]));
        let out = render_search_markdown(&search_result(vec![hit]), 0);
        assert!(
            out.contains("**Heading path:** Purpose › Rationale; Specifies › Responses"),
            "heading path line wrong; got:\n{out}"
        );
    }

    #[test]
    fn render_search_emits_expansion_line() {
        let mut hit = make_hit("specs--e2", "Entity Two", "spec", &[("identity", "b")]);
        hit.expansion = Some(ExpansionInfo {
            of: EntityId("specs--seed".to_string()),
            via_edge: "refines".to_string(),
            depth: 1,
        });
        let out = render_search_markdown(&search_result(vec![hit]), 0);
        assert!(
            out.contains("**Expansion:** from `specs--seed` via `refines` (depth 1)"),
            "expansion line wrong; got:\n{out}"
        );
    }

    #[test]
    fn render_search_emits_facets_block() {
        let mut result = search_result(vec![]);
        result.facets = Some(sample_facets());
        let out = render_search_markdown(&result, 0);
        assert!(
            out.contains("## Facets"),
            "facets header missing; got:\n{out}"
        );
        assert!(
            out.contains("- **by_type:** spec=7, memo=3, decision=2"),
            "by_type bucket wrong; got:\n{out}"
        );
        assert!(
            out.contains("- **by_mem:** specs=10, memos=2"),
            "by_mem bucket wrong; got:\n{out}"
        );
        assert!(
            out.contains("- **by_level:** high=4"),
            "by_level bucket wrong; got:\n{out}"
        );
        assert!(
            out.contains("- **by_status:** active=6"),
            "by_status bucket wrong; got:\n{out}"
        );
        assert!(
            out.contains("- **by_confidence:** medium=3"),
            "by_confidence bucket wrong; got:\n{out}"
        );
        assert!(
            out.contains("- **by_expansion:** primary=8, expanded=4"),
            "by_expansion bucket wrong; got:\n{out}"
        );
        assert!(
            out.contains("- **by_subsection:**"),
            "by_subsection header missing; got:\n{out}"
        );
        assert!(
            out.contains("`specifies › Response Shapes`: 4"),
            "subsection facet wrong; got:\n{out}"
        );
    }

    #[test]
    fn render_search_omits_facets_block_when_all_empty() {
        let mut result = search_result(vec![]);
        result.facets = Some(Facets::default());
        let out = render_search_markdown(&result, 0);
        assert!(
            !out.contains("## Facets"),
            "empty facets must not emit header; got:\n{out}"
        );
    }

    /// Every field the search-tool description promises must be rendered
    /// in Markdown. This test exercises all of them in one result and
    /// asserts they all appear.
    #[test]
    fn search_markdown_covers_every_sidecar_field() {
        let mut hit = make_hit(
            "specs--e1",
            "Entity One",
            "spec",
            &[("identity", "Body text.")],
        );
        hit.matched_terms = Some(HashMap::from([(
            "entity".to_string(),
            vec![tm("title", "...entity...", Some(&["Purpose", "Rationale"]))],
        )]));
        hit.score_breakdown = Some(ScoreBreakdown {
            bm25: 1.5,
            title_boost: 1.0,
            field_weights: HashMap::from([("body".to_string(), 0.4)]),
            expansion_decay: Some(0.5),
        });
        hit.expansion = Some(ExpansionInfo {
            of: EntityId("specs--seed".to_string()),
            via_edge: "refines".to_string(),
            depth: 2,
        });

        let mut result = search_result(vec![hit]);
        result.facets = Some(sample_facets());

        let out = render_search_markdown(&result, 0);
        for marker in [
            "## Facets",
            "- **by_type:**",
            "- **by_mem:**",
            "- **by_level:**",
            "- **by_status:**",
            "- **by_confidence:**",
            "- **by_expansion:**",
            "- **by_subsection:**",
            "**Matched terms:**",
            "**Score:**",
            "**Heading path:**",
            "**Expansion:**",
        ] {
            assert!(
                out.contains(marker),
                "lockstep marker `{marker}` missing from search markdown; \
                 update render_search_markdown when adding sidecar fields. got:\n{out}"
            );
        }
    }

    /// The envelope's `relationships[].source` field reads the store's
    /// `EdgeSource` discriminator rather than a hardcoded `"explicit"`,
    /// which would disagree with the stub-adoption
    /// response for alias-synthesised edges (and would be
    /// misleading because REFERENCES carries `manual_authoring:
    /// forbidden`).
    #[test]
    fn build_entity_envelope_source_field_reads_edge_source() {
        let mut entity = test_entity();
        let body_link_target = EntityId("specs--body-link-target".to_string());
        let explicit_target = EntityId("specs--explicit-target".to_string());
        entity.relationships = vec![
            crate::entity::Relationship::new("REFERENCES".to_string(), body_link_target.clone()),
            crate::entity::Relationship::new("USES".to_string(), explicit_target.clone()),
        ];

        let edges = vec![
            crate::store::Edge {
                rel_type: "REFERENCES".to_string(),
                target: body_link_target.clone(),
                source: crate::store::EdgeSource::BodyLink,
            },
            crate::store::Edge {
                rel_type: "USES".to_string(),
                target: explicit_target.clone(),
                source: crate::store::EdgeSource::Explicit,
            },
        ];

        let env = build_entity_envelope(&entity, 0, None, None, None, &edges);
        let relationships = env["relationships"].as_array().expect("array");
        let refs = relationships
            .iter()
            .find(|r| r["rel_type"] == "REFERENCES")
            .expect("REFERENCES present");
        assert_eq!(
            refs["source"], "body_link",
            "alias-synthesised edge must label body_link"
        );
        let uses = relationships
            .iter()
            .find(|r| r["rel_type"] == "USES")
            .expect("USES present");
        assert_eq!(
            uses["source"], "explicit",
            "explicit-authored edge must label explicit"
        );
    }

    /// A relationship whose store edge is missing
    /// (transitional drift, store-rebuild lag) falls back to
    /// `"explicit"` so the envelope doesn't crash. The fallback is
    /// the conservative label — agents already branch on it.
    #[test]
    fn build_entity_envelope_source_field_falls_back_to_explicit_when_edge_missing() {
        let mut entity = test_entity();
        let target = EntityId("specs--unmapped".to_string());
        entity.relationships = vec![crate::entity::Relationship::new("USES".to_string(), target)];
        let edges: Vec<crate::store::Edge> = Vec::new();
        let env = build_entity_envelope(&entity, 0, None, None, None, &edges);
        let relationships = env["relationships"].as_array().expect("array");
        assert_eq!(relationships[0]["source"], "explicit");
    }

    /// Every schema-declared frontmatter key surfaces under the nested
    /// `metadata` map — its single home. The four
    /// formerly-hoisted scalars are not at the top level; the
    /// read-only identity triple (mem/id/type) and underscore-prefixed
    /// internal keys are excluded from the nested map.
    #[test]
    fn build_entity_envelope_nested_metadata_carries_every_schema_field() {
        use crate::entity::MetadataValue;
        let mut entity = test_entity();
        entity.entity_type = "contract".to_string();
        // Pre-fix the envelope dropped every non-promoted key.
        entity.metadata = IndexMap::from([
            ("level".to_string(), MetadataValue::String("M0".to_string())),
            (
                "stability".to_string(),
                MetadataValue::String("stable".to_string()),
            ),
            (
                "created_date".to_string(),
                MetadataValue::String("2026-01-01".to_string()),
            ),
            (
                "last_modified".to_string(),
                MetadataValue::String("2026-05-19".to_string()),
            ),
            (
                "protocol".to_string(),
                MetadataValue::String("https".to_string()),
            ),
            (
                "version".to_string(),
                MetadataValue::String("0.1.0".to_string()),
            ),
            (
                "deprecation_status".to_string(),
                MetadataValue::String("none".to_string()),
            ),
        ]);

        let env = build_entity_envelope(&entity, 0, None, None, None, &[]);

        // Metadata scalars are NOT hoisted to the top level — the
        // nested map is their single home.
        assert!(
            env.get("level").is_none(),
            "level must not be hoisted top-level"
        );
        assert!(
            env.get("stability").is_none(),
            "stability must not be hoisted"
        );
        assert!(
            env.get("created_date").is_none(),
            "created_date must not be hoisted"
        );
        assert!(
            env.get("last_modified").is_none(),
            "last_modified must not be hoisted"
        );
        // `type` stays top-level as identity.
        assert_eq!(env["type"], "contract");

        // Nested map carries every non-internal, non-identity frontmatter key.
        let metadata = env["metadata"].as_object().expect("metadata map");
        assert_eq!(metadata["level"], "M0");
        assert_eq!(metadata["stability"], "stable");
        assert_eq!(metadata["created_date"], "2026-01-01");
        assert_eq!(metadata["last_modified"], "2026-05-19");
        assert_eq!(metadata["protocol"], "https");
        assert_eq!(metadata["version"], "0.1.0");
        assert_eq!(metadata["deprecation_status"], "none");

        // Internal underscore-prefixed keys and the read-only identity
        // triple (mem/id/type) do NOT appear inside the nested map.
        for k in metadata.keys() {
            assert!(
                !k.starts_with('_'),
                "metadata map must not carry underscore-prefixed key `{k}`"
            );
            assert!(
                !["mem", "id", "type"].contains(&k.as_str()),
                "metadata map must not carry identity key `{k}` (it lives top-level)"
            );
        }
    }

    /// Stub envelopes carry an
    /// empty `metadata: {}` map so consumers don't branch on the
    /// map's presence.
    #[test]
    fn build_entity_envelope_stub_carries_empty_metadata_map() {
        let mut entity = test_entity();
        entity.stub = true;
        entity.stub_kind = Some(crate::entity::StubKind::ForwardReference);
        entity.metadata = IndexMap::new();
        let env = build_entity_envelope(&entity, 0, None, None, None, &[]);
        let metadata = env["metadata"]
            .as_object()
            .expect("metadata key present even on stubs");
        assert!(metadata.is_empty(), "stub metadata map must be empty");
    }

    /// A user-defined schema names a
    /// metadata field colliding with structured envelope slots
    /// (`sections`, `relationships`). The colliding name surfaces
    /// under `metadata.sections` / `metadata.relationships` without
    /// disturbing the top-level structured arrays — the nested map
    /// decouples user namespace from engine namespace.
    #[test]
    fn build_entity_envelope_user_field_collisions_isolated_to_nested_map() {
        use crate::entity::MetadataValue;
        let mut entity = test_entity();
        entity.metadata = IndexMap::from([
            (
                "sections".to_string(),
                MetadataValue::String("user-supplied-shadow".to_string()),
            ),
            (
                "relationships".to_string(),
                MetadataValue::String("also-shadowed".to_string()),
            ),
        ]);
        let env = build_entity_envelope(&entity, 0, None, None, None, &[]);
        // Top-level structured slots stay structured.
        assert!(
            env["sections"].is_object(),
            "top-level sections stays a map"
        );
        assert!(
            env["relationships"].is_array(),
            "top-level relationships stays an array"
        );
        // User-supplied collisions land inside the nested map.
        let metadata = env["metadata"].as_object().expect("metadata map");
        assert_eq!(metadata["sections"], "user-supplied-shadow");
        assert_eq!(metadata["relationships"], "also-shadowed");
    }

    /// `_tokens_unfiltered_body` on the structured envelope rides only
    /// when `full_tokens` is supplied (a section filter was active);
    /// the legacy `_tokens_full` name is not present as an alias.
    #[test]
    fn build_entity_envelope_unfiltered_body_token_field_name() {
        let entity = test_entity();
        // Filter-active path — field present under new name.
        let env_filtered = build_entity_envelope(&entity, 10, Some(42), None, None, &[]);
        assert_eq!(env_filtered["_tokens_unfiltered_body"], 42);
        assert!(
            env_filtered.get("_tokens_full").is_none(),
            "_tokens_full must not survive — rename is one-way"
        );
        // No-filter path — field absent under both names.
        let env_unfiltered = build_entity_envelope(&entity, 10, None, None, None, &[]);
        assert!(env_unfiltered.get("_tokens_unfiltered_body").is_none());
        assert!(env_unfiltered.get("_tokens_full").is_none());
    }

    // ------------------------------------------------------------------
    // Schema verbosity (lite vs. full) — Plan 01.
    // ------------------------------------------------------------------

    /// Load the embedded `software` schema (~42 rel-types, 9 entity
    /// types, `alias_target_rel_type: REFERENCES`) — the heaviest builtin,
    /// so the lite cut has something to bite into.
    fn software_schema() -> Arc<Schema> {
        memstead_schema::builtins::load_builtin_schemas()
            .expect("builtins load")
            .into_iter()
            .find(|s| s.manifest.name == "software")
            .expect("software schema is a builtin")
    }

    #[test]
    fn schema_verbosity_wire_round_trips() {
        assert_eq!(
            SchemaVerbosity::from_wire("full"),
            Some(SchemaVerbosity::Full)
        );
        assert_eq!(
            SchemaVerbosity::from_wire("lite"),
            Some(SchemaVerbosity::Lite)
        );
        assert_eq!(SchemaVerbosity::from_wire("brief"), None);
        assert_eq!(SchemaVerbosity::from_wire(""), None);
        assert_eq!(SchemaVerbosity::Full.as_wire(), "full");
        assert_eq!(SchemaVerbosity::Lite.as_wire(), "lite");
        assert_eq!(SchemaVerbosity::default(), SchemaVerbosity::Full);
    }

    /// A first-party schema labels its origin and serves its full prose
    /// under `full`. The origin field is additive and present in both
    /// verbosities so a consuming host can always read it.
    #[test]
    fn first_party_origin_is_labelled_and_keeps_prose() {
        let schema = software_schema();
        let full = build_schema_payload(
            &schema,
            vec!["v".into()],
            SchemaVerbosity::Full,
            OriginClass::FirstParty,
        );
        assert_eq!(full["origin"], "first-party");
        // First-party full keeps the prose-instruction fields.
        assert!(full["description"].is_string());
        let t = &full["types"].as_array().unwrap()[0];
        assert!(t.get("system_context").is_some());
        assert!(t.get("writing_guidance").is_some());

        // The origin label rides the lite skeleton too.
        let lite = build_schema_payload(
            &schema,
            vec!["v".into()],
            SchemaVerbosity::Lite,
            OriginClass::FirstParty,
        );
        assert_eq!(lite["origin"], "first-party");
    }

    /// A third-party schema is de-framed: a `full`-verbosity request is
    /// overridden to the structural-only skeleton, so NONE of the
    /// prose-instruction fields (`system_context`, `writing_guidance`,
    /// section `write_rules`, schema `description` / `when_to_use`,
    /// `default_writing_guidance`, rel `description` / `when_to_use`)
    /// reach a consuming agent — even though `full` was asked for. The
    /// structural skeleton (type/section/field/rel shape) survives so the
    /// mem stays understandable and queryable. This is the refusal
    /// complement: a `full` request cannot re-admit the prose.
    #[test]
    fn third_party_origin_forces_structural_only_even_under_full() {
        let schema = software_schema();
        let full_requested = build_schema_payload(
            &schema,
            vec!["v".into()],
            SchemaVerbosity::Full,
            OriginClass::ThirdParty,
        );

        // Origin label.
        assert_eq!(full_requested["origin"], "third-party");

        // Prose-bearing rich arrays are GONE despite the full request;
        // the structural-only summaries are present instead.
        assert!(
            full_requested.get("types").is_none(),
            "third-party omits the rich `types` array even under full"
        );
        assert!(
            full_requested.get("relationships").is_none(),
            "third-party omits the rich `relationships` array even under full"
        );
        assert!(
            full_requested["types_summary"].is_array(),
            "third-party serves the structural `types_summary` skeleton"
        );
        assert!(
            full_requested["relationships_summary"].is_array(),
            "third-party serves the structural `relationships_summary` skeleton"
        );

        // Schema-level prose-instruction fields dropped.
        assert!(
            full_requested.get("description").is_none(),
            "third-party drops schema description prose"
        );
        assert!(
            full_requested.get("when_to_use").is_none(),
            "third-party drops schema when_to_use prose"
        );
        assert!(
            full_requested.get("default_writing_guidance").is_none(),
            "third-party drops default_writing_guidance prose"
        );

        // Per-type prose-instruction fields dropped.
        for t in full_requested["types_summary"].as_array().unwrap() {
            assert!(
                t.get("system_context").is_none(),
                "third-party drops system_context"
            );
            assert!(
                t.get("writing_guidance").is_none(),
                "third-party drops writing_guidance"
            );
            assert!(
                t.get("description").is_none(),
                "third-party drops type description"
            );
            for s in t["sections"].as_array().unwrap() {
                assert!(
                    s.get("write_rules").is_none(),
                    "third-party drops section write_rules"
                );
            }
        }
        // Per-rel prose dropped.
        for r in full_requested["relationships_summary"].as_array().unwrap() {
            assert!(
                r.get("description").is_none(),
                "third-party drops rel description"
            );
            assert!(
                r.get("when_to_use").is_none(),
                "third-party drops rel when_to_use"
            );
        }

        // A third-party schema served under `full` is byte-identical to
        // the same schema served under `lite` (modulo the origin label,
        // which is identical here) — the override fully collapses to Lite.
        let lite_requested = build_schema_payload(
            &schema,
            vec!["v".into()],
            SchemaVerbosity::Lite,
            OriginClass::ThirdParty,
        );
        assert_eq!(
            full_requested, lite_requested,
            "third-party full must collapse to the lite skeleton"
        );
    }

    #[test]
    fn full_payload_carries_the_rich_arrays_and_prose() {
        let schema = software_schema();
        let full = build_schema_payload(
            &schema,
            vec!["v".into()],
            SchemaVerbosity::Full,
            OriginClass::FirstParty,
        );

        // Full keeps today's contract: rich arrays + schema-level prose.
        assert!(full["types"].is_array(), "full has `types`");
        assert!(full["relationships"].is_array(), "full has `relationships`");
        assert!(
            full.get("types_summary").is_none(),
            "full omits `types_summary`"
        );
        assert!(
            full.get("relationships_summary").is_none(),
            "full omits `relationships_summary`"
        );
        assert!(
            full["description"].is_string(),
            "full keeps schema description"
        );
        assert!(
            full["when_to_use"].is_string(),
            "full keeps schema when_to_use"
        );
        assert_eq!(full["alias_target_rel_type"], "REFERENCES");

        // A full type entry keeps the prose the lite cut drops.
        let t = &full["types"].as_array().unwrap()[0];
        assert!(t["description"].is_string());
        assert!(t.get("writing_guidance").is_some());
        assert!(t.get("system_context").is_some());
        // A full rel entry keeps its prose.
        let r = &full["relationships"].as_array().unwrap()[0];
        assert!(r["description"].is_string());
        assert!(r.get("when_to_use").is_some());
        assert!(r.get("default_weight").is_some());
    }

    #[test]
    fn lite_payload_is_the_structural_skeleton_without_prose() {
        let schema = software_schema();
        let lite = build_schema_payload(
            &schema,
            vec!["v".into()],
            SchemaVerbosity::Lite,
            OriginClass::FirstParty,
        );

        // Heavy arrays under the distinct lite keys; rich keys absent.
        let types = lite["types_summary"]
            .as_array()
            .expect("lite has `types_summary`");
        let rels = lite["relationships_summary"]
            .as_array()
            .expect("lite has `relationships_summary`");
        assert!(lite.get("types").is_none(), "lite omits rich `types`");
        assert!(
            lite.get("relationships").is_none(),
            "lite omits rich `relationships`"
        );

        // Alias pointer + endpoint constraints survive the cut — every
        // flag an agent needs to author a legal write.
        assert_eq!(lite["alias_target_rel_type"], "REFERENCES");

        // Schema-level prose dropped.
        assert!(
            lite.get("description").is_none(),
            "lite drops schema description"
        );
        assert!(
            lite.get("when_to_use").is_none(),
            "lite drops schema when_to_use"
        );
        assert!(
            lite.get("default_writing_guidance").is_none(),
            "lite drops default_writing_guidance"
        );

        // Every entity-type name carries its section keys (with `required`)
        // and field shapes — and NO type/section prose.
        for t in types {
            assert!(t["name"].is_string());
            let sections = t["sections"].as_array().expect("lite type has sections");
            for s in sections {
                assert!(s["key"].is_string(), "section carries its key");
                assert!(s["required"].is_boolean(), "section carries required flag");
                assert!(
                    s.get("write_rules").is_none(),
                    "lite section drops write_rules prose"
                );
                assert!(s.get("heading").is_none(), "lite section drops heading");
            }
            assert!(
                t.get("description").is_none(),
                "lite type drops description"
            );
            assert!(
                t.get("writing_guidance").is_none(),
                "lite type drops writing_guidance"
            );
            assert!(
                t.get("system_context").is_none(),
                "lite type drops system_context"
            );
            // `propagating_relationships` rides along — it governs the
            // self-loop relate refusal, a write-time refusal lite must let
            // an agent avoid.
            assert!(
                t.get("propagating_relationships").is_some(),
                "lite type keeps propagating_relationships"
            );
            // Field shapes present (name + required), prose absent.
            if let Some(fields) = t["fields"].as_array() {
                for f in fields {
                    assert!(f["name"].is_string());
                    assert!(f["required"].is_boolean());
                    assert!(
                        f.get("description").is_none(),
                        "lite field drops description"
                    );
                }
            }
        }

        // Every relationship name carries its allowed endpoints and the
        // refusal-governing flags — and NO description/when_to_use prose.
        for r in rels {
            assert!(r["name"].is_string());
            assert!(
                r.get("allowed_sources").is_some(),
                "lite rel has allowed_sources"
            );
            assert!(
                r.get("allowed_targets").is_some(),
                "lite rel has allowed_targets"
            );
            assert!(
                r.get("manual_authoring").is_some(),
                "lite rel keeps manual_authoring"
            );
            assert!(r.get("acyclic").is_some(), "lite rel keeps acyclic");
            assert!(
                r.get("per_edge_description").is_some(),
                "lite rel keeps per_edge_description"
            );
            assert!(r.get("description").is_none(), "lite rel drops description");
            assert!(r.get("when_to_use").is_none(), "lite rel drops when_to_use");
            assert!(
                r.get("default_weight").is_none(),
                "lite rel drops default_weight"
            );
        }
    }

    #[test]
    fn lite_is_measurably_smaller_than_full() {
        let schema = software_schema();
        let full = build_schema_payload(
            &schema,
            vec!["v".into()],
            SchemaVerbosity::Full,
            OriginClass::FirstParty,
        );
        let lite = build_schema_payload(
            &schema,
            vec!["v".into()],
            SchemaVerbosity::Lite,
            OriginClass::FirstParty,
        );
        let full_len = serde_json::to_string(&full).unwrap().len();
        let lite_len = serde_json::to_string(&lite).unwrap().len();
        assert!(
            lite_len * 2 < full_len,
            "lite ({lite_len} B) must be well under half of full ({full_len} B)"
        );
    }

    #[test]
    fn lite_full_carry_the_same_type_and_rel_names() {
        // The cut drops prose, never an entity type or a rel-type — an
        // agent orienting on lite sees the full vocabulary.
        let schema = software_schema();
        let full = build_schema_payload(
            &schema,
            vec!["v".into()],
            SchemaVerbosity::Full,
            OriginClass::FirstParty,
        );
        let lite = build_schema_payload(
            &schema,
            vec!["v".into()],
            SchemaVerbosity::Lite,
            OriginClass::FirstParty,
        );

        let names = |arr: &serde_json::Value| -> Vec<String> {
            arr.as_array()
                .unwrap()
                .iter()
                .map(|v| v["name"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(names(&full["types"]), names(&lite["types_summary"]));
        assert_eq!(
            names(&full["relationships"]),
            names(&lite["relationships_summary"])
        );
    }
}
