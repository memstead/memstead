---
type: architecture
title: Portable SIMD — Vector[T, N]
updated_round: 10
---

# Portable SIMD — `Vector[T, N]`

**New in round 5.** Kāra shipped a **portable SIMD** surface: a fixed-lane
**`Vector[T, N]`** value type with codegen lowering to LLVM vector ops and interpreter
parity. This graduates the **v67 "SIMD strategy"** brainstorm into a real feature. Tracked
under Phase 10 lines L289–L312; see [[wasm-targets]] for the WASM SIMD-128 leg. Reporting
lives in `src/simd_report.rs` (+853) with `tests/simd_report.rs`.

## Construction and lanes

- **`Vector[T, N]`** — the core type (slice 1), with **interpreter parity** (slice 1b).
- Constructors: **`splat(x)`** (scalar broadcast), **`from_array(a)`** (fixed-array),
  **`from_slice(s)`** (runtime-length), and lane-literal ergonomics (slice 4).
- Lane ops: **`replace(i, x)`**, **`shuffle([..]) -> Vector[T, M]`**, **`reverse`**,
  **`rotate_lanes_left/right`**.

## Arithmetic, reductions, comparisons

- Element-wise arithmetic; **`dot`**, **`cross`** (for `Vector[T, 3]`).
- Reductions: **`reduce_sum`**, **`reduce_product`**, **`reduce_and/or/xor`**,
  **`reduce_min/reduce_max`** (signed + float; unsigned `reduce_min/max` via a
  **signedness side-table**).
- **Bitwise** `& | ^` (binary) and `~` (unary) on integer-lane vectors.
- **Comparison → `Mask[N]`** plus **`mask.select(a, b)`** for branchless blends.
- **`Numeric` trait** — a first-class trait unifying numeric-lane operations.
- **Element cast** — `Vector[U, N].cast_from(v)` (lane-wise conversion).
- **Masked memory** — `Vector::load_masked(slice, mask)` / `v.store_masked(slice_mut, mask)`.
- **Gather / scatter** — `Vector::gather(slice, indices)` / `v.scatter(slice_mut, indices)`.

## Scalarization guarantee and reporting

- **`#[require_simd]`** — a **scalarization guarantee**: the attribute asserts the vector op
  must lower to real SIMD (not scalarized), with a detection core enforcing it (slice 5a).
- **`--simd-report[=verbose]`** — a CLI report of which ops vectorized, with fixed
  reduction/comparison detection (slice 5b). See [[cli]].

## WASM SIMD-128

The [[wasm-targets|WASM target]] enables **`+simd128` by default** and lowers `Vector` ops
to `v128` WebAssembly SIMD (Phase 10 line 118). See [[wasm-targets]].

## `Vector` is Copy in ownership analysis (round 10)

Bug **B-2026-07-11-1** (fixed, `4346b48b`): **`Vector[T, N]` is now correctly classified
`Copy`** in the ownership analysis. A bare rebind `let e1 = ux` no longer fires a spurious
use-after-move. This unblocked the Slipstream SIMD collide kernel. See [[bug-tracker]].

## `std.simd.math` — SIMD numerical surface (new in round 10)

A new numerical stdlib surface of SIMD-friendly kernels on `Vector[T, N]`
(cross-referenced from [[numerical-stdlib-and-tensors]]):

- **SIMD-friendly transcendentals** on `Vector` floats.
- **Vector rounding** — floor / ceil / round / trunc.
- **Vector bit-reinterpretation** — `to_bits` / `bits_as_f*`.
- **Element-wise integer-vector shift** — `<<` / `>>`.

Related: [[codegen]], [[gpu-compute]], [[wasm-targets]],
[[numerical-stdlib-and-tensors]], [[attributes]], [[stdlib-and-traits]], [[bug-tracker]].
