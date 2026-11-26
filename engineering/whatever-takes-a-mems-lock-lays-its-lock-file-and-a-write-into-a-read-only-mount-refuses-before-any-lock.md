---
type: decision
created_date: 2026-11-22T13:15:47Z
last_modified: 2026-11-26T16:45:19Z
status: accepted
decided_on: 2026-11-22
deciders: operator (shared-engine bundle, go of 2026-11-21), implementing agent
scope: subsystem
tags: write-lock,read-only,mem-repo,mount
---

# Whatever takes a mem's lock lays its lock file, and a write into a read-only mount refuses before any lock

## Decision
We chose one rule for content and one for lock files. Content: through a read-only mount no ref, no object and no registry entry is ever written into the storage it names, and an unregister leaves no tombstone in a mem-repo the workspace does not own. Lock files: an operation that takes a mem's lock lays that mem's lock file beside its storage when it is not there yet, for a read-only mount as for any other, because the lock is what holds the mem still against a writer elsewhere. The mount itself, reads, a refused write that names the mem or a qualified entity id, and the unregister of a read-only mount take no lock of the mounted mem.

## Context
While the mount verb was graded, its documentation claimed that a read-only mount lays no file at all in the git directory it names. Two grade passes each found one more path that lays a lock file, because the engine locks a read-only mount on purpose. The released engine also took write locks before it refused a write into a read-only mount, which laid a lock file and refused `MEM_VIEW_STALE` where the directory is not writable.

## Consequences
- A read-only mount of a directory that is not writable mounts, reads, refuses writes with `READ_ONLY_MOUNT` and unregisters.
- An operation that has to hold the mem still (an entity delete, rename or retype in another mem, a write that refers to one of its entities, a call that names its entity by a bare slug) lays the lock directory there on first use, and refuses `MEM_VIEW_STALE` where it cannot.
- What a user can rely on is a statement about content, not about every file.
- The unregister of a write mount in service takes that mem's lock and so may lay its lock file. The unregister of a quarantined mount does not take that mem's write lock and lays no write-lock file; where it writes the marker of a folder mem it takes the short lock on that mem's config file, as for a mount in service (amended 2026-11-22).

## Relationships
- **DERIVED_FROM**: [[a-write-whose-guard-or-write-set-reads-the-workspace-locks-every-writable-mem]]

## Options

- Promise that nothing is ever laid there: rejected after two grade passes, the lock design contradicts it.
- Lock a read-only mount only where a lock file already exists: rejected, a reader would not hold still against the first writer of that branch, which is what the lock on a read-only mount is for.
- Keep lock files outside the git directory: rejected, engines of other versions lock in the git directory and would not exclude each other.
- One rule for content, one for lock files: chosen.

## Notes

2026-11-26 amendment (shared-checks bundle, plan 02): a read-only mount whose roster row grants `check: true` writes one thing into the storage it names, the mem's checks ref (`refs/memstead/checks/<mem>`), and nothing else; every mutation through it still refuses `READ_ONLY_MOUNT` before any lock. Without the grant the rule stands as written. Deciders: operator 2026-11-26, executed by session memstead-93.
