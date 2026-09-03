---
type: decision
created_date: 2026-09-03T12:47:32Z
last_modified: 2026-09-03T12:47:32Z
status: accepted
decided_on: 2026-09-03
deciders: backlog-repairs bundle, C3 executing session
scope: subsystem
tags: schema, lifecycle, version-skew
---

# The completed schema switch is the one lifecycle event that moves the mutation stamp

## Decision
We will have the mem's mutation stamp (the engine-owned record of which engine and which resolved schema last validated a write, the marker the version-skew hint reads) move on exactly one lifecycle event beside entity writes: the completed schema switch, which re-stamps it with the target through the same writer and equality guard entity mutations use. A dual-pin entry and the below-gate quarantine repair stamp nothing, because they validated nothing.

## Context
Entity mutations stamped the config with the engine version and the resolved schema, but a schema switch moved only the pin, so the marker kept naming the old generation until the next entity write and a grader read a mem as still sitting on the old pin after its migration had completed. Deriving the marker from the pin at read time was rejected: the stamp records a validation that happened, and the pin is only one of its inputs.

## Consequences
- After a completed migration the marker and the pin agree without waiting for an entity write.
- The stamp writer's warnings have no channel on the set-schema outcome and are dropped there; an entity write surfaces them.
- The overview mem entry carries a last-mutation line for every stamped mem, a small token cost on the cold-start surface accepted for the marker's readability.
- Any future lifecycle setter that changes what a write validates against must call the stamp writer too.

## Options

- Re-stamp on the completed switch only: CHOSEN.
- Derive the marker from the pin at read time: rejected; the stamp records a validation that happened, not a configuration.
- Stamp on every lifecycle event including dual-pin entry: rejected; those validated nothing, so the stamp would assert a validation that did not occur.
