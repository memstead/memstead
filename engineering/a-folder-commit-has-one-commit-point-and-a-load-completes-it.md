---
type: decision
created_date: 2026-11-11T11:30:34Z
last_modified: 2026-11-11T11:35:19Z
status: accepted
decided_on: 2026-11-10
deciders: engine-soundness bundle, executing session and its independent graders
scope: subsystem
tags: filesystem-backend, durability, commit
---

# A folder commit has one commit point and a load completes it

## Decision
We will commit to a folder mem by writing every new content beside its target and syncing it, syncing every directory from each new file up to the mem root, then putting a commit record (`.memstead/commit.json`, naming each file operation and the commit's ledger lines) in place and syncing its name: that is the commit point. After it the engine moves the files into place, syncs their directories, appends and syncs the ledger lines, and removes the record and syncs the removal; success is returned after that. A commit cut off after the commit point is completed from the record, only by a load of the mem (boot, reload) or by the engine in the same call; a read that goes to the mem's files while a record is on disk waits for a live writer and then refuses.
A provenance record written on its own (the delete of a stub, a review mark) is a commit of one ledger line behind the same point.

## Context
The folder backend applied its staged files one by one, returned at the first error with the rest still staged, renamed temp files without syncing them or their directory, and appended its ledger line after the files, unsynced. A crash between two files of one commit left no trace and a half-applied commit.

## Consequences
- A write to a folder mem costs disk syncs: tens of milliseconds where it took one (about 60 ms on macOS, where a sync flushes the drive).
- The storage trait carries a required staged-provenance method and a completion method.
- Readers that walk a folder mem's directory or ledger directly must call the refusal first.
- A killed attempt can leave empty directories; they hold no content and are not removed. Directory sync is skipped off unix, where the platform offers none.
- A load that sweeps leftover temps looks for a commit record again under the mem's lock, so a sibling killed just past its commit point keeps its commit.
- Not covered: the markdown export writes a folder mem's files directly, past the commit point and without the lock.

## Relationships
- **GOVERNS**: [[engine:filesystem-backend]]
- **GOVERNS**: [[engine:storage-backend-trait-surface]]

## Options

- Detect a broken commit from the ledger alone: rejected, the ledger cannot name what a half-applied commit had still to do.
- Keep pre-images and undo: rejected, an undo after the files moved is a second commit that can fail the same way.
- A general write-ahead log: rejected as more than one commit needs; the record covers one commit and is removed when it is applied.
- One commit record and completion on load: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 01; it held through an outside audit by two other model families.
