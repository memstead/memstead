---
type: decision
created_date: 2026-09-11T00:04:49Z
last_modified: 2026-09-11T00:04:49Z
status: accepted
decided_on: 2026-09-11
deciders: architecture-seams bundle plan 04 (agent, under the standing engine-change rule)
scope: subsystem
tags: reload, notices, coherence, kernel
---

# Reload notices ride an operation scope and nothing on the engine hands them out on demand

## Decision
We will route the reload-before-operation notices through an OperationScope instead of an accumulator on the engine: a caller opens the scope over its engine handle (an owned engine, a mutable borrow, a box or a mutex guard; the MCP lock macro opens one for every tool call, the CLI commands and ui-api where they obtain the engine), reload_if_stale records its notices into the scope, finish hands them out as a value together with the handle, opening a scope discards what an unscoped caller left behind, and a dropped scope discards its own. Engine::take_mem_changed_notices is gone ([[engine--reload-before-operation-coherence]], [[engine--mem-changed-reload-notice]]).

## Context
Until 2026-09-11 the notices accumulated on the engine and some fifty call sites drained them by convention, two of them with a bare let-underscore drain and an explanatory comment; the engine's own doc comment named the hazard: an undrained notice leaked into the next operation's response. The 2026-09-10 architecture assessment ranked it fourth among the surviving risks because it corrupts answers rather than descriptions, and both counter-checks kept it as the cheapest fix on the list.

## Consequences
- No operation can carry another operation's notice into its response; the folder-drift test pins that a second scope inherits nothing and that an unscoped notice is discarded at the next begin.
- A legitimate notice still rides the operation that reloaded; the reload-notice suite pins it.
- Consumers changed in the same session (MCP macro, ten CLI commands, ui-api's per-request drain); the wire is unchanged.
- Cost accepted: a caller that never opens a scope loses the notices of its own reloads, visibly, instead of leaking them later.

## Relationships
- **REFERENCES**: [[engine:reload-before-operation-coherence]]
- **REFERENCES**: [[engine:mem-changed-reload-notice]]

## Options

- Mark the drain getter must_use: rejected, a must_use on a getter still permits a bare let-underscore, the pattern found in the tree.
- Thread the notices through every operation's outcome type: rejected, a wire-shape change across every mutation result for a leak the scope closes structurally.
- Keep the convention and add a lint: rejected, cause before gate.
