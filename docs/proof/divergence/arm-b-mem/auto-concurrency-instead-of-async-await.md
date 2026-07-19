---
type: decision
created_date: 2026-07-15T07:22:51Z
last_modified: 2026-07-15T07:34:21Z
status: accepted
decided_on: 2026-05-01
deciders: kara-maintainers
scope: system
tags: concurrency, effects, language-design
---

# Auto-Concurrency Instead of Async-Await

## Decision
We chose to derive concurrency automatically from effect analysis rather than expose async/await. Independent effect regions are parallelized by the compiler; there are no `async` functions, no `await`, and no function coloring. Users write straight-line code and the compiler spawns work where the effect graph proves independence.

## Context
Async/await splits an ecosystem into colored (sync vs async) functions and forces manual concurrency plumbing. Kāra already tracks effects (reads/writes/sends/receives/allocates/panics) per function; that dependency information is exactly what a scheduler needs to know which regions can run in parallel. Reusing it removes both the coloring problem and the manual-async burden.

## Consequences
- No colored functions: any function composes with any other regardless of I/O.
- The effect checker becomes load-bearing for correctness of parallelization, not just documentation.
- Codegen must lower auto-parallel regions to a runtime worker pool (`karac_par_run`) and guard captured mutations; auto-par is gated behind `KARAC_AUTO_PAR` while it stabilizes.
- Requires a concurrency-analysis pass and a `--concurrency-report` surface so users can see what the compiler parallelized.

## Relationships
- **MOTIVATED_BY**: [[colored-functions]]

## Options

- async/await with colored functions — rejected: ecosystem split, manual plumbing, no reuse of effect data.
- Manual threads only — rejected: pushes concurrency entirely onto the user.
- Effect-driven auto-concurrency — chosen: reuses effect analysis the language already performs.

## Notes


