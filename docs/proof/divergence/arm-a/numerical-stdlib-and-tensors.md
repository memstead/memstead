---
type: architecture
title: Numerical stdlib — Tensor[T, Shape] (Phase 11)
updated_round: 10
---

# Numerical stdlib — `Tensor[T, Shape]`

**New in round 5.** Phase 11 (the [[implementation-phases|stdlib longtail]]) gained a
**numerical stdlib** built around a shape-typed **`Tensor[T, Shape]`**. Phase 11 items were
physically reorganized into `docs/implementation_checklist/phase-11-stdlib-longtail.md`.
Codegen and typecheck for tensors are large new surfaces: `src/codegen/tensor.rs` (+3360),
`src/interpreter/method_call_tensor.rs` (+1043), `src/typechecker/expr_method_tensor.rs`
(+1006), and `runtime/stdlib/tensor.kara` (+270).

## Shape-typed generics (`Dim` / `Shape`)

Tensors are typed by their **shape at the type level** — a new **generic-parameter kind**
system distinct from ordinary type parameters (Phase 11 Q1):

- **`Dim` / `Shape` parameter kinds** — shape unification with a dedicated **`E_SHAPE`**
  mismatch diagnostic.
- **`...S` variadic shape params** + **`Dim` bound** parsing (Q1 stage 1).
- **Shape-literal grammar** — `[3, 4, ?]` and `[...S, M]` in generic-argument position
  (Q2); `?` is an inferred/dynamic dimension.
- **Cross-argument `?`-dim call-boundary asserts** — a function taking two tensors with a
  shared `?` dimension emits a runtime assert that they agree.
- **Shape-generic body tensor-param indexing** — a function generic over shape can index
  its tensor params.

## Constructors and layout

- **`Tensor[T, Shape]`** interpreter MVP, then **core lowering** (layout, constructors,
  indexing, drop).
- **`Tensor.from`** literal constructor; **`Tensor.zeros` / `ones`** filled from the `let`
  annotation's shape.
- **`Vec[Tensor]` element drop** handled.

## Operations

- **Element-wise arithmetic + `Neg` + scalar broadcast** (Slice A).
- **Explicit broadcasting methods** — `broadcast_add/sub/mul/div` (broadcast semantics made
  explicit rather than implicit).
- **Reductions** — `sum/mean/prod/min/max` plus axis reductions `sum_axis/mean_axis`
  (Slice B).
- **Shape transforms** — `reshape / permute / slice / squeeze` (a shape-transform family);
  owned fn-return / method-return receivers of shape transforms are freed correctly.
- **Axis iteration** — `iter_axis(n)`.

## Reduce / ElementwiseMap trait surface — new in round 8

`Tensor` now implements the shared **Reduce / ElementwiseMap** surface traits (spike slices
S6a / S6c), gaining reduction and element-wise methods in lockstep with `Column`
(see [[stdlib-and-traits]] and the shared reduce kernel in [[columnar-data]]):

- **`fold[A](init, |acc, x| ...)`** general left-fold, parity with `Column.fold` (`db1d636a`).
- **`map`** (`5c145eed`) and **`zip_with`** (`3ddd60ed`).
- **`argmin / argmax -> Option[i64]`** (`92138782`).
- **`sorted() -> Vec[T]` / `argsort() -> Vec[i64]`** (`a1375fb4`).
- Builtin `Tensor` inherits **`Reduce.range` = max − min** (`82691ca8`, `14ab65ae`).

Both `Column` and `Tensor` now `impl Reduce` (`85277bd3`), and users can write their own
**trait impls over `Tensor[T, S]`** (S6c-12 slice 2, `ade24684`).

## Narrow-width tensor storage fix (round 8)

Bug **B-2026-07-03-35** (fixed, `98ae6e12`, follow-on `9cce1610`): a `Tensor` with a **narrow
numeric element** (`i8`/`i16`/`i32`/`u8`/`u16`/`u32`/`f32`) built via `Tensor.from([...])`
stored elements at **8 bytes** while readers strided at the **narrow width** — silent wrong
output under `karac build`, correct under `karac run`. Fixed by **coercing each leaf to the
declared element width before store**. The follow-on also fixed `Tensor.full` fill coercion,
int→float literal promotion into a float tensor, and `Tensor.mean` over `f32` (`fdiv` width),
and lifted the narrow-width rejection on `sorted` / `argsort` — **all numeric widths sort now;
only `u64` is rejected** (blocked by **B-2026-07-04-8**, the interpreter having no `u64`
model). See [[bug-tracker]].

## `std.embeddings` — vector-embedding module (new in round 10)

Phase 11 (`stdlib-longtail`) gained **`std.embeddings`**, a numerical stdlib module for
vector embeddings over generic-dim `Tensor[f32, [D]]` (`runtime/stdlib/embeddings.kara`,
+182):

- **1-D core** — `dot`, `l2_norm` (L2), `cosine_similarity`, `l2_normalize`.
- **Batched forms** — `dot_batched`, `cosine_similarity_batched` (a single query against a
  corpus).
- **Q×N matrix form + `top_k`** — the surface is complete (`994e0134`).
- **End-to-end demo** — `examples/semantic_search.kara`, a `std.embeddings` semantic-search
  demo.

The 1-D core ships because bug **B-2026-07-13-5 gap C** (a `ref Tensor` arg to a tensor
method such as `zip_with`) was **fixed** — the typechecker now unwraps a `Ref`/`MutRef`
argument before the same-container check. Two ergonomic gaps remain **open**, each with a
clean workaround: **gap A** — a reduction on a chained/non-identifier receiver
(`a.zip_with(b, f).sum()`); bind the intermediate to a `let`. **Gap B** — a generic shape
param `D` used in a function-**body** type annotation; omit the body annotation. See
[[bug-tracker]].

## `std.simd.math` — SIMD-friendly numerical kernels (new in round 10)

New numerical kernels on `Vector[T, N]` (see [[simd]]):

- **SIMD-friendly transcendentals** on `Vector` floats.
- **Vector rounding** — floor / ceil / round / trunc.
- **Vector bit-reinterpretation** — `to_bits` / `bits_as_f*`.
- **Element-wise integer-vector shift** — `<<` / `>>`.

## Tensor updates (round 10)

- **`Tensor.iter_axis(n)` returns `Vec[Tensor]` row sub-tensor VIEWS** — a `let r = rows[i]`
  bind of a returned row now **deep-clones the tensor block** (`B-2026-07-13-7`, `0de5fc8`;
  was a double-free). This unblocks row-wise 2-D tensor workloads, including the batched
  `std.embeddings` forms.
- **`f16` / `bf16` primitive numeric types added** (`09a2fc88`) — bare `f16` / `bf16` lex as
  type-name identifiers like `f32` / `f64`. Reduced-precision arithmetic and backend are a
  Phase-7 follow-up; F16/BF16 wrappers + `Tensor[f16]` are also scoped as f16/bf16 follow-ups
  (`docs/implementation_checklist/phase-11`).

Related: [[implementation-phases]], [[codegen]], [[simd]], [[gpu-compute]],
[[stdlib-and-traits]], [[columnar-data]], [[design-generics-and-impl-trait]],
[[design-data-layout]], [[bug-tracker]].
