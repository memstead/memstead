---
type: decision
created_date: 2026-11-11T11:30:34Z
last_modified: 2026-11-12T01:14:38Z
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
- Not covered: fork, check transport and the seed commit of mem create commit outside it, each on a backend that is not a mount of the engine yet or belongs to no mem (the fork before it is mounted, the workspace's check ref, the new mem before it is registered); their failures roll back the operation instead (amended 2026-11-12: proposal merge and the markdown export now commit through it, and a merge that fails after a landed commit reports per commit and leaves what landed).

## Relationships
- **GOVERNS**: [[engine:storage-backend-trait-surface]]
- **GOVERNS**: [[engine:per-mem-commit-and-provenance-trailer-layer]]

## Options

- Fix each call site's cleanup: rejected, the defect was that a call site could forget.
- Report a failure after the commit as an error: rejected, it tells the caller a write that is in storage did not happen.
- One exit in the lock guard and one commit call with three reports: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 01; it held through an outside audit by two other model families.


2026-11-12, engine 6a9dbc7f and d0590a47: proposal merge's three commit sites and the markdown export route through commit_staged_pinned and commit_staged, and both are rows of the generated failed-write matrix. Building the rows found and repaired five absorbed failures: the export's store kept the hashes of the bytes it had replaced, the merge left its planned store in place when the proposal record read failed, read the fork's anchors as empty on a read failure, read an unreadable sidecar in its validation baseline as a finding, and the brief's precheck reported a storage failure as the entry's result.
