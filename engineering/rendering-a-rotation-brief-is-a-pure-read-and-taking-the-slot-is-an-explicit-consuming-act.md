---
type: decision
created_date: 2026-08-19T01:24:59Z
last_modified: 2026-08-21T17:50:22Z
status: accepted
decided_on: 2026-08-19
deciders: agent session (backlog-sweep plan 03) under the standing engine-change directive
scope: subsystem
tags: projection-pipeline, loop, selection, scheduler, idempotence
---

# Rendering a rotation brief is a pure read and taking the slot is an explicit consuming act

## Decision
`projection brief --all` renders without touching scheduler state by default: the rotation selection runs in a non-mutating peek form that reads the cursor and backoff caches and predicts exactly what a consuming selection would pick, writing nothing. Taking the rotation slot is a separate, explicit act — the `--consume` flag — reserved for the caller that will actually act on the returned brief (the `/sync --all` loop driver and the `/ingest` router both pass it). The same render also discloses, never silently drops, the (binding, operation) pairs the requested filter admits but the binding does not loop-declare: structurally in the JSON envelope (`not_rotated`), on stderr for the markdown form so stdout stays a verbatim prompt.

## Context
The 2026-08-08 sync-loop shutdown exposed the trap: every `--all` render advanced the round-robin cursor and ramped per-pair backoff, so a diagnostic re-render silently skipped the next binding (observed as an engine → flagship skip). The prior posture — "state advances only on advance and its family" — could not carry the fix alone, because build pairs have no advance-family act (`projection advance` refuses sync-less bindings): a pure render whose only consumption channel is advance would re-select an un-acted build pair forever and never ramp its backoff. The explicit `--consume` flag resolves the tension: reads are pure, and consumption is named by the one caller per rotation that acts on the slot.

## Consequences
A diagnostic or repeated render is now safe at any frequency — the rotation stays byte-identical until a consumer takes the slot, and the peek's prediction matches the subsequent consuming selection by construction (the skip decision is computed by a non-mutating twin of the backoff mutation). Callers that act on briefs must pass `--consume`, and any new loop driver inherits that duty; forgetting it degrades to a loop that re-presents the same pair rather than one that silently starves a binding — the failure mode is now loud instead of lossy. Silent eligibility-filter drops are gone from `--all` rendering: an operator reading the envelope sees why a binding never rotates and the one-command enablement remedy. Realized in [[engine--operation-aware-loop-selection-and-backoff]].

## Relationships
- **REFERENCES**: [[engine:operation-aware-loop-selection-and-backoff]]

## Options

Rejected: keeping consumption implicit in every render and documenting "don't re-render" in the sync skill (a documented trap is still a trap; every future consumer re-arms it). Rejected: routing build-slot consumption through the advance family (build has no advance act, and inventing one would gate a read-side fix on a write-surface redesign). Rejected: silently skipping non-loop-declared bindings in `--all` (silence is how the 2026-08-08 pattern went unnoticed).

## Notes

Extended 2026-08-19 (backlog-sweep 09a): the peek's purity covers DERIVED caches, not just scheduler state. The plugin's deny-paths hook cache (`.memstead.cache/projection/active-deny-paths.json`) is now written only by consuming renders — a peek of binding A no longer points the hook at A while a consuming run of B is what actually acts. Any future cache derived from a render inherits the same rule: a read changes no state a later actor depends on.

Carried forward 2026-08-21 (flywheel W6/02): the deny-list cache itself retired with the engine-answered check (`projection check-path`); the consuming render now publishes only the ACTIVE-BINDING pointer (`.memstead.cache/projection/active-binding.json`), and the deny list is read fresh from the binding record on every check. The peek/consume rule is unchanged and still pinned: only a consuming render moves the pointer.
