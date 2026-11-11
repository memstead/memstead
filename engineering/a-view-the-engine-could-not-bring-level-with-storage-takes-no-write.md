---
type: decision
created_date: 2026-11-11T11:30:34Z
last_modified: 2026-11-11T11:30:34Z
status: accepted
decided_on: 2026-11-10
deciders: engine-soundness bundle, executing session and its independent graders
scope: subsystem
tags: reload, coherence, concurrency
---

# A view the engine could not bring level with storage takes no write

## Decision
We will mark a mem when the reload before an operation fails, when it could not read every file, when the drift probe itself fails, or when the reload that levels the view after a failed store update fails: the mem keeps serving the view it has, the commit call refuses a write with MEM_VIEW_STALE and nothing is written, and the mem is reloaded before every following operation until a reload succeeds. A reload that cannot read every file fails and keeps the view in place; a boot serves what it could read and the mem takes no write until a reload reads all of it.

## Context
The engine logged a failed drift reload and went on with the stale view, read a failed drift probe as no drift, and built a reloaded view from whatever files it could read. For a write that was a lost update: the caller's hash matched the stale view and the commit replaced what a sibling had put in storage.

## Consequences
- One error code, MEM_VIEW_STALE, also used for a mem out of service and for a read-only mem whose lock cannot be taken; its message text still speaks of a change in storage.
- A mem with a file storage cannot deliver is readable and not writable until the file reads again, and costs a reload attempt per operation meanwhile.
- A failed operation's response is its error, so a warning its reload produced rides the next successful response or stands among the load warnings.
- A discard that keeps failing is repaired only by an operation that holds no lock, so writes refuse until a read runs.

## Relationships
- **GOVERNS**: [[engine:reload-before-operation-coherence]]

## Options

- Serve the stale view and log: rejected, it is the lost update.
- Refuse reads as well: rejected, a read of the last good view is useful and honest when it is marked.
- Readable but not writable until a reload succeeds: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 01; it held through an outside audit by two other model families.
