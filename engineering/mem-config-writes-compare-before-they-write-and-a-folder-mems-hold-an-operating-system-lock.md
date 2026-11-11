---
type: decision
created_date: 2026-11-11T11:24:21Z
last_modified: 2026-11-11T11:24:21Z
status: accepted
decided_on: 2026-11-11
deciders: engine-soundness bundle, executing session and its independent graders
scope: subsystem
tags: config, concurrency, filesystem-backend
---

# Mem config writes compare before they write and a folder mem's hold an operating-system lock

## Decision
We will make every mem config write a compare-and-set that compares before it writes, on every backend: the folder backend under the engine's operating-system file lock on `.memstead/locks/config.lock` (the plain write included), replacing the file through a synced temp file and a rename; the git-branch backend against the config blob at the tip it builds on, with the ref advance pinned to that tip; the in-memory backend under its state lock. A schema-pin change is a bounded compare-and-set loop that re-reads and re-applies.

## Context
A folder config write truncated the file in place, so a crash left it empty; its compare-and-set broke a held lock file after fifty tries and went on without exclusivity; the git-branch and in-memory compare-and-set ignored the expected bytes; the schema-pin change rewrote the whole config without lock or compare, so a field a concurrent writer set was lost while both reported success. A first repair broke the lock file by age; a second audit round showed that an age cannot tell a stalled holder from a dead one.

## Consequences
- The operating system releases the lock when its holder dies; a live holder is waited for however long it stalls.
- A folder config write costs a sync.
- A lost race reports a mismatch and the caller re-reads; no field a sibling wrote is overwritten.

## Relationships
- **GOVERNS**: [[engine:filesystem-backend]]
- **GOVERNS**: [[engine:git-tree-backend]]
- **GOVERNS**: [[engine:per-mem-config]]

## Options

- A lock file created exclusively and broken by age: rejected, because a stalled live writer is overtaken and two breakers can race.
- A lock file carrying its owner's token: rejected for the same reason; ownership does not tell whether the owner is alive.
- The operating system's file lock: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 06, findings of both audit rounds.
