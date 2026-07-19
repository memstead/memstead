---
type: spec
created_date: 2026-07-15T07:27:40Z
last_modified: 2026-07-15T18:15:26Z
level: M1
stability: evolving
tags: compiler, typechecker, phase-3
---

# Type Checker

## Identity
The Kāra type checker: a bidirectional inference-and-checking pass over the resolved AST that validates types, method resolution, generic bounds, derives, closure captures, and const generics.

## Purpose
To guarantee type soundness and resolve method/trait dispatch before effect checking and codegen, with agent-friendly diagnostics on failure.

## Relationships
- **REFERENCES**: [[standard-library]]
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[name-resolver]]
- **REFERENCES**: [[advanced-trait-features-impl-trait-and-gats]]
- **REFERENCES**: [[gpu-compute-shaders]]

## Realization

- src/typechecker.rs and src/typechecker/ (env.rs, env_build.rs, inference.rs, exprs.rs, items.rs, patterns.rs, closures.rs, bounds.rs, derives.rs, lowering.rs, const_eval.rs, fields.rs, expr_call.rs, expr_method_call.rs, expr_ops.rs, stdlib_*.rs)
- tests/typechecker.rs

## Specifies

- Bidirectional subsumption with function-type variance; fresh-metavar instantiation; unsolved-type-param diagnosis at synthesis-mode `let`.
- Method resolution: inherent priority, autoref, receiver-form and UFCS dispatch, conditional-impl filtering, ambiguity detection, `Self`-receiver dispatch, E0236 with typo suggestions, NoMethodFound.
- Const generics (IntSize::I128, array-size refactor, inference solver, where-clause discharge, trait-bounds-at-codegen).
- Owned-to-ref coercion at type-compat boundaries; derive validation; generic bound/where-clause validation.
- Baked stdlib as source of truth for Option/Result/Vec (CR-202).

- Advanced trait/type features — `impl Trait` (RPIT/RPITIT/TAIT) and generic associated types (GATs) — are specified in [[advanced-trait-features-impl-trait-and-gats]].


- GPU front-end gates for [[gpu-compute-shaders]]: the `GpuSafe` structural type-check on `#[gpu]` signatures and local bindings (gpu_safe.rs), and the `#[gpu]` call-graph checks — no recursion, no calls to generic non-`#[gpu]` functions, no host-capturing closures (gpu_call_graph.rs).
- `Map.new()` / `SortedMap.new()` back-propagate `K`/`V` from the first insert/get so an un-annotated map binding or field type-checks (was a spurious mismatch; B-2026-06-22-1), and `Map.new()` / `Set.new()` are admitted as module-binding const-init special forms.
- `Column[T]` nullable-column typing with SQL three-valued-logic (3VL) arithmetic/comparison, plus `OnceCell[T]` single-task structural enforcement (write-once).
- Uppercase-receiver field access on value bindings; scalar transcendental + rounding math method typing on floats.

- Trait-system dispatch machinery: user trait impls on primitive scalar types; trait default-method inheritance (spliced into implementing impls, including generic-trait defaults with trait-arg substitution); method dispatch through generic type-param bounds; a parameterized bound's trait args checked against the matched impl; generic-struct field/method monomorphization by receiver instantiation; arithmetic admitted on an operator-trait-bounded type param; and user trait impls over the builtin `Column[T]` / `Tensor[T, S]` containers — the machinery behind the stdlib `Reduce` / `ElementwiseMap` / `ElementwiseOrd` surface (see [[standard-library]]).

## Constraints

- A program with unresolved method dispatch, unsatisfied bounds, or unsolved type params is rejected.
- Trait bounds are additionally verified at monomorphization request time.

## Rationale

Phase 3 semantic analysis. Method-resolution precision is load-bearing for the [[standard-library]] surface.
