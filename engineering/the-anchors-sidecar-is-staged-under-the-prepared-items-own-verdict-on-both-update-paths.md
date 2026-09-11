---
type: decision
created_date: 2026-09-11T08:18:17Z
last_modified: 2026-09-11T08:18:17Z
status: accepted
decided_on: 2026-09-11
deciders: one-provenance bundle plan 03 (agent, under the standing engine-change rule)
scope: subsystem
tags: write-gate, anchors, update, batch, kernel
---

# The anchors sidecar is staged under the prepared item's own verdict on both update paths

## Decision
We will stage the anchors sidecar of an update exactly when the prepared item's own verdict says the merge changes it: `anchors_changed == Some(true)`, the verdict the wire reports to the caller. The stage function decides from the prepared item and takes no caller flag, so the single path and the batch call it the same way. The batch's broader predicate (stage whenever the item carries rows or unsets) is retired. This resolves the first divergence the phase-split decision recorded as preserved ([[engineering--the-write-gate-is-cut-by-phase-resolve-validate-compose-stage-commit-outcome]]).

## Context
The phase cut of 2026-09-10 gave the single-item commit and the batch one stage function with a caller-supplied flag: the single path passed the item's verdict, the batch passed whether rows were present. The verdict is computed on a copy of the sidecar before any write and drives the no-op guard (a restating anchors-only update is a no-op item) and the `anchors_changed` field the update-mutation contract documents as: `false` means every row restated what was stored and nothing was written. The sidecar staging helper itself writes nothing when the merge changes nothing, so the batch's broader predicate never produced a different sidecar; it produced a predicate the contract contradicted, and a second place to reason about when anchors are staged. The principle that a guard on one write path exists on all of them with one implementation ([[engineering--a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]) settles which of the two survives: the one the wire promises.

## Consequences
- `anchors_changed: false` means nothing was staged on both paths, and the batch's receipt words (`updated`, `noop`) and commits are unchanged: the golden of the fixture mutation sequence over the MCP binary is byte-identical before and after.
- A batch item whose rows restate what is stored beside a content change re-pins its rows through the same verdict (the rebaseline is part of the merge the verdict evaluates), so nothing an agent observes moves.
- A future writer that stages anchors reads the verdict from the prepared item; there is no flag to pass wrongly.

## Relationships
- **REFERENCES**: [[the-write-gate-is-cut-by-phase-resolve-validate-compose-stage-commit-outcome]]
- **REFERENCES**: [[a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]

## Options

- Keep the batch's broader predicate: rejected, it contradicts the documented `anchors_changed` contract on the batch path and could only stage a write that changes nothing.
- Make the single path stage whenever rows are present: rejected, the single path's contract is on the wire and pinned by tests.
- Keep both predicates behind the flag as the cut did: rejected, one write path with a guard the other lacks is the class the bundle removes.
