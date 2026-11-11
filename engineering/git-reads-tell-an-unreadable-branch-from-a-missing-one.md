---
type: decision
created_date: 2026-11-11T11:24:22Z
last_modified: 2026-11-11T11:24:22Z
status: accepted
decided_on: 2026-11-11
deciders: engine-soundness bundle, executing session and its independent graders
scope: subsystem
tags: git-branch-backend, storage, errors
---

# Git reads tell an unreadable branch from a missing one

## Decision
We will read a git-branch mem so that only a missing ref reads as absent. A repository that does not open, or a ref that exists and does not resolve to a readable commit, is an error, in the config read, the head probe, the change feed and the agent-notes feed alike, through one helper. A read of a whole branch reports an unreadable blob in that blob's own row, not as a failure of the whole read.

## Context
Several reads collapsed a failure into absence: a configured mem whose repository did not open read as unconfigured; the head probe returned no head on failure, against the storage contract that an error is what makes the engine reload and a write wait; the change feed read an unreadable tip as the empty tree and reported every entity removed. A whole-branch read failed outright on one bad blob, though boot is built to carry on past per-path failures.

## Consequences
- A damaged repository stops writes through the stale-view refusal instead of serving a wrong answer.
- A name that is not a ref still resolves as a revision, because diff takes commit ids.
- One old test asserted the contract's opposite and was removed.

## Relationships
- **GOVERNS**: [[engine:git-tree-backend]]
- **GOVERNS**: [[engine:storage-backend-trait-surface]]

## Options

- Read any failure as absence: rejected, because an absent answer is a wrong answer that the next write builds on.
- Missing reads as absent, everything else is an error: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 06, round-two findings of the outside audit.
