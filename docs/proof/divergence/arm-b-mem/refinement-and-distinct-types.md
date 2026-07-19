---
type: spec
created_date: 2026-07-15T10:18:22Z
last_modified: 2026-07-15T10:18:22Z
level: M1
stability: evolving
tags: types, refinement, distinct-types, verification, phase-9
---

# Refinement and Distinct Types

## Identity
Kāra's Phase-9 type-refinement features: refinement types (a base type narrowed by a predicate) and distinct types (`distinct type T = B`, an opaque newtype over a base), with compile-time predicate validation, runtime predicate enforcement, and base-layout codegen.

## Purpose
To make illegal values unrepresentable at the type level — carrying a validated invariant in the type itself — without runtime layout cost beyond the base type.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[type-checker]]
- **REFERENCES**: [[design-by-contract-enforcement]]
- **REFERENCES**: [[algebraic-data-types-and-pattern-matching]]

## Realization

- Refinement: src/typechecker/refinement_elision.rs (compile-time elision pass), src/codegen/refinement.rs (runtime predicate emission), src/typechecker/types.rs
- Distinct types: src/typechecker (representation, constructor, .raw(), no-deref), src/codegen (base layout), src/interpreter, src/typechecker/derives.rs (derive gating)
- tests/typechecker.rs, tests/codegen.rs, tests/interpreter.rs

## Specifies

- Refinement types: type representation + predicate validation; widening + method base-deref; construction surface via `try_from` and `as` cast; arithmetic on a refinement returns the base type; value-dispatch as base; runtime predicate enforcement in both interpreter and codegen; a compile-time refinement-elision pass (line 37) that drops checks proven redundant; LUB-to-base widening for refinement branch arms.
- Distinct types: `distinct type T = B` with a constructor and `.raw()` accessor, no auto-deref to the base; base memory layout; combined predicate form `distinct type T = B where P`; derive opt-in that gates `Eq`/`Ord`/`Hash`/`Display` (a distinct type does not inherit the base's derives implicitly).

## Constraints

- Refinement and distinct values share the base type's memory layout — no runtime representation cost.
- Distinct types do not auto-deref to their base; access the underlying value via `.raw()`.
- `Eq`/`Ord`/`Hash`/`Display` on a distinct type are opt-in via derive, not inherited from the base.

## Rationale

Phase 9 (verification). The compile-time / type-level half of Phase 9; the runtime-enforcement half is [[design-by-contract-enforcement]]. Both make illegal states unrepresentable, complementing enum exhaustiveness in [[algebraic-data-types-and-pattern-matching]].
