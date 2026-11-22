---
type: decision
created_date: 2026-11-22T13:15:46Z
last_modified: 2026-11-22T13:15:46Z
status: accepted
decided_on: 2026-11-21
deciders: operator (shared-engine bundle, go of 2026-11-21), implementing agent
scope: subsystem
tags: write-lock,mutation,workspace-policy
---

# A write waits for a mem's lock no longer than a workspace deadline

## Decision
We chose to bound a write's wait for a mem's write lock by a workspace setting, `write_lock_timeout_secs` in the `[mutations]` table: 30 seconds when absent, `0` for the unbounded wait. Past the deadline the write refuses `WRITE_LOCK_TIMEOUT` with the mem and the time it waited, writes nothing, and holds none of the locks it had taken. The deadline is a property of the deployment and no parameter of a tool call.

## Context
The per-mem write lock is an exclusive file lock that dies with its process, so a crashed writer holds nothing. A live writer that hangs held it without bound, and every later writer of that mem waited behind it. With one engine behind a server and several callers, one hung writer parked them all.

## Consequences
- A hung writer costs the others one deadline each, then a typed refusal they can act on.
- Ordinary contention still resolves by waiting; nothing an agent has to retry appears below the deadline.
- The folder backend's completion of an interrupted commit at load time is not bounded by it: a fresh process still waits behind a writer that hung in the middle of its commit.
- An operator who wants the old behaviour sets `0`.

## Relationships
- **DERIVED_FROM**: [[a-write-whose-guard-or-write-set-reads-the-workspace-locks-every-writable-mem]]

## Options

- Fail at once with a try-lock: rejected, contention between two writers resolves in milliseconds and would surface as refusals.
- Keep waiting without bound and log a warning: rejected, the caller is still parked.
- A timeout parameter on every mutation tool: rejected, it changes every tool's shape for a value that belongs to the deployment.
- A workspace setting with a default: chosen.
