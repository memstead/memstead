---
type: memo
created_date: 2026-08-06T16:38:36Z
last_modified: 2026-09-10T22:55:52Z
status: active
tags: performance, sizing, measurement, boot, scale
---

# The sizing curve is measured and load is the only cold-path cost

## Claim
On the cold CLI path every everyday operation costs what boot costs: workspace load dominates so completely that mutation commits, search-index rebuilds, and community detection are invisible next to it. Load is linear in the entity count (0.05 ms per entity at 7,500, measured 2026-09-10 on Apple M5 Max, release build); the super-linear growth the first curve reported (0.36 to 0.75 ms per entity, 2026-08-06) was a defect in the git-branch backend's per-entity read, not a property of loading, and is gone since the linear-boot fix that ships in 0.20.0.

## Context
The engine advertised "designed for 1,000-5,000 entities" without ever measuring it (plenum channel, finding 10); the largest real deployment reached 7,414 entities at ~0.5 ms/entity boot. Three backlog redesigns — real lazy mounts, incremental maintenance of derived structures, deferred cross-mem target resolution — explicitly wait for numbers. The agent-toolbox plan 02 built the measurement: `cargo run -p xtask -- sizing-curve` in [[engine--xtask-crate]] generates graded synthetic mem-repo workspaces through the product surface and times boot, update, search-after-mutation, and overview as fresh processes; the committed curve lives in `public/docs/sizing-curve.md`, machine-readable results in `sizing-curve/v1` JSON.

## Relationships
- **REFERENCES**: [[engine:xtask-crate]]
- **REFERENCES**: [[engine:engine-boot-and-construction-surface]]
- **REFERENCES**: [[engine:entity-load-pipeline]]
- **REFERENCES**: [[engine:per-mem-search-index]]
- **REFERENCES**: [[engine:community-detection]]

## Substance

Medians at 500 / 2,500 / 5,000 / 7,500 entities: boot 181 / 1,162 / 3,043 / 5,647 ms; update, search, and overview each within noise (±10 ms) of boot at every size. What this implies per redesign, as data: (1) lazy mounts are the largest lever the curve can see — every mounted mem adds its full entity count to every cold command via [[engine--engine-boot-and-construction-surface]] and [[engine--entity-load-pipeline]], so load cost is proportional to inventory, not working set; (2) the incremental-index case ([[engine--per-mem-search-index]], [[engine--community-detection]]) cannot be argued from the cold path — the rebuild hides inside load's shadow; its case rests on the warm MCP path, which is the missing measurement; (3) deferred cross-mem targets are priced by the same load dominance — each mem mounted only to satisfy write-time target checks adds its entities x 0.6-0.75 ms to every command permanently. One `batch-create` call lands 7,500 entities in ~4.7 s — the batch path already erases the per-call-boot ingest pain of plenum finding 1.


Dated record, 2026-08-06 curve: boot 181 / 1,162 / 3,043 / 5,647 ms at 500 / 2,500 / 5,000 / 7,500; 15x the entities cost 31x the time. Re-measured 2026-09-10 on the linear-boot engine: 59 / 145 / 255 / 371 ms; 15x the entities cost 6x the time, and the small end is process spawn and repository open (~40 ms flat), not entities.

## Alternatives



## Outcome

The MCP server instructions cite docs/sizing-curve.md as the measured grounding of the 1,000-5,000 span; rerunning the curve after an engine change is one command plus a JSON diff, and it is what caught the fix's effect the same day. Plenum finding 10 closed. Amendment 2026-09-10: a CPU profile of the 7,500-entity boot put 96 % of the time in the backend's per-entity read (repository opened, ref peeled, root tree inflated once per entity); the backend trait gained `read_all_entities`, one tree walk for the whole mem, and the curve became 59 / 145 / 255 / 371 ms at 500 / 2,500 / 5,000 / 7,500. The three redesigns this memo priced keep their mechanisms; their per-entity multipliers are a twentieth of what they were argued from.
Amendment 2026-09-11: the warm path is measured (architecture-seams bundle, plan 03). The harness gained a warm leg, one memstead-mcp server per size point timed per call after a warm-up: at 500 / 2,500 / 5,000 / 7,500 entities, warm search 427 / 487 / 624 / 693 µs (0.04 µs per entity, a fixed ~0.4 ms floor), warm entity read with relations 822 / 1,079 / 1,393 / 1,711 µs (0.13 µs per entity, the incoming-edge scan), warm overview 1,563 / 6,270 / 12,578 / 19,156 µs (2.5 µs per entity, linear). The reading: after boot a session pays under two milliseconds per search or entity read at the advertised ceiling, and the overview, not search, is the warm read that scales like a load; the 2026-08-22 incremental-maintenance finding named query-side work as the next lever without a number, and the number names the workspace-global overview assembly first and the include_relations scan second.
