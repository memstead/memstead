---
name: graph
description: Interact with the Memstead Knowledge Graph — create, query, update, and connect entities via MCP tools. Use when the user wants to work with the knowledge graph.
allowed-tools: mcp__memstead__*
---

# Memstead — Knowledge Graph

You have access to a self-describing Knowledge Graph via the `memstead` MCP tools (prefix: `memstead_`).

## Step 1: Bootstrap (every /graph invocation)

Call `memstead_overview` to understand the graph. The response is token-budget-driven: hard-required content (mem roster, slim schema list `{ref, description}`, community titles) always ships; heavy content (community members, per-mem distribution, community bridges, dangling links) is greedy-filled into the remaining budget by default-priority. Anything that didn't fit appears in `hints[]` with `estimated_tokens` — re-query by passing each `key` into `include[]`. Use `include` to force content you need: `community_bridges` for graph-structure audits, `community_members` for drill-down. Default budget is 8000 tokens; override via `token_budget`. Do not skip this — do not use prior knowledge.

Schema bodies (per-type sections, fields, write_rules, full relationship vocabulary) live on `memstead_schema(name=<ref>)`. Before any `memstead_create` / `memstead_update` / `memstead_relate` against mem X, call `memstead_schema(name=<X.schema_ref>)` once per session and cache the result — schema is workspace-stable.

After reading, briefly summarize the graph structure (2-3 sentences). Then proceed to Step 2.

## Step 2: Execute the task

$ARGUMENTS

## How to work with Memstead

**Exploring:** `memstead_search` (see "Query planning" below), `memstead_entity` (pass `include_relations:true` for typed edges, `include_context:true` for cluster membership + neighbors), `memstead_schema(name=<ref>)` for full per-type bodies, `memstead_health` (include most_connected)
**Creating:** `memstead_create` (always provide: title, mem, identity, level, purpose, specifies)
**Connecting:** `memstead_relate` (from, to, type — types are UPPER_SNAKE_CASE)
**Modifying:** `memstead_update`, `memstead_delete`
**Exporting:** Not an MCP tool anymore — run `memstead-cli export` (human/script) or use the macOS app when you need a full markdown regen or a `.mem` archive.

## Query planning

`memstead_search` runs BM25 lexical search over the graph — it does no semantic expansion on its own. You are expected to expand a concept into keyword variants before calling.

Compose a `query` struct with four optional fields:

- `any: [terms]` — OR semantics. BM25 ranks entities matching more terms higher automatically — no explicit AND is needed. Put every variant here (synonyms, morphology, abbreviations).
- `not: [terms]` — exclusion.
- `phrase: "exact adjacency"` — case- and diacritic-folded exact match.
- `field: "title" | <section-key>` — narrow all three above to one field.

There is **no stemming** — include morphological variants explicitly:

```jsonc
{ "any": ["run", "running", "runs", "ran"] }
```

Example for "how does auth work?":

```jsonc
{
  "any": ["auth", "authentication", "login", "oidc", "token", "session"],
  "not": ["test", "mock", "deprecated"]
}
```

**Omit `query` entirely** (or pass `{}`) to use `memstead_search` as a pure metadata/structural filter — hits come back in title-ascending order, all other scope fields still apply.

**Stub filter.** `stub` is tri-state: omit (default) returns both stubs and real entities, `stub: true` returns only stubs (paginated replacement for `memstead_health include=["stubs"]`), `stub: false` returns only real entities. Each hit carries `stub: bool` so mixed-mode results stay decodable.

**Graph-aware exploration.** Set `expand_via: ["REALIZES", "REFERENCES", ...]` to pull in neighbours reachable via those edge types from each primary hit. Expanded hits carry `expansion: { of, via_edge, depth }` and a decayed score (`0.5^depth`). Read `expansion`, `score_breakdown.expansion_decay`, and the `by_expansion` facet to distinguish primary vs. expanded results.

**Reading the response.** Every hit carries:

- `matched_terms` — per-term snippets keyed by query term, each with `field` and optional `heading_path` (outermost → innermost when a match falls under an H3–H6 sub-heading).
- `score_breakdown` — proportional allocation of `score` across BM25, title boost, and per-field weights. Expanded hits zero the lexical components and populate `expansion_decay`.

`facets` summarises the unpaginated result set: `by_type`, `by_mem`, `by_level`, `by_status`, `by_confidence`, `by_subsection` (structured sub-heading paths — often more precise than entity-level faceting when entities are long), `by_expansion`. Use them to decide whether to narrow or to paginate.

## Versioning

- `/commit` — export + git commit
- `/rollback` — restore entities from git + rebuild graph

## Rules

- **Markdown is source of truth** — the graph is a runtime cache with write-through. Always mutate via MCP tools (never edit markdown directly), because they ensure validation, relationships, and path computation.
- **Never invent** — Write Guidance defines this rule globally
- **IDs are mem-prefixed** — `mem--entity-name` with `--` separator
- **Wiki-links**: `[[name]]` or `[[path/to/name]]` — always resolve within the current mem. Target must be slug-form (lowercase letters, digits, hyphens; path segments separated by `/`). Natural-form `[[Some Title]]` refuses with `INVALID_WIKI_LINK_TARGET` and a `suggested` slug — lift the suggestion into a retry
- **Ask, don't assume** — if information is missing, ask the user
