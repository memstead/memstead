//! Parameter structs for read-only graph tools.

use memstead_base::ops::Query;
use rmcp::schemars;

/// Parameters for memstead_entity.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityParams {
    #[schemars(
        description = "Full entity ID as returned by search/list (e.g. \"specs--my-entity\")"
    )]
    pub id: String,
    #[schemars(
        description = "Append a `## Relations` section with typed edges grouped by direction."
    )]
    pub include_relations: Option<bool>,
    #[schemars(
        description = "Append a `## Community Context` section — the entity's cluster summary, members, and bridges to other clusters."
    )]
    pub include_context: Option<bool>,
    #[schemars(
        description = "Only return these sections (default: all). Use to read specific parts of large entities."
    )]
    pub sections: Option<Vec<String>>,
    #[schemars(
        description = "Max tokens for the rendered-markdown text channel only. If the text exceeds this, returns chunk 1 of N with _truncated in its frontmatter; use the chunk param to read subsequent chunks. The structured_content envelope is never chunked or truncated by this — it always ships whole (size it ahead via its _tokens field)."
    )]
    pub token_budget: Option<usize>,
    #[schemars(
        description = "Which chunk to read (1-based). Only needed for entities that exceed the token budget."
    )]
    pub chunk: Option<usize>,
}

/// Parameters for memstead_search.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchParams {
    #[schemars(
        description = "Structured flat query. Fields: `any: [terms]` (OR, ranks entities matching more terms higher — no explicit AND needed), `not: [terms]` (exclusion), `phrase: \"exact adjacency\"`, `field: \"title\"|section-key` (narrow all three). Omit (or pass `{}`) to use search as a pure structural/metadata filter — hits come back in title-ascending order. No stemming: include morphological variants explicitly (run, running, runs)."
    )]
    pub query: Option<Query>,
    #[schemars(description = "Only entities in this mem")]
    pub mem: Option<String>,
    #[schemars(description = "Only entities of this type (e.g. \"spec\", \"memo\")")]
    pub entity_type: Option<String>,
    #[schemars(
        description = "Relationship types to follow from primary hits to pull in graph-proximal neighbours (e.g. [\"REALIZES\", \"REFERENCES\"]). Expanded hits carry `expansion: { of, via_edge, depth }` and a decayed score (0.5^depth). `by_expansion` facet shows the primary/expanded composition."
    )]
    pub expand_via: Option<Vec<String>>,
    #[schemars(description = "Max hops to traverse via `expand_via` (default: 1).")]
    pub expand_depth: Option<usize>,
    #[schemars(
        description = "Full entity ID — only return entities within depth hops (BFS, undirected). Results are ranked by proximity: nearer hops first, then a typed (dependency) link to the anchor before a co-mention at the same hop. A neighbourhood larger than the cap is bounded to its nearest members with a `NEIGHBOURHOOD_CAPPED` warning (`kept`/`total`)."
    )]
    pub related_to: Option<String>,
    #[schemars(description = "Max hops from related_to (default: 1, ignored without related_to)")]
    pub depth: Option<usize>,
    #[schemars(description = "Only entities having this edge type (e.g. IMPLEMENTS, USES)")]
    pub edge_type: Option<String>,
    #[schemars(description = "Max results to return (default: all, max: 200)")]
    pub limit: Option<usize>,
    #[schemars(description = "Skip first N results for pagination. Use with limit.")]
    pub offset: Option<usize>,
    #[schemars(
        description = "Equality filters on schema-declared filterable fields, keyed by field name (e.g. `{\"level\": \"M0\", \"status\": \"active\", \"tags\": \"auth\", \"scope\": \"subsystem\"}`). Every field with `filterable: equality` in the type's schema is reachable here. One typed warning per outcome, branch on `code`: `FILTER_TYPE_SCOPED` (a *filterable* key declared only on other types — applied with strict type-narrowing), `FIELD_NOT_FILTERABLE` (declared but not filterable on any reachable type — ignored in both the scoped and unscoped case, result unfiltered not emptied), `UNKNOWN_FILTER_KEY` (no schema declares it — ignored), `INVALID_ENUM_VALUE` (a value outside the field's `enum_values` — the filter applies but matches nothing, so a 0-hit result isn't a true no-match; `details.allowed` lists the values). The per-field `level`/`status`/`confidence` parameters are retired — agents declare any filterable field uniformly through this map. Use `entity_type` (typed parameter) and `edge_type` (typed parameter) for the engine's first-class graph axes, not for metadata filters."
    )]
    pub filters: Option<std::collections::HashMap<String, String>>,
    #[schemars(
        description = "Range filters on schema-declared range-filterable fields, keyed by `min_<field>` / `max_<field>` (numeric) or `<field>_before` / `<field>_after` (date). Example: `{\"created_date_after\": \"2026-01-01\", \"max_score\": \"5\"}`. Every field with `filterable: range` in the type's schema is reachable here. Composable with `filters` (equality). One typed warning per outcome, branch on `code`: `RANGE_FILTER_KEY_MALFORMED` (key lacks a `min_`/`max_`/`*_before`/`*_after` shape), `RANGE_FILTER_TYPE_SCOPED` (a *range-filterable* field declared only on other types — applied with strict type-narrowing), `UNKNOWN_RANGE_FILTER_FIELD` (derived field name not declared on any reachable schema — ignored), `FIELD_NOT_RANGE_FILTERABLE` (field declared but not `filterable: range` on any reachable type — ignored in both the scoped and unscoped case, result unfiltered not emptied)."
    )]
    pub range_filters: Option<std::collections::HashMap<String, String>>,
    #[schemars(
        description = "Filter by stub status. Omit (default) = both stubs and real entities. `true` = stubs only. `false` = real entities only."
    )]
    pub stub: Option<bool>,
    #[schemars(
        description = "Token budget bounding the returned hit payload (default: 12000). A page whose hits exceed it is greedily trimmed to the highest-ranked hits that fit (at least one always returns) and a `SEARCH_RESULTS_TRUNCATED` warning carries `kept`/`budget`; `_total` still reflects the full match count, so page the remainder with `offset` or narrow the query. Raise it to pull more hits in one call when the agent can afford the tokens. Independent of `limit`, which caps the count before the budget trims by size."
    )]
    pub token_budget: Option<usize>,
}

/// Parameters for memstead_overview.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverviewParams {
    #[schemars(
        description = "Re-run community detection before returning overview (default: false). Detection is workspace-global: `rebuild` recomputes the Louvain partition over the *whole* workspace graph — it never scopes to `mem`, even when `mem` is also passed."
    )]
    pub rebuild: Option<bool>,
    #[schemars(
        description = "Which chunk to read (1-based). Only needed if overview exceeds the MCP response cap."
    )]
    pub chunk: Option<usize>,
    #[schemars(
        description = "Restrict `mems[]` and `schemas[]` to any single visible mem — read-only mounts included. `used_by` inside each schema still lists all mems sharing it. Community scope: `mem` filters which clusters are *reported* (and makes `community_bridges` source-in-mem only) — it does NOT re-run detection per mem. Detection is always workspace-global and cluster ids stay the global-pass ids; passing `mem` never renumbers or re-scopes the partition. Because detection is global and disconnected / sparsely-connected nodes collapse into a single catch-all rather than forming their own cluster, a small or isolated mem-local subgraph may surface as no cluster at all under a `mem` filter."
    )]
    pub mem: Option<String>,
    #[schemars(
        description = "Opt into heavy content. Allowed keys: \"community_members\" (entity lists per cluster), \"community_bridges\" (inter-cluster edge aggregation with up to 3 sample edges per pair), \"mem_distribution\" (per-mem type_distribution), \"dangling_links\" (renders a `## Dangling Links` section listing each unresolved body wiki-link as `source → target (in section)`; richer aggregation tracked in #12/#13). `include` keys are always shipped regardless of the token budget — use it to force content you need. Unknown keys emit a typed `warnings` entry. Schema bodies are not in this set — call memstead_schema(name=...) for the full per-type catalogue."
    )]
    pub include: Option<Vec<String>>,
    #[schemars(
        description = "Target token budget for heavy content only (`community_members`, `community_bridges`, `mem_distribution`, `dangling_links`). Default: 8000. Hard-required content (mem roster, schema refs with relationship vocabulary, community titles, workspace policy) always ships in addition — total response size will exceed this budget. When hard-required content alone exceeds the budget, `overview_mode=\"overbudget\"` signals the agent to raise the budget or scope via `mem`. Heavy content not in `include` is greedy-filled until the budget is exhausted; anything left over is advertised in `hints[]` with `estimated_tokens`. `include` keys bypass the budget. Budgets below ~10 tokens are safe but unproductive — the structured envelope still arrives (`overview_mode=\"overbudget\"`) but no useful chunking happens and the full body ships as one chunk."
    )]
    pub token_budget: Option<usize>,
}

/// Parameters for memstead_schema. Exactly one of `name` or `mem`
/// must be supplied; passing both is an `INVALID_INPUT` error.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaParams {
    #[schemars(
        description = "Schema name as listed in memstead_overview's `## Schemas` section (e.g. \"default\" or \"default@1.0.0\"). Schemas are workspace-globally unique by name; the workspace registry resolves a bare name to the pinned version. Mutually exclusive with `mem`."
    )]
    pub name: Option<String>,
    #[schemars(
        description = "Mem name as listed in memstead_overview's `## Mems` section. The engine resolves the mem's pinned `schema_ref` from the workspace's mount roster and proceeds identically to the `name`-driven path. Mutually exclusive with `name`. Returns `UNKNOWN_MEM` when the mem is not mounted."
    )]
    pub mem: Option<String>,
    #[schemars(
        description = "Verbosity of the schema body. `\"lite\"` (default, absent) returns a cheap cold-start skeleton: entity-type names with their section keys (and `required` markers) and metadata-field shapes (name, `required`, `enum`, `default`), relationship-type names with their `allowed_sources`/`allowed_targets`, `manual_authoring`, `acyclic`, and `per_edge_description` — plus the top-level `alias_target_rel_type` pointer — with the long-form prose dropped. The lite skeleton carries every flag needed to author a legal write. `\"full\"` returns the complete payload — every description, `when_to_use`, write-rule, and writing-guidance string; escalate to full for the human-readable guidance before substantial authoring. Heavy arrays ship under distinct keys per mode (`types`/`relationships` vs. `types_summary`/`relationships_summary`). Any value other than `\"full\"`/`\"lite\"` returns `INVALID_INPUT` naming the bad value."
    )]
    pub verbosity: Option<String>,
}
