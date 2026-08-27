---
type: decision
created_date: 2026-08-06T06:41:20Z
last_modified: 2026-08-27T01:04:22Z
status: accepted
decided_on: 2026-08-06
deciders: operator (stability-sweep plan 01), implementing agent
scope: subsystem
tags: anchors, provenance, mutation-surface, mcp
---

# Anchors merge with explicit unset instead of silently replacing

## Decision
Writing `anchors` on a mutation now **merges** into the entity's existing anchor set instead of replacing it wholesale: an incoming anchor replaces the existing anchor with the same `(artifact, grain, class)` triple and appends otherwise. Writing anchors never removes an anchor the call did not name. Removal is explicit through a new `anchors_unset` list on the update surface (MCP `memstead_update`, CLI `--anchor-unset` / `--from` payload / batch-update entries): each selector names an `artifact` and may narrow by `grain` and/or `class`; a bare artifact removes every anchor on it; unset applies **before** the merge in the same mutation; unsetting a nonexistent target is an idempotent no-op — mirroring the `metadata_unset` / `relations_unset` conventions. An empty or absent `anchors` list stays a no-op, never a prune. Full-replace stays expressible with no special mode: unset the artifact(s) and write the new set in one call. The sidecar document format, `INVALID_ANCHOR` validation, and the `_hash` exclusion are unchanged; the shared staging path serves create and update without forking (create passes an empty unset list). Realized in [[engine--anchor-primitive]] and the [[engine--update-mutation]] path.

## Context
The prior write path replaced an entity's whole anchor row per update (`AnchorSidecar::set`), which was a silent data-loss footgun in the provenance layer — observed live when a sync loop regressed an entity's engine coverage from 38 to 31 anchored files because each later anchor batch displaced the earlier set, and no surface said so. `anchors` was also the only collection input on the update args without an unset twin (`metadata` and `declare_relations` both have one).

## Consequences
Incremental anchoring works: sync/ingest batches accumulate instead of displacing each other, so per-batch anchoring no longer needs to re-send the full set. Removal is a deliberate act that shows up in the call, never a side effect of writing. Callers that re-send the full current set keep producing exactly the same final state as under replace, so existing full-set idioms are unaffected. Stale anchors now outlive the write that would previously have swept them — pruning dead anchors is explicit work (a sync/tidy concern), which is the accepted cost of never losing provenance silently.

## Relationships
- **REFERENCES**: [[engine:anchor-primitive]]
- **REFERENCES**: [[engine:update-mutation]]

## Options

**Merge key `(artifact, grain, class)` vs. artifact alone** — artifact-alone would collapse the multiple grains/classes the model explicitly supports per artifact; the triple is the finest identity the record carries. **Idempotent unset vs. refusing a missing target** — refusal would catch typos but forces read-before-every-unset in recovery flows; the relations/metadata twins are the family precedent. **Behaviour switch (replace-mode flag) vs. hard change** — the old semantics are the bug; nothing sanctioned depends on silent loss, and pre-1.0 posture says fix the design.

## Notes

Two refinements landed with consistency-sweep 03/03, both about the same merge key. Within ONE payload the `(artifact, grain, class)` triple must appear at most once: repeats collapsed to the last occurrence and the caller was never told an anchor it sent had gone, which is the same silent loss this decision removed one level up. A later call carrying the triple still replaces the stored row, which is what the merge is for. And a replacing row that omits `hash` now inherits the stored one: dropping it made the next verify re-baseline silently, so drift became unfalsifiable. Supplying a hash still replaces it; unsetting the row before writing it fresh is the explicit clear.
