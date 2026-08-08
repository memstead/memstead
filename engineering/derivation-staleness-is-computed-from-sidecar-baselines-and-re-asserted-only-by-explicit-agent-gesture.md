---
type: decision
created_date: 2026-08-08T22:00:42Z
last_modified: 2026-08-08T22:00:42Z
status: accepted
decided_on: 2026-08-09
deciders: operator (agent-trust plan 12)
scope: subsystem
tags: derivation, staleness, sidecar, baseline, health, schema-vocabulary
---

# Derivation staleness is computed from sidecar baselines and re-asserted only by explicit agent gesture

## Decision
A schema declares per rel-type `derivation: true` — the source derives from the target — and the engine does the bookkeeping the anker project's stamp scripts did by hand: every EXPLICIT write of such an edge (create relations, update declare_relations, relate, and every batch sibling through the same shared predicate) records the target's current content hash as the edge's baseline in the engine-owned derivations sidecar (`.memstead/derivations.json` — the anchors precedent exactly: staged into the same pending set so baseline and edge ride one commit, invisible in the markdown, excluded from `_hash`, filtered from entity listings, exported as `.memstead/` members). The include-gated `stale_derivations` health axis compares baselines against current hashes: differ → stale (source, rel-type, target, both hashes); no baseline → `unbaselined`, distinctly — never fabricated as fresh or stale. The re-baseline gesture is precise: a duplicate-add `memstead_relate` on a declared rel-type refreshes the baseline as its ONE effect, via a sidecar-only commit (the anchor-only-update precedent — a persisted effect rides a real commit), with the refresh STATED on the response (`DERIVATION_BASELINE_REFRESHED` warning + the commit sha) while `_hash`, the markdown, and the edge stay byte-untouched; undeclared rel-types keep the exact historical bare no-op. Warn-tier forever — staleness reviews, never blocks.

## Context
A change to a parent silently invalidated every child derived from it; the anker holding worked around this with scripts copying content hashes into metadata fields as text. The engine already computed both quantities the scripts copied — per-entity hashes and directional edges — and made agents carry them by hand. The retired `propagating_relationships` name ([[engineering--schema-vocabulary-says-only-true-things-via-leaf-declaration-and-the-self-loop-rename]]) had promised propagation behaviour for a month without delivering it; this mechanism delivers it, properly declared. The write-rehearsal and exemplar work supplied the shared-gate discipline: one predicate (`rel_type_declares_derivation`), one staging helper, called from every verb per [[engineering--a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]].

## Consequences
"Which conclusions predate their evidence's latest change" is now a query, and the review gesture is the cheapest possible honest one — re-issue the relate you already know. Costs accepted deliberately: whole-entity baseline granularity over-triggers on unrelated edits (a typo in another section marks derivations stale) — over-triggering costs a cheap re-review while under-triggering silently serves wrong conclusions; section-granular baselines wait on friction-ledger evidence. Batch no-op entries do NOT re-baseline (the explicit gesture is the single relate / single-op MCP list — a batch of all no-ops commits nothing, so a staged refresh would dangle); alias-emitted body-link and hierarchy edges never carry baselines (load-derived, not written — they report unbaselined if their rel-type is declared). Rejected: recomputing against git history (binds staleness to backend history semantics, unboundedly expensive, and destroys the gesture — a review is a decision, not a historical fact); baselines in the relationship's markdown entry (hash noise, diff churn); auto-refresh on sync runs (only an explicit assertion may declare a derivation reviewed — anything else converts the signal back into silence); a constraint form (constraints predicate on current state; staleness needs a recorded past).

## Relationships
- **REFERENCES**: [[schema-vocabulary-says-only-true-things-via-leaf-declaration-and-the-self-loop-rename]]
- **REFERENCES**: [[a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]

## Options

Schema constraint form — rejected (no pure-state predicate can express a recorded past). Markdown-entry baselines — rejected (body noise, diff churn; the sidecar precedent exists for exactly this). Git-history recomputation — rejected (backend-bound, unbounded, gesture-destroying). Sync auto-refresh — rejected (silence regained).

## Notes


