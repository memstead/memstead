---
type: decision
created_date: 2026-07-15T18:44:55Z
last_modified: 2026-07-15T18:44:55Z
status: accepted
decided_on: 2026-07-08
deciders: kara-maintainers
scope: system
tags: jit, lljit, run, roadmap, phase-7
---

# Productionize the LLJIT as the Default Run Engine

## Decision
We chose to productionize the ORC2/LLJIT into a hardened in-process engine and flip `karac run` (alongside `karac test` and the REPL) to JIT-default, with an `--interp`/`KARAC_RUN_JIT=0` escape hatch to the tree-walking interpreter. The LLJIT-productionization epic was resequenced to land BEFORE Phase-12 self-hosting.

## Context
AOT link + cold-start cost dominated the run/test/REPL loop, and a prior full run/build JIT flip had been landed then reverted for stability. Running `karac run` through AOT-only left a large class of codegen-vs-interpreter (run-vs-build) divergences latent, because programs were routinely exercised only through the interpreter. Making the JIT the default run surface both removes cold-start cost and forces those divergences into the light. The resequence ahead of self-hosting was owner-confirmed.

## Consequences
- The JIT becomes the primary execution surface for `karac run`; `karac build` stays AOT; the tree-walking interpreter is retained as the reference oracle and dev/debug backend.
- run==build parity is now a first-class CI gate (codegen E2E via LLJIT) — it caught a shared-RC-drop scalar-key miscompile that AOT had masked.
- Landing it required Linux/ELF dynamic export of the runtime `karac_*` symbols so ORC's dlsym generator resolves the statically-linked FFI.
- A Slice-6c blast-radius sweep surfaced ~16 codegen gaps (Map-insert-in-a-match-arm, Option/Result & concrete-enum Display, ref-enum non-word payload binding, chained call-result field access, `?` on a concrete-enum Result, the `gpu.dispatch` symbol gap) — most closed, a few tracked.
- A `gpu.dispatch` program still cannot run under the JIT (opt-in `gpu` runtime symbols absent from the JIT rlib) — `--interp` / `karac build` is the workaround.

## Options

- Productionize the LLJIT and flip run/test/REPL to JIT-default — chosen.
- Keep run/test/REPL on AOT — rejected: cold-start cost, and the JIT is the only way to force run==build parity as a standing invariant.
- Make the tree-walking interpreter the default run engine — rejected: the interpreter is the reference oracle, not a shippable/performant engine.

## Notes


