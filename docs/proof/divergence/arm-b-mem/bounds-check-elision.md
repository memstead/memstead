---
type: spec
created_date: 2026-07-15T07:29:43Z
last_modified: 2026-07-15T18:14:51Z
level: M0
stability: experimental
tags: codegen, performance, phase-7
---

# Bounds-Check Elision

## Identity
A codegen optimization that removes provably-unnecessary array/slice bounds checks — for `for i in start..end` range loops, Slice[T] reads/writes, monotone induction variables (via `llvm.assume` range facts), and rolling-DP length pins — plus a `Vec.get_unchecked` escape hatch.

## Purpose
To eliminate redundant bounds checks on indexing that the loop shape already proves safe, recovering native-array performance without unsafe user code.

## Relationships
- **REFERENCES**: [[unsafe-and-ffi-surface]]
- **REFERENCES**: [[llvm-codegen-backend]]
- **REFERENCES**: [[deferred-work-tracker]]
- **PART_OF**: [[llvm-codegen-backend]]
- **DEPENDS_ON**: [[unsafe-and-ffi-surface]]

## Realization

- src/codegen/control_flow_bce.rs (bounds-check elision plumbing)
- src/codegen/bce_length_pin.rs (rolling-DP length-pin analysis)
- Vec.get_unchecked source-level elision path
- docs/implementation_checklist/phase-7-codegen.md

## Specifies

- Elision for `for i in start..end` ranges and for Slice[T] reads and writes.
- Source-level elision plus a `Vec.get_unchecked` intrinsic.
- Monotone-variable BCE: `llvm.assume` range facts fold write-head bounds checks; induction-monotonicity elision promoted to its own lever.
- Further tiers dispositioned by simulated-demand measurement: the merge tier was judged unsound and the KMP interprocedural tier held before coding; a table-range tier was filed, and kata-28's brute-force compound index confirmed as a non-trigger.

- Rolling-DP length pins (src/codegen/bce_length_pin.rs): a counted fill that builds a Vec to exactly `bound` elements establishes a `bound == v.len()` pin, so a later scan `dp[c] = dp[c] + dp[c-1]` elides both reads' checks even when the guard is spelled `c < cols` rather than `dp.len()`. Recognizes for-range fills, seed preludes, and arithmetic bounds, and fires in nested blocks; a fail-closed shadow-soundness scan (`region_bindings`) refuses the pin under any nested shadow or reassignment of the Vec or a bound identifier. Measured ~3× on kata #62, tying rustc -O.

## Constraints

- Elision applies only where the index is provably in range.
- `Vec.get_unchecked` and source-level elision were gated on the [[unsafe-and-ffi-surface]] enforcement predecessor.

## Rationale

Phase-7 optimization in the [[llvm-codegen-backend]]. An earlier blanket `llvm.assume`-based approach (brainstorm v68) was a measured negative result and archived — see [[deferred-work-tracker]] — but the mechanism was later revived in a narrowed form: monotone-variable BCE emits `llvm.assume` range facts to fold write-head bounds checks where an induction variable is provably monotone (docs/investigations/bce_monotonic_assume.md).
