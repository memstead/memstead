---
type: design-decision
title: Algebraic data types and pattern matching
updated_round: 6
---

# Algebraic data types and pattern matching

Kāra has **Rust-style enums** (algebraic data types) with **exhaustive pattern
matching**.

## Patterns

Supported patterns: wildcards, bindings, literals, struct/tuple destructuring, qualified
paths, **`@` bindings**, **range patterns** (including a bounded-exclusive form),
**slice / array patterns**, and the **`..` rest pattern** (enabled in round 2 alongside
`non_exhaustive`). Refutable patterns are rejected in plain `let`.

Round 3 added **byte-literal (`b'A'`) patterns** and fixed byte-literal and range patterns
that were matching **unconditionally** (see [[bug-tracker]]). Codegen also gained **`match`
over a `String` scrutinee** (see [[codegen]]).

**Round 5** added **const-expression bounds for range patterns**, an
**`E_AT_BINDING_DOUBLE_CONSUME`** check + **`ref`-`@` bindings** (closing the `@`-binding
entry), and lowered **`while-let` / `let-else`** (after a fail-loud stopgap). Crucially,
**`match` on string literals now lowers to a switch tree** rather than a `memcmp` cascade —
the **#1 real-world codegen lever** identified by the [[self-hosting|self-hosted-lexer]]
profile. See [[codegen]].

## Extended Patterns complete (Phase 9, round 4)

**Phase 9 "Extended Patterns" is complete.** Round 4 closed the two remaining gaps in
compiled matching:

- **`@` bindings in compiled match arms** — bound and tested in codegen (previously
  interpreter-only).
- **Range-pattern exhaustiveness via Maranget interval splitting** — the exhaustiveness
  checker now correctly reasons about range patterns (it previously could not, and round 3
  had separately fixed range patterns that *matched* unconditionally). See
  [[design-contracts-and-verification]].

## Exhaustiveness

- Exhaustiveness checking uses the **Maranget** algorithm (round 4 added **interval
  splitting** for range patterns); an earlier O(N²) performance issue was fixed (see
  [[bug-tracker]]).
- Unreachable arms produce a **`W0237`** unreachable-arm warning.
- **`#[non_exhaustive]`** (new in round 2) — a struct or enum marked non-exhaustive imposes
  cross-package rules: foreign struct literals are rejected, and matching a foreign
  non-exhaustive enum **requires a `_` wildcard arm**. A machine-applicable fix-it inserts
  it. See [[attributes]].

## Enum payloads

- **Compound-payload enums** are supported end to end (typecheck, codegen "Slice CP"),
  including a drop-path for compound-payload enums ("Slice DP") and
  **compound / tuple-payload destructure** (`Some(node) => let (a, b) = node`).
- `Vec.pop` / `VecDeque.pop_*` return `Option[T]` via multi-word handling.

## Round-6 additions

- **Recursive `shared enum` — RC-heap representation.** Direct recursive shared enums
  (`shared enum Expr { Num(i64), Add(Expr, Expr) }`) are supported as reference-counted heap
  boxes (`9b2ce71b`), and the [[self-hosting|self-host parser]]'s AST was rewritten to this
  **direct shared-enum model** (`1f226847`). This recursive-heap shape drove a long run of
  [[codegen|drop/ownership]] fixes (box rc-drop walking payloads, move-out cap-zeroing, and
  the render-leak cluster — see [[bug-tracker]]).
- **Parallel / destructuring assignment** — `a, b = b, a` is a first-class statement
  (`2436ced2`, `c9fce676`): every RHS is evaluated before any target is written, so it swaps.
  Added for the parser port's `(node, pos)` recursive-descent returns. See the CHANGELOG,
  [[self-hosting]].
- **`#[default]` enum-variant marker** — selects the variant `#[derive(Default)]` uses
  (`18de9672`), lowered through the [[metaprogramming|comptime/derive]] substrate. See
  [[attributes]].
- **Bounded-refinement finite-domain exhaustiveness** — the exhaustiveness checker gained a
  **bounded-refinement finite-domain** step (`dd00e3e1`, step 8), extending Maranget reasoning
  to finite refinement domains. See [[design-contracts-and-verification]].

## Related enums in the prelude

`Option`, `Result`, `Ordering`, `Entry` (with `Occupied` / `Vacant` variants), `IoError`,
`VarError` — see [[stdlib-and-traits]]. Note the **`Ordering` split** into comparison and
memory-ordering variants (see [[history-reversals-and-deprecations]]). FFI **unions** are a
distinct low-level sum-like type with their own rules — see
[[design-unsafe-ffi-and-pointers]].

Related: [[compiler-pipeline]], [[codegen]], [[attributes]],
[[design-unsafe-ffi-and-pointers]], [[metaprogramming]], [[self-hosting]].
