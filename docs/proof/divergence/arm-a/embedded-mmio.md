---
type: architecture
title: Embedded — memory-mapped I/O
updated_round: 10
---

# Embedded — memory-mapped I/O

**New in round 10.** Kāra shipped a bare-metal / embedded surface for driving hardware:
volatile memory-mapped register access, a stdlib cell over it, interrupt-mask guards, and
memory barriers. The intrinsics live in `runtime/stdlib/intrinsics.kara` (+114). See
[[design-unsafe-ffi-and-pointers]], [[design-effect-system]].

## Volatile MMIO intrinsics

- **`volatile_read` / `volatile_write`** — MMIO intrinsics for memory-mapped register
  access, guarded by a new **`Hardware` effect** (see [[design-effect-system]]).
- **`volatile_write` stores at the pointee width** — bug **B-2026-07-12-7** (fixed): it
  previously stored the value's full i64, corrupting a narrow register *and* the 4 bytes
  past it.
- **Interpreter behavior** — the MMIO intrinsics are **codegen-only**. Under the interpreter
  they record a clean *"unsupported in interpreter"* runtime error that names the intrinsic
  and the `karac build` workaround, rather than an internal panic (part of the
  B-2026-07-12-7 fix).

## `VolatileCell[T: Copy]`

- **`VolatileCell[T: Copy]`** — a stdlib wrapper over the volatile MMIO intrinsics
  (`runtime/stdlib/volatile_cell.kara`, +64).
- The **`T: Copy` bound is sound** — a `#[derive(Copy)]` or primitive discharges it; this
  required fixing **B-2026-07-12-19**, a Copy/derive-builtin bound on primitives.

## Critical sections and barriers

- **`critical_section.acquire()`** — an **RAII interrupt-mask guard**
  (`runtime/stdlib/critical_section.kara`, +59); its `Drop` was audited for the
  **`drop_carries_soundness`** gate.
- **`fence` / `compiler_fence`** — standalone memory-barrier intrinsics.

## Atomics

- **`Atomic[T]` + memory ordering** — complete; a non-atomic inner type is rejected with
  **`E0272`**.

Related: [[design-unsafe-ffi-and-pointers]], [[design-effect-system]], [[stdlib-and-traits]].
