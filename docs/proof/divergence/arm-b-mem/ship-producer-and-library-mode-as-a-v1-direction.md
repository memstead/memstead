---
type: decision
created_date: 2026-07-15T18:45:21Z
last_modified: 2026-07-15T18:45:21Z
status: accepted
decided_on: 2026-07-08
deciders: kara-maintainers
scope: system
tags: interop, ffi, producer, library, v1, positioning
---

# Ship Producer and Library Mode as a v1 Direction

## Decision
We chose to make additive interop — Kāra compiled as a library and linked into an existing C or Rust host (the "producer" direction) — a v1 direction, not a post-v1 nicety. Kāra ships as an addable component, not only as a whole-program rewrite target.

## Context
Kāra already had a consumer FFI surface (calling out to C via `extern`, raw pointers, opaque types). The additive-interop spike reframed adoption: a team can add a Kāra module to a Rust/C codebase and call INTO it, rather than committing to a rewrite. Lowering the adoption barrier this way was judged strategically worth the v1 ABI cost.

## Consequences
- Requires a stable, per-target export ABI: `pub extern "C"` exports, `#[repr(C)]` struct-by-value calling conventions (AArch64 AAPCS, x86-64 SysV, Windows x64), and honest cross-boundary ownership handoff (`forget[T]`, auto-boxing/auto-destructors, owning `CString`/`NulError`).
- Producer-mode build artifacts: static/dynamic libraries via a `[lib]` manifest table, Windows library artifacts, and a smoothed Rust-host static-link std collision.
- A new outward-facing surface to keep stable across versions (contrasts with the inward consumer-FFI surface).
- Deferrals: the Windows native-execution CI leg is parked on an upstream `llvm-config.exe` gap (the ABI is still gated by a Linux forced-arch signature-match test).

## Options

- Ship producer/library mode as a v1 direction — chosen (owner decision).
- Keep Kāra a whole-program (consumer-only) language until post-v1 — rejected: raises the adoption barrier to an all-or-nothing rewrite.

## Notes


