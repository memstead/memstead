---
type: spec
created_date: 2026-07-15T07:27:50Z
last_modified: 2026-07-15T17:51:16Z
level: M1
stability: evolving
tags: compiler, effects, phase-3
---

# Effect Checker

## Identity
The Kāra effect-checking pass (Phase 3): infers each function's effects bottom-up, verifies them against declarations with call-site subtyping, unifies `with E` handler regions, and lints extern/FFI effects.

## Purpose
To enforce effect discipline and produce the effect information [[auto-concurrency]] consumes.

## Relationships
- **REFERENCES**: [[auto-concurrency]]
- **REFERENCES**: [[effect-system]]
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[type-checker]]
- **IMPLEMENTS**: [[effect-system]]
- **REFERENCES**: [[gpu-compute-shaders]]

## Realization

- src/effectchecker.rs and src/effectchecker/ (inference.rs = Phase B inference, verify.rs, subtyping.rs = call-site subtyping, with_e.rs = `with E` unification, bounds.rs = generics/trait-bounds/mutual recursion, extern_ffi.rs = extern-fn effects + FFI linter + profile gate, target_gate.rs = per-target provided-resource sets, profile_compat.rs = [profile]-knob compatibility)
- src/ffi_lint.rs; tests/effectchecker.rs

## Specifies

- Bottom-up effect inference; declared-vs-inferred verification.
- Call-site effect subtyping; `with E` unification checks.
- Per-method allocator effects for Map/Set; extern-fn effect annotation with FFI linter and a profile gate.
- Block-level `@noblock` propagation for unsafe extern blocks.
- Target-gate pass: per-target provided-resource sets, rejecting a resource the target does not provide (E0411); dispatch call sites inherit declared clause effects on other resources.
- E0412 resource-receiver contradiction with a machine-applicable `ref self` rewrite.

- GPU effect gate for [[gpu-compute-shaders]] (gpu_effect_gate.rs): effect enforcement flowing from `#[gpu]` roots — the reachable call graph must stay within the GPU-permitted effect set — plus rejection of explicit panics in `#[gpu]` code (FE-4 / FE-4b).
- Effect surface for the `OnceLock[T]` / `OnceCell[T]` write-once lazy-init cells.

## Constraints

- A function's inferred effects must be covered by its declared effects.
- Effects must unify across `with E` boundaries.

## Rationale

This pass is what turns the [[effect-system]] from documentation into an enforced, load-bearing analysis. It realizes the [[effect-system]] design.
