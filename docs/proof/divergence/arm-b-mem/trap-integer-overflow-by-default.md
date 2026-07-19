---
type: decision
created_date: 2026-07-15T18:45:32Z
last_modified: 2026-07-15T18:47:06Z
status: accepted
decided_on: 2026-06-15
deciders: kara-maintainers
scope: system
tags: arithmetic, overflow, codegen, safety, phase-7
---

# Trap Integer Overflow by Default

## Decision
We chose checked integer arithmetic that TRAPS on overflow by default in AOT/JIT code; wrapping is opt-in via a scoped `#[wrapping]` attribute. Overflow is a defined trap, never silent two's-complement wraparound.

## Context
Silent wraparound is a classic source of undefined-behavior-flavored bugs. Kāra's positioning as a safe systems language makes a defined-failure default preferable to a fast-but-silent one. This round recorded the trap-by-default rationale explicitly and began tracking the scoped `#[wrapping]` opt-out.

## Consequences
- Codegen emits checked arithmetic (`llvm.*.with.overflow` + a trap edge); a wide family of arithmetic bugs is about maintaining this invariant (e.g. `-i64::MIN` neg-trap on Column/Tensor, checked `pow`, saturating/overflowing method families).
- AOT overflow-trapping is load-bearing for other soundness arguments — several bounds-check / monotone-assume elisions are sound precisely because no-wrap holds on all defined executions.
- The cost is measurable on arithmetic-heavy hot loops; equal-safety benchmarking compares against `rustc -C overflow-checks=on`, not the wrapping release build.
- `#[wrapping]` provides an escape for code that genuinely wants modular arithmetic without turning off the global guarantee.

## Relationships
- **ENABLES**: [[bounds-check-elision]]

## Options

- Trap by default, `#[wrapping]` opt-out — chosen.
- Wrap by default (C/Rust-release semantics) — rejected: silent wrong results, and it would forfeit the no-wrap facts that downstream check-elisions depend on.
- Saturate by default — rejected: masks bugs as clamped values rather than surfacing them.

## Notes


