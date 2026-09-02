---
type: decision
created_date: 2026-09-02T20:10:22Z
last_modified: 2026-09-02T20:10:22Z
status: accepted
decided_on: 2026-09-02
deciders: coordinating session (bundle B), implementing agent, operator directive 2026-09-02
scope: subsystem
tags: health, mem-filter, integrity
---

# Health reports one mem under a filter and one condition per edge

## Decision
Under a mem filter one rule scopes the whole health report: every section (the anchors section, the folder-ledger map and the per-mem config entries included) carries the named mem only, and every warning stays only when it concerns that mem, a warning attributed to no mem concerning every mem; only the mem rosters and `default_writable_mem` stay global. A cross-mem edge whose target mem is not mounted is one finding, the dangling one, never a grant finding while the grant table still names the pair; the grant check answers only for mounted targets.

## Context
Bundle B plans 8 and 9 (2026-09-02). Graders of every earlier plan saw other mems' warnings ride along under a filter (the folder-mem out-of-band notice fell through the attribution), and an edge into a vanished mem was invisible to the integrity axis because its target lingered as a load-time stub.

## Consequences
A scoped read is one mem's picture; the lean MCP server applies the same scope; the CLI markdown files consistency rows under their own heading. The verdict-coverage line's `not_examined` wording, which means not folded into the verdict, stays a backlog candidate.

## Options

Filtering only the anchors section rejected (a half-filter is the same lie in a smaller room); suppressing every grant finding beside a dangling one rejected (it hides a genuinely missing grant).
