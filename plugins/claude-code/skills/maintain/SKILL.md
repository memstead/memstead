---
name: maintain
user-invocable: false
description: >
  Full graph health check — consistency, community summaries, and issue report.
  Single pass, no state needed.
context: fork
allowed-tools: mcp__memstead__*, Read, Glob, Grep, Bash
---

# Memstead — Maintain

Single-pass graph health check. Fixes what's auto-fixable, flags the rest, refreshes stale community summaries.

## Step 1: Health check

Call `memstead_health` with `include: ["orphans", "stubs", "missing_fields", "stale", "most_connected"]`.

If `total_nodes` is 0 (no real entities) → report "graph is empty, nothing to maintain" and stop.

Collect all issues from the response (each `include` key lands as a top-level field; the `summary` object carries the counts):

| Source field | Issue type |
|-------------|------------|
| `orphans` | Disconnected entities (no relationships) |
| `stubs` | Unresolved wiki-link targets |
| `missing_fields` | Entities missing schema-required health fields |
| `stale` | Entities not modified past their type's staleness threshold |

## Step 2: Fix auto-fixable issues

Only one type of issue is auto-fixable: **broken explicit relations** where the target entity doesn't exist (not even as a stub). For each, call `memstead_relate` with `remove: true`.

Everything else is flagged for human attention or other skills:

| Issue | Route to |
|-------|----------|
| Orphans | `/refactor` or `/structure` |
| Stubs | `/refactor` or `/structure` |
| Missing fields | `/ingest` or manual |
| Stale entities | `/ingest` |

## Step 3: Community structure

Community summaries are generated automatically by the engine — there is no manual
refresh tool. This step *reviews* the clusters, it does not write summaries.

1. Call `memstead_overview`
2. For each cluster, read its auto-generated summary and member list
3. A summary that reads as an incoherent grab-bag of unrelated titles signals a
   *structural* problem — a too-broad community or missing relationships — not a
   summary to hand-author. Record such clusters as findings for the report
   (candidates for `/refactor`), rather than trying to edit the summary directly.

## Step 4: Report

Structured report with all findings:

```
## Maintain Report

**Graph:** X entities, Y edges, Z stubs
**Fixed:** [list of auto-fixed issues, or "none"]
**Open issues:** [grouped by type, or "all clear"]
**Community structure:** M clusters [list any flagged as incoherent, or "all coherent"]
```

Include drift signals from `memstead_health` if available — entities with recent source changes or broken references that may need attention.

## Rules

- **Graph-internal only** — no code reading (that's `/audit`), no structural refactoring (that's `/refactor`).
- **Fix broken relations only** — don't attempt semantic fixes that need human judgment.
- **Community summaries describe the cluster's shared concern** — "These entities define the MCP transport layer" not "Contains entity-a, entity-b, entity-c."
- **No state file** — every invocation is self-contained. Run it whenever you want a health check.
- **Respect read-only mems** — never attempt to fix issues in read-only mems.
