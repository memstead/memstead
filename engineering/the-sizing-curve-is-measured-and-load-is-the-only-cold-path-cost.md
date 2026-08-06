---
type: memo
created_date: 2026-08-06T16:38:36Z
last_modified: 2026-08-06T16:38:36Z
status: active
tags: performance, sizing, measurement, boot, scale
---

# The sizing curve is measured and load is the only cold-path cost

## Claim
On the cold CLI path every everyday operation costs what boot costs: workspace load dominates so completely that mutation commits, search-index rebuilds, and community detection are invisible next to it, and per-entity load cost grows super-linearly (~0.36 ms/entity at 500 entities to ~0.75 ms/entity at 7,500 — 15x the entities costs 31x the time, measured on Apple M5 Max, release build).

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

## Alternatives



## Outcome

The MCP server instructions now cite docs/sizing-curve.md as the measured grounding of the 1,000-5,000 span; rerunning the curve after an engine change is one command plus a JSON diff, making it the machine evidence the three backlog redesigns wait for. Plenum finding 10 closed.
