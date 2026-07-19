---
type: design-decision
title: Phased runtime model
updated_round: 6
---

# Phased runtime model

Kāra's runtime is delivered in phases (a design commitment from the redesign, CHANGELOG):

- **v1 — blocking I/O.** The first release uses simple blocking I/O.
- **v1.1 — network event loop.** A network event loop is added. **Under active
  construction in round 2** (Phase 6, see below).
- **v2 — full hybrid.** A full hybrid runtime.

This is distinct from the compiler **implementation** phases (Phase 1–11); see
[[implementation-phases]]. The v1 positioning was **backend-first** (adopted as the **v64**
brainstorm graduation — see [[history-reversals-and-deprecations]]): the LLVM codegen
path is prioritized for the first shippable release, tracked as "Phase 8.5 v1 ship
readiness". In round 3 the README **reframed** this from "backend-first at v1" to a
**"Production-Ready skeleton"** — the same codegen-first bet, described as a working
end-to-end skeleton rather than a backend-only cut. See [[history-reversals-and-deprecations]].

Runtime concerns already built: a **long-lived worker pool** for parallel execution, an
HTTP server surface, binary-size reduction (strip, `panic=abort`, cross-archive LTO +
DCE, fat LTO), and `std.runtime` introspection. See [[codegen]] and
[[design-concurrency-and-providers]].

## v1.1 network event loop — the round-2 build (Phase 6)

The v1.1 event loop is being built now, in the `runtime/` crate (`event_loop.rs`) and a
family of Phase 6 lines:

- **Line 15 — event-loop substrate**: an epoll / kqueue / IOCP poller via **`mio`**.
- **Line 17 — runtime FFI + scheduler** (slices 1–5): register/poll/wake FFI,
  `KaracParkedTask` + `KaracPollResult` ABI, a background event-loop poller + `EventLoop`
  refactor, a **scheduler dispatcher thread** that re-polls parked tasks on wakeup, and a
  scheduler-stats snapshot FFI. (A Windows IOCP + codegen sub-item is tracked for later.)
- **Line 26 — network-boundary state-machine transform** (slices 1–8w, the round's single
  largest feature): each **network-boundary function** is compiled into a **state
  machine** — a side-table of such functions, per-function yield-point enumeration,
  per-yield captured-locals sets, a synthesized **state struct** (LLVM type + layout +
  constructor helper), a **poll function** with **switch-on-tag dispatch**, caller-side
  argument-storing and network-boundary **intercept routing**, **cooperative yield on
  `Pending`**, captured-local reload prologues + cross-yield writebacks, terminal-field
  return values, a **state-struct destructor** (unified unwind for `?`-Err and
  cooperative-cancel), and per-monomorphization emission. This is how Kāra gets
  suspend/resume I/O **without async/await** (see [[design-concurrency-and-providers]]).
- **Line 31 — `E_RAII_ACROSS_YIELD`**: a compile error when an RAII value would straddle a
  yield point (foundation, slice 1).
- **Line 7 — specs** for the above: state-machine transform spec, RAII-across-yield error
  spec, panic-during-suspend semantics, a **debugger-contract extension for parked tasks**,
  an FFI-across-yield spec, and RC-drop ordering across yield points.

## Round-3 maturation: scheduler, network stack, structured concurrency

The event loop went from substrate to a usable I/O runtime in round 3:

- **Scheduler dispatcher** (`runtime/src/scheduler.rs`) — the async-scheduler slice 1 test
  confirms the dispatcher **drives concurrent parked tasks** end to end. It backs both the
  [[design-concurrency-and-providers|structured-concurrency `spawn`/`TaskGroup` surface]]
  and the leaf **`karac_park_on_fd`** parking primitive.
- **The [[networking|network I/O stack]]** — non-blocking **TCP**, **rustls TLS**,
  **RFC 6455 WebSocket**, and **File** handles all built over `karac_park_on_fd`, with
  **close-on-drop** for TCP/File handles. This is the concrete payoff of the state-machine
  transform: blocking-looking stdlib code that suspends/resumes cooperatively.
- **`E_RAII_ACROSS_YIELD` / `CancelSafe`** — the yield-straddling guard grew a
  **`CancelSafe` marker trait** (user-extensible opt-in), a flow-sensitive
  **`#[cancel_unsafe_until]`** attribute, raw-pointer detection, and binding-construction
  span anchoring (Phase 6 line 155, slices 2–5). See [[design-effect-system]], [[attributes]].

## Round-4: async I/O re-based on LLVM coroutines (A2)

The round-2/3 network async model — a **hand-written state-machine transform** that split
each network-boundary function into a poll function with switch-on-tag dispatch (Phase 6
line 26) — was **superseded** in round 4 by an **LLVM-coroutine transform** (the "A2"
track). A bug in the state-machine **body-splitter** (a network call inside a helper
function miscompiled — "bug C") forced an architectural fork; a spike
(`docs/spikes/network-async-coroutine-transform.md`) recommended the coroutine path.

- LLVM coroutine passes are wired into the codegen pipeline; network-boundary functions are
  compiled as **dispatcher-driven coroutines** with a resume-shim drive bridge.
- **Drop-across-suspend correctness** on the destroy edge, and a **cooperative cancellation
  mechanism** (cancel-check shim + destroy-edge slot-signal).
- **Flipped on by default** for `karac build`/`run`; also runs correctly under the new JIT.

This does **not** reverse the "no async/await, no colored functions" bet — like the
state-machine transform it replaces, it is an internal, compiler-driven transform. See
[[codegen]], [[design-concurrency-and-providers]], [[history-reversals-and-deprecations]].

## Round-5: reactor sharding, Nagle fix, and WASM lowering

Round 5 productionized the event loop for the [[examples-and-benchmarks|density
benchmarks]] and extended it to WASM:

- **Sharded reactor (Stage B2/B3)** — the event-loop reactor is **sharded by fd** (Stage
  B2), then **combined poll-and-dispatch per shard removes the queue** (Stage B3); the `fds`
  lock is released across `epoll_ctl`. Isolated-burst evidence JSONs pin the stages.
- **Nagle / `TCP_NODELAY` p50 fix** — `TCP_NODELAY` was set on accepted WS-TLS sockets and
  **extended to all WS/TLS handshake paths**, diagnosing and fixing a p50 latency cliff:
  **45.01 → 1.63 ms at 250K** connections — now **field-leading**. See
  [[networking]], [[examples-and-benchmarks]].
- **Robustness** — **SIGPIPE masked** in reactor init (silent server death under reconnect
  storm), shutdown **drains real fd readiness** instead of eating it, and a **lean, panic-free
  stable merge sort** drops the ~262 KiB large-N sort floor (with lean fatal paths dropping a
  ~250 KB std-IO anchor).
- **Async-sleep substrate** — an **event-loop timer wheel** + C-ABI async-sleep timer backs
  the [[design-concurrency-and-providers|auto-parallel `sleep_ms`]] path.
- **WASM concurrency lowering** — a **sequential default** (`runtime/src/seq_scheduler.rs`)
  runs `par`/`spawn` in order on plain wasm32; **`--features wasm-threads`** swaps in a
  pool-backed **`wasm_threads_scheduler`**. See [[wasm-targets]].

## Round-6: native Windows and cross-platform parity

Round 6 gave the event loop a **third platform backend** and flipped the cross-platform gate:

- **[[windows-and-cross-platform|Native Windows IOCP]]** — an IOCP event loop + TLS + the AOT
  build pipeline ported to Windows, validated at 250k loopback connections and holding 45k
  cleanly. This flipped the **M3 cross-platform-parity gate to DONE** — the runtime is now
  tri-platform (Linux epoll / macOS kqueue / Windows IOCP on the same `mio` substrate).
- **Spawn-leak reap** — a 1M-connection IOCP soak root-caused an unbounded ~100 B/connection
  leak in the canonical `loop { tg.spawn(|| handle(conn)) }` accept shape (a `TaskGroup`
  freed its children only at scope exit, which never happens in an infinite loop). Fixed with
  a **detached-gated eager-reap** in `runtime/src/scheduler.rs` — platform-agnostic; Windows
  just surfaced it at scale. See [[design-concurrency-and-providers]], [[bug-tracker]].

## Hot-swap, JIT, and online-JIT (runtime directions)

Later positioning brainstorms (graduated) cover **PGO and online-JIT** (v65). Round 2 laid
groundwork: an **`--enable-hot-swap`** codegen-indirection flag (benchmarked) and a
**`.kara_jit_template` section + version manifest**. **Round 4** built the first real
**[[codegen|JIT execution path]]** (LLJIT/orc2), now the **default** for `karac test` and
`karac repl`; the AOT path stays the target for shipped binaries. See [[codegen]].
