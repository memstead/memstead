---
type: architecture
title: Columnar data — Column[T], DataFrame, Stats
updated_round: 9
---

# `Column[T]` — nullable columnar data

**New in round 7.** Kāra grew a **`Column[T]`** stdlib type: a **nullable, Arrow-buffer-backed
column** with **SQL three-valued-logic (3VL)** arithmetic and comparison. It is the columnar
data-engineering primitive — the building block a dataframe / ETL surface would rest on.
(The **data-engineering pipeline demo itself stays demoted to post-launch**, per round 5 —
see [[examples-and-benchmarks]] — but the primitive is being built ahead of it.) Delivered as
`runtime/stdlib/column.kara` (+147), interpreter `src/interpreter/method_call_column.rs`
(+263), and a large codegen `src/codegen/column.rs` (+2079).

## What it is

A `Column[T]` is Arrow-shaped: a **validity bitmap** (which slots are non-null) over a
**data buffer** of `T`. Nulls are first-class — every element is logically `T` *or null*,
distinct from `Option[T]` in that the whole column shares one packed validity mask.

## SQL three-valued logic (3VL)

Arithmetic and comparison over `Column[T]` follow **SQL 3VL**: an operation touching a null
input yields **null** (not zero, not a trap), and comparisons produce a three-valued result
(true / false / null) rather than a plain `bool`. This is the semantic core — it makes
`Column` behave like a SQL column, not a `Vec` of `Option`. Typecheck + interpreter landed
first (`04dedfc3`, slice 3), then codegen (`fcf3f790`, SQL 3VL arithmetic/comparison
lowering).

## Method surface

- **`from_iter_nullable`** — build a column from an iterator of nullable values.
- **`fillna(v)`** — replace nulls with a fill value. A **`treat_nan_as_null` flag**
  (`3d1cf2bb`, on **both** interpreter and codegen surfaces) makes `fillna` also treat a
  floating-point **NaN** as a null to be filled — so a NaN payload doesn't silently survive.
- **`dropna`** — drop the null slots.
- **`iter`** — iterate all slots (nullable).
- **`iter_valid`** — iterate only the valid (non-null) slots.

Both iterators lower to **`Vec`-returning** forms in codegen (`928aa5f6`).

## Build order (slices)

1. **Interpreter MVP** (`093e4fda`) — the nullable Arrow column, interpreter-only.
2. **Slice 2** (`53ada752`) — iterators + null-handling transforms (interpreter).
3. **Slice 3** (`04dedfc3`) — SQL 3VL arithmetic (typecheck + interpreter).
4. **Codegen core** (`42323571`) — Arrow-buffer lowering.
5. **Codegen** (`5c422c34`) — `fillna` / `dropna` / `from_iter_nullable`.
6. **Codegen** (`928aa5f6`) — `iter` / `iter_valid` Vec-returning.
7. **Codegen** (`fcf3f790`) — SQL 3VL arithmetic + comparison.
8. **`treat_nan_as_null`** (`3d1cf2bb`) — the fillna NaN flag on both surfaces.

So `Column[T]` is now wired through **typecheck + interpreter + codegen** (a full A/B backend
pair), unlike the round-7 stdlib primitives that landed interpreter-only (see
[[stdlib-and-traits]]). It sits in the **Phase 11 stdlib longtail** as a numerical/data
surface, a sibling of the [[numerical-stdlib-and-tensors|`Tensor[T, Shape]`]] type but for
*nullable tabular* rather than *dense shaped* data.

## DataFrame — new in round 8

Phase 11 grew a **`DataFrame`**: a set of named `Column`s, with **value-copy column
semantics** (`44e8b5fc` — DataFrame columns are copied by value, not aliased). It lands as
interpreter `src/interpreter/method_call_dataframe.rs`, codegen `src/codegen/dataframe.rs`
(+1402), and stdlib `runtime/stdlib/dataframe.kara`. Slices:

- **Interpreter MVP** (`a4456602`, slice 1); **`DataFrame.select(cols) -> DataFrame`**
  (`2f769693`, slice 2).
- **Codegen core** — new / insert / column / accessors (`11b8a323`, slice 2b); guard +
  `column_names` + `select` (`481d6bb6`, slice 2c).
- **DataFrame–`String` integration** codegen (`a6187eff`, slice 4).
- **`DataFrame.describe()`** — interpreter MVP (`75de0b16`, stats slice 4), then Arrow-buffer
  codegen (`53dfa871`, slice 5).

Bug **B-2026-06-29-1** (fixed, `5679f78b`): `DataFrame.select` leaked its fresh `Vec[String]`
argument — a 48-byte leak, Linux-LSan-only — because `select` is dispatched *before* the
generic owned-temp arg loop. See [[bug-tracker]].

## Column statistics and reductions (round 8)

`Column` gained scalar statistics: interpreter MVP (`87475f85`) + Arrow-buffer codegen
(`6622917a`); **median / quantile** Arrow codegen (`e47408ee`); `Column[String]` codegen
heap-element lifecycle (`0ff2f67c`, slice 9). New methods arrive via the **Reduce /
ElementwiseMap** traits (see [[stdlib-and-traits]]):

- **`prod()`** (`6318f3c5`), **`fold[A](init, |acc, x| ...)`** general left-fold (`213c9c`).
- **`map` / `zip_with`** (`5c145eed` / `3ddd60ed`).
- **`sorted() -> Vec[T]` / `argsort() -> Vec[i64]`** on every numeric width under build
  (`bb3285ab` / `a1375fb4`).
- **`argmin / argmax -> Option[i64]`** (`92138782`).
- Builtin `Column` inherits **`Reduce.range` = max − min** (`82691ca8`).

## `Stats.*` free-function statistics (round 8)

**`Stats.*`** — `sum / mean / variance / min / max / percentile / argmin / argmax / sort /
argsort` over a `Slice`: interpreter MVP (`a30b5c7c` / `9019e2f3`) + Arrow/Vec codegen, with
percentile / argmin / argmax / sort / argsort codegen (`ce2b3b25`). Two important fixes:

- **B-2026-07-01-9** (fixed, `327a11e0`): `Stats.*` over an **integer** slice silently
  **bit-reinterpreted the i64 buffer as f64** (`Stats.sum(vec![3,1,2])` printed denormal
  garbage under build, `6` under run). Fixed by element-typed rules — i64 lowers to **checked
  folds that trap on overflow**; other narrow numeric elements are now a hard typecheck error
  (blocked on the interpreter width-laxity class **B-2026-07-01-3**). Element kind is recorded
  in a `stats_elem_types` side-table. This was the "S5" non-f64-element axis of the
  reduce-kernel spike.
- **B-2026-07-01-10** (fixed, `1fd799b7`): `Stats.*` args **moved** the slice (a bare `Slice`
  param is consume-mode), so two stats over one dataset failed `karac check`. Fixed by
  declaring the `stats.kara` params **`ref Slice[f64]`** — the root enabler was that
  `collect_callee_param_modes` never walked baked-stdlib signatures, so the whole baked
  surface was consume-default; both collectors now walk `STDLIB_PROGRAMS`.

## Shared reduce kernel

All of `Column` / `Tensor` / `Stats` reductions now share a codegen **reduce kernel**
(`src/reduce_kernel.rs`, `src/codegen/kernel.rs`, `src/codegen/reduce.rs`) — a shared reduce
vocabulary plus fold / min-max / variance-mean / element-wise-map / argmin-argmax /
sort-scratch emitters (spike slices S0–S5,
`docs/spikes/reduce-elementwise-trait-unification.md`). Two neg miscompiles were fixed here:
**B-2026-07-01-1** (`Tensor -t` float neg now emits a true `fneg`, not `0.0 - x` —
signed-zero) and **B-2026-07-01-2** (`Column -c` int neg must be a checked `0 - x` so
`i64::MIN` traps).

**Round 9 — `u64` element reductions/sort.** The reduce kernel's sort keyed integers as signed
`i64`, mis-ordering `u64 ≥ 2⁶³`. Once the [[bug-tracker|interpreter `u64` model]] landed
(`B-2026-07-04-8`, `45eb926`), an `unsigned` flag was threaded through the sort-scratch keys so
`Column[u64]` / `Tensor[u64]` `sorted`/`argsort` order by unsigned magnitude at run==build
parity (`B-2026-07-07-2`, `7e5ef5f`) — the last `u64`-sort gap on both surfaces.

Related: [[stdlib-and-traits]], [[codegen]], [[numerical-stdlib-and-tensors]],
[[examples-and-benchmarks]], [[implementation-phases]], [[bug-tracker]].
