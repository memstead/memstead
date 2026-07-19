---
type: spec
created_date: 2026-07-15T07:26:14Z
last_modified: 2026-07-15T07:34:16Z
level: M1
stability: evolving
tags: ownership, memory, core
---

# Tiered Ownership Model

## Identity
Kāra's memory-management model: inferred parameter passing modes, owned-by-default returns, explicit `ref`/`mut ref` borrows, and reference-counting fallback with budget controls — no lifetime annotations and no garbage collector.

## Purpose
To deliver memory safety without a GC and without the lifetime-annotation burden that dominates Rust's learning curve.

## Relationships
- **REFERENCES**: [[tiered-ownership-without-lifetime-annotations]]
- **MOTIVATED_BY**: [[tiered-ownership-without-lifetime-annotations]]

## Realization

- src/ownership.rs and src/ownership/ (borrow.rs, block_stmt.rs, capture_body.rs, closure_escape.rs, expr_check.rs, par_helpers.rs, rc_promote.rs)
- RC decision: src/rc_predicate.rs, tests/rc_fallback.rs, tests/rc_predicate_parity.rs
- Cost reporting: src/cost_summary.rs
- Type surface: Type::Shared / Type::Rc / Type::Arc (typechecker)

## Specifies

- Parameter mode inference (owned vs borrowed) with no annotations.
- Owned return values by default; `ref` / `mut ref` for explicit borrows, including ref-self / mut-ref-self methods.
- Borrow tracking, closure-escape ref-capture detection, and Slice[T] borrow tracking.
- RC fallback when static ownership cannot be proven, with Rc→Arc promotion for values shared across parallel threads.
- Budget controls and a cost summary that surface where RC is paid.
- `weak` references with defined runtime behavior.

## Constraints

- A `ref` may not outlive or alias-conflict with its source (enforced without lifetimes).
- Values escaping into a par region are promoted to Arc.

## Rationale

Rationale and rejected alternatives (explicit lifetimes, GC) in [[tiered-ownership-without-lifetime-annotations]].
