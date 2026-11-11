---
type: decision
created_date: 2026-11-11T11:30:34Z
last_modified: 2026-11-11T11:30:34Z
status: accepted
decided_on: 2026-11-10
deciders: engine-soundness bundle, executing session and its independent graders
scope: subsystem
tags: multi-mem, rename, batch
---

# An operation over several mems lands its precondition last and reports a partial failure item by item

## Decision
We will commit an operation over several mems once per mem, under every lock it needs, ordered so that the write which uses up the caller's precondition lands last: an entity rename commits the peer mems first and the renamed entity's own mem last; a mem rename sweeps the peers first, the renamed mem last, and flips the mem's identity after the sweep. A commit that fails before any other landed fails the operation whole. A commit that fails after another landed returns the one partial-failure report, RENAME_PARTIAL_FAILURE, naming every mem and every entity the operation writes with whether it landed and the entity's current hash, built after the committed mems were reloaded. Completion needs nothing but the report: the identical rename completes a rename, the items named as not landed complete a batch.

## Context
A rename committed the entity's own mem first, so a failure at a peer left the old id gone and the caller's hash stale; a storage failure at a peer returned a bare error; the mem-rename sweep and the batch forms returned the storage error alone, with no way to complete.

## Consequences
- The code keeps the name RENAME_PARTIAL_FAILURE although batches and the mem rename report it too; details.mems carries the per-mem account.
- A stub at the new id no longer refuses a rename: after a partial rename the peers already point there.
- A batch labels a lost race on a moved head as failure cause storage, where a rename says drift.
- The check ledger is carried to the new id before the renamed entity's own commit; a failure of that commit leaves carried lines under an id that does not exist yet, and a completing call can append them again.

## Relationships
- **GOVERNS**: [[engine:rename-mutation]]
- **GOVERNS**: [[engine:mem-rename-operation]]
- **GOVERNS**: [[engine:batch-update-atomic-mutation]]

## Options

- One atomic commit across mems: rejected, it is a journal across mems the backends do not have.
- Roll landed commits back: rejected, impossible on a folder mem and racy on git.
- Precondition last, one report, completion from the report: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 02; it held through an outside audit by two other model families.
