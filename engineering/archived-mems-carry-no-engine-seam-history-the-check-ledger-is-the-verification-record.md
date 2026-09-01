---
type: memo
created_date: 2026-08-28T16:30:47Z
last_modified: 2026-09-01T09:25:49Z
status: active
---

# Archived mems carry no engine-seam history: the check ledger is the verification record

## Claim
A .mem archive carries entities, schema, body prose, and per-entity authoring provenance, but no mutation history and no check records: an archive mount answers entity-history queries with a by-design refusal (a generic INVALID_INPUT naming the source mem as the place to read the story), so the exporting workspace's committed check ledger ([[engine--check-records-and-review-marks]], the engine-owned checks state under the workspace's .memstead state directory) plus that repo's git history are the durable verification record for archived plan bundles.

## Context
- Found 2026-08-28 executing graph-plans plan 02 (the plan-mem lifecycle): AC2 expected a spot-read of a re-mounted archive to show check records, and the engine answered "archives record no history at the engine seam; open the source mem for the entity's story".
- The source mem is deleted at the end of the lifecycle, so for archived plan bundles the ledger is the only survivor.
- Ledger rows are append-only and reference entity ids that may no longer exist; that is by design, not rot.

## Relationships
- **INFORMED_BY**: [[detecting-that-a-published-archive-lags-its-mem-is-a-gate-re-cutting-it-stays-a-decision]]
- **INFORMED_BY**: [[sealed-content-is-read-by-the-same-reader-that-admitted-it]]
- **REFERENCES**: [[engine:check-records-and-review-marks]]
- **REFERENCES**: [[engine:mem-archive-export-surface]]
- **REFERENCES**: [[engine:archive-ingress-validator]]

## Substance

- Cross-mem citations also degrade at archive time: export --self-contained ([[engine--mem-archive-export-surface]]) drops typed rows (each reported as CROSS_MEM_EDGE_DROPPED) while body wiki-link prose survives, because install ([[engine--archive-ingress-validator]]) refuses archives with cross-mem edges.
- A hierarchical mem exports under its leaf name (the publish contract; fixed 2026-08-28 in the git-branch export path).
- Candidate future work, deliberately not done now: an archive sidecar carrying check-record snapshots would make sealed bundles self-verifying; weigh against the ledger already being committed and greppable.

## Alternatives



## Outcome


