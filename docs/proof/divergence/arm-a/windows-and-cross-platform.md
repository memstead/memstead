---
type: architecture
title: Windows IOCP and cross-platform parity
updated_round: 6
---

# Windows IOCP and cross-platform parity

**New in round 6.** Native **Windows** support landed — an **IOCP event loop**, **TLS**, and an ahead-of-time **AOT build pipeline** — flipping the project's **M3 cross-platform-parity gate to DONE** (`fabe54e7`). Kāra's runtime is now **tri-platform**: Linux **epoll** / macOS **kqueue** / Windows **IOCP**, all on the same `mio`-based [[design-runtime-phases|event-loop]] substrate and [[networking|network stack]].

## Windows IOCP event loop

- **Native IOCP event-loop + TLS** (`bf5b0cac`), validated on a real box. Code lives in `runtime/src/event_loop.rs`.
- Built incrementally as a spike (`9c560da8`): event-loop bodies **register/deregister**, **TCP**, and plain **WebSocket** across steps 1–4.
- **Step 0** widened the socket fd ABI from **i32 to i64** (`e9b0ea2b`) as IOCP prep — Windows `SOCKET` handles are pointer-width, not 32-bit.
- Spike docs: `docs/spikes/windows-iocp-eventloop.md`, `docs/spikes/windows-iocp-scale-investigation.md`.

## Windows AOT build pipeline

- The ahead-of-time build pipeline was ported to Windows — **linker driver + stdio** (`c26c3f5d`).
- `karac build` now produces **native Windows binaries**. See [[codegen]], [[implementation-phases]].

## Validated on real hardware

- Windows IOCP validated at **250k loopback connections** (`ffeae92a`).
- Port marked **DONE** with all steps validated natively (`8b0117fd`).
- A connection-density run held **45k connections cleanly** on Windows (`6c91e590`, "Finding 5").
- Windows-parity status reconciled; **Slice 6 + M3 staging** flipped to done (`5e3841d3`).
- Headline: **M3 cross-platform-parity gate → DONE** (`fabe54e7`) — Linux + macOS + Windows all supported.

## CI

- A **windows-latest** job now lints the `cfg(windows)` runtime surface (`e0f2432d`).
- The Windows IOCP bridge is **reset between tests** to fix flaky windows-latest CI (`48075301`).
- `scripts/bug-curve.py` was made **UTF-8/LF-safe** so it runs on Windows (`73d3dfce`).

## The 1M IOCP scale investigation

Scaling a spawn/TaskGroup echo server to **~1M connections** on Windows surfaced an unbounded **~100 bytes/connection heap leak** in the **spawn / structured-concurrency model** (`6aca27ac`). The culprit is the CANONICAL accept-loop shape:

```
loop { tg.spawn(|| handle(conn)) }
```

- **Root cause** (`runtime/src/scheduler.rs`): a `TaskGroup` registers each child handle and frees them only at the group's **scope exit** — which never happens in an infinite accept loop; **free spawn** orphaned its handle outright.
- **Platform-agnostic** — affects Linux/macOS too; **Windows just surfaced it at scale**.
- **Fix — detached-gated eager-reap:** a `detached` flag on the task handle + a `karac_runtime_task_detach` FFI. Codegen marks discarded `spawn`/`tg.spawn` handles **detached**; the group's register-time sweep frees detached **terminal** children, bounding the child set to **live** tasks; a free-spawn worker **self-reaps** on completion. Retained `.join()`-able handles are **never detached**, preserving structured-wait invariants.
- **Bug-ledger** ([[bug-tracker]]): **B-2026-06-17-2** (the canonical `tg.spawn` shape), **B-2026-06-17-3** (free-spawn+coroutine residual, fixed via a slot-armed self-reap), **B-2026-06-17-8** (WASM link failure — `karac_runtime_task_detach` had to be defined in both [[wasm-targets|wasm]] schedulers).
- **Verified** on the Linux **ASAN+LSan** gate — the authoritative leak gate. See [[design-concurrency-and-providers]].

### Other Windows findings

- **Finding 2** — raising the Windows **timer resolution to 1 ms** in reactor init (`8f0c56c6`); the inline-I/O floor is Windows-timer-specific, **not** platform-agnostic.
- **Finding 4** — **parallel accept is NOT the fix** (tested); the throughput ceiling is **kernel loopback churn**.

Related: [[design-runtime-phases]], [[networking]], [[design-concurrency-and-providers]], [[bug-tracker]], [[codegen]], [[wasm-targets]], [[examples-and-benchmarks]], [[implementation-phases]], [[index]]
