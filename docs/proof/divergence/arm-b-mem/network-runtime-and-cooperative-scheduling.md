---
type: spec
created_date: 2026-07-15T08:40:21Z
last_modified: 2026-07-15T17:31:37Z
level: M1
stability: experimental
tags: runtime, concurrency, event-loop, coroutines, phase-6
---

# Network Runtime and Cooperative Scheduling

## Identity
The Kāra Phase-6 network runtime: a mio-based event-loop substrate (epoll / kqueue / IOCP) with a background poller and parked-task scheduler, plus the LLVM-coroutine codegen transform that turns network-boundary functions into cooperatively-yielding coroutines re-driven by the event loop.

## Purpose
To let Kāra run high-connection network servers on non-blocking I/O without async/await keywords — the compiler lowers I/O-bearing functions to LLVM coroutines that suspend on `Pending` and are resumed by the dispatcher, so I/O concurrency is derived, not colored.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[karac-runtime-library]]
- **DEPENDS_ON**: [[effect-checker]]
- **MOTIVATED_BY**: [[phased-runtime-model]]
- **REFERENCES**: [[phased-runtime-model]]
- **REFERENCES**: [[auto-concurrency]]
- **REFERENCES**: [[colored-functions]]
- **REFERENCES**: [[effect-checker]]
- **MOTIVATED_BY**: [[llvm-coroutines-for-the-network-async-transform]]
- **REFERENCES**: [[http-server-surface]]
- **REFERENCES**: [[llvm-coroutines-for-the-network-async-transform]]

## Realization

- Event loop: runtime/src/event_loop.rs (mio epoll/kqueue/IOCP substrate, background event-loop poller + EventLoop, scheduler dispatcher thread, parked-task re-poll on wakeup, scheduler stats snapshot FFI)
- Async scheduler: runtime/src/scheduler.rs (parallel scheduler dispatch + herd-free wakeup handoff); structured-concurrency lowering src/codegen/task_group.rs
- Runtime FFI: register/poll/wake surface, KaracParkedTask + KaracPollResult ABI (runtime/src/lib.rs)
- Coroutine transform: src/codegen/coro.rs (LLVM coroutine emission, drive bridge / resume shim, drop-across-suspend, cooperative cancellation); coro passes wired into the pipeline and also run on the JIT install path
- RAII-across-suspend guard: E_RAII_ACROSS_YIELD

- TCP: runtime/stdlib/tcp.kara, src/codegen/tcp.rs; tests/tcp_stream.rs, tests/tcp_listener.rs, tests/park_and_wake.rs
- TLS: runtime/src/tls.rs (gated behind a `tls` cargo feature), runtime/stdlib/tls.kara, src/codegen/tls.rs; tests/tls_codegen.rs
- WebSocket: runtime/stdlib/ws.kara; tests/ws_framing.rs, tests/coro_e2e.rs
- HTTP: runtime/stdlib/http.kara, src/codegen/http.rs; tests/http_server.rs, tests/http_client_codegen.rs

## Specifies

- Network event-loop substrate over mio; background poller; dispatcher thread that re-polls parked tasks on wakeup; parallel scheduler dispatch with herd-free wakeup handoff; scheduler stats snapshot exposed via FFI.
- LLVM-coroutine async transform: network-boundary free fns and method handlers compiled as coroutines, dispatcher-driven (not caller-resumed); leaf-suspend emission shape; control-flow-around-suspend correctness; drop-across-suspend on the destroy edge; cooperative cancellation via cancel-check shim + destroy-edge slot-signal. Enabled by default for `karac build`/`run`.
- E_RAII_ACROSS_YIELD: RAII values may not be held live across a suspend point.
- TCP: TcpStream / TcpListener bind+accept+read+write composed via `karac_park_on_fd` (leaf parking primitive); TcpStream.connect (plain-TCP client); read/write return `Result[i64, TcpError]`; named construction-error variants (AddrInUse/ConnectionRefused); network construction returns `Result`, not an fd:-1 sentinel; write_all looping wrapper; close-on-drop.
- TLS: rustls server-side + client-side TLS (TlsStream.connect); `TlsListener` / `TlsStream` stdlib surface; `TlsError` enum; TLS-aware runtime archive selection (lean archive linked for compute-only programs); `WebSocket.accept_tls` (wss://); handshake off the accept thread.
- WebSocket: RFC 6455 framing, HTTP upgrade handshake with §4.2.1 validation, slowloris read-timeout + bounded total handshake duration + capped TLS handshake pool, control + binary frames, fragmentation reassembly + client-side masking.
- HTTP: std.http server + client over this stack; HTTP/2 via hyper `auto::Builder` (ALPN h2 + h2c). See [[http-server-surface]].
- Structured concurrency: `spawn()` / `TaskGroup` drive concurrent tasks on the async scheduler (see [[auto-concurrency]]).

- Event-loop scaling and robustness: the reactor is sharded by fd (Stage B2) and does combined poll-and-dispatch per shard with the queue removed (Stage B3); the fds lock is released across `epoll_ctl` in register/deregister; SIGPIPE is masked in reactor init (silent server death under reconnect storm); `llvm.coro.save` is emitted before the I/O-park publish to avoid a cross-thread frame UAF. Windows IOCP is now SHIPPED: a native Windows IOCP event-loop + TLS validated on a real box (register/deregister + TCP + plain WebSocket), the AOT build pipeline (linker driver + stdio) ported to Windows, the socket-fd ABI widened i32→i64, and the reactor timer resolution raised to 1ms. Validated at 250k and 1M loopback connections; the M3 cross-platform-parity gate is flipped to DONE. A 1M-conn IOCP soak surfaced an unbounded per-connection spawn/TaskGroup handle leak in the canonical accept-loop server shape (children never join in an infinite loop); fixed by a detached-gated eager-reap (KaracTaskHandle detached flag + karac_runtime_task_detach FFI; register-time sweep of terminal detached children; free-spawn coro self-reap via a slot-armed reap), ported to both wasm schedulers.

## Constraints

- A value with a destructor (RAII) may not be held live across a suspend point.
- Cooperative only: a task suspends at I/O boundaries; there is no preemption.
- Suspended tasks support cooperative cancellation (shim cancel-check + destroy-edge slot-signal); drop-across-suspend runs on the coroutine destroy edge.
- The pool worker threads are shared with `karac_par_run` auto-parallel workers.

## Rationale

Phase 6. Realizes the v1.1 event-loop leg of the [[phased-runtime-model]]; the async transform is how [[auto-concurrency]] extends to I/O without reintroducing [[colored-functions]]. Effect signatures from the [[effect-checker]] identify network-boundary functions. The transform is now LLVM-coroutine-based per [[llvm-coroutines-for-the-network-async-transform]], which replaced the earlier hand-rolled state-machine body-splitter.
