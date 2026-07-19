---
type: spec
created_date: 2026-07-15T19:06:43Z
last_modified: 2026-07-15T19:06:43Z
level: M1
stability: experimental
tags: embedded, mmio, volatile, atomics, phase-11
---

# Embedded and MMIO Surface

## Identity
Kāra's bare-metal / embedded surface: memory-mapped I/O intrinsics (`volatile_read`/`volatile_write` carrying a `Hardware` effect), a safe `VolatileCell[T: Copy]` wrapper, an RAII `critical_section.acquire()` interrupt-mask guard, standalone `fence`/`compiler_fence` memory barriers, and `Atomic[T]` with explicit memory ordering.

## Purpose
To let Kāra drive hardware registers and write interrupt-safe concurrent code directly, with the volatility, ordering, and critical-section discipline embedded targets require made explicit and type-checked.

## Relationships
- **PART_OF**: [[standard-library]]
- **DEPENDS_ON**: [[effect-system]]
- **USES**: [[unsafe-and-ffi-surface]]
- **REFERENCES**: [[effect-system]]
- **REFERENCES**: [[llvm-codegen-backend]]

## Realization

- Intrinsics: runtime/stdlib/intrinsics.kara (volatile_read/write, fence/compiler_fence); MMIO stores pointee-width, not i64
- Safe wrappers: runtime/stdlib/volatile_cell.kara (VolatileCell[T: Copy]), critical_section.kara (CriticalSectionGuard RAII)
- Atomics + ordering: runtime/stdlib/atomic.kara, memory_ordering.kara
- Effect: Hardware effect verb on the MMIO intrinsics

## Specifies

- `volatile_read(ptr)` / `volatile_write(ptr, v)` MMIO intrinsics that carry a `Hardware` effect; volatile_write stores the pointee's width (not a widened i64).
- `VolatileCell[T: Copy]` — a safe stdlib wrapper over the volatile MMIO intrinsics (the `Copy` bound is a derive-builtin bound satisfiable by primitives).
- `critical_section.acquire()` — an RAII interrupt-mask guard (`CriticalSectionGuard`, drop restores the mask; audited under the drop-soundness gate).
- `fence` / `compiler_fence` — standalone memory-barrier intrinsics.
- `Atomic[T]` with memory ordering; a non-atomic inner type is rejected (E0272).

## Constraints

- MMIO through the raw intrinsics is `unsafe`; the `Hardware` effect must be declared and flows through the call graph.
- A `VolatileCell` read must observe a prior write through the same pointee-typed cell (no i64-width store/load mismatch).

## Rationale

Phase-11 targets work, built on the effect system ([[effect-system]] gains a `Hardware` effect) and the [[llvm-codegen-backend]] intrinsic lowering. The safe wrappers sit on top of the raw intrinsics so most code stays outside `unsafe`.
