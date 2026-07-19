---
type: spec
created_date: 2026-07-15T07:27:57Z
last_modified: 2026-07-15T18:43:31Z
level: M1
stability: evolving
tags: compiler, ownership, phase-3
---

# Ownership Checker

## Identity
The Kāra ownership-analysis pass (Phase 3): infers parameter modes, tracks borrows, detects closure-escape captures, decides RC fallback, and promotes Rc to Arc for values shared across parallel threads.

## Purpose
To enforce the [[tiered-ownership-model]] statically — memory safety without lifetimes or a GC — and to compute where reference counting must be paid.

## Relationships
- **REFERENCES**: [[tiered-ownership-model]]
- **REFERENCES**: [[auto-concurrency]]
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[type-checker]]
- **IMPLEMENTS**: [[tiered-ownership-model]]
- **REFERENCES**: [[reference-counting-elision]]

## Realization

- src/ownership.rs and src/ownership/ (borrow.rs, block_stmt.rs, capture_body.rs, closure_escape.rs, expr_check.rs, par_helpers.rs, rc_promote.rs)
- src/rc_predicate.rs; src/use_classifier.rs; tests/ownership.rs, tests/rc_fallback.rs, tests/slice_aliasing.rs, tests/safety_design.rs

- Mechanized model: src/ownership_oracle.rs (+ src/ownership_oracle/tests.rs), src/drop_differential.rs, src/bin/drop_fuzz.rs, scripts/drop-fuzz.sh, tests/drop_differential.rs

## Specifies

- Per-closure capture-path enumeration (disjoint capture) and closure-escape ref-capture detection.
- Borrow tracking and call-site borrow analysis; Slice[T] borrow tracker with interpreter aliasing parity.
- Rc→Arc promotion and RC enforcement; RC predicate with parity tests.
- Cross-borrow and shape-analysis polish; borrow-return gap tracking.

- RC governance: module-level `#![rc_budget(max: N)]` enforcement (Phase 7 line 43) and a G12 RC-creep monitoring surface that tracks where reference-counting cost accrues, reported via `karac build --cost-summary`.
- RC elision — count-op removal and headerless 16-byte nodes for provable single-owner / append-only shapes — is specified in [[reference-counting-elision]].
- Returned borrows (`-> ref T`): sound Tier-1 returns, Tier-1.5 direct use of a borrow-returning call result, Tier-2 borrows from `if` of ref params, Tier-2c binding-free destructuring match arms; chained borrow returns with `.len()`/`.is_empty()` and read-only methods on borrow locals; returned borrows through scalar-selector `match`; method `-> ref` accessors; borrowed-struct returns (`-> ref Struct`) with borrowed-field reads. Realized in src/ownership/ref_return.rs.
- Raw pointers (`*const T` / `*mut T`) are Copy; loop-local `let` rebind suppresses loop-of-consume RC.

- Mechanized ownership/drop model (core COMPLETE): an executable ownership/drop judgment (src/ownership_oracle.rs) with a sound branch-state merge, a drop-soundness fuzzer (src/bin/drop_fuzz.rs) generating conditional-move/capture shapes, and an oracle↔codegen drop differential run as a standing CI gate (src/drop_differential.rs, tests/drop_differential.rs) — the spawn/closure capture edge is closed (par too) at 100% differential agreement. Replaces the per-bug drop-soundness whack-a-mole with a generative correctness oracle.

## Constraints

- References may not alias-conflict or escape their source scope.
- Any value escaping into a par region is promoted to Arc.

## Rationale

Realizes [[tiered-ownership-model]] and supplies the safety net [[auto-concurrency]] relies on for captured mutations.
