---
type: decision
created_date: 2026-07-15T07:23:31Z
last_modified: 2026-07-15T07:34:23Z
status: accepted
decided_on: 2026-06-25
deciders: kara-maintainers
scope: component
tags: codegen, llvm, performance
---

# Run LLVM O2 Mid-End Passes on Emitted IR

## Decision
We chose to run LLVM's mid-end optimization pipeline (the `default<O2>` pass set) over the IR Kāra emits, rather than handing unoptimized IR to the backend. A performance investigation had ruled out the runtime and the interpreter as the bottleneck and pinned it on unoptimized emitted IR.

## Context
The Parallax benchmark showed Kāra far behind Rust/Go/Node. A probe sweep (docs/investigations/parallax_perf.md) eliminated the runtime and confirmed the emitted IR was never being optimized — the compiler was emitting naive IR and relying on nothing to clean it up.

## Consequences
- Emitted binaries run the O2 mid-end pipeline, closing a large part of the Kāra-vs-Rust gap.
- Compile time rises with optimization work.
- Establishes the pattern of measuring before optimizing (probe sweep first, then fix).

## Relationships
- **MOTIVATED_BY**: [[parallax-performance-investigation]]

## Options

- Emit naive IR, no mid-end passes — rejected: leaves large, obvious performance on the table.
- Hand-roll peephole optimizations in codegen — rejected: duplicates what LLVM already does.
- Run LLVM `default<O2>` on emitted IR — chosen.

## Notes


