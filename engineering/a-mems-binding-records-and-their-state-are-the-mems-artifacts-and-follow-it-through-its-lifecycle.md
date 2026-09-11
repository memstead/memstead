---
type: decision
created_date: 2026-09-11T09:02:52Z
last_modified: 2026-09-11T09:02:52Z
status: accepted
decided_on: 2026-09-11
deciders: agent, under the standing engine-change rule, on the operator's request
scope: subsystem
tags: mem-lifecycle, binding-store, projections, rename, delete, kernel
---

# A mem's binding records and their state are the mem's artifacts and follow it through its lifecycle

## Decision
We will treat a mem's binding records and the per-binding state the maintenance loop writes (findings, advance) as the mem's own artifacts, on disk and on the schema-and-config ref: a rename moves them with the mem and rewrites every id inside that names it, a destructive delete removes them, a create seeds the ref's rows from the records already on disk for the name, and an unregister keeps them as it keeps the branch. One list, `pipeline_store::PER_MEM_STORE_DIRS`, names the per-mem directories; one enumeration on the ref, every blob under `mems/<leaf>/` and `pipeline/<kind>/<leaf>/`, is what a delete prunes and a rename moves. A state kind added later joins the list and is carried by both operations without a second implementation.

## Context
The one-provenance bundle's plan 01 grade (2026-09-11) observed in passing that after `mem rename` and `mem delete` the schema-and-config ref still held `pipeline/projections/<old-mem>/<stem>.json`: the rename moved the on-disk projections and findings directories and the config blob on the ref, but not the ref's mirror rows nor the advance state, and the delete pruned the config blob only, leaving the binding records, their state and the mirror rows behind, a row pointing at a mem that no longer existed. The cause was that each operation carried its own partial list of what a mem owns. The engineering principle that a guard on one write path exists on all of them with one implementation ([[engineering--a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]) names the class; the fix replaced the partial lists with one list and one enumeration.

## Consequences
- After a rename, `projection` verbs under the new name find the binding, the advance pass resumes with its dispositions under the new artifact ids, and the ref's rows read the new leaf and the rewritten record; nothing under the old name remains.
- After a destructive delete no binding names the deleted mem on disk or on the ref; an unregister changes nothing, so a reattach finds everything where it was.
- A binding declared before its destination exists (legal) gains its mirror row when the mem is created; a force-overwrite create, whose residue prune drops the previous life's rows, re-seeds them from disk.
- Hierarchical mem names (`team/sub`) key no store directory, so the store steps are no-ops for them; the ref's rows follow the leaf as before.

## Relationships
- **REFERENCES**: [[a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]

## Options

- Fix the two observed cases in place: rejected, each operation would keep its own list and the next state kind would miss one of them again.
- Drop the mirror on the ref instead of maintaining it: rejected, the mirror is the provenance record of edits that have no commit of their own, and the repo's history is the only durable record of who changed a binding.
- Rewrite only the file names, not the ids inside the state files: rejected, an advance state whose artifact ids name the old mem refuses every disposition the next pass presents, and findings whose entity ids name the old mem point at nothing.
