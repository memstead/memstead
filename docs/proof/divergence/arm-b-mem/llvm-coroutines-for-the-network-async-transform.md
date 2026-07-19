---
type: decision
created_date: 2026-07-15T10:19:01Z
last_modified: 2026-07-15T10:19:01Z
status: accepted
decided_on: 2026-07-15
deciders: kara-maintainers
scope: subsystem
tags: runtime, codegen, coroutines, async, phase-6, phase-7
---

# LLVM Coroutines for the Network Async Transform

## Decision
We chose to implement Kāra's network async transform with LLVM coroutine intrinsics and passes (src/codegen/coro.rs) rather than a hand-rolled state-machine body-splitter, and to enable coroutines by default for `karac build`/`run`.

## Context
The Phase-6 network runtime originally rewrote network-boundary functions into cooperatively-yielding poll functions via a hand-rolled state-machine body-splitter (src/codegen/state.rs). A demo-affecting bug ('bug C') surfaced by the flagship WebSocket demo showed the body-splitter could not correctly split a network call made inside a helper function — it only handled suspension points in the boundary function's own body — and the root cause was localized precisely to the body-splitter's IR. An architectural fork was opened; a spike (docs/spikes/network-async-coroutine-transform.md) compared repairing the splitter against delegating suspend/resume/frame-layout to LLVM's coroutine lowering.

## Consequences
- New coroutine codegen in src/codegen/coro.rs; LLVM coroutine passes wired into the pipeline (A2 slice 1) and validated through the builder + llvm-sys emission path.
- Coroutines flipped on by default for `karac build`/`run`; two coro×auto-par interactions fixed as part of the flip.
- A coroutine drive bridge / resume shim drives network-boundary free fns and method handlers, dispatcher-driven end-to-end (not caller-resumed).
- Correctness work: drop-across-suspend on the destroy edge; control-flow-around-suspend validation; leaf-suspend emission shape; the concurrent WS-over-TLS coroutine resume race fixed by dropping a redundant accept park.
- Cooperative cancellation via a shim cancel-check plus a destroy-edge slot-signal (A2 slice 5c); coro correctness passes also run on the JIT install path.
- The hand-rolled state-machine body-splitter is retired as the network-async mechanism.

## Relationships
- **REFERENCES**: [[network-runtime-and-cooperative-scheduling]]
- **REFERENCES**: [[auto-concurrency]]

## Options

- Repair the state-machine body-splitter to split network calls nested in helper functions — rejected: the bug was IR-localized and the splitter did not generalize past single-body boundaries.
- Delegate suspension to LLVM coroutine intrinsics/passes — chosen: LLVM owns frame layout, suspend/resume, and destroy edges, and composes with the existing dispatcher-driven scheduler.

## Notes

Realizes the coroutine-based transform now described as current state in [[network-runtime-and-cooperative-scheduling]]. Extends effect-driven [[auto-concurrency]] to I/O without reintroducing colored functions.
