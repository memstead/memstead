---
type: spec
created_date: 2026-07-15T10:52:37Z
last_modified: 2026-07-15T19:07:53Z
level: M1
stability: experimental
tags: simd, vectors, codegen, phase-10
---

# Portable SIMD

## Identity
Kāra's portable SIMD surface: a fixed-width `Vector[T, N]` type with lane construction, arithmetic, reductions, comparisons-to-`Mask[N]`, bitwise ops, gather/scatter, masked load/store, and lane permutations — lowered to native vector instructions with an interpreter-parity fallback.

## Purpose
To expose data-parallel vector operations portably, so numeric and lexer-style hot loops vectorize explicitly without target-specific intrinsics.

## Relationships
- **PART_OF**: [[llvm-codegen-backend]]
- **DEPENDS_ON**: [[const-generics]]
- **REFERENCES**: [[llvm-codegen-backend]]
- **REFERENCES**: [[const-generics]]
- **REFERENCES**: [[wasm-target-backend]]

## Realization

- Codegen + detection: src/simd_report.rs; SIMD lowering threaded through codegen
- Interpreter parity: Vector[T,N] eval; tests/simd_report.rs
- Feature reporting: `--simd-report=verbose`

## Specifies

- Construction: `splat(x)`, `from_array(a)`, `from_slice(s)` (runtime-length); interpreter-parity slice.
- Arithmetic + reductions: `dot` + `reduce_sum`, `reduce_product/and/or/xor`, `reduce_min/reduce_max` (signed/unsigned + float), `Vector[T,3]` cross product.
- Comparisons: vector comparison → `Mask[N]` + `mask.select(a, b)`.
- Bitwise `& | ^` (binary) + `~` (unary) on integer-lane vectors.
- Memory ops: `gather`/`scatter`, masked `load_masked`/`store_masked`.
- Lane ops: `shuffle` → `Vector[T, M]`, `replace(i, x)`, `reverse` / `rotate_lanes_left/right`, element `cast_from`.
- Ergonomics: first-class `Numeric` trait + lane-literal ergonomics; `#[require_simd]` scalarization guarantee + detection core; `--simd-report=verbose` reduction/comparison detection.


- **std.simd.math** (this round): SIMD-friendly numerical kernels over `Vector` floats — element-wise transcendentals (the trig/exp/log family), vector rounding (`floor`/`ceil`/`round`/`trunc`), element-wise integer-vector shifts (`<<` / `>>`), and vector bit-reinterpretation (`to_bits` / `bits_as_f*`).

## Constraints

- Vector results must match between the native lowering and the interpreter fallback.
- `#[require_simd]` guarantees a function scalarizes only where the target cannot vectorize — a scalarization it must report.

## Rationale

A codegen feature over the [[llvm-codegen-backend]]; `N` is a [[const-generics]] parameter. WASM SIMD-128 lowering (see [[wasm-target-backend]]) shares the v128 Vector ops. Ships as slices 1–6.
