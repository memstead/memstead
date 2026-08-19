---
type: memo
created_date: 2026-08-19T14:33:58Z
last_modified: 2026-08-19T15:58:13Z
status: closed
tags: prune, never-clobber, provenance, attribution, projection-pipeline
---

# The never-clobber merge input has no mechanical source — build writes are unattributed

## Claim
The prune machinery's documented never-clobber guarantee (three-way merge; `PruneMerge::Clean` → a confident delete proposal) cannot be wired at any real call site today, because the engine has no mechanical way to decide "the model side is unchanged since the build wrote it". The `CleanDelete` disposition stays unreachable from `prune_proposals` — every never-clobber candidate conservatively conflict-flags — until mutations carry build attribution.

## Context
Backlog-sweep plan 09b set out to wire the merge input (the plan's decided constraint: wire the promise, do not cut it — with the escape hatch that an unobtainable merge base is a handover finding). The wiring attempt found: (a) the retrievable base leg — the SOURCE artifact at a git-pinned commit — has no byte-level correspondence to the LLM-authored destination entity, so comparing them cannot detect a model-side edit; (b) the destination's mutation trail (folder ledger and commit trailers: actor, client, role author/checker/verifier, logical_operation_id) records WHO mutated but never WHICH FLOW — a build write by an agent acting on a consuming brief is indistinguishable from the same agent's hand edit; (c) briefs instruct no distinguishing declaration. Supplying `Some(PruneMerge::Clean)` from any of these would assert a cleanliness the engine cannot ground — a false clean authorizes exactly the clobber the guarantee forbids.

## Substance

The honest fix direction is an attribution channel, not a cleverer comparison: mutations made while enacting a binding's brief carry the binding id as provenance (a `Binding:` commit trailer / ledger field, instructed by the consuming brief and accepted by the mutation surface). Model-side divergence then becomes computable: an entity is clean when no mutation after its last binding-attributed write came from outside that binding. This is a cross-surface design (brief text, MCP mutation params, ledger/commit wire shape, prune consumption) — its own plan, not a call-site patch. Until then the conservative `None` merge input is the only honest one, and the prune module doc's "not wired this cycle" sentence remains the accurate statement of record.

## Alternatives



## Outcome

Resolved 2026-08-19 by operator re-decision (delegated in-session): the merge input stays unwired and conflict-flag-only is the ACCEPTED posture, not a temporary degradation. Rationale — agent-enriched build entities are not memstead's intended model: source-derived mems are rebuildable mirrors; enrichment belongs in authored entities, so the never-clobber distinction protects a mixed-write flow the product does not encourage, and an attribution channel would be over-engineering for that margin. Plan 09b's criterion 2 was amended to this posture (recorded openly in the plan). The attribution-channel design in ## Substance stays available as a future option; a backlog pointer references this memo.
