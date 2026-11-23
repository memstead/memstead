---
type: decision
created_date: 2026-11-22T22:11:51Z
last_modified: 2026-11-22T22:11:51Z
status: accepted
decided_on: 2026-11-22
deciders: operator (stuck-mem bundle, go of 2026-11-22), implementing agent
scope: subsystem
tags: folder-backend,locks,load,quarantine
---

# A load waits for a folder mem's write lock no longer than the workspace deadline

## Decision
We chose to bound the wait of a load, and of a direct read, of a folder mem that holds a commit which is decided and not applied, by the workspace's write-lock deadline: the one setting, its default of 30 seconds, and `0` for no bound. When the limit passes with the commit still unapplied, nothing of the mem is served and nothing in it is touched. A boot puts the mem out of service as it does any mem that fails to load, with the reason `COMMIT_NOT_APPLIED`; an engine that already serves the mem keeps the state it had; a direct read returns that condition. The next load after the lock is free completes the commit. The engine hands the limit to every backend it builds or is handed, at boot, at a mount registration and when it takes up changed settings; a reader that walks a mem's directory with no workspace to ask waits the default. A wait that ran out is not repeated through the same backend for as long again as the limit.

## Context
The write-lock deadline of 0.27.0 bounded a write's wait and left one wait without bound, which that release's changelog named: completing an interrupted commit at load waited for the mem's write lock for as long as another process held it. A fresh process hung behind a writer that had stopped inside its commit, and so did every direct read of such a mem's storage.

## Consequences
- A one-shot command returns with a refusal that names the cause, the wait and the setting.
- A mem that a boot put out of service for this reason comes back with the next process, or with a reload in a running one; it is not retried on every operation.
- A writer that is alive is never disturbed: a lock is not broken, and a commit finished within the limit lets the load through.
- The storage trait carries the limit as a setter with a default that does nothing, since a backend whose commit is one indivisible step never waits there.
- Readers without a backend from the engine are bounded by the default, not by the workspace's number.
- A held lock is now a reason for quarantine, so the schema pin switch that repairs a quarantined mem takes the mem's write lock before it writes and refuses `WRITE_LOCK_TIMEOUT` with nothing changed while another process holds it.
- The change feed and each reload of a mem that is out of service read through a backend of their own and wait the limit anew.

## Relationships
- **DERIVED_FROM**: [[a-write-waits-for-a-mems-lock-no-longer-than-a-workspace-deadline]]
- **IMPLEMENTS**: [[degrade-never-disappear]]

## Options

- Refuse with the code of a timed-out write: rejected, that code says nothing was written, and here a decided commit is in storage.
- Break the lock after the deadline: rejected, a hung holder cannot be told from a live writer.
- Serve the state before the commit: rejected, parts of the commit may already be in place.
- A deadline of its own for loads: rejected, a second setting for the same wait on the same lock.
- One deadline for writes, loads and direct reads: chosen.
