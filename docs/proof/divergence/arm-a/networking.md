---
type: architecture
title: Networking stack — TCP, TLS, WebSocket, File
updated_round: 6
---

# Networking and I/O stack

**New in round 3.** On top of the [[design-runtime-phases|v1.1 event loop]], Kāra grew a
real non-blocking I/O stack — TCP, TLS, WebSocket, and File handles — all built over a
single leaf parking primitive. This is Kāra's async I/O **without async/await**: the stdlib
surfaces are ordinary blocking-looking methods; the compiler's state-machine transform and
the scheduler make them cooperatively-scheduled underneath.

## The leaf parking primitive (`karac_park_on_fd`)

- **`karac_park_on_fd`** (Phase 6 line 17 slice 6) — a leaf primitive that parks the current
  task on a file descriptor until it is ready, integrating with the scheduler dispatcher
  (see [[design-runtime-phases]]). A **programmatic park-and-wake E2E** pins the behavior
  (slice 7, `tests/park_and_wake.rs`).

## TCP (`runtime/stdlib/tcp.kara`, `src/codegen/tcp.rs`)

- **`TcpListener`** bind + accept, composing via `karac_park_on_fd` (line 17 slice 8).
- **`TcpStream`** read + write, composing via `karac_park_on_fd` (slice 9); read-direction
  E2E plus an Array→Slice coercion fix (slice 9a).
- **`TcpStream` read/write return `Result[i64, TcpError]`** (slice 9b); a
  **`TcpStream.write_all`** looping wrapper (slice 9c).
- **Close-on-drop** for `TcpStream` / `TcpListener` (slice 9d) — the handles release their
  fd at scope exit via the drop machinery (see [[codegen]]).
- **Round 4**: **`TcpStream.connect`** — a plain-TCP client (line-74 prereq); network
  construction methods now return `Result` (with **named error variants**
  `AddrInUse`/`ConnectionRefused`) instead of an `fd:-1` sentinel (see
  [[history-reversals-and-deprecations]]).

## TLS (`runtime/src/tls.rs`, `runtime/stdlib/tls.kara`, Phase 6 line 236)

Server-side TLS via **rustls**:

- **rustls server-side TLS FFI** in the runtime (slice 1).
- **`TlsListener` / `TlsStream`** kara surface (slice 2).
- **`WebSocket.accept_tls`** (slice 3) — a TLS-terminated WebSocket accept.
- **`FileSystem.read_to_string`** lowering (a follow-on to line 236) — reads certs/keys.
- **TLS test fixtures** (`tests/fixtures/tls/`, a generated self-signed cert/key) plus
  **Demo 1 `wss://`** (slices 4+5). See [[examples-and-benchmarks]].

**Round 4 — client-side TLS + `TlsError`**:

- **`TlsStream.connect`** — client-side TLS (Phase 8 line 22).
- **`Server.serve_tls`** — HTTPS for `std.http` (line 23), plus a **`TlsError` enum** +
  `wrap_tls_io_result` (line 24). The runtime gates the rustls surface behind a **`tls`
  cargo feature**, linking a **lean runtime archive** for compute-only programs. A CI job
  strips the Alpine ca-certificates-bundle in a stripped-image-HTTPS test.

## WebSocket (`runtime/stdlib/ws.kara`, Phase 6 line 17 slice 9e)

An RFC 6455 WebSocket implementation, delivered in four sub-slices:

- **RFC 6455 framing protocol** (9e.1).
- **HTTP upgrade handshake** (9e.2).
- **Control frames + binary** messages (9e.3).
- **Fragmentation reassembly + client-side masking** (9e.4). Framing is pinned by
  `tests/ws_framing.rs`.

**Round 4 — RFC 6455 handshake hardening** (Phase 8 line 128, closed after an audit walk):
**§4.2.1 handshake validation** + a **slowloris read-timeout**, **bounded total handshake
duration** + a **capped TLS handshake pool**, and a plain-WebSocket E2E round-trip +
handshake-rejection test. A macOS bug where an accepted socket stayed non-blocking after a
non-blocking listener was fixed (force blocking after accept).

## File handles (`runtime/src/file.rs`, `src/codegen/file.rs`, Phase 8 line 87)

A `File` handle type delivered as slices F1–F6 (the slice plan mirrors `Map[K,V]`):

- **Interpreter MVP** (F1), a **File handle ABI shim** in the runtime (F2), **type lowering
  + extern declarations** (F3), **method codegen + a `KaracIoResult` unpacker** (F4), and a
  **`FreeFileHandle` scope-exit close** (F4b — the codegen counterpart of close-on-drop).
- **`?`-propagation E2E** tests (F5) and full **File E2E coverage** (F6); a **`BufReader`**
  was lifted into a tracker entry alongside it.

## HTTP server + client (`std.http`, `src/codegen/http.rs`)

Round 3 exposed `Request.body()` (line 11) and `Request.header(name) -> Option[String]`.
**Round 4** built out both the server and a real HTTP client:

- **Server** — handler-set **response headers** (line 14), **`Request.headers()` + `query()`**
  full-map iteration (line 13), verified **keep-alive + chunked** transfer (line 16), and
  **HTTP/2 via hyper `auto::Builder`** (ALPN **h2** + **h2c**, line 145). Manual dispatch was
  ratified as the v1 routing answer (line 15). `serve_static` is gated `#[unstable]`.
- **Client** — **`Client.get` / `Client.post`** end-to-end (closes line 17), a **chained
  `RequestBuilder`**, and `Response.text()` / `.bytes()` / `.header(name)` / `.headers()`.
- **`karac new`** scaffolds a **Backend HTTP server template** (line 63). See [[cli]].
- The `http.kara` surface was frozen (line 64 pre-lock audit) ahead of v1.

## Round-5 runtime hardening (density benchmarks)

Driven by the [[examples-and-benchmarks|idle-connection-density]] campaign:

- **`TCP_NODELAY` / Nagle p50 fix** — set on accepted WS-TLS sockets and extended to **all
  WS/TLS handshake paths**, fixing a p50 latency cliff: **45.01 → 1.63 ms at 250K** (now
  field-leading).
- **Sharded event-loop reactor** (Stage B2/B3) and **SIGPIPE masking** in reactor init
  (silent server death under reconnect storm). See [[design-runtime-phases]].
- **AOT channel lowering** — `Channel[T]` / `BoundedChannel[T]` and **blocking `recv`
  park-on-empty** now work in compiled binaries. See [[design-concurrency-and-providers]].
- **`std.web.time.after`** — a host-async timer as a channel producer, for the browser
  [[wasm-targets|WASM target]].

## Round-6 additions — full-duplex splice, Windows, the Relay proxy

- **`TcpStream.try_clone`** (dup-backed fd sharing) + **`TcpStream.shutdown_write`**
  (half-close to propagate EOF across a splice) — together they enable a **full-duplex
  bidirectional splice**, the backbone of the **[[examples-and-benchmarks|Relay]]** reverse
  proxy (single-upstream → round-robin LB → duplex splice → L7 path routing → live
  `par struct` `Atomic` metrics).
- **`TcpStream.connect` parks on the reactor** instead of blocking it (`3d9382b0`).
- **Native [[windows-and-cross-platform|Windows IOCP]]** — the event loop and TLS gained a
  third backend (Linux epoll / macOS kqueue / **Windows IOCP**), flipping the **M3
  cross-platform-parity gate to DONE**. The socket fd ABI was widened i32 → i64 for it.
- **Relay benchmark** — a **wrk-based 3-language reverse-proxy benchmark** (Go / Node / Kāra),
  HTTP/1.1 keep-alive proxy, pooled Go upstream conns, and cross-host results
  (`examples/relay/bench/`).

Related: [[design-runtime-phases]], [[design-concurrency-and-providers]], [[codegen]],
[[stdlib-and-traits]], [[examples-and-benchmarks]], [[wasm-targets]],
[[windows-and-cross-platform]].
