---
type: decision
created_date: 2026-08-23T18:07:56Z
last_modified: 2026-09-01T09:25:27Z
status: accepted
decided_on: 2026-08-23
deciders: operator (consolidation bundle, plan 08), implementing agent
scope: system
tags: code-mems, sync, deletions, historical-records
---

# A modelled subject that left the tree is deleted unless a knowledge mem cites it, then kept as a frozen historical record

## Decision
We will handle a code-mem entity whose modelled subject left the source tree in exactly one of two ways. DEFAULT, delete: the entity, its edges and its file are removed through the engine (inbound references from staying entities are removed in the same act, which is also when the staying entities' prose sheds the retired subject). EXCEPTION, historical record: when an entity in a knowledge mem (`engineering`, `project`) cites the subject, the code-mem entity stays with its stability frozen and a dated note in its body naming what retired it and why the record stands; a contract additionally sets `deprecation_status: removed` with the removal date. A subject that moved rather than died (a crate now living in the private repo) is kept as a frozen record with the move named, since its cross-mem citations still resolve and the thing still exists.

## Context
The plugin mem modelled nine skills and four hooks the 2026-07-11 diet removed, and the engine mem modelled three crates that left the open workspace; several of those retired subjects are cited by engineering decisions as their historical baseline (the pre-rebuild ingest, the retired outer-repo versioning concept, the secret-guard duplication memo). Deleting those would dangle the knowledge mem's links; keeping everything would let the code mems rot into museums. The 2026-08-23 sync (consolidation plan 08) needed one rule, applied uniformly: eleven plugin entities deleted, eight kept frozen, three engine crate entities and their contracts kept frozen with their moves and retirements named.

## Consequences
- A code mem's live entity set equals what the source tree holds, plus explicitly frozen records that knowledge mems cite; a reader can tell them apart by `stability: frozen` and the dated note.\n- Cross-mem edges never dangle from a sync deletion: the referrer keeps its target, or the deletion removed the referring edge in the same act.\n- The rule is the binding-built mems' half of the walk's supersession idea: records of their time stay marked as such, never presented as current.\n- Cost: frozen records sit in the mem's counts; the verify report's coverage excludes them only where their artifacts left scope, which is exactly right.

## Relationships
- **MOTIVATED_BY**: [[degrade-never-disappear]]
- **MOTIVATED_BY**: [[plugin:old-ingest-skill]]
- **MOTIVATED_BY**: [[plugin:commit-skill]]

## Options



## Notes


