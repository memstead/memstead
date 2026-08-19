---
type: decision
created_date: 2026-08-19T08:09:19Z
last_modified: 2026-08-19T08:09:19Z
status: accepted
decided_on: 2026-08-19
deciders: backlog-sweep plan 06a (operator decision 28, 2026-08-18)
scope: subsystem
tags: mcp-surface, schema-discovery, response-budget, agent-ergonomics
---

# The schema serves by the types you will write, and an oversized full reply steers instead of spilling

## Decision
memstead_schema gains per-type retrieval as the primary serving shape for the full-verbosity tier: `types: [names]` returns the complete prose for exactly the named types plus the package-level context, with every unserved type listed in `types_omitted` and an unknown name refusing UNKNOWN_ENTITY_TYPE with the valid roster. The unscoped full reply stays reachable but is budget-guarded: past the schema budget (default 15k estimated tokens, calibrated so the measured 60.2 KB software@0.4.0 spill degrades while today's fitting packages keep serving whole) it degrades visibly — per-type prose drops to the lite skeleton, `_schema_mode: reduced` is stamped, and `_hint` steers to per-type retrieval. Never silent truncation, never harness file-spill. Both MCP flavours share the one builder (`build_schema_payload_scoped`), so the shapes cannot drift.

## Context
The schema-discovery contract routes every fresh writing agent through memstead_schema at session start, and at full verbosity a real schema blew past the MCP response cap (observed 60.2 KB for software@0.4.0, WOENENN ingest 2026-08-18) — the harness spilled it to a file the agent had to read back, a recurring tax the field asked not to let rot. Pure overview-style chunking was rejected as the primary fix: it fits the pipe but still ships the whole package to an agent that needs three types. An agent about to write needs the write_rules for the handful of types it will author — the serving shape should match the need.

## Consequences
No fresh session's first schema call spills to disk on a realistically sized schema; the drill-down step of the discovery contract now has a named parameter (lite skeleton → `types`-scoped full). Existing consumers calling without the new parameters see today's behaviour on today's reply sizes — the lite skeleton is byte-compatible and a fitting unscoped full is untouched. The budget default is a calibration, not a law: if the client response cap moves, the constant moves with it.

## Options



## Notes


