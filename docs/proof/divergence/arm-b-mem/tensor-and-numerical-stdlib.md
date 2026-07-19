---
type: spec
created_date: 2026-07-15T10:52:19Z
last_modified: 2026-07-15T19:07:44Z
level: M1
stability: experimental
tags: tensor, numerical, stdlib, phase-11, shape-types
---

# Tensor and Numerical Stdlib

## Identity
Kāra's Phase-11 numerical stdlib: a shape-typed `Tensor[T, Shape]` with compile-time Dim/Shape generic-parameter kinds, plus element-wise arithmetic, broadcasting, reductions, axis iteration, and shape transforms — in both the interpreter and the native backend.

## Purpose
To give Kāra first-class multi-dimensional numeric arrays whose shapes are checked at compile time, delivering the 'data' bonus of the backend-first positioning without runtime shape errors.

## Relationships
- **PART_OF**: [[standard-library]]
- **DEPENDS_ON**: [[const-generics]]
- **REFERENCES**: [[const-generics]]
- **REFERENCES**: [[standard-library]]
- **REFERENCES**: [[portable-simd]]

## Realization

- Types: src/typechecker/expr_method_tensor.rs, Dim/Shape kinds + shape unification (src/typecheck E_SHAPE), variadic `...S` shape params, shape-literal grammar (src/parser/generics.rs)
- Codegen: src/codegen/tensor.rs; interp: src/interpreter/method_call_tensor.rs
- Stdlib: runtime/stdlib/tensor.kara
- Tracker: docs/implementation_checklist/phase-11-stdlib-longtail.md

## Specifies

- `Tensor[T, Shape]` with layout / constructors / indexing / drop; `Tensor.from` literal constructor; typed `Tensor.zeros`/`ones` fill from a `let` annotation.
- Dim/Shape generic-parameter kinds with shape unification and an `E_SHAPE` diagnostic (Q1); `...S` variadic shape params + Dim bound parsing; shape-literal grammar `[3, 4, ?]` / `[...S, M]` in generic-arg position (Q2).
- Cross-argument `?`-dim call-boundary asserts; shape-generic function bodies indexing tensor params.
- Element-wise arithmetic + Neg + scalar broadcast; explicit `broadcast_add/sub/mul/div`.
- Reductions `sum/mean/prod/min/max` + `sum_axis/mean_axis`; `iter_axis(n)` axis iteration; `Vec[Tensor]` element drop.
- Shape-transform family: `reshape/permute/slice/squeeze`.

- Narrow-width element storage: `Tensor.from` / `Tensor.full` store elements at the declared element width (i8/i16/i32/u*/f32) instead of a wide 8-byte slot, fixing a store/read stride mismatch; int-literal→float promotion and f32 `mean` handled.
- Reduce surface: `Tensor.sorted` / `argsort` (every numeric width via a widened scratch sort), `fold` / `map` / `zip_with`, `argmin` / `argmax`, and `prod`; Tensor implements the stdlib `Reduce` / `ElementwiseMap` traits for bound-generic dispatch (see [[standard-library]]).


- **std.embeddings** (runtime/stdlib/embeddings.kara): a vector-similarity surface — 1-D `dot` / `l2_norm` / `cosine_similarity` / `l2_normalize`, batched single-query forms (`dot_batched`, `cosine_similarity_batched`), a Q×N matrix form, and `top_k`. Dogfooded by examples/semantic_search.kara (a std.embeddings end-to-end demo). Row-wise batched forms are unblocked by the `iter_axis` row-view deep-clone fix (B-2026-07-13-7) and by accepting a `ref Tensor` argument to `zip_with` (B-2026-07-13-5 gap C).
- **f16 / bf16** reduced-precision floats: added as primitive numeric type-name identifiers (they lex as identifiers like f32/f64, matching the Rust seed — a lexer-reservation reversal, B-2026-07-14-2). Tensor[f16] and f16/bf16 wrapper support are scoped as follow-ups.
- SIMD-friendly numerical kernels live in the [[portable-simd]] `std.simd.math` surface.

## Constraints

- Shapes must unify at call boundaries or the program is rejected (E_SHAPE); `?` dims assert at the boundary.
- Tensor semantics must match between the interpreter and the native backend.

## Rationale

Phase 11 (numerical stdlib long-tail). Shapes are carried as generic-parameter kinds, extending the [[const-generics]] machinery with Dim/Shape kinds and shape unification (E_SHAPE). Part of the [[standard-library]].
