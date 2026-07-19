---
type: spec
created_date: 2026-07-15T08:41:48Z
last_modified: 2026-07-15T18:43:46Z
level: M0
stability: evolving
tags: types, traits, impl-trait, gats, phase-5
---

# Advanced Trait Features impl Trait and GATs

## Identity
Kāra's advanced trait/type-system features layered on the type checker: `impl Trait` in argument and return position (RPIT), return-position `impl Trait` in trait methods (RPITIT), type-alias `impl Trait` (TAIT, v1 stub), and generic associated types (GATs).

## Purpose
To let signatures name abstract capabilities without spelling out concrete types — opaque returns, existential parameters, and type constructors as associated types — bringing Kāra's trait system closer to Rust's expressiveness.

## Relationships
- **PART_OF**: [[type-checker]]
- **DEPENDS_ON**: [[type-checker]]
- **REFERENCES**: [[type-checker]]
- **REFERENCES**: [[const-generics]]

## Realization

- Typechecker: argument-position desugar to anonymous generic params, return-position + RPITIT semantics, capture-set checker + borrow-checker wiring, GAT projection resolution + parameter substitution + impl-site bound enforcement + where-clause projection-bound discharge (src/typechecker/*.rs)
- Parser/AST: `impl Trait` positions with nested-position + trait-method-arg-position rejections; generic params on associated types
- Effect-checker integration for `impl Trait` (Phase 8)

## Specifies

- `impl Trait`: slice 1 parser/AST (+ nested-position and trait-method-arg-position rejections), slice 2 argument-position desugar to anonymous generic params, slice 3 return-position + RPITIT semantics, slice 4 capture-set checker + borrow-checker wiring, slice 5 RPITIT blocks `dyn Trait`, slice 6 TAIT declaration + v1 stub.
- GATs: generic params on associated types, AssocProjection.args plumbing, projection resolution + parameter substitution, impl-site bound enforcement, where-clause projection-bound discharge, coherence regression pin for duplicate GAT impls, `types_compatible` tightening.


- Trait-dispatch maturity (the "S6" surface-trait epics, run==build across interp/JIT/AOT): trait DEFAULT methods are inherited onto implementors (a pre-resolve desugar splices non-overridden defaults into each `impl`, generic-trait defaults substitute the impl's trait-args); user trait impls on PRIMITIVE types (`impl Tr for u8`); generic-bound trait-method dispatch monomorphized in codegen (`fn f[T: Tr](x: T) { x.m() }`, over user, container, and primitive implementors); generic impl/trait METHODS on concrete receivers; `-> Self` return-type resolution (typecheck + codegen); operator-on-bounded-`T` (`a + b` under `T: Add`, admitted at typecheck since user operator-trait impls are stdlib-forbidden); associated-type projection resolved in monomorphized signatures; and `.cmp() -> Ordering` on derived-`Ord` types.
- These land the bound-generic dispatch the stdlib Reduce/ElementwiseMap/ElementwiseOrd surface traits rely on.

## Constraints

- `impl Trait` is rejected in nested positions and (for arguments) in trait methods.
- RPITIT return types block coercion to `dyn Trait`.

## Rationale

Phase-5 type-system epics built incrementally on the [[type-checker]], parallel to [[const-generics]].
