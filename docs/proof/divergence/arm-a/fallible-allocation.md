---
type: design-decision
title: Fallible allocation and the [profile] knob
updated_round: 5
---

# Fallible allocation

**New in round 5 (Phase 8).** Kāra added a **fallible-allocation** mode so allocation
failure can be a recoverable `Result` rather than a panic — targeted at systems / embedded
/ kernel-adjacent programs that must not `panic` on OOM. It is driven by a **`[profile]`
manifest knob** and enforced by the type checker.

## The `[profile]` knob substrate

- A general **`[profile]`-table knob substrate** was added to the manifest with a
  **`ProfileConfig`** threaded through the pipeline (`0df8aad6`) — the same substrate other
  profile settings ride on. See [[design-effect-system]].
- **`panic_on_alloc_failure`** — the specific `[profile]` knob. When `false`, allocation is
  fallible and panicking-allocation paths are rejected. A **moot rejection** guards the
  contradictory case.

## Type-checker enforcement (rejection errors)

Under `panic_on_alloc_failure = false`, the type checker rejects the ways a program could
silently panic on OOM (`src/typechecker/alloc_rejection.rs`, +414):

- **`E_PANICKING_ALLOC_REJECTED`** — a panicking (infallible) allocation call is rejected.
- **`E_DERIVE_CLONE_ALLOCATES`** — a `derive(Clone)` that would allocate is rejected.
- **`E_RC_FALLBACK_ALLOCATES_UNDER_FALLIBLE_PROFILE`** — the [[design-ownership|RC
  fallback]] itself allocates, so it is rejected under the fallible profile.

`?`-propagation through **`AllocError`** is verified.

## The `try_*` companions and `AllocError`

- **`AllocError`** — a prelude error type (`runtime/stdlib/alloc_error.kara`),
  interpreter-complete.
- **`try_*` companions** on the builtin collections (`src/fallible_alloc.rs`, +112, plus
  runtime wrappers): **`Vec.try_push`**, **`Vec.try_with_capacity`** /
  `VecDeque.try_with_capacity` / `String.try_with_capacity` (the complete trio),
  **`Vec.try_from_slice`**, **`Vec.try_extend_from_slice`**, **`String.try_push_str`**,
  **`VecDeque.try_push_front`**, and **`try_clone`** for `Vec` / `VecDeque` / `String` /
  `Tuple` (a fallible deep clone — Phase 8 item 8). Each returns a `Result[_, AllocError]`.

Related: [[design-ownership]], [[stdlib-and-traits]], [[design-effect-system]],
[[implementation-phases]].
