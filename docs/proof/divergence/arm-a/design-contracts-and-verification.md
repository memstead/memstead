---
type: design-decision
title: Contracts, refinement types, and distinct types (Phase 9 verification)
updated_round: 4
---

# Contracts, refinement types, and distinct types

**New in round 4.** Phase 9 (Verification) went from a thin "fuzz targets + sanitizer
tests" line to a major language surface: **design-by-contract** (`requires`/`ensures`/
`old()`/invariants), **refinement types**, and **distinct types**. All three add
compile-time and runtime correctness guarantees, and all three are **stripped in
`--release`** so they cost nothing in production. See [[implementation-phases]] (Phase 9)
and [[codegen]] for the lowering.

## Contracts — `requires` / `ensures` / `old()` / invariants

Design-by-contract landed as **Phase 9 "Contracts"** (steps 1–8, closed with
`b1fa41ce`):

- **`requires` / `ensures`** — function pre/post-conditions with **runtime enforcement**
  (contracts core, `d4a8dbff`). `requires` preconditions and `ensures` postconditions are
  emitted in **AOT binaries** (not just the interpreter), plus method-level
  `requires`/`ensures`.
- **`old(expr)`** — an `ensures` clause can refer to the pre-call value of an expression
  via `old()`; emitted in AOT binaries.
- **Struct / impl invariants** — struct invariants are checked at **pub method exits**
  (`e623701c`) and at **method exits** generally (`44efa3b2`); an **impl invariant** applies
  to **all methods** in scope (`7f5dc511`, step 5b). Invariants are also checked at
  **constructor** boundaries — for **owned structs** (`61e3ac54`), **shared structs**
  (`5782bda6`), and in the interpreter for any **pub assoc fn returning `Self`**
  (`74a57335`).
- **Consumed-self check in `ensures`** (`0fb1a3a6`, step 4) — the typechecker rejects an
  `ensures` that refers to a consumed `self`.
- **Contract purity** — the effect checker enforces that a contract predicate's **effect
  set ⊆ `{panics}`** (`cc0d5552`, step 2): predicates must be pure (they may only panic).

### Contract-fault categorization

Contract failures are given **typed fault categories** so an agent can tell *why* a
contract failed, surfaced in the `test_fail` JSONL:

- **contract-violated** vs **contract-predicate-panicked** are distinguished
  (`b028d093`, `f3676d66`) — a predicate that returns false is not the same as a predicate
  that itself panicked.
- **Cross-call contract-predicate panics** are categorized (`8183f6c7`).
- A **typed contract-fault category in the `test_fail` JSONL** (`4c8ebd79`, step 7), so
  the [[design-ai-first-compiler|agent surface]] sees a structured fault class. The JIT
  test runner preserves the category out of runner stdout (`a68e72b2`).

## Refinement types

**Refinement types** (Phase 9 line 37/line 5) — a base type narrowed by a predicate —
landed as steps 1–5c:

- **Type representation + predicate validation** (`d45d79a9`, step 1).
- **Widening + method base-deref** (`3c6f7eab`, step 2) — a refinement widens to its base
  type; methods deref through to the base.
- **Construction surface** (`ee8634f5`, step 3) — `try_from` (checked) and `as` cast
  routes into a refinement.
- **Arithmetic returns base** (`781caf6d`, step 4) — arithmetic on a refinement yields the
  base type (the predicate is not preserved through arithmetic); codegen uses the
  **base layout**.
- **Value-dispatch as base** (`4c25edb7`, step 5a) — a refinement value dispatches methods
  as its base.
- **Runtime predicate enforcement** — in the interpreter (`39ab9815`, step 5b) and via
  **codegen predicate emission** (`a706a5b1`, step 5c).
- **LUB-to-base widening for refinement branch arms** (`13695913`) — `if`/`match` arms of
  differing refinements widen to the common base.
- **Compile-time elision pass** (`ad095c0e`, line 37, `src/typechecker/refinement_elision.rs`)
  — a pass that **elides runtime predicate checks** the compiler can prove hold, so a
  refinement is zero-cost where statically discharged.

## Distinct types

**Distinct types** (`distinct type T = B`) — a nominal newtype over a base `B` that does
**not** implicitly deref to the base:

- **Constructor, `.raw()`, no-deref** across typechecker (`ebf10630`), interpreter
  (`6736dc14`), and codegen (`ffbca3cb`) — construct a distinct value, extract the base via
  **`.raw()`**, and the type does not implicitly coerce to its base layout.
- **Combined distinct-type predicates** (`d8f96811`) — `distinct type T = B where P`
  fuses the newtype with a refinement predicate.
- **Derive opt-in gates** (`31e0670d`) — a distinct type only gets `Eq`/`Ord`/`Hash`/
  `Display` if it **opts in** via `derive`; it does not inherit the base's trait impls
  automatically.

## Extended Patterns (Phase 9, complete)

**Phase 9 "Extended Patterns" is complete** (`c1df00f3`):

- **`@` bindings in compiled match arms** — bound and tested in codegen (`e4c07a26`).
- **Range-pattern exhaustiveness** via **Maranget interval splitting** (`abceed62`) — the
  exhaustiveness checker now correctly handles range patterns; see
  [[design-adt-and-pattern-matching]].

## Release stripping

Contracts (and refinement predicate checks) are **development-time guarantees**: a
`karac build --release` **strips contracts** (`99a7c088`, step 8; `a53b4def`; project-mode
`e55bccb3`) and also **strips the `?`-error-return-trace** (`aa9ea316`). Production
binaries pay nothing for the verification surface.

Related: [[codegen]], [[implementation-phases]], [[design-effect-system]],
[[design-adt-and-pattern-matching]], [[design-ai-first-compiler]],
[[history-reversals-and-deprecations]].
