---
type: memo
created_date: 2026-08-28T16:30:47Z
last_modified: 2026-08-28T16:45:00Z
status: active
---

# Archived mems carry no engine-seam history: the check ledger is the verification record

## Claim
A .mem archive carries entities, schema, and body prose, but no mutation provenance and no check records: an archive mount answers provenance queries with a typed by-design refusal, so the exporting workspace's committed check ledger (the engine-owned checks state under the workspace's .memstead state directory) plus that repo's git history are the durable verification record for archived plan bundles.

## Context
- Found 2026-08-28 executing graph-plans plan 02 (the plan-mem lifecycle): AC2 expected a spot-read of a re-mounted archive to show check records, and the engine answered "archives record no history at the engine seam; open the source mem for the entity's story".
- The source mem is deleted at the end of the lifecycle, so for archived plan bundles the ledger is the only survivor.
- Ledger rows are append-only and reference entity ids that may no longer exist; that is by design, not rot.

## Substance

- Cross-mem citations also degrade at archive time: export --self-contained drops typed rows (each reported as CROSS_MEM_EDGE_DROPPED) while body wiki-link prose survives, because install refuses archives with cross-mem edges.
- A hierarchical mem exports under its leaf name (the publish contract; fixed 2026-08-28 in the git-branch export path).
- Candidate future work, deliberately not done now: an archive sidecar carrying check-record snapshots would make sealed bundles self-verifying; weigh against the ledger already being committed and greppable.

## Alternatives



## Outcome


