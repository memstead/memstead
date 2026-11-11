---
type: decision
created_date: 2026-11-11T11:30:34Z
last_modified: 2026-11-11T11:35:06Z
status: accepted
decided_on: 2026-11-10
deciders: engine-soundness bundle, executing session and its independent graders
scope: subsystem
tags: mutation, commit, staging
---

# Every mutation shares one exit and one commit call

## Decision
We will discard a mutation's staged writes when the write lock it holds is released, lock-less backends included, and have both backends end their own transaction on every exit of a commit; a discard that fails is remembered on the mount and made good before the next operation, by a fresh backend over the same storage if it keeps failing. Every verb commits through one call that makes one of three reports. Not committed: the error, with storage and the backend as they were. Committed: success, and a step that failed after the commit (a worktree refresh, the engine's store update, a folder commit completed inside the call) is the warning WRITE_FOLLOW_UP_FAILED naming the step, with the mem reloaded from storage where the engine's view was the failed step. Decided and not applied: COMMIT_NOT_APPLIED, only when completing the commit in the same call failed too; the next load completes it and reports INTERRUPTED_COMMIT_COMPLETED.

## Context
Single create and update never discarded what they had staged when a commit failed, the git-branch backend kept its staged set on early returns and on a parent mismatch, and a step that failed after a commit returned an error for a write that was in storage. The rule lived per call site.

## Consequences
- Three codes: the error COMMIT_NOT_APPLIED, the warnings WRITE_FOLLOW_UP_FAILED and INTERRUPTED_COMMIT_COMPLETED.
- Provenance is staged and lands with its commit, so appending it is no longer a step that can fail after one.
- A test matrix generated from the engine's writers and the storage trait's methods fails one call per run on both backends; a function that calls a writing storage method must be part of a matrix verb or be named with a reason.
- Conflict resolution and the mem-rename sweep commit through the one call.
- Not covered: proposal merge, fork, check transport, the seed commit of mem create and the markdown export still commit outside it; a proposal merge whose landed commit has a failed follow-up is unwound and reported as failed.

## Relationships
- **GOVERNS**: [[engine:storage-backend-trait-surface]]
- **GOVERNS**: [[engine:per-mem-commit-and-provenance-trailer-layer]]

## Options

- Fix each call site's cleanup: rejected, the defect was that a call site could forget.
- Report a failure after the commit as an error: rejected, it tells the caller a write that is in storage did not happen.
- One exit in the lock guard and one commit call with three reports: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 01; it held through an outside audit by two other model families.
