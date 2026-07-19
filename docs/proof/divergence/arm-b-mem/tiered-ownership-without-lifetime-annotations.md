---
type: decision
created_date: 2026-07-15T07:22:39Z
last_modified: 2026-07-15T07:22:39Z
status: accepted
decided_on: 2026-05-01
deciders: kara-maintainers
scope: system
tags: ownership, memory, language-design
---

# Tiered Ownership Without Lifetime Annotations

## Decision
We chose a tiered ownership model with no lifetime annotations: parameter passing mode is inferred, functions return owned values by default, `ref`/`mut ref` mark explicit borrows, and reference-counting (RC) is the fallback when static ownership cannot be proven — governed by budget controls that surface RC cost to the user.

## Context
Rust's borrow checker with explicit lifetimes is the largest single source of its learning curve. Kāra wants memory safety without a garbage collector but also without lifetime syntax. The tiered model keeps the common case annotation-free and falls back to RC (with an Rc→Arc promotion for values shared across parallel threads) only where borrow analysis cannot guarantee safety.

## Consequences
- No lifetime syntax anywhere in the surface language.
- Ownership analysis must infer parameter modes, track borrows, detect closure-escape captures, and promote Rc to Arc for values crossing a par-region boundary.
- RC fallback has a runtime cost; budget controls and cost-summary reporting expose where RC is used so it can be tuned.
- Correctness of `ref` returns and aliasing is enforced by the ownership checker rather than by lifetimes.

## Options

- Rust-style explicit lifetimes — rejected: primary learning-curve cost.
- Garbage collection — rejected: incompatible with predictable systems-level memory.
- Tiered ownership with inferred modes and RC fallback — chosen.

## Notes


