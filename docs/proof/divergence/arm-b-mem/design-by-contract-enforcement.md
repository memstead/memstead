---
type: spec
created_date: 2026-07-15T10:18:06Z
last_modified: 2026-07-15T10:18:06Z
level: M1
stability: evolving
tags: verification, contracts, phase-9
---

# Design-by-Contract Enforcement

## Identity
Kāra's design-by-contract layer (Phase 9): runtime-enforced `requires`/`ensures` preconditions and postconditions with `old(expr)` snapshots, plus struct and impl invariants checked at public method boundaries — emitted in both the interpreter and AOT binaries, and stripped in release builds.

## Purpose
To let functions, methods, and types declare verifiable behavioral guarantees the compiler enforces at runtime, turning invariant violations into typed, located faults instead of silent corruption.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[effect-checker]]
- **DEPENDS_ON**: [[type-checker]]
- **REFERENCES**: [[refinement-and-distinct-types]]
- **REFERENCES**: [[effect-checker]]
- **REFERENCES**: [[llvm-codegen-backend]]

## Realization

- Contracts codegen: src/codegen/contracts.rs (requires/ensures/old(), struct+impl invariants at method exits, constructor invariants, release-strip)
- Type/effect checking: src/typechecker (consumed-self check in ensures), src/effectchecker.rs (contract purity — effect set ⊆ {panics})
- Interpreter: src/interpreter (requires/ensures + constructor-invariant enforcement)
- CLI fault reporting: typed contract-fault category in `test_fail` JSONL (src/cli.rs)
- tests/codegen.rs, tests/interpreter.rs, tests/typechecker.rs, tests/effectchecker.rs

## Specifies

- `requires` preconditions and `ensures` postconditions with runtime enforcement; `old(expr)` snapshots the pre-call value for use in `ensures`.
- Method `requires`/`ensures`; a consumed-self check forbids referencing a moved-out `self` in `ensures`.
- Struct invariants checked at every `pub` method exit; impl invariants apply an invariant across all methods of an impl block (all-method scope).
- Constructor invariants: `pub` associated fn returning `Self` re-checks invariants on construction, for both owned and shared/`par` structs.
- Contract purity: predicate expressions must be pure — inferred effect set ⊆ {panics} — enforced by the [[effect-checker]].
- AOT emission of `requires`/`ensures`/`old()`/invariants in native binaries (via [[llvm-codegen-backend]]).
- Two distinct fault categories: contract-violated (predicate returned false) vs contract-predicate-panicked (predicate itself panicked), each categorized across cross-call boundaries and surfaced in the JSONL test output.

## Constraints

- Contract predicates must be pure (effect set ⊆ {panics}).
- `--release` / `karac build --release` strips all contract checks (requires/ensures/invariants) and the `?`-error-return-trace; contracts are a debug-time guarantee.

## Rationale

Phase 9 (verification). Contracts are the runtime-enforcement half of Phase 9; the compile-time half is [[refinement-and-distinct-types]].
