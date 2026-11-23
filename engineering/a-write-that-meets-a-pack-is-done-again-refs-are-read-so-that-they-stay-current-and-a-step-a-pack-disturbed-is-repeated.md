---
type: decision
created_date: 2026-11-23T11:12:59Z
last_modified: 2026-11-23T11:23:33Z
status: accepted
decided_on: 2026-11-23
deciders: operator (backlog bundle, go of 2026-11-23), implementing agent
scope: subsystem
tags: git-branch,mem-repo,refs,concurrency,git
---

# A write that meets a pack is done again: refs are read so that they stay current, and a step a pack disturbed is repeated

## Decision
We chose to make the git-branch backend keep working while another process packs its git directory, whoever starts the pack: the engine, a user's `git gc`, the maintenance git runs after a fetch. Three things a pack does under a writer are answered. It removes an object directory it has emptied, so the step that stores objects and then moves one ref is done again when an object fails to land (entity writes, a mem's configuration, schema installs). It packs the refs by writing each ref's value into `packed-refs` and then deleting the ref's file, while the git library reads `packed-refs` first and the file second: the engine looks a ref up through one function of its own, which reads `packed-refs` again when no ref file vouches for the answer and repeats until two lookups agree (four times at most), lists branches the same way, and does a commit again when the library reports a moved ref that is still on the parent the commit was built on, or a lock that was held for a moment. And it moves objects from loose files into a pack file, so an entity write reads its branch and builds its commit again through a newly opened repository when an object is reported as not found.

## Context
Grading the automatic pack found writes refused beside a `git gc`, with nothing lost and nothing written by the refused write: `MEM_ERROR` for an object that failed to land, `HASH_MISMATCH` or `ENTITY_NOT_FOUND` for a write planned on a branch read as it stood at the pack before, a storage error for a ref locked by the pack. A loop that packs only the refs refused about 10 of 1,500 write rounds. The same races existed before the automatic pack wherever a fetch triggered git's maintenance.

## Consequences
- Measured with the refs packed in a tight loop beside the writes: none of 15,000 write rounds refused, and a second engine that reloads all the while finds everything written.
- A real conflict is still a conflict: the repetition fires only when the engine's own lookup finds the branch on the parent the commit was built on, and the repeated commit goes through the library's compare-and-set again.
- The repetition for an object that is not found is reasoned and not measured: the case was seen twice on a loaded machine and could not be provoked again.
- The verbs that move several refs (a mem rename, a mem delete) are not repeated. A rename that is refused completes when it is sent again. A delete that cannot remove the mem's branch is not refused: it takes the mem off the roster, leaves the branch and says so (`Files deleted: false`, `MEM_FILES_NOT_DELETED`), and a rerun answers `UNKNOWN_MEM`; that is as it was before, and an open backlog item.
- The documentation promises that nothing is lost and that a refused write can be sent again, not that no write is ever refused.
- The lookups of a mem's branch and of the engine's own refs go through the one function, and so does the listing of local branches; a new lookup of that kind that calls the library directly brings the race back, and the test that packs refs in a tight loop is what shows it. Not covered: the walk over remote-tracking refs in the branch reset's check for pushed commits, and the reads of `HEAD`, which call the library directly.

## Relationships
- **DERIVED_FROM**: [[a-mem-repo-is-packed-by-gits-own-maintenance-which-the-engine-starts-after-a-write]]
- **DERIVED_FROM**: [[git-reads-tell-an-unreadable-branch-from-a-missing-one]]

## Options

- Tell users to pack with the refs left unpacked: rejected, a fetch packs them without anyone asking.
- Repeat the whole write at the verb level on any storage error: rejected, too broad, it would hide real failures.
- Wait for the library to read refs in the safe order: rejected as the only answer, the engine can read them that way today with the library's own primitives.
- The engine's own ref lookup plus repetition of the disturbed step: chosen.
