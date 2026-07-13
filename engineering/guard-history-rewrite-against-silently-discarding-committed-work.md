---
type: decision
created_date: 2026-07-13T16:43:04Z
last_modified: 2026-07-13T16:44:02Z
status: accepted
decided_on: 2026-07-04
deciders: memstead-core
scope: subsystem
tags: git-branch, history-rewrite, concurrency, safety, branch-reset, engine
---

# Guard history-rewrite against silently discarding committed work

## Decision
We chose to make `branch_reset` — the engine's single history-rewrite primitive — a *guarded* operation that refuses to silently discard committed work, rather than an unguarded force-reset in git's own mould. Two independent guards compose: (a) pushed-commit protection refuses (`PUSHED_COMMITS_PROTECTED`, offending SHAs carried verbatim) when any commit the reset would drop is reachable from a `refs/remotes/*` ref; and (b) an optional caller-supplied `expected_head` compare-and-swap refuses (`BRANCH_RESET_HEAD_MOVED` `{mem, expected, current}`) when the live branch tip has advanced past the head the caller observed, so a commit a sibling process landed between observation and reset can never vanish silently. Read-only (folder/archive) mounts refuse the write outright (`READ_ONLY_MOUNT`) before any dispatch. The residual read-to-write window is closed by the underlying `gix` ref-update transaction (`PreviousValue::MustExistAndMatch`), whose worst case is a refusal, never an overwrite.

## Context
The git-branch backend is Memstead's affordance for multi-actor, multi-process work: a long-lived MCP engine shares one mem-repo with out-of-band CLI siblings and with remotes reached over fetch/pull/push. `branch_reset` exists for replay and recovery workflows that must rewind a branch pointer over existing commits — the one place in the engine that moves a branch backwards. An unguarded rewind in that setting would let already-published history or a sibling's just-landed commit disappear with no signal — unrecoverable data loss disguised as a routine operation. The guard had to distinguish work that is safe to drop (un-pushed, and observed by the caller as current) from work that must never be dropped silently (reachable from a remote-tracking ref, or landed since the caller's observation).

## Consequences
- Replay/recovery workflows operate only over un-pushed commit segments; a reset that would cross a pushed commit refuses and surfaces the offending SHAs for the operator to reconcile rather than proceeding.
- `expected_head` is optional: the CLI path passes `None` and relies on the git-standard ref-update CAS alone, while the macOS guarded-undo surface passes the observed head for the stronger sibling-safety guarantee — the stricter guard is offered, not imposed.
- "Pushed" is defined purely by local `refs/remotes/*` reachability; the probe never contacts the network, so an operator who hand-edits remote-tracking refs can fool it. The trade accepted is git-standard local semantics over a network round-trip on every reset.
- Each reset adds a reachability walk over remote-tracking refs — negligible against the history rewrite itself.

## Relationships
- **REFERENCES**: [[use-optimistic-content-hash-locking-for-all-mutations]]
- **REFERENCES**: [[engine:git-transport-and-history-surface]]
- **REFERENCES**: [[engine:memstead-swift-uniffi-foreign-function-contract]]
- **MOTIVATED_BY**: [[use-optimistic-content-hash-locking-for-all-mutations]]

## Options

- Unguarded force-reset (git `reset --hard` / `push --force` semantics): rejected — silently discards pushed and sibling-landed commits, the exact failure the multi-process model exists to prevent.
- Make `expected_head` mandatory (always require an observed-head token): rejected — a CLI operator rewinding their own local branch has no sibling to race, so forcing the token adds friction without added safety there; the guard is offered per-call, not imposed.
- Contact the remote to define "pushed" authoritatively: rejected — a network round-trip on every reset for a guarantee the local `refs/remotes/*` refs already approximate to git's own standard.

## Notes

Extends the optimistic-locking discipline of [[engineering--use-optimistic-content-hash-locking-for-all-mutations]] from entity-hash to branch-head granularity — one uniform "never overwrite what you did not observe" posture across entity mutations and history rewrite. Realized in `crates/memstead-git-branch/src/ops/branch_reset.rs`; surfaced through [[engine--git-transport-and-history-surface]] and exposed over the [[engine--memstead-swift-uniffi-foreign-function-contract]] for the app's guarded-undo affordance (`branch_reset_stranded_refs` gives that surface a cross-mem strand preview before confirming).
