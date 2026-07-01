---
name: start
user-invocable: false
description: Inspect the current state of the Memstead Knowledge Graph — shows stats, entity directory, stubs, and orphans as a compact dashboard.
allowed-tools: mcp__memstead__*
---

# Memstead — Graph Overview

Inspect the current state of the Memstead Knowledge Graph. No task execution — just show what's there.

## Steps

Run both calls in parallel:

```
memstead_search   stub=false   # omit `query`, exclude stubs — returns every real entity, title-sorted
memstead_health  include=["orphans", "stubs"]
```

`memstead_health` with `include=["orphans", "stubs"]` returns the default graph-size counts and edge-type distribution alongside the requested drill-downs — one call, not two.

## Output Format

Present a compact dashboard:

### 1. Stats
From `memstead_health`: node count (`total_nodes`), edge count (`total_edges`), stub count (`stub_nodes`), and edge type distribution (`edge_types`) as a small table.

### 2. Directory
Group entities from `memstead_search` `hits[]` (structured response) by vault. For each entity show: `id` [level]: identity

### 3. Health
- **Stubs** (unresolved links): list them, or "None ✓"
- **Orphans** (no relationships): list them, or "None ✓"

Keep the output concise — this is a quick status check, not a deep dive.
