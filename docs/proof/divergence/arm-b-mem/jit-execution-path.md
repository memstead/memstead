---
type: spec
created_date: 2026-07-15T10:18:44Z
last_modified: 2026-07-15T18:46:30Z
level: M1
stability: evolving
tags: jit, lljit, orc2, codegen, phase-7
---

# JIT Execution Path

## Identity
Kāra's JIT execution path (Phase 7): an ORC2/LLJIT-based in-process executor that JIT-compiles and runs checked Kāra code without emitting a linked binary, used as the default execution engine for `karac test` and the REPL.

## Purpose
To eliminate per-run native link and cold-start cost for the test suite and REPL by JIT-compiling in-process and amortizing module compilation across a persistent runner.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[llvm-codegen-backend]]
- **REFERENCES**: [[llvm-codegen-backend]]
- **REFERENCES**: [[interactive-surfaces-repl-jupyter-playground]]
- **REFERENCES**: [[tree-walking-interpreter]]
- **MOTIVATED_BY**: [[productionize-the-lljit-as-the-default-run-engine]]

## Realization

- JIT engine: src/codegen/lljit.rs (ORC2/LLJIT install path, coro correctness passes on install)
- Subprocess runner: src/bin/karac_jit_runner.rs (--repl-mode persistent-engine protocol)
- Test dispatch: src/test_jit_dispatch.rs (per-test timeout + watchdog), src/test_main_synth.rs (per-test main synthesizer, with_provider fixture wrapping)
- REPL client: src/repl/jit_runner_client.rs (KARAC_REPL_JIT subprocess dispatch)
- runtime/src/emutls.rs (__emutls_get_address for JIT thread_local)
- tests/lljit_e2e.rs, tests/lljit_prototype.rs, tests/mcjit_prototype.rs, tests/karac_jit_runner_repl.rs, tests/repl_jit.rs

## Specifies

- Migration path: an MCJIT prototype (phase-7 L558) was superseded by an ORC2/LLJIT path (L560/L581, worksteps W1–W5): printf round-trip, ResourceTracker lifetime story, JIT dispatch, `par` + `?` + exit-code surface, error-handling + threading edge cases.
- JIT-default flips (LLJIT-productionization, an owner-confirmed resequence ahead of Phase-12 self-hosting): Slice 5 flipped `karac test` + the REPL to JIT-default; Slices 6a/6b/6c routed `karac run` through the LLJIT and flipped it to JIT-default with an `--interp` escape. The LLJIT-productionization spike is now COMPLETE (macOS arm64 green in CI). Each `karac test` runs in a JIT subprocess with a per-test timeout and watchdog, recovering the contract-fault category from runner stdout; a faulting REPL cell salvages its buffered stdout + panic text via an atexit frame.
- A run==build parity CI leg (codegen E2E via LLJIT) guards the divergence invariant; it caught a shared-RC-drop scalar-key miscompile that AOT masked. On Linux/ELF the runner exports its `karac_*` runtime symbols dynamically (`--export-dynamic-symbol`) so ORC's dlsym generator can resolve the statically-linked FFI. A `gpu.dispatch` program still cannot run under the JIT (the opt-in `gpu` runtime symbols are absent from the JIT rlib) — `--interp` or `karac build` is the workaround.
- Test-JIT amortization: a shared-module cache compiles the suite's module once; a persistent batch runner amortizes cold-start; a signature-only skeleton and per-test resolve+typecheck elision cut per-test parent-side compile for no-fixture tests.
- JIT-published SPAWN_SITES addresses (not binary stand-ins); contract-predicate FFIs preserved in the JIT keep-list; per-line newline preservation in JIT stdout capture.

## Constraints

- JIT and AOT must produce identical observable behavior; divergence is a bug (mirrors the interpreter/codegen agreement invariant). The JIT-default flip made the JIT the primary run surface, flushing a large class of run-vs-build divergences (Map-insert-in-a-`match`-arm, `ref`-enum non-word payload binding, Option/Result & concrete-enum `Display`, chained call-result field access, `?` on a concrete-enum Result, and the GPU-dispatch symbol gap, among ~16 swept in the Slice-6c gap sweep).
- `karac build` remains AOT. `karac run` now DEFAULTS to the JIT, with an `--interp` (`KARAC_RUN_JIT=0`) escape hatch (LLJIT Slice 6c); `karac test` and the REPL also default to JIT. The tree-walking interpreter is retained as the reference oracle and dev/debug backend.

## Rationale

Phase 7. Built on the [[llvm-codegen-backend]] IR emission; the REPL half integrates with the JIT session model of [[interactive-surfaces-repl-jupyter-playground]]. Reference semantics still come from the [[tree-walking-interpreter]].
