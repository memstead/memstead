---
type: decision
created_date: 2026-07-15T19:07:14Z
last_modified: 2026-07-15T19:10:05Z
status: accepted
decided_on: 2026-07-11
deciders: kara-maintainers
scope: system
tags: panic, abort, unwind, manifest, ffi
---

# Abort-Only Panic Strategy at v1

## Decision
We chose abort as Kāra's only panic strategy at v1. The manifest `[profile]` table parses `panic = "unwind" | "abort"` with per-profile defaults, but the build entry rejects `panic = "unwind"` — v1 is abort-only — and an `extern "C-unwind"` FFI declaration is rejected under the abort strategy (E0415).

## Context
A panic strategy is a whole-program property: unwinding requires landing pads, cleanup tables, and unwind-safe FFI boundaries throughout codegen and the runtime, whereas abort terminates immediately. Supporting both from the start would double the codegen and FFI-ABI surface (including the `C-unwind` ABIs) before the language is otherwise stable. The manifest knob and the ABI reservations (see [[unsafe-and-ffi-surface]]) were put in place so the eventual unwind path has a home, without committing to it now.

## Consequences
- `karac build` refuses `panic = "unwind"`; every v1 binary aborts on panic.
- `extern "C-unwind"` is rejected under abort (E0415); the unwinding FFI ABIs (and reserved stdcall/fastcall/win64/sysv64) are reserved at v1 but not yet honored.
- The `[profile]` manifest table and per-profile defaults are wired, so enabling unwind later is a knob flip plus the codegen/runtime work, not a schema change.
- Simpler codegen and runtime for v1: no landing pads or cleanup-table maintenance on the abort path.

## Relationships
- **REFERENCES**: [[unsafe-and-ffi-surface]]
- **MOTIVATED_BY**: [[backend-first-v1-positioning]]

## Options

- Support unwinding at v1 — rejected: a large codegen + runtime + FFI-ABI surface to carry before the language stabilizes.
- Abort-only, with the manifest knob + ABIs reserved for a later unwind path — chosen.
- Hard-code abort with no manifest knob — rejected: leaves no forward-compatible home for the eventual strategy choice.

## Notes


