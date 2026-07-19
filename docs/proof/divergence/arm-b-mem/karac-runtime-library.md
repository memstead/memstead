---
type: spec
created_date: 2026-07-15T07:28:38Z
last_modified: 2026-07-15T10:24:25Z
level: M1
stability: evolving
tags: runtime, native, phase-6
---

# Karac Runtime Library

## Identity
The Kāra runtime (Phase 6): the Rust support library (`karac_runtime`) linked into every native binary, providing the worker pool, collection layouts, string/UTF-8 helpers, RC frame tracking, and the intrinsics generated code calls.

## Purpose
To supply the low-level machinery the LLVM backend emits calls into — memory, collections, concurrency, I/O, error tracing — as a separately-audited native crate.

## Relationships
- **REFERENCES**: [[phased-runtime-model]]
- **REFERENCES**: [[binary-size-reduction-strategy]]
- **PART_OF**: [[kara-compiler]]
- **USES**: [[provider-system]]
- **MOTIVATED_BY**: [[phased-runtime-model]]
- **MOTIVATED_BY**: [[binary-size-reduction-strategy]]
- **REFERENCES**: [[network-runtime-and-cooperative-scheduling]]

## Realization

- runtime/src/lib.rs, runtime/src/map.rs (KaracMap), runtime/src/clone.rs
- runtime/Cargo.toml, runtime/SYMBOL_KEEP_LIST.md
- tests/memory_sanitizer.rs (ASAN/LSan), tests/http_server.rs

- runtime/src/tracing.rs (std.tracing runtime), runtime/src/emutls.rs (__emutls_get_address for JIT thread_local), runtime/src/tls.rs (gated behind the `tls` cargo feature), runtime/src/event_loop.rs + runtime/src/scheduler.rs (network runtime — see [[network-runtime-and-cooperative-scheduling]])

## Specifies

- The long-lived worker pool for `karac_par_run`; parent-frame ref + KaracWaitTarget surface; SpawnSiteId metadata table.
- KaracMap layout with direct-field length; single-char O(1) `karac_string_decode_char`; UTF-8 decode helpers.
- Atomic-RC frame tracking (ACTIVE_FRAMES) and std.runtime introspection APIs (the debugger contract).
- `KARAC_ERROR_TRACE_FORMAT` (json/jsonl/text) atexit error-trace printer; `KARAC_AUTO_PAR`, `KARAC_HTTP_BLOCK_IN_PLACE` env gates.

- A `tls` cargo feature gates the rustls/TLS surface; TLS-aware runtime archive selection links a lean archive for compute-only programs so non-networked binaries avoid the TLS bulk.
- std.tracing runtime backing (Span / LogEvent / Exporter emission).
- JIT support: `__emutls_get_address` for JIT thread_local; the production staticlib is built without co-emitting an rlib (strips the backtrace symbolizer).

## Constraints

- Symbols the generated code needs must survive stripping (SYMBOL_KEEP_LIST).
- v1 is blocking (see [[phased-runtime-model]]); no async event loop yet.

## Rationale

Phase 6. Binary-size and worker-pool decisions ([[binary-size-reduction-strategy]], [[phased-runtime-model]]) are realized here.
