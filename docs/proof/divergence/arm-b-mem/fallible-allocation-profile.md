---
type: spec
created_date: 2026-07-15T10:53:31Z
last_modified: 2026-07-15T18:48:06Z
level: M1
stability: experimental
tags: allocation, fallible, profile, phase-8, no-panic
---

# Fallible Allocation Profile

## Identity
Kāra's fallible-allocation surface: a `panic_on_alloc_failure` `[profile]` knob that, when false, forbids panicking allocation and requires the `try_*` companion methods on builtin collections, which return `Result[_, AllocError]` instead of aborting.

## Purpose
To let Kāra target environments (kernels, constrained embedded) where an allocation failure must be a recoverable `Result`, not a panic — without forcing every program to pay the fallible ergonomics.

## Relationships
- **PART_OF**: [[standard-library]]
- **DEPENDS_ON**: [[effect-checker]]

## Realization

- Manifest: `panic_on_alloc_failure` [profile] knob + moot rejection (src/manifest.rs), ProfileConfig threading (src/effectchecker, profile_compat.rs)
- Rejection gates: src/typechecker/alloc_rejection.rs, src/fallible_alloc.rs
- Stdlib: runtime/stdlib/alloc_error.kara + `try_*` companions on collections
- Prelude: AllocError prelude type

## Specifies

- `panic_on_alloc_failure` `[profile]` knob (default true) with moot-rejection when the value is redundant.
- `try_*` companions on builtin collections: `Vec.try_push` / `try_with_capacity` / `try_from_slice` / `try_extend_from_slice`, `String.try_push_str`, `VecDeque.try_push_front` / `try_with_capacity`, `try_clone` for Vec/VecDeque/String/Tuple — all returning `Result[_, AllocError]`; fallible-allocation runtime wrappers.
- Under `panic_on_alloc_failure=false`: `E_PANICKING_ALLOC_REJECTED` (a panicking alloc is rejected), `E_DERIVE_CLONE_ALLOCATES` (a derived Clone that allocates is rejected), and `E_RC_FALLBACK_ALLOCATES_UNDER_FALLIBLE_PROFILE` (the RC fallback's allocation is rejected).
- `?`-propagation through `AllocError`.
- `AllocError` as a prelude type (interpreter-complete).

- `Map.try_insert` / `Set.try_insert` → `Result[Option[V], AllocError]` / `Result[bool, AllocError]`, now with codegen (a `karac_map_try_insert` runtime path over a null-checked, self-unchanged-on-OOM resize) — extending the `try_*` surface to the hash collections on the native/JIT backend (SortedSet.try_insert follows the sorted-container codegen).

## Constraints

- With `panic_on_alloc_failure=false`, any panicking allocation — including a derived Clone or the RC fallback — is a compile error; only the `try_*` surface is legal.
- The knob lives in the manifest `[profile]` table and threads through as ProfileConfig.

## Rationale

Phase-8 stdlib-floor work built on the manifest `[profile]`-table knob substrate + ProfileConfig threading. The prelude's `AllocError` type and the effect/typecheck gates make the choice enforceable. Complements the RC-fallback path, which must itself be fallible under this profile.
