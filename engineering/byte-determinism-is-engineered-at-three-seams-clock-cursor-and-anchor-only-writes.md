---
type: decision
created_date: 2026-08-06T11:25:02Z
last_modified: 2026-08-06T11:25:02Z
status: accepted
decided_on: 2026-08-06
deciders: stability-sweep/05 session (operator-delegated loop)
scope: component
tags: determinism, testing, flakes
---

# Byte-determinism is engineered at three seams clock cursor and anchor-only writes

## Decision
Canonical-byte and event-stream determinism are enforced at three engine seams rather than tolerated in tests. (1) Mutation timestamps route through an engine-owned injectable clock (`Engine::set_mutation_clock`, default system clock) — every schema-flagged `init_timestamp`/`auto_timestamp` stamp reads it, so tests that assert over canonical entity bytes pin the clock instead of loosening hash assertions. (2) The folder-mem drift cursor — the last-line `ts` of the [[engine--filesystem-mem-jsonl-changelog]] — strictly advances per commit: `append_change_monotonic` bumps a same-millisecond or backwards timestamp to `last + 1ms`, comparing at the cursor's own millisecond granularity. (3) An anchor-only update never touches entity bytes: the auto-stamp is skipped when content is unchanged, so anchors never move `_hash`, including across wall-clock second boundaries.

## Context
Three intermittent failures each traced to wall-clock nondeterminism leaking into a promised-deterministic surface. The ui-api parity test flaked because two engines stamping `created_date` in different seconds produce different canonical bytes. `create_entity_emits_one_event_per_commit` flaked because two commits inside one millisecond shared a changelog cursor, and the self-write dedup in [[engine--mem-change-event-subscription]] swallowed the second event (convicted by instrumentation: identical `previous`/`recorded` cursors in a failing run). The CLI anchors test flaked because the anchor-only leg of the [[engine--update-mutation]] fell past the no-op guard into the unconditional auto-stamp, restamping `last_modified` and moving `_hash` — a real contract violation of the documented anchors-never-move-hash promise, made deterministic by a pinned-clock regression test before fixing.

## Consequences
Flaky-test fixes land at the mechanism, never as tolerant assertions — all three tests keep exact counts and strict equality. Any future test asserting over canonical bytes pins the mutation clock instead of normalizing timestamps. The changelog cursor dialect (fixed-width RFC 3339 milliseconds, lexicographically ordered) is unchanged; provenance timestamps may read up to a few milliseconds later than wall clock under same-millisecond bursts. Anchor-only updates now genuinely preserve `last_modified`, so agents can refresh anchors without invalidating cached `expected_hash` values.

## Relationships
- **REFERENCES**: [[engine:mem-change-event-subscription]]
- **REFERENCES**: [[engine:update-mutation]]
- **REFERENCES**: [[engine:filesystem-mem-jsonl-changelog]]

## Options



## Notes


