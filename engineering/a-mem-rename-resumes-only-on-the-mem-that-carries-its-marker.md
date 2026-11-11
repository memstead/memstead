---
type: decision
created_date: 2026-11-11T11:24:22Z
last_modified: 2026-11-11T11:24:22Z
status: accepted
decided_on: 2026-11-11
deciders: engine-soundness bundle, executing session and its independent graders
scope: subsystem
tags: mem-rename, lifecycle
---

# A mem rename resumes only on the mem that carries its marker

## Decision
We will stamp the old name on the mem a rename moves (the mem config's `renamingFrom`) when the rename flips the mem's identity, clear it when the rename finishes, and treat 'old name absent, new name present' as a resumed rename only when the new mem's marker names this rename's old name. A fork resets the marker.

## Context
A mem rename treated 'old absent, new present and writable' as a resumption, so a rename from a name no mount held onto the name of a live, unrelated mem rewrote the absent name's links and grants onto that mem and reported the rename as resumed. Resumption itself is needed: an operation over several mems lands its precondition last, and a cut-off rename completes by being called again.

## Consequences
- One new field in the mem config wire shape, `renamingFrom`.
- A rename cut off after the flip resumes and clears its marker; before this rule no test exercised a genuine resumption.
- A rename onto an unrelated live mem refuses instead of resuming.

## Relationships
- **GOVERNS**: [[engine:mem-rename-operation]]
- **GOVERNS**: [[engine:per-mem-config]]

## Options

- Infer resumption from the mount state alone: rejected, it is the defect.
- Forbid resumption and require a manual repair: rejected, because the multi-mem ordering rule promises completion from the call alone.
- A marker on the moved mem: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 06, round-two findings of the outside audit.
