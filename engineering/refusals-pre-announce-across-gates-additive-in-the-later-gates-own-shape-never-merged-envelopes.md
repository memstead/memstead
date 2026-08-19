---
type: decision
created_date: 2026-08-19T12:22:49Z
last_modified: 2026-08-19T12:22:49Z
status: accepted
decided_on: 2026-08-19
deciders: operator, execute-plan loop (backlog-sweep/09, decision 22)
scope: subsystem
tags: validation, error-envelope, agent-surface, cold-start
---

# Refusals pre-announce across gates — additive, in the later gate's own shape, never merged envelopes

## Decision
When one validation gate refuses a write and a later gate's demands are already fully knowable from the same inputs, the refusal pre-announces those demands additionally — expressed in the later gate's own established payload shape, clearly marked (a `pre_announced` block in `details`), and present only when non-empty. The refusal keeps its code and its established payload byte-for-byte; no code changes name, no field changes shape or meaning; existing decoders keep working unread. First landing: `MISSING_REQUIRED_SECTION` on create pre-announces the metadata gate's `REQUIRED_FIELD_UNSET` demand set under `details.pre_announced.required_field_unset.missing[]`, on all four render surfaces (engine details/prose, full MCP, lean MCP, CLI JSON).

## Context
A first write against an unfamiliar schema cost three refusal round-trips in a fixed order (section gate, metadata gate, value checks — 0-8-0 cold-start triage, backlog F14). Each refusal was self-sufficient, so this was a cost, not a confusion — but first contact is exactly when an agent has least context, and the engine already pre-announced inside one gate (`REQUIRED_FIELD_UNSET` lists the other unset fields under "Also unset"). The alternative — one merged multi-code envelope — was rejected: it breaks every decoder keyed on `code` and re-litigates the typed-error grammar for one optimisation.

## Consequences
Cold writes failing both gates recover in two round-trips instead of three, at zero decoder cost. The contract is best-effort truth: what is announced is true (the announcing computation is the same one the later gate runs), never that everything is announced — gates that cannot run meaningfully against a broken body are not forced. Surfaces that already report every gate separately (the integrity linter) do not pre-announce, avoiding duplication. Future gate pairs wanting the same saving follow this shape instead of inventing envelope merges.

## Options



## Notes


