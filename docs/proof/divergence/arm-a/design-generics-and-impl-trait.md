---
type: design-decision
title: Generics — const generics, GATs, and impl Trait
updated_round: 6
---

# Generics — const generics, GATs, and `impl Trait`

Round 2 completed several major generics features that had been in progress. They build on
the [[compiler-pipeline|type checker and resolver]] and feed [[codegen|monomorphization]].

## Const generics

**Const generics** are now **complete** (roadmap marked done this round), alongside
`IntSize::I128`. Const-generic parameters flow through the type checker's `const_eval`
helpers and are monomorphized in codegen.

## Generic associated types (GATs)

**GATs** landed as slices 1–9:

- Generic params on associated types (slice 1); resolver scope for the assoc-type
  RHS/bound/where (slice 3); **`AssocProjection.args`** plumbing (slice 4).
- Projection resolution + parameter substitution (slice 5); a coherence regression pin for
  **duplicate GAT impls** (slice 6).
- **Impl-site bound enforcement** (slice 7), **where-clause projection bound discharge**
  (slice 8a), `types_compatible` tightening + implicit-trigger walker (8b/8c), and
  negative-space test coverage (slice 9).

## `impl Trait`

**`impl Trait`** shipped as 6 v1 sub-slices (the epic was split from one line into six,
with a Phase 8 effect-check follow-up deferred — see [[deferred-work]]):

- Slice 1 — parser + AST, with **nested-position** and **trait-method-arg-position**
  rejections.
- Slice 2 — **argument-position `impl Trait` (APIT)** desugars to an anonymous generic param.
- Slice 3 — typechecker semantics for **return-position (RPIT)** and **RPITIT**
  (return-position `impl Trait` in trait methods).
- Slice 4 — a **capture-set checker** wired into the borrow checker.
- Slice 5 — **RPITIT blocks `dyn Trait`** (a trait using RPITIT is not object-safe).
- Slice 6 — **TAIT** (type-alias `impl Trait`) declaration, shipped as a **v1 stub** (see
  [[deferred-work]]).

## `Option[shared T]` chains

Related codegen work made **`Option[shared T]`** (an `Option` wrapping a reference-counted
value) work end to end: parameter tracking + call-site ref-share, refcount-aware
field-store, chained field-access on a call return, and cleanup with recursive drop for
shared chains. A regression pins an `Option[shared T]` chain through a helper fn. See
[[design-ownership]].

## Round-5 additions

- **`Dim` / `Shape` generic-parameter kinds** — a new **kind** of generic parameter (beyond
  type and const) that carries **tensor shapes** at the type level, with **shape
  unification** and an **`E_SHAPE`** mismatch diagnostic, **`...S` variadic shape params** +
  **`Dim` bounds**, and a **shape-literal grammar** (`[3, 4, ?]`, `[...S, M]`). This powers
  the [[numerical-stdlib-and-tensors|`Tensor[T, Shape]`]] type. (Phase 11 Q1/Q2.)
- **Generic type-alias substitution** — a generic type-alias's args are substituted with
  **use-site bound enforcement**.
- **Variance declarations** — **per-stdlib-type `+T` / `-T` / `=T`** markers with a verifier
  and use-site rule (`src/typechecker/variance.rs`). See [[design-ownership]].
- **`Option[shared T]` niche call ABI** — the niche ABI for `Option[shared T]` extends to
  impl methods (plus soundness fixes its convergence tests surfaced). See [[codegen]].

## Round-6 additions

- **`Type` as a first-class comptime value + reflection** — the new
  **[[metaprogramming|comptime]]** layer makes `Type` a first-class value that comptime code
  can reflect over (fields, variants, structure) and build/emit code from. This is the
  substrate `#[derive]` and [[protobuf]]'s `#[derive(Message)]` run on.
- **`karac query specialization`** (P1.2) — a **generic-specialization / monomorphization
  fan-out** query (`src/specialization_queries.rs`) surfacing where a generic instantiates
  heavily. See [[design-ai-first-compiler]], [[cli]].

Related: [[compiler-pipeline]], [[codegen]], [[design-ownership]], [[stdlib-and-traits]],
[[numerical-stdlib-and-tensors]], [[metaprogramming]].
