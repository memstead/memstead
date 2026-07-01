---
name: refactor
user-invocable: false
description: Analyze the knowledge graph for structural issues (granularity, orphans, stubs, relationship gaps) and propose improvements.
allowed-tools: mcp__memstead__*
argument-hint: "[entity-id]"
---

# Memstead — Refactor

Analyze the graph for structural issues and propose improvements. Never execute changes without explicit user approval.

## Step 1: Load graph state

Run in parallel:

```
memstead_search   # omit `query` — pure structural scan returns every entity
memstead_health  include=["orphans", "stubs", "most_connected"]
```

`memstead_health` with no `include` already returns node/edge counts and the type distribution — no separate stats call needed. Request drill-downs only when you plan to act on them.

## Step 2: Analyze

Iterate the `hits[]` array from the `memstead_search` structured response (or `### <id>` headings in the markdown). Each hit carries `_tokens` (estimated read cost) and a `summary_value` (lead-section content).

Check every entity against these criteria. For detailed patterns, see [refactoring-patterns.md](refactoring-patterns.md).

### Granularity
- Count `###` headings in each `specifies`. More than 10 headings = candidate for splitting into separate entities with `PART_OF` relationships.
- Very short `specifies` (< 2 sentences) = either missing content or candidate for merging into a related entity.

### Structure
- **Orphans**: real entities with zero relationships — likely missing connections.
- **Stubs**: unresolved wiki-link targets — either create the entity or fix the link.
- **Missing fields**: entities without `identity` or `purpose` — incomplete entities.

### Relationships
- Entities that should be connected but aren't (based on content overlap in `specifies`).
- Relationship types that seem wrong (e.g., `USES` where `PART_OF` fits better).

## Step 3: Present findings

Output a prioritized list grouped by category:

```
### Split candidates (too many headings)
- `entity-id`: 5 headings — suggest splitting into: X, Y, Z

### Incomplete entities
- `entity-id`: missing identity, specifies is 1 sentence

### Orphans
- `entity-id`: no relationships, could connect to X via PART_OF

### Stubs
- `stub-id`: referenced by X — create entity or fix link

### Relationship issues
- `from` → `to`: USES should be PART_OF
```

If no issues found in a category, skip it.

## Step 4: Wait for user

Ask the user which items to address. Only proceed with explicit approval. Execute changes one at a time, showing what changed after each step.

## Scoped mode

If called with an entity ID as argument (`$ARGUMENTS`), analyze only that entity and its direct neighbors instead of the full graph.
