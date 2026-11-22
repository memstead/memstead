---
type: decision
created_date: 2026-11-22T13:15:46Z
last_modified: 2026-11-22T13:15:46Z
status: accepted
decided_on: 2026-11-22
deciders: operator (shared-engine bundle, go of 2026-11-21), implementing agent
scope: subsystem
tags: mem-repo,reload,roster,mcp
---

# A roster row is the same mount only while its capability, storage and pin stand

## Decision
We chose to have the roster reconciliation count every row, whatever its capability, and to compare a row with the mount it serves by capability, storage and schema pin. A row that differs in any of the three is another mount under an old name: it is unmounted and mounted again, and the one roster notice reports it under both removed and added. Verbs that judge writability before they take a write lock reconcile the roster first. A schema pin change is a write, so a read-only mount refuses it in service, in quarantine and below boot.

## Context
Until 0.26.1 the reconciliation followed writable rows only and compared a row with its mount by name and capability. A read-only mount another process added or withdrew was not followed. Once a verb could put a name back onto another branch or git directory between two operations of a running engine, the engine kept serving the old storage, and a write through it landed in the store the roster no longer named; the released engine did the same for a row edited by hand. Independent graders found both while the shared-engine bundle was graded on 2026-11-21 and 2026-11-22.

## Consequences
- A running engine serves and writes the store the roster names now, from its next operation on.
- A capability change, a storage change and a pin change all read as one remount; a consumer of the roster notice needs no new field.
- A pin change made by another process is followed as a remount, which costs a cold mount where a lighter re-pin would do.
- The comparison normalises paths, so a row a verb built and the row the roster file gives back are one place.

## Relationships
- **DERIVED_FROM**: [[mem-membership-follows-reload-before-operation-removals-included-and-every-surface-learns-of-it]]

## Options

- Compare by name and capability alone: rejected, that is the defect.
- A new field on the roster report for a changed row: rejected, removed plus added says what happened and every consumer reads both already.
- Keep read-only rows out of the reconciliation: rejected, a read-only mount is membership like a writable one.
- Compare capability, storage and pin: chosen.
