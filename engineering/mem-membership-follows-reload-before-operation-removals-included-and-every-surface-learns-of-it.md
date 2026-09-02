---
type: decision
created_date: 2026-09-02T12:41:03Z
last_modified: 2026-09-02T12:41:03Z
status: accepted
decided_on: 2026-09-02
deciders: operator (backlog-engine bundle A, go of 2026-09-02), implementing agent
scope: subsystem
tags: mem-repo,reload,mcp,ui-api
---

# Mem membership follows reload-before-operation, removals included, and every surface learns of it

## Decision
The engine reconciles its mount roster before every operation (`reload_if_stale` phase minus one): a mem registered or unregistered by another process is mounted or unmounted in place, schemas refreshed first, removals atomic, cold mounts passing the same boot quarantine (`MOUNT_UNBACKED` parity with a booted engine). A roster change is announced once per surface: the MCP marker line `MEM_ROSTER_CHANGED`, the ui-api server event `mem-roster-changed` that invalidates the web app's per-mem subscriptions, and the CLI's full refresh report (`mems_unmounted`, `mems_quarantined`). An operation on a mem that is no longer mounted refuses `MEM_UNMOUNTED`; a mem initialised and unregistered between two operations reconciles as no change and answers `ENTITY_NOT_FOUND`, consistent with the memory covering mems this engine mounted.

## Context
Until 2026-09-02 a long-running MCP server or ui-api kept serving a mem another process had deleted, and never saw one another process had created; the reload covered content, not membership.

## Consequences
Two engines on one workspace agree on the roster within one operation of each other. A quarantined-only roster change carries the SSE event but not yet the MCP marker line (observation left open at close). The probe benchmark reads 2.3 times the content-only reload's cost; accepted for correctness.

## Options

A roster watch file with a restart: rejected, restarts lose the session. Membership only at boot: rejected, that is the defect.
