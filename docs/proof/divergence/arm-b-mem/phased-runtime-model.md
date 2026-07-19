---
type: decision
created_date: 2026-07-15T07:23:20Z
last_modified: 2026-07-15T10:20:41Z
status: accepted
decided_on: 2026-05-10
deciders: kara-maintainers
scope: subsystem
tags: runtime, concurrency, roadmap
---

# Phased Runtime Model

## Decision
We chose to ship the runtime in phases: v1 uses blocking I/O on a worker pool, v1.1 adds a network event loop, and v2 delivers a full hybrid runtime. v1 deliberately does not attempt an async event loop.

## Context
A full hybrid async runtime is a large, risky build. The project wants a working native language early. Blocking I/O on a pool is enough to run real workloads (including the Parallax HTTP demo) while the event loop is deferred, and it composes with effect-driven auto-concurrency because parallel regions run on the pool rather than an async executor.

## Consequences
- v1 HTTP/server work uses blocking calls; the server originally spawned a thread per call and was later fixed to a long-lived worker pool.
- Network event loop and full hybrid scheduling are explicitly deferred to v1.1 and v2.
- Keeps the runtime small and debuggable during the codegen-heavy phases.

## Relationships
- **REFERENCES**: [[network-runtime-and-cooperative-scheduling]]

## Options

- Full async event loop in v1 — rejected: too large and risky for the first native runtime.
- Blocking-only forever — rejected: won't scale to high-connection network servers.
- Phased v1 blocking → v1.1 event loop → v2 hybrid — chosen.

## Notes

The v1.1 event-loop leg is now underway as Phase 6: a mio-based non-blocking event-loop substrate plus a codegen LLVM-coroutine transform that turns network-boundary functions into cooperatively-yielding coroutines. See [[network-runtime-and-cooperative-scheduling]]. The v1 blocking-pool server still stands; the two coexist during the transition.
