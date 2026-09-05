---
type: decision
created_date: 2026-09-05T16:16:40Z
last_modified: 2026-09-05T16:27:28Z
status: accepted
decided_on: 2026-09-05
deciders: engine-moves plan (census posture C, 2026-09-04); agent decision under the engine-changes rule
scope: subsystem
tags: mem-repo, remote, status, multi-machine
---

# Remote staleness is an axis of memstead status, read-only and fail-open

## Decision
memstead status --remote [NAME] (default origin) compares every mounted git-branch mem and the __MEMSTEAD schemas ref against the named remote with one read-only git ls-remote per mem-repo and classifies each ref, mount first: a remote branch nothing mounts is unmounted_remote whether or not a local ref of that name exists (a probable leftover, or not graph state at all: a notice, never staleness); a tracked ref is in_sync, local_ahead, behind, forked, unfetched (the remote head is not in the local object store, so behind or forked, which memstead fetch tells: the ancestry of a commit this clone never fetched cannot be tested, and guessing forked would be a lie), or missing_local; a mounted branch the remote lacks is not_on_remote (a notice). The run moves no ref and never refuses: no git-branch mount, no remote, an unreachable remote and a lean flavour without git hooks each become a named notice and the outcome stands as not stale. Exit 6 (REMOTE_STALE, after the report) when any ref is behind, forked, unfetched or missing locally, exit 0 otherwise; the comparison rides the --json payload as remote. The reconcile itself stays memstead fetch and memstead pull. The exit code is 6, the CLI's findings code for a completed run whose result the caller asked to be gated on, not 3 as the retired shell checker used: 3 is the CLI's documented not-found code, and a status read that reports staleness under it would contradict the exit-code table every consumer reads. Read-only is pinned by two source-scan tests: the engine's remote_status names no hook that moves a ref, and the git-branch helpers it reaches run no mutating git verb.

## Context
From 2026-08-06 a shell checker in the workspace's dev tree compared the dogfood mem-repo against its remote at session start, mount-aware and fail-open, and the graph-plan walker ran it before its first mutation. The engine already owned fetch, pull, push --all and remote-add, with ls-remote and resolve-ref hooks behind push --all, so the remote and its credentials were the engine's business and the comparison the one thing left outside. The 2026-09-04 census (posture C) moved it in: a flag on status, because status is where a caller asks where the workspace stands and the remote is one more axis of that; a separate verb was rejected as one more top-level command for one question status already answers locally, and fetching during status was rejected because a status read must never move a ref.

## Consequences
Any workspace with a mem-repo remote gets the check, on every machine, with no script to install; the walker and the sync skill run it at session start and reconcile on exit 6. The git-branch hook table gains is_ancestor (git merge-base --is-ancestor), read-only like ls_remote and resolve_ref. The shell checker and its fixture test retired. Not carried over: the checker's fallback for an unreadable mount table, which cannot arise inside the engine (it booted from that table). Rejected: fetching during status; a memstead remote-status verb.
