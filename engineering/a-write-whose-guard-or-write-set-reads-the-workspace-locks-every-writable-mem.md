---
type: decision
created_date: 2026-11-11T11:30:34Z
last_modified: 2026-11-12T02:15:13Z
status: accepted
decided_on: 2026-11-10
deciders: engine-soundness bundle, executing session and its independent graders
scope: subsystem
tags: locking, concurrency, multi-mem
---

# A write whose guard or write set reads the workspace locks every writable mem

## Decision
We will take, before a write plans anything, the write lock of every mem it writes and of every mem it refers to (declared relation targets, wiki-links in the sections it leaves behind, links in relation descriptions), in the lock's one order by canonical path, after the mount roster is reconciled and again if it changed; under its locks the write reloads every loaded mem. A write that learns what it writes, or what its guard reads, only by reading the workspace takes the lock of every mount that has one, read-only mounts included, reloads every mem and loads the deferred ones: entity rename, mem rename, delete, retype, mem delete, fork, proposal merge, conflict resolution, any write adding an edge on an acyclic relationship, any write whose schema or an edge target's schema declares aggregate signals, and any call naming an entity by a bare slug. Such a verb refuses MEM_VIEW_STALE, naming the mem, when a mem cannot be reloaded under the lock, is out of service, or is read-only and cannot be locked.
No verb takes a lock after it began planning, and a load or read that runs under another mem's lock never waits for a lock. A write whose relation target lies in a mem that could not be reloaded is refused the same way.

## Context
Rename, the mem-rename sweep, delete and retype read other mems from views taken before any lock was held; the sweep, proposal merge and conflict resolution committed without the lock; the lock set was computed before the roster was reconciled. An outside audit later found a write with aggregate signals that read every mem under its own mems' locks only.

## Consequences
- Lock scope is wide: a write that refers to another mem waits for a write in progress there, and the whole-workspace verbs wait for any write in progress.
- One mem that cannot be read, is out of service or cannot be locked blocks the whole-workspace verbs until it can or is removed; deleting that mem stays possible, and so do a pull, a pull of every mem, a branch reset and a schema pin change on that mem itself, which lock the rest of the workspace and proceed (amended 2026-11-12).
- A reference added while a delete or rename holds its locks lands afterwards as a forward reference to a stub.
- Not covered: a reference into a mem the engine does not mount is checked against storage without that mem's lock (amended 2026-11-12).

## Relationships
- **GOVERNS**: [[engine:reload-before-operation-coherence]]
- **GOVERNS**: [[engine:delete-mutation]]
- **GOVERNS**: [[engine:rename-mutation]]

## Options

- Lock only the mems known to be touched and start over when the set grows: rejected, finding the set needs a current view of every mem.
- One workspace lock for every write: rejected, it serialises writes that cannot conflict.
- The referred mems for ordinary writes, every mem for writes that read the workspace: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 02; it held through an outside audit by two other model families.


2026-11-12: pull, pull of every mem, branch reset and the schema pin change take the workspace write lock before they plan, with their subject tolerated when it cannot be read or locked (engine a5d5fa00 onward). The lock decision receives one pointer edge per body wiki-link of the content a create, batch create, update or batch update writes, derived by the decision itself from that content, so a cycle a body link closes across mems is guarded under the workspace lock. Conflict resolution passes the chosen side's relationship rows through the declared-edge checks and no longer writes over a refused link synthesis.
Correction to the 2026-11-12 note: the change landed in engine 2fd0f724, not a5d5fa00.
2026-11-12, second amendment: the MCP write tools (update, rename, retype, delete) no longer resolve a bare slug before the lock; the verb resolves it under its lock and reload, so the Not-covered clause on that pre-resolve is dropped (engine 346a1b88).
