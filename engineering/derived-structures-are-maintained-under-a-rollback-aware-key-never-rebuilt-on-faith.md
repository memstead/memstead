---
type: decision
created_date: 2026-08-24T13:03:26Z
last_modified: 2026-08-24T13:03:26Z
status: accepted
decided_on: 2026-08-22
deciders: flywheel SPEC (W8), implementing agent
scope: subsystem
tags: engine, store, memoization, search-index, community-detection
---

# Derived structures are maintained under a rollback-aware key, never rebuilt on faith

## Decision
The engine's derived structures (the [[engine--per-mem-search-index]] and the community-partition memo of [[engine--community-detection]]) are maintained incrementally under a rollback-aware generation key, never discarded and rebuilt on faith. The Store carries a monotonic generation counter bumped by every mutating method; the counter travels with Clone, so a restored snapshot restores the number its state was recorded under, and a given number never names two different states along one engine's timeline. Both memos store (generation, value) pairs and are served only while the store still stands at that generation. Where identity with a from-scratch rebuild cannot be maintained (a schema switch changing the field set is the known case), that case falls back to an explicit, named rebuild, never a silent widening back to whole-map drop.

## Context
Before this work the search index was dropped wholesale on any mutation, and the Louvain community memo sat behind a once-cell keyed on nothing, so any invalidation forced a full recomputation. The store had no generation, epoch, or dirty set. Because batch update snapshots the store wholesale for rollback, a naive counter was ruled out from the start: a restored snapshot would resurrect a stale generation and serve a wrong memo as fresh, so rollback-awareness was the binding constraint. The cold-path sizing curve could not see the rebuild cost (full load dwarfs it); the warm path of a long-lived server absorbing mutations was the missing measurement that motivated the change.

## Consequences
- Every incremental path is gated by a property-test oracle: indistinguishable from a from-scratch rebuild after arbitrary mutation sequences, driven by hand-rolled seeded generation in the house discipline.
- A refused engine batch leaves the pre-batch memo standing at its generation, and a memo computed from an interim mid-batch state never survives a rollback as fresh (pinned by the generation-keyed rollback regression test).
- Silent staleness became detectable: the memo getters assert in debug builds that a filled memo matches the live generation, so a mutation path that misses its invalidation call fails loudly instead of serving stale results.
- The existing invalidation call sites kept their semantics; the generation check lives inside the invalidation helpers, which clear only when the store has moved past the memo's generation.

## Relationships
- **REFERENCES**: [[engine:per-mem-search-index]]
- **REFERENCES**: [[engine:community-detection]]

## Options

- Keep whole-map drop on every mutation: rejected, the warm path pays a full rebuild for every touched entity.
- A non-rollback-aware counter: rejected, a restored snapshot would serve a memo computed from rolled-back state as current.
- Generation-keyed memos with the generation traveling through Clone: chosen.

## Notes

Re-created 2026-08-24: the original create of this entity (logged in the mem's change history on 2026-08-22) was authored on another machine and its file was lost uncommitted. This entity restores the record from the incremental-derived design work's session record; the decision content is unchanged from what that work landed in the engine.
