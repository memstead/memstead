---
type: decision
created_date: 2026-07-13T16:43:06Z
last_modified: 2026-07-13T16:44:06Z
status: accepted
decided_on: 2026-07-03
deciders: dasboe
scope: subsystem
tags: mcp, concurrency, integrity, fail-safe, engine
---

# Refuse tool calls on a poisoned engine lock instead of recovering or panicking

## Decision
We chose to route every engine-mutex acquisition on the MCP servers' tool-dispatch paths through one `lock_engine!` macro (`crates/memstead-mcp/src/lib.rs`): on a poisoned lock — a prior tool call panicked while holding the guard — it early-returns the typed `ENGINE_LOCK_POISONED` refusal (via `engine_lock_poisoned` in `error_envelopes.rs`) telling the caller to restart the server, rather than panicking the whole process or clearing the poison and continuing. Recovery is a restart, which reloads in-memory state from mem-repo git truth.

## Context
The two servers' dispatch paths held 31 bare `lock().unwrap()` calls. A single panicked tool call would poison the mutex and every subsequent `unwrap` would panic the server, taking down every session sharing that process. But silently clearing the poison is worse: a panic mid-mutation can leave the in-memory [[engine--graph]] half-applied, and resuming over that corrupts the store with no signal.

## Consequences
- A single panicked tool call degrades to a typed refusal on subsequent calls, not a dead server; the code joins the [[engine--typed-error-and-warning-envelope]] catalogue and the generated error index.
- There is deliberately no in-process recovery: correctness after a mid-mutation panic depends on reloading from git, which only a restart does. This rests on [[engineering--engine-owns-mem-repo-state]] — the mem-repo, not the RAM store, is the trusted state.
- The embedding-API entry points (export / count / with_engine) keep their documented panic contract — they are not tool-dispatch paths and are not wrapped by the macro.
- A refusal test poisons the mutex for real, so the safe-degradation path is covered rather than assumed.

## Relationships
- **REFERENCES**: [[engine:graph]]
- **REFERENCES**: [[engine:typed-error-and-warning-envelope]]
- **REFERENCES**: [[engine-owns-mem-repo-state]]
- **REFERENCES**: [[engine:mcp-tool-surface]]
- **MOTIVATED_BY**: [[engine-owns-mem-repo-state]]

## Options

- Keep `lock().unwrap()` — rejected: one panicked call kills the server for everyone.
- Clear the poison and continue (poison-recovery) — rejected: resuming over a half-applied mutation corrupts the in-memory store silently, the worst failure mode.
- Typed refusal + restart-from-git (chosen): degrade the poisoned lock to an `ENGINE_LOCK_POISONED` refusal on the [[engine--mcp-tool-surface]] and let a restart reload authoritative state from the mem-repo.

## Notes


