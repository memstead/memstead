---
type: principle
created_date: 2026-09-11T00:05:15Z
last_modified: 2026-09-11T00:05:26Z
authority: accepted
universality: domain-wide
tags: sync, refactoring, code-mems, ordering
---

# Order refactoring work by sync cost: in place before moves, an inventory before a gated cut

## Statement
When a change touches source that a code mem is bound to, sequence the work by what it costs the sync loop: prose and schema first (no bound source touched), in-place edits next (anchors stay, claims are re-walked), file moves and splits last and one at a time, and run the binding's full inventory with a clean verify before any cut whose behaviour the tests cannot fully pin.

## Scope
Every plan that edits source under a code-mem binding (engine, plugin, registry) in this workspace; a move is any change that gives an anchored artifact a new address.

## Justification

A code mem anchors files at file grain, so an in-place change makes a few entities stale and the sync loop re-reads them, while a move loosens every anchor on the old address and the loop must re-home rather than re-read. The architecture-seams bundle of 2026-09-11 ran in this order: three in-place seams and two test moves, then one inventory over the engine binding (162 drifted rows, 73 entities, one pass to CLEAN at 623 of 623 anchors) and the registry binding (15 rows to CLEAN), before the operator-gated phase split of the write gate. The one-flavour fold of 2026-09-05 had shown the loop absorbs a large in-place change; the moves were the untested part, and the inventory carried them.

## Exceptions

- A move the source itself forces (a crate cut, a rename decided elsewhere) happens when it is decided; the inventory then follows at once rather than waiting for the end of a bundle.
- Tests and fixtures move freely: they are excluded artifacts of the code bindings, not anchored ones.

## Consequences

- A bundle that mixes moves and in-place edits declares its order in the plans' REQUIRES edges, and carries an inventory-plus-verify plan as the evidence gate before its riskiest cut.
- A pure move (verbatim body plus rustfmt, no behaviour change in the same commit) is the only move the loop re-homes without a claim walk; a move that also changes behaviour is two commits.
