---
name: audit
user-invocable: false
description: Detect entity-realization drift — compares entities against current code and reports findings with severity and suggested fixes. Read-only analysis, no mutations.
context: fork
allowed-tools: mcp__memstead__*, Read, Grep, Glob, Bash(git *)
argument-hint: "[full | entity-id]"
---

# Memstead — Audit (Entity-Realization Drift Detection)

Detect where entities have drifted from their realizations. Three modes via `$ARGUMENTS`:

- **Default** (no args): Cheap drift signals first, then deep analysis on flagged entities
- **Full** (`full`): Validate every entity against current code, regardless of history
- **Single** (entity ID): Deep analysis of one specific entity

## Step 1: Select entities

### Default mode (no arguments)

Engine-side `compute_drift` is gone — drift signals are now skill-side via Bash git queries. Run in parallel:

```
memstead_overview include=["community_bridges"]   # inter-cluster edges reveal structural drift between clusters
memstead_search   # omit `query` — pure structural scan returns every entity
```

For each entity with realization paths (look for `### File:` headers and inline backtick paths containing `/` in the `specifies` section), classify it cheaply:

1. **Broken references** — realization path doesn't exist on disk (check via `Glob` / `Bash ls`).
2. **Realization activity** — paths where code changed recently:
   ```bash
   git log --since=30.days.ago --oneline -- <path1> <path2> ...
   ```
   Any output = activity since the entity's last_modified date. Cross-check `last_modified` from metadata against the commit dates to rule out changes the entity already covers.
3. **Commit-message hits** — entity-name or id mentioned in recent commits:
   ```bash
   git log --grep="<entity-name>" --max-count=5 --format="%h %s"
   ```
4. **Propagation** — entities with DEPENDS_ON / USES edges to any drifted entity (via `memstead_entity` with `include_relations=true` on the drifted set).

Rank findings: broken_references > heavy_activity (>5 commits) > medium_activity > commit_keyword_hit > propagated.

Select the top 5 entities by rank.

### Full mode (`$ARGUMENTS` = "full")

Call `memstead_search` with no `query` to get every entity (iterate `hits[]` in the structured response or `### <id>` headings in markdown). Identify every entity with realization file paths in its content (look for `### File:` headers and backtick paths). Skip entities with no realization paths (pure conceptual entities).

### Single mode (`$ARGUMENTS` contains `--`)

It is an entity ID — go directly to Step 2 for that single entity.

## Step 2: Deep analysis

For each selected entity:

1. Call `memstead_entity` with `include_relations=true` to get the full entity and its `_hash`.
2. Extract realization file paths from the entity content (look for `### File:` headers and backtick paths).
3. Read the realization code files using the Read tool.
4. **Default/Single mode only**: Run `git log --oneline --since=<last_modified> -- <realization_paths>` to see what changed. If significant changes, also: `git log --format="%s%n%b" --since=<last_modified> -- <realization_paths>` for commit context.

## Step 3: Adversarial reasoning

For each entity, cross-reference with skepticism:

### The Entity (what should be)
- What concrete claims does this entity make? (data structures, API surfaces, patterns, constraints)
- At what abstraction level does it operate? (ignore implementation details below that level)

### The Code (what is)
- Do the claimed data structures, APIs, patterns still match?
- Were new capabilities added that the entity doesn't cover?
- Were constraints removed or relaxed?

### The Git History (default/single mode only)
- Do commit messages explain WHY changes were made?
- Were changes intentional refactors or incidental drift?
- Treat commit messages with skepticism — "fix bug" could mean anything.

## Step 4: Report

Present findings grouped by severity:

```
### Critical Drift (entity contradicts code)
- `entity-id`: Entity says X but code does Y
  - Evidence: commit abc123 changed Z on 2026-03-05
  - Suggested fix: update entity to document the change

### Structural Gaps (entity is incomplete)
- `entity-id`: Entity documents N items but code has M
  - New items: A, B (added in PR #42)
  - Suggested fix: add the new items to entity content

### Broken References
- `entity-id`: References `packages/old/module.js` which was deleted
  - Deleted in commit def456
  - Suggested fix: update path to `packages/new/module.js`

### Suspect (propagated or uncertain)
- `entity-id`: DEPENDS_ON `drifted-entity` — may need review
```

If no drift detected, say so explicitly: "All analyzed entities match their realizations."

## Rules

- Always show evidence (commit hashes, code snippets) for drift claims.
- Never claim drift without reading the actual code — git history alone is not proof.
- Respect abstraction levels: an entity about "the operations layer" doesn't drift when a variable is renamed.
- When uncertain, report as "suspect" not "drifted".
- Cost awareness: deep analysis of one entity may read 3-5 files. For large audits, batch wisely.
- This is a **report-only** skill. Do not propose to apply fixes — fixes are applied separately by talking to Claude, which routes the changes through the `memstead_*` MCP tools.
