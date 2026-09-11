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
///
/// Projection-free by contract: this is the canonical form (anchor
/// hashing, export, parser round-trips). Serving surfaces that
/// present declared signals or the grounded labelling call
/// [`render_entity_markdown_with_signals`] instead — computed values
/// are a projection and must never enter the canonical bytes.
pub fn render_entity_markdown(entity: &Entity, sections_filter: Option<&[String]>) -> String {
    render_entity_markdown_with_signals(entity, sections_filter, None, None)
}

/// Serving-surface variant of [`render_entity_markdown`]: when the
/// entity's type declares signals, the headline (`name`, `value`,
/// `level` per signal) rides in the frontmatter block — the one
/// pre-body slot the format has — and the contributors in a
/// `## Signals` section appended after the body, in the style of
/// `## Relations`. When the mem's schema declares labelling, the
/// grounded label rides as `_label` in the frontmatter and the
/// evidence in a `## Labelling` section. `None`/`None` renders
/// byte-identically to the canonical form.
pub fn render_entity_markdown_with_signals(
    entity: &Entity,
    sections_filter: Option<&[String]>,
    signals: Option<&[crate::ops::signals::ComputedSignal]>,
    labelling: Option<&crate::ops::labelling::LabellingView>,
) -> String {
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
    // Signal headline — name, value, level per declared signal, in
    // declaration order. The contributors ride in the appended
    // `## Signals` section below, never here.
    if let Some(sigs) = signals
        && !sigs.is_empty()
    {
        let headline: Vec<String> = sigs
            .iter()
            .map(|s| format!("{}: {} ({})", s.name, s.value, s.level_wire()))
            .collect();
        lines.push(format!("_signals: [{}]", headline.join(", ")));
    }
    // Grounded-label headline; the evidence rides in the appended
    // `## Labelling` section below.
    if let Some(lab) = labelling {
        lines.push(format!("_label: {}", lab.label.wire()));
    }
    // The text channel's half of `_unread_sections`. The JSON envelope carries
    // the structured block; a reader on this channel sees the same sections
    // rendered blank and, without this line, has nothing to tell them apart
    // from sections the author left empty. This is the channel a cold agent
    // reads, so it cannot be the one that stays quiet.
    if let Some((absorbing, _)) = entity.sections.iter().find_map(|(k, v)| {
        crate::markdown::closing_fence_if_unterminated(v.trim()).map(|f| (k.clone(), f))
    }) {
        let unread: Vec<&str> = entity
            .sections
            .iter()
            .filter(|(k, v)| **k != absorbing && v.trim().is_empty())
            .map(|(k, _)| k.as_str())
            .collect();
        if !unread.is_empty() {
            lines.push(format!(
                "_unread_sections: [{}] NOT empty: an unterminated code fence in `{absorbing}` \
                 swallowed them, and their content is inside that section's body",
                unread.join(", "),
            ));
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

    // Emit entity metadata. Same predicate as the JSON envelope's
    // metadata map: `_`-prefixed keys are computed read-channel slots
    // (the `_hash` line above, `_tokens`, `_signals`, ...) and the
    // reserved triple is structural identity — a stored key in either
    // namespace would render as a second, stale copy beside the
    // computed one.
    for (key, value) in &entity.metadata {
        if key.starts_with('_')
            || crate::runtime_validator::READ_ONLY_METADATA_KEYS.contains(&key.as_str())
        {
            continue;
        }
        lines.push(format!("{key}: {value}"));
    }
    lines.push("---".to_string());
    lines.push(String::new());

    lines.push(body_text);

    // Contributors — the evidence ships with the number, always. One
    // bullet per signal, mirroring the `## Relations` append style.
    if let Some(sigs) = signals
        && !sigs.is_empty()
    {
        lines.push(String::new());
        lines.push("## Signals".to_string());
        lines.push(String::new());
        for s in sigs {
            if s.contributors.is_empty() {
                lines.push(format!(
                    "- **{}**: {} ({})",
                    s.name,
                    s.value,
                    s.level_wire()
                ));
            } else {
                let ids: Vec<String> = s.contributors.iter().map(|c| c.to_string()).collect();
                lines.push(format!(
                    "- **{}**: {} ({}) — {}",
                    s.name,
                    s.value,
                    s.level_wire(),
                    ids.join(", ")
                ));
            }
        }
    }
    // Labelling evidence — a defeated label always carries its
    // accepted direct attackers (one unanswered counter-claim
    // flipping a well-supported claim is visible as exactly that);
    // an undecided label the open attacker set that keeps it open.
    if let Some(lab) = labelling {
        lines.push(String::new());
        lines.push("## Labelling".to_string());
        lines.push(String::new());
        lines.push(format!("- label: {}", lab.label.wire()));
        if !lab.defeated_by.is_empty() {
            lines.push(format!("- defeated_by: {}", lab.defeated_by.join(", ")));
        }
        if !lab.undecided_by.is_empty() {
            lines.push(format!("- undecided_by: {}", lab.undecided_by.join(", ")));
        }
        if let Some(shape) = &lab.shape {
            let share = match shape.terminal_share {
                Some(s) => format!("{s:.2}"),
                None => "null".to_string(),
            };
            lines.push(format!(
                "- shape: depth {}, branching {:.2}, terminal_share {}, defeated_in_support {}, undecided_in_support {}",
                shape.depth,
                shape.branching,
                share,
                shape.defeated_in_support,
                shape.undecided_in_support,
            ));
        }
    }
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
                "rel_type": e.rel_type,
                "target": e.target.to_string(),
                "source": format!("{:?}", e.source).to_lowercase(),
            })
        })
        .collect();

    let inc: Vec<serde_json::Value> = incoming
        .iter()
        .map(|e| {
            serde_json::json!({
                "rel_type": e.rel_type,
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
/// [out|in] (depth N)`. The direction rides wherever the label does,
/// so a `both` walk stays interpretable per hit.
fn render_expansion_line(expansion: Option<&ExpansionInfo>) -> Option<String> {
    let e = expansion?;
    let dir = match e.via_direction {
        crate::graph::query::TraversalDirection::Out => "out",
        crate::graph::query::TraversalDirection::In => "in",
        // A concrete reaching edge always has one direction; `Both`
        // cannot occur here by construction.
        crate::graph::query::TraversalDirection::Both => "both",
    };
    Some(format!(
        "**Expansion:** from `{}` via `{}` [{dir}] (depth {})",
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
    /// Data-origin label of the hit's mem: `first-party` for a writable
    /// workspace mem, `third-party` for a read-only mount (an installed
    /// read-mem, an adopted foreign folder). Stamped here, once, so the
    /// CLI `--json` and the MCP `structured_content` carry the same key.
    pub origin: &'static str,
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
    /// Always on the wire, `[]` when nothing warned: the envelope is one
    /// stable shape, so a consumer reads `warnings` without testing for
    /// the key first.
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
    /// Always on the wire, `[]` when nothing warned (as on the search
    /// envelope).
    pub warnings: &'a Vec<crate::ops::WarningHint>,
}

/// Build the structured `memstead_entity` envelope. Identity fields
/// (`_hash`, `id`, `mem`, `entity_type`, `title`, `_stub_kind`) come from the
/// parsed `Entity` and live at the top level. Every schema-declared frontmatter
/// key surfaces under a nested `metadata: {...}` map — its single home.
/// Read a metadata
/// value as `envelope.metadata.<key>`; generic consumers iterate the map
/// without per-type branching. The prior shape additionally hoisted
/// `level`/`stability`/`created_date`/`last_modified` to the top level,
/// serialising those fields twice; that hoist is gone. The read-only
/// identity triple (frontmatter `mem`/`id`/`type`, served as `mem`/`id`/`entity_type`) is excluded from the nested map
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
#[allow(clippy::too_many_arguments)] // a pure builder: every arg is used, a params struct would churn 4 call sites for no clarity
pub fn build_entity_envelope(
    entity: &Entity,
    rendered_body_tokens: usize,
    full_tokens: Option<usize>,
    sections_filter: Option<&[String]>,
    schema_anchor: Option<&str>,
    origin: OriginClass,
    outgoing_edges: &[crate::store::Edge],
    incoming_edges: Option<&[crate::store::InEdge]>,
    signals: Option<&[crate::ops::signals::ComputedSignal]>,
    labelling: Option<&crate::ops::labelling::LabellingView>,
) -> serde_json::Value {
    let mut envelope = serde_json::Map::new();
    // Declared aggregate signals — present exactly when the entity's
    // type declares any (the schema author opted in by declaring; a
    // reader who must ask for the signal is a reader who forgets to).
    // Undeclared types keep their byte-identical envelope.
    if let Some(sigs) = signals
        && !sigs.is_empty()
    {
        envelope.insert(
            "_signals".to_string(),
            crate::ops::signals::signals_json(sigs),
        );
    }
    // Grounded labelling — present exactly when the mem's schema
    // declares `relationships.labelling`; the label ships with its
    // evidence, and the shape block exactly when `support` is
    // declared.
    if let Some(lab) = labelling {
        envelope.insert("_labelling".to_string(), lab.to_json());
    }
    envelope.insert(
        "_hash".to_string(),
        serde_json::Value::String(entity.content_hash.clone()),
    );
    // Data-origin trust class, rendered at the shared envelope layer so
    // no read surface can compose an entity read without it. It was
    // previously inserted post-hoc by the MCP handler alone, which left
    // the CLI's `--json` envelope silently unlabelled — a script
    // branching on trust class treated third-party content as
    // first-party there (cold-start 0-8-0, F9/F13).
    envelope.insert(
        "origin".to_string(),
        serde_json::Value::String(origin.as_wire().to_string()),
    );
    // An entity whose body ends inside an unterminated fence has absorbed the
    // sections after it; they come back as `""`, and a reader with no marker
    // cannot tell that from a section the author left blank. Rendered at the
    // shared envelope layer for the same reason `origin` is: it was first
    // written on the conformance axis alone, which is opt-in, so the plain
    // read — the one an agent actually makes — still reported them as merely
    // empty (found by the final grade of that fix).
    //
    // A marker, not a repair: the bytes are unchanged and no fence is closed.
    // `_unread_sections` names the keys whose content is really sitting in
    // `absorbed_into`, so a reader that branches on emptiness has something to
    // branch on.
    if let Some((absorbing, fence)) = entity.sections.iter().find_map(|(k, v)| {
        crate::markdown::closing_fence_if_unterminated(v.trim()).map(|f| (k.clone(), f))
    }) {
        let unread: Vec<String> = entity
            .sections
            .iter()
            .filter(|(k, v)| **k != absorbing && v.trim().is_empty())
            .map(|(k, _)| k.clone())
            .collect();
        envelope.insert(
            "_unread_sections".to_string(),
            serde_json::json!({
                "reason": "UNTERMINATED_FENCE",
                "absorbed_into": absorbing,
                "fence": fence,
                "sections": unread,
                "note": "these sections read as empty because an unterminated code fence in \
                         `absorbed_into` swallowed them: their content is inside that section's \
                         body. Repair through the engine by replacing that section; a write that \
                         does not is refused.",
            }),
        );
    }
    envelope.insert(
        "id".to_string(),
        serde_json::Value::String(entity.id.to_string()),
    );
    envelope.insert(
        "mem".to_string(),
        serde_json::Value::String(entity.mem.clone()),
    );
    // `entity_type`, not `type`: one concept, one wire spelling. The wasm
    // read surface has always served the serialized Entity's `entity_type`,
    // and this envelope said `type` for the same read — the F7-adjacent
    // split the 2026-08-28 wire batch closed. The FRONTMATTER key stays
    // `type:` (on-disk storage format, not the wire), and the metadata
    // read-only triple keeps refusing it under that name.
    envelope.insert(
        "entity_type".to_string(),
        serde_json::Value::String(entity.entity_type.clone()),
    );
    // The `# H1` display title. Structural identity like `id`/`mem`/
    // `entity_type`, so it lives top-level next to them; before this slot the
    // structured envelope had no title at all and consumers had to
    // parse the rendered markdown's H1 to recover it.
    envelope.insert(
        "title".to_string(),
        serde_json::Value::String(entity.title.clone()),
    );

    // Metadata has exactly one home on the envelope — the nested
    // `metadata` map. Scalars like `level`/`stability`/`created_date`/
    // `last_modified` are NOT hoisted to the top level; agents read
    // `envelope.metadata.<key>`. The nested map is authoritative because
    // it carries every schema-declared frontmatter key (including
    // type-specific fields a top-level hoist never covered).
    //
    // Identity keys stay top-level and are excluded here so they too
    // appear exactly once: `_hash`, `id`, `mem`, `entity_type` are the
    // entity's structural identity (inserted above), not free-form
    // metadata. Frontmatter `mem`/`id`/`type` is the engine's read-only key triple
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
    // Every entry declares its direction explicitly. The authored
    // entries (the entity's own `## Relationships` section) are
    // outgoing; incoming edges — when the caller opted in — are
    // appended with `direction: "in"` and the other endpoint under
    // `from`. Before the marker existed the array was silently
    // one-directional: a consumer had no signal that "what depends on
    // this?" was unanswerable from the block (cold-start 0-8-0, F15).
    let mut relationships: Vec<serde_json::Value> = entity
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
                "direction".to_string(),
                serde_json::Value::String("out".to_string()),
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
    if let Some(incoming) = incoming_edges {
        for e in incoming {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "rel_type".to_string(),
                serde_json::Value::String(e.rel_type.clone()),
            );
            obj.insert(
                "from".to_string(),
                serde_json::Value::String(e.from.to_string()),
            );
            obj.insert(
                "direction".to_string(),
                serde_json::Value::String("in".to_string()),
            );
            obj.insert(
                "source".to_string(),
                serde_json::Value::String(
                    match e.source {
                        crate::store::EdgeSource::BodyLink => "body_link",
                        crate::store::EdgeSource::Hierarchy => "hierarchy",
                        crate::store::EdgeSource::Explicit => "explicit",
                    }
                    .to_string(),
                ),
            );
            relationships.push(serde_json::Value::Object(obj));
        }
    }
    envelope.insert(
        "relationships".to_string(),
        serde_json::Value::Array(relationships),
    );

    serde_json::Value::Object(envelope)
}

/// Build a `SearchResultEnvelope` borrowing from `result`. `origin_of`
/// resolves a mem name to its data-origin class (`Engine::mem_origin_class`
/// on a live engine); every hit carries the resolved label, so the two
/// surfaces that serialise this envelope never diverge on the key.
pub fn build_search_envelope<'a>(
    result: &'a SearchResult,
    offset: usize,
    origin_of: &dyn Fn(&str) -> OriginClass,
) -> SearchResultEnvelope<'a> {
    SearchResultEnvelope {
        total: result.total,
        returned: result.returned,
        offset,
        total_tokens: result.total_tokens,
        hits: result
            .hits
            .iter()
            .map(|h| build_hit_envelope(h, origin_of))
            .collect(),
        facets: result.facets.as_ref(),
        warnings: &result.warnings,
    }
}

/// Build a `ListResultEnvelope` borrowing from `result`; `origin_of` as on
/// [`build_search_envelope`].
pub fn build_list_envelope<'a>(
    result: &'a ListResult,
    origin_of: &dyn Fn(&str) -> OriginClass,
) -> ListResultEnvelope<'a> {
    ListResultEnvelope {
        total: result.total,
        returned: result.returned,
        offset: result.offset,
        total_tokens: result.total_tokens,
        hits: result
            .hits
            .iter()
            .map(|h| build_hit_envelope(h, origin_of))
            .collect(),
        warnings: &result.warnings,
    }
}

fn build_hit_envelope<'a>(
    hit: &'a SearchHit,
    origin_of: &dyn Fn(&str) -> OriginClass,
) -> SearchHitEnvelope<'a> {
    let (heading, value) = hit_summary_pair(hit);
    SearchHitEnvelope {
        hit,
        summary_heading: heading,
        summary_value: value,
        origin: origin_of(&hit.mem).as_wire(),
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
        "Run `memstead type <name>` to see its metadata fields, sections, relationship types, and writing guidance — over MCP, `memstead_schema` takes the *schema* name and returns every type at once."
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
    render_type_info_markdown_in(schema, None)
}

/// Like [`render_type_info_markdown`], with the parent [`memstead_schema::Schema`]
/// supplied so relationship rows can carry their schema-level authoring
/// posture. Without it, `memstead type` rendered `REFERENCES` beside 40
/// authorable rel-types with no marker, and the manual-authoring refusal
/// arrived only AFTER an agent had composed (and lost) an all-or-nothing
/// batch — the ban must be visible before the write.
pub fn render_type_info_markdown_in(
    schema: &TypeDefinition,
    parent: Option<&memstead_schema::Schema>,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("# Type: {}", schema.name.as_str()));
    lines.push(String::new());
    lines.push(format!(
        "Staleness threshold: {} days. Hierarchy: `{}`.",
        schema.staleness_threshold_days, schema.hierarchy_relationship,
    ));
    if schema.last_resort {
        lines.push(String::new());
        lines.push(
            "Last resort: this is the schema's fallback type, chosen only when no more \
             specific type fits; the `vital_signs` health axis counts what sits on it."
                .to_string(),
        );
    }
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
            .no_self_loop_relationships
            .iter()
            .any(|r| r == rel_type)
        {
            flags.push("no-self-loop");
        }
        // Schema-level authoring posture, when the parent schema is in
        // hand: a rel-type the alias machinery owns (e.g. REFERENCES)
        // is marked here, BEFORE a write, instead of only refusing
        // after a batch is composed.
        if let Some(p) = parent {
            match p.relationship_manual_authoring(rel_type) {
                memstead_schema::ManualAuthoring::Forbidden => {
                    flags.push("manual authoring FORBIDDEN — emitted from body wiki-links only");
                }
                memstead_schema::ManualAuthoring::Warn => {
                    flags.push("manual authoring warns");
                }
                memstead_schema::ManualAuthoring::Allow => {}
            }
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

    // Canonical exemplar — the engine-validated
    // few-shot entity, rendered in the mem markdown shape. The CLI's
    // full-depth type view matches `memstead_schema verbosity: full`.
    if let Some(ex) = &schema.exemplar {
        lines.push("## Exemplar (engine-validated)".to_string());
        lines.push(String::new());
        lines.push(format!("Title: {}", ex.title));
        if !ex.metadata.is_empty() {
            lines.push("Metadata:".to_string());
            for (k, v) in &ex.metadata {
                lines.push(format!("- {k}: {v}"));
            }
        }
        for (key, body) in &ex.sections {
            let heading = schema
                .section(key)
                .map(|s| s.heading.clone())
                .unwrap_or_else(|| key.clone());
            lines.push(format!("### {heading}"));
            lines.push(body.clone());
        }
        if !ex.relations.is_empty() {
            lines.push("Relations (placeholder targets):".to_string());
            for r in &ex.relations {
                match &r.description {
                    Some(d) => lines.push(format!(
                        "- {} → {} — {d}",
                        r.rel_type_name(),
                        r.target_slug()
                    )),
                    None => lines.push(format!("- {} → {}", r.rel_type_name(), r.target_slug())),
                }
            }
        }
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
/// (`memstead_schema`). Shared by the MCP server and the HTTP surface
/// so every surface emits identical
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
/// Append a section's format declaration (plan 08) to its rendered
/// object — only the declared keys, so undeclared sections keep their
/// exact pre-plan shape. `format_severity` renders whenever a
/// `content` declaration exists (the default `block` is a legality
/// fact, not noise).
fn append_section_format(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    s: &memstead_schema::SectionDef,
) {
    if let Some(content) = &s.content {
        obj.insert("content".into(), serde_json::json!(content));
        obj.insert(
            "format_severity".into(),
            serde_json::json!(s.format_severity),
        );
    }
    if let Some(pattern) = &s.item_pattern {
        obj.insert("item_pattern".into(), serde_json::json!(pattern));
    }
    if let Some(table) = &s.table {
        obj.insert("table".into(), serde_json::json!(table));
    }
    if let Some(example) = &s.example {
        obj.insert("example".into(), serde_json::json!(example));
    }
}

/// Unknown type names in a `types` selection passed to
/// [`build_schema_payload_scoped`] — the caller raises a typed refusal
/// naming the valid types (recovery-payload posture, never a silent
/// empty section).
#[derive(Debug, Clone)]
pub struct UnknownSchemaTypes {
    pub unknown: Vec<String>,
    pub known: Vec<String>,
}

/// Token estimate for a serialized JSON payload — routed through the
/// house heuristic ([`crate::chunking::estimate_tokens`]) so "fits the
/// pipe" is judged by the same yardstick every budgeted surface uses.
fn estimate_payload_tokens(value: &serde_json::Value) -> usize {
    serde_json::to_string(value)
        .map(|s| estimate_tokens(&s))
        .unwrap_or(0)
}

/// Default budget for the UNSCOPED full-verbosity schema reply, in
/// estimated (bytes/4) tokens — ~60 KB of JSON. Calibrated against the
/// primary client's ~25k real-token response cap: dense JSON tokenizes
/// well above bytes/4, so 15k estimated sits at the cap's edge. The
/// two measured packages land on the intended sides: `default@1.3.0`
/// (~52 KB) keeps serving in full — today's behaviour on today's reply
/// sizes — while `software@0.4.0` (60.2 KB, the observed harness spill,
/// 2026-08-18 WOENENN ingest) degrades visibly to the per-type steer
/// instead of overflowing the pipe.
pub const DEFAULT_SCHEMA_FULL_BUDGET: usize = 15_000;

pub fn build_schema_payload(
    schema: &Arc<Schema>,
    used_by: Vec<String>,
    verbosity: SchemaVerbosity,
    origin: OriginClass,
) -> serde_json::Value {
    // Unscoped, unbudgeted — the classic shape every existing consumer
    // gets. Infallible by construction (no selection to refuse).
    build_schema_payload_scoped(schema, used_by, verbosity, origin, None, None)
        .expect("no type selection, no refusal")
}

/// [`build_schema_payload`] with the serving-shape controls
/// (an earlier plana): `type_selection` scopes the heavy per-type
/// prose to the named types — the reply carries the full package-level
/// context, the selected types in full, and a `types_omitted` roster
/// naming what was not served (visible scope, never silent truncation).
/// An unknown name refuses with [`UnknownSchemaTypes`]. Under
/// [`SchemaVerbosity::Lite`] the selection filters the skeleton the
/// same way (coherent, though the full tier is the use case).
///
/// `token_budget` guards the UNSCOPED full reply: when the complete
/// payload's estimated tokens exceed the budget, the reply degrades
/// visibly — per-type prose drops to the lite `types_summary` skeleton,
/// `_schema_mode: "reduced"` is stamped, and `_hint` steers the caller
/// to per-type retrieval via `types`. A scoped request is what the
/// budget steers TOWARD, so the selection path is never re-degraded.
pub fn build_schema_payload_scoped(
    schema: &Arc<Schema>,
    used_by: Vec<String>,
    verbosity: SchemaVerbosity,
    origin: OriginClass,
    type_selection: Option<&[String]>,
    token_budget: Option<usize>,
) -> Result<serde_json::Value, UnknownSchemaTypes> {
    let manifest = &schema.manifest;

    // Validate the selection against the manifest roster before any
    // rendering — refuse-with-the-known-names beats a silent empty
    // `types` array.
    if let Some(sel) = type_selection {
        let unknown: Vec<String> = sel
            .iter()
            .filter(|t| !manifest.types.iter().any(|m| m == *t))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(UnknownSchemaTypes {
                unknown,
                known: manifest.types.clone(),
            });
        }
    }
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
            // Combined with each type's `no_self_loop_relationships`
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
            let mut o = serde_json::json!({
                "name": d.name,
                "description": d.description,
                "when_to_use": d.when_to_use,
                "default_weight": d.default_weight,
                "acyclic": d.acyclic,
                "per_edge_description": per_edge_description_str(d.per_edge_description),
                "manual_authoring": manual_authoring_str(d.manual_authoring),
                "allowed_sources": d.source_types,
                "allowed_targets": d.target_types,
            });
            // Derivation declaration — a
            // behaviour-bearing flag (baseline recording, the
            // stale_derivations axis, duplicate-add re-baseline), so
            // it must be visible at introspection time. Emitted only
            // when true so undeclared schemas keep their bytes.
            if d.derivation {
                o["derivation"] = serde_json::json!(true);
            }
            o
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
                    let mut obj = serde_json::json!({
                        "key": s.key,
                        "heading": s.heading,
                        "required": s.required,
                        "write_rules": s.write_rules,
                    });
                    // Section-format declarations (plan 08) — a
                    // legality condition, so it must never be
                    // invisible in the schema response (rendered at
                    // BOTH verbosity levels via the lite projection
                    // below).
                    append_section_format(obj.as_object_mut().unwrap(), s);
                    obj
                })
                .collect();

            let fields: Vec<serde_json::Value> = td
                .metadata_fields
                .iter()
                .map(|f| {
                    let mut obj = serde_json::json!({
                        "name": f.key,
                        "description": f.description,
                        "required": f.is_required(),
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
                    // A declared `value_pattern` is the shape every write
                    // must match (per member on a csv-array field), so an
                    // agent forms a legal value from the skeleton alone.
                    if let Some(pattern) = &f.value_pattern {
                        obj.as_object_mut()
                            .unwrap()
                            .insert("pattern".into(), serde_json::json!(pattern));
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

            // Expose the per-type `no_self_loop_relationships` list so agents
            // can predict self-loop refusal. The engine refuses
            // `memstead_relate type=R from=X(type=T) to=X` whenever R
            // appears here, independent of R's `acyclic` flag.
            //
            // `required_outgoing` is the only declared legality condition
            // on an entity's outgoing edges: each block lists the
            // relationship-name alternatives and the cardinality bound,
            // in declaration order. Always present — a type with no
            // blocks emits an empty list, because an absent key would
            // read as "unknown" and send agents back to the authoring
            // YAML. Cardinality is rendered exactly as declared
            // (`at_least_one` — an open upper bound stays open, never
            // normalised into a number).
            let required_outgoing: Vec<serde_json::Value> = td
                .required_outgoing
                .iter()
                .map(|block| {
                    let mut b = serde_json::json!({
                        "relationships": block.relationships,
                        "cardinality": block.cardinality.to_string(),
                        "severity": block.severity,
                    });
                    // Conditional blocks carry their trigger at both
                    // verbosity levels (the lite skeleton projects this
                    // object unchanged); unconditional blocks keep
                    // their byte-identical three-key shape.
                    if let (Some(wf), Some(wv)) = (&block.when_field, &block.when_value) {
                        b["when_field"] = serde_json::json!(wf);
                        b["when_value"] = serde_json::json!(wv);
                    }
                    b
                })
                .collect();

            // Declared `constraints` — like `required_outgoing`, a
            // legality/health condition that must never be invisible
            // in the schema response (a hidden legality condition is
            // a defect class of its own). Always present, empty list
            // for a type declaring none; each entry restates the
            // declaration with its `severity` (`warn` = health
            // finding, `block` = write-time refusal), in declaration
            // order, at BOTH verbosity levels.
            let constraints: Vec<serde_json::Value> = td
                .constraints
                .iter()
                .map(|c| match c {
                    memstead_schema::ConstraintDef::RequiresWhen {
                        field,
                        when_field,
                        when_value,
                        severity,
                    } => serde_json::json!({
                        "kind": "requires_when",
                        "field": field,
                        "when_field": when_field,
                        "when_value": when_value,
                        "severity": severity,
                    }),
                    memstead_schema::ConstraintDef::Unique { fields, severity } => {
                        serde_json::json!({
                            "kind": "unique",
                            "fields": fields,
                            "severity": severity,
                        })
                    }
                    memstead_schema::ConstraintDef::EnumFromNeighbour {
                        field,
                        rel_type,
                        section,
                        severity,
                    } => serde_json::json!({
                        "kind": "enum_from_neighbour",
                        "field": field,
                        "rel_type": rel_type,
                        "section": section,
                        "severity": severity,
                    }),
                    memstead_schema::ConstraintDef::StatusPropagation {
                        field,
                        value,
                        rel_type,
                        rel_types,
                        direction,
                        severity,
                    } => {
                        let mut c = serde_json::json!({
                            "kind": "status_propagation",
                            "field": field,
                            "value": value,
                            "direction": direction,
                            "severity": severity,
                        });
                        // Echo the declaration's own shape: the
                        // single-name key stays byte-identical, a
                        // relation set rides under `rel_types`.
                        if let Some(single) = rel_type {
                            c["rel_type"] = serde_json::json!(single);
                        }
                        if let Some(set) = rel_types {
                            c["rel_types"] = serde_json::json!(set);
                        }
                        c
                    }
                    memstead_schema::ConstraintDef::TransitionRequiresChecks {
                        field,
                        to_value,
                        relationships,
                        direction,
                        min_related,
                        severity,
                    } => serde_json::json!({
                        "kind": "transition_requires_checks",
                        "field": field,
                        "to_value": to_value,
                        "relationships": relationships,
                        "direction": direction,
                        "min_related": min_related,
                        "severity": severity,
                    }),
                    memstead_schema::ConstraintDef::TransitionRequiresSelfCheck {
                        field,
                        to_value,
                        check_kind,
                        severity,
                    } => serde_json::json!({
                        "kind": "transition_requires_self_check",
                        "field": field,
                        "to_value": to_value,
                        "check_kind": check_kind,
                        "severity": severity,
                    }),
                })
                .collect();
            let mut obj = serde_json::json!({
                "name": td.name,
                "description": td.description,
                "when_to_use": td.when_to_use,
                "sections": sections,
                "fields": fields,
                "writing_guidance": td.write_rules,
                "system_context": td.system_message_str(),
                "staleness_threshold_days": td.staleness_threshold_days,
                "no_self_loop_relationships": td.no_self_loop_relationships,
                "required_outgoing": required_outgoing,
                "constraints": constraints,
            });
            // Reachability obligations — like `required_outgoing`, a
            // health condition the schema response must not hide; the
            // declaration is echoed in its YAML shape. Emitted only
            // when declared so undeclared schemas keep their payload
            // bytes unchanged.
            if !td.must_reach.is_empty() {
                obj["must_reach"] = serde_json::to_value(&td.must_reach)
                    .expect("must_reach declarations serialize");
            }
            // Aggregate-signal declarations — served behaviour (the
            // `_signals` read insert, the health axis, the crossing
            // warning) an agent must see at introspection time; the
            // declaration is echoed in its YAML shape. Emitted only
            // when declared.
            if !td.signals.is_empty() {
                obj["signals"] =
                    serde_json::to_value(&td.signals).expect("signal declarations serialize");
            }
            // Leaf declaration — a legality-relevant fact an agent
            // planning writes must see; emitted only when true so
            // undeclared schemas keep their payload bytes unchanged.
            if td.leaf {
                obj["leaf"] = serde_json::json!(true);
            }
            // Last-resort declaration — the schema's fallback type, the
            // one a writer picks only when no more specific type fits
            // (the `vital_signs` health axis counts what sits on it).
            // A planning agent must see it beside `leaf`; emitted only
            // when declared so undeclared schemas keep their bytes.
            if td.last_resort {
                obj["last_resort"] = serde_json::json!(true);
            }
            // The type's canonical exemplar —
            // engine-validated at install/seal, so what it teaches is
            // exactly what the validator accepts. Rides FULL mode only
            // (this array); the lite projection below drops it by
            // allowlist, so the per-session skeleton stays unchanged.
            // Relation targets are placeholder slugs by contract.
            //
            // The relation entries are emitted in the MUTATION
            // vocabulary (`target` / `rel_type`) — since the
            // 05-front-door/08 rider landed, that is also the authoring
            // spelling (legacy sealed content is translated at load),
            // so an agent copying this payload into `memstead_create`
            // gets a shape the write gate accepts.
            if let Some(ex) = &td.exemplar {
                let relations: Vec<serde_json::Value> = ex
                    .relations
                    .iter()
                    .map(|r| {
                        let mut o = serde_json::json!({
                            "target": r.target_slug(),
                            "rel_type": r.rel_type_name(),
                        });
                        if let Some(d) = &r.description {
                            o["description"] = serde_json::json!(d);
                        }
                        o
                    })
                    .collect();
                obj["exemplar"] = serde_json::json!({
                    "title": ex.title,
                    "metadata": ex.metadata,
                    "sections": ex.sections,
                    "relations": relations,
                });
            }
            obj
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

    // Declared acyclicity sets — a legality condition on the relate
    // path (a cycle in a set's union subgraph refuses), so it ships in
    // BOTH modes; emitted only when declared so undeclared schemas
    // keep their payload bytes unchanged.
    if !manifest.relationships.acyclic_sets.is_empty() {
        obj.insert(
            "acyclic_sets".into(),
            serde_json::to_value(&manifest.relationships.acyclic_sets)
                .expect("acyclic_sets serialize"),
        );
    }
    // Grounded-labelling declaration — served behaviour (the
    // `_labelling` read insert and the `labelling` health axis) an
    // agent must see at introspection time; echoed in its YAML shape,
    // in BOTH modes, only when declared.
    if let Some(lab) = &manifest.relationships.labelling {
        obj.insert(
            "labelling".into(),
            serde_json::to_value(lab).expect("labelling declaration serializes"),
        );
    }

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
        // Schema-level `system_message`, wire-named `system_context` to
        // match the per-type key. Without this the manifest's voice/
        // posture prose is unreachable from the agent surface entirely
        // (its only other consumer is the `memstead type` CLI markdown).
        // Omitted when undeclared so existing schemas render unchanged.
        if let Some(msg) = &manifest.system_message {
            obj.insert(
                "system_context".into(),
                serde_json::Value::String(msg.clone()),
            );
        }
    }

    // One-line effect note for the per-type `no_self_loop_relationships`
    // arrays — present in BOTH modes, right where the field is read.
    // The retired `propagating_relationships` name misled outside
    // schema authors into declaring impact propagation; the renamed
    // key states the single functional effect. Top-level (not
    // per-type) so the note costs one key, not one per type.
    obj.insert(
        "no_self_loop_relationships_effect".into(),
        serde_json::Value::String(
            "Per-type `no_self_loop_relationships` governs exactly one behaviour: \
             memstead_relate refuses a self-loop (from == to) on a rel-type the \
             source type lists here. It does not propagate impact, imply an \
             evidence obligation, or have any other effect (the name says it \
             all). To declare real impact propagation, use the \
             `status_propagation` constraint (`constraints:` on the type), which \
             taints dependents of a terminal status value via a named rel-type \
             and direction and surfaces them as health findings."
                .to_string(),
        ),
    );

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

    // The selection partitions the manifest-ordered type roster into
    // served and omitted halves. `types_omitted` is emitted whenever
    // any type was NOT served in the requested tier — the visible-scope
    // guarantee (a reader always sees what a reply does not carry).
    let selected = |name: &serde_json::Value| -> bool {
        match type_selection {
            None => true,
            Some(sel) => name.as_str().is_some_and(|n| sel.iter().any(|s| s == n)),
        }
    };
    let omitted_names: Vec<serde_json::Value> = types_full
        .iter()
        .filter(|t| !selected(&t["name"]))
        .map(|t| t["name"].clone())
        .collect();

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
        match type_selection {
            Some(_) => {
                let served: Vec<serde_json::Value> = types_full
                    .iter()
                    .filter(|t| selected(&t["name"]))
                    .cloned()
                    .collect();
                obj.insert("types".into(), serde_json::Value::Array(served));
                if !omitted_names.is_empty() {
                    obj.insert(
                        "types_omitted".into(),
                        serde_json::Value::Array(omitted_names),
                    );
                }
            }
            None => {
                obj.insert("types".into(), serde_json::Value::Array(types_full.clone()));
                // Budget guard on the UNSCOPED full reply: when the
                // assembled payload exceeds the budget, degrade
                // visibly — the per-type prose drops to the lite
                // skeleton, the mode is stamped, and the hint steers
                // to per-type retrieval. Never silent truncation: the
                // caller sees `_schema_mode: "reduced"` plus the full
                // roster in `types_omitted`.
                if let Some(budget) = token_budget {
                    let estimated = estimate_payload_tokens(&payload);
                    if estimated > budget {
                        let obj = payload.as_object_mut().unwrap();
                        obj.remove("types");
                        let all_names: Vec<serde_json::Value> =
                            types_full.iter().map(|t| t["name"].clone()).collect();
                        obj.insert(
                            "types_summary".into(),
                            serde_json::Value::Array(lite_types_projection(&types_full)),
                        );
                        obj.insert("types_omitted".into(), serde_json::Value::Array(all_names));
                        obj.insert(
                            "_schema_mode".into(),
                            serde_json::Value::String("reduced".into()),
                        );
                        obj.insert("_estimated_tokens".into(), serde_json::json!(estimated));
                        obj.insert("_token_budget".into(), serde_json::json!(budget));
                        obj.insert(
                            "_hint".into(),
                            serde_json::Value::String(format!(
                                "the full prose for all {} types (~{estimated} tokens) exceeds \
                                 the response budget ({budget}); per-type prose is served as the \
                                 lite skeleton here — request the full prose for exactly the \
                                 types you will write via `types: [\"<name>\", …]` (valid names \
                                 in `types_omitted`)",
                                types_full.len(),
                            )),
                        );
                    }
                }
            }
        }
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
                let mut o = serde_json::json!({
                    "name": r["name"],
                    "allowed_sources": r["allowed_sources"],
                    "allowed_targets": r["allowed_targets"],
                    "manual_authoring": r["manual_authoring"],
                    "acyclic": r["acyclic"],
                    "per_edge_description": r["per_edge_description"],
                });
                if r.get("derivation") == Some(&serde_json::json!(true)) {
                    o["derivation"] = serde_json::json!(true);
                }
                o
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

        // Lite entity-type form — see [`lite_types_projection`]. The
        // selection filters the skeleton the same way it filters the
        // full tier, with the same visible `types_omitted` roster.
        let served: Vec<serde_json::Value> = types_full
            .iter()
            .filter(|t| selected(&t["name"]))
            .cloned()
            .collect();
        obj.insert(
            "types_summary".into(),
            serde_json::Value::Array(lite_types_projection(&served)),
        );
        if !omitted_names.is_empty() {
            obj.insert(
                "types_omitted".into(),
                serde_json::Value::Array(omitted_names),
            );
        }
    }

    Ok(payload)
}

/// Lite entity-type form: name + section keys (each with its
/// `required` marker) + metadata-field shapes (name, required,
/// `enum`, `default`) + `no_self_loop_relationships` +
/// `required_outgoing` — the structural minimum to author a
/// legal write — with the type/section prose (descriptions,
/// write_rules, writing_guidance, system_context) dropped.
/// `no_self_loop_relationships` rides along because it governs
/// the self-loop relate refusal (relate R X→X when type T lists
/// R), one of the refusals the lite view must let an
/// agent avoid. `required_outgoing` rides along because it is
/// the only declared legality condition on outgoing edges —
/// dropping it would make "enough to plan a legal write" false.
/// Projected from the rich array so each field value has one
/// source; also the degrade target for an over-budget unscoped
/// full reply.
fn lite_types_projection(types_full: &[serde_json::Value]) -> Vec<serde_json::Value> {
    types_full
        .iter()
        .map(|t| {
            let sections: Vec<serde_json::Value> = t["sections"]
                .as_array()
                .map(|secs| {
                    secs.iter()
                        .map(|s| {
                            let mut o = serde_json::Map::new();
                            o.insert("key".into(), s["key"].clone());
                            o.insert("required".into(), s["required"].clone());
                            // The format declaration is a
                            // legality condition — the lite
                            // skeleton carries it in full.
                            for k in [
                                "content",
                                "item_pattern",
                                "table",
                                "example",
                                "format_severity",
                            ] {
                                if let Some(v) = s.get(k) {
                                    o.insert(k.into(), v.clone());
                                }
                            }
                            serde_json::Value::Object(o)
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
                            // The declared `value_pattern` binds every
                            // written value (`INVALID_FIELD_VALUE` on a
                            // miss): a legality flag the skeleton must
                            // carry, or an agent that plans from it is
                            // refused on a shape it was never shown
                            // (B6 grader finding, 2026-09-02).
                            if let Some(p) = f.get("pattern") {
                                o.insert("pattern".into(), p.clone());
                            }
                            serde_json::Value::Object(o)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut o = serde_json::json!({
                "name": t["name"],
                "sections": sections,
                "fields": fields,
                "no_self_loop_relationships": t["no_self_loop_relationships"],
                "required_outgoing": t["required_outgoing"],
                "constraints": t["constraints"],
            });
            // Leaf declaration rides the lite skeleton too — it is
            // a legality-relevant per-type fact.
            if t.get("leaf") == Some(&serde_json::json!(true)) {
                o["leaf"] = serde_json::json!(true);
            }
            // The last-resort declaration rides the lite skeleton for
            // the same reason: a writer choosing a type must see which
            // one the schema names as its fallback.
            if t.get("last_resort") == Some(&serde_json::json!(true)) {
                o["last_resort"] = serde_json::json!(true);
            }
            // Reachability obligations ride whole — a health condition
            // the skeleton must not hide; key present only when the
            // full payload carries it.
            if let Some(mr) = t.get("must_reach") {
                o["must_reach"] = mr.clone();
            }
            // Signal declarations ride whole for the same reason.
            if let Some(sig) = t.get("signals") {
                o["signals"] = sig.clone();
            }
            o
        })
        .collect()
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
    if !field.is_required() {
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
    if let Some(pattern) = &field.value_pattern {
        extras.push(format!("pattern: `{pattern}`"));
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
mod tests;
