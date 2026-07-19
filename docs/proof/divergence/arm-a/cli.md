---
type: architecture
title: karac CLI
updated_round: 9
---

# `karac` CLI

The compiler binary is **`karac`**. Command surface (the CLI is split into
`src/cli/{args,help,explain}.rs`):

- **`karac build`** — compile; supports **`--offline`** (round 3: consults `vendor/` for
  path-deps), **`--no-proxy`**, **`--enable-hot-swap`** (round 2, see [[codegen]]),
  **`--release`** (round 4: strips [[design-contracts-and-verification|contracts]] and the
  `?`-error-return-trace), **`--monomorphization-budget`** (round 4, opt-in cap), and
  multi-file project mode (concat-super-program codegen). Late-phase diagnostics carry
  per-module file context.
  - **Round-5 target flags** (see [[wasm-targets]]): **`--target=wasm_wasi`** /
    **`--target=wasm_browser`** (WASM build paths), **`--features wasm-threads`** (opt-in
    threaded WASM), **`--bindings <shape>`** (WASM output-shape selector), **`--target-cpu`**
    (CPU baseline) and **`--target-features`** (feature-string override) — each also
    settable via env and a `[release]`-table entry. **`--simd-report[=verbose]`** reports
    which [[simd|SIMD]] ops vectorized. Round 6 fixed **`karac project build --target=wasm_*`**
    to drive platform-suffix module selection, and added a native **[[windows-and-cross-platform|
    Windows]]** AOT build target.
- **`karac check`** — round 5 added **multi-target verification** (check a program against
  several [[wasm-targets|targets]] at once); **round 9 surfaces dependency-resolution
  diagnostics** in `check` (and `run`), and refuses a single-file build/check of a
  [[package-management|package member]] with guidance to build the whole package.
- **`karac new`** — scaffold a project; round 4 ships a **Backend HTTP server template**
  (phase-8 line 63). See [[networking]].
- **`karac run`** — run a program; **`--timeout DURATION`** (round 4, P0) bounds execution.
  **Round 9 flipped `run` to the [[codegen|LLJIT execution path]] by default** (Slice 6c,
  `ef7d355d`), with an **`--interp`** escape to the tree-walk interpreter (env
  **`KARAC_RUN_JIT=0`** is equivalent). This is the deliberate close of the **run-vs-build
  divergence**: codegen becomes the default execution backend, the interpreter a dev/debug
  fallback. Round 6's **type-leniency → hard abort** and round 5's effect-violation leniency
  still apply; **Slice 6a additionally stripped `run`-leniency** so `run` rejects like
  `check`/`build`. A run of gaps a lenient interpreter had hidden were fixed in the flip's
  wake (see [[bug-tracker]]); `gpu.dispatch` under the JIT stays an open gap
  (`B-2026-07-10-6` — use `KARAC_RUN_JIT=0` / `karac build`).
- **`karac test`** — round 4 runs tests via a **[[codegen|JIT subprocess]]** by default, with
  a **per-test timeout** (default 30 s, env-var / `kara.toml` / per-test-attribute override)
  and a watchdog; emits a **typed contract-fault category** in the `test_fail` JSONL (see
  [[design-contracts-and-verification]]). **Round 8** added **cross-package module loading**:
  `karac test` now loads dep modules + dev-deps so `import <pkg>.…` reaches the test surface.
  This fixed B-2026-07-01-4 (per-test program missing imported items → interpreter panic) and
  B-2026-07-01-5 (a test-companion re-import tripped a false "already defined" E0101; exact
  re-imports are now deduped).
- **`karac clean`** — clean build artifacts.
- **`karac install`** — install a path-source binary; consumes an **install-spec** parser
  (round 3). See [[package-management]].
- **`karac vendor`** — copy path-deps into `./vendor/` (round 3).
- **`karac update [pkg]`** — bare-form (update all) and surgical-form (a named package),
  round 3.
- **`karac cache`** — `info` + build-cache key inspection (round 3).
- **`karac resolve`** — **new in round 8**: a read-only dependency-graph inspection command,
  with an **`--output=json`** shape pinned for proxy-fetch diagnostics. See
  [[package-management]].
- **registry-proxy `build` subcommand** — **new in round 8**: assemble a store in one
  command. See [[package-management]].
- **`karac explain`** — concept explainer pages via **`--concept=<name>`** (e.g.
  `--concept=closures`); round 2 added **`--format=json`** and **`--class=`** for structured
  diagnostic-class output. See [[design-ai-first-compiler]].
- **`karac query …`** (round 2 — the agent query surface): **`affected-by`** (call-graph
  reach), **`monomorphization`**, **`attributes [--tool PREFIX]`** (tool-namespaced
  attributes), and **`ownership` / `effects` / `concurrency`** — round 5 fixed these to
  **target impl methods** (previously they could not resolve a method target; surfaced by
  the [[examples-and-benchmarks|Tangle]] dogfood). **Round 6** added four analyzers —
  **`rc-fallback`** (P1.1), **generic `specialization`** (P1.2), **`layout`-choice** (P1.5,
  the [[per-layout-monomorphization|SoA-opportunity]] query), and auto-concurrency
  **`fork-threshold`** (P1.6) — plus a whole-program **effect/concurrency graph** behind the
  [[examples-and-benchmarks|Cartographer]] studio, and self-locating spans + a structured
  exclusion-reason on **`query concurrency`**. See [[design-ai-first-compiler]],
  [[attributes]].
- **`karac catalog`** — emit a **public-API surface index as JSONL** (round 2, line 643).
- **`--concurrency-report`** — human-readable concurrency-analysis renderer (Slice D). See
  [[design-concurrency-and-providers]].
- **Lint-level flags** (round 2): **`-A` / `-W` / `-D` / `-F`** (allow/warn/deny/forbid)
  with cross-module fall-through; forbid mode cannot be overridden. See [[attributes]].

The `clean`/`install`/`vendor`/`--offline` group landed together as **P1**. Round 3 grew
this into a full **[[package-management|package manager]]** (PubGrub resolver, `kara.lock`,
registry proxy, build cache, `toolchain.toml`).

## `karac migrate` (new in round 3)

**`karac migrate`** applies large concurrency-safety rewrites automatically (the L215
series), driven by the [[design-ownership|concurrent-struct diagnostics]] `fix_diff`
envelopes:

- **`shared → par <Type>`** — a type-def rewrite plus **consumer-site write/read rewrites**
  (lock-block wraps, `lock self.field` shape), with **typecheck-aware binding discovery**
  and a **project-mode cross-file walk**.
- **`--atomic`** — a heuristic that rewrites a shared binding to an `Atomic[T]`, with a
  matching **consumer-site rewrite**. **Round 4 flipped this to the default**: `karac migrate`
  now applies the `Atomic[T]` heuristic unless **`--no-atomic`** is passed. See
  [[design-ownership]].

## REPL

A REPL (`repl`) with:

- **`--auto-clone`** opt-in mode + a perf-note channel.
- Notebook-aware use-after-move diagnostics with cell-byte-range tracking.
- **Round 4**: the REPL now runs on the **[[codegen|JIT execution path]]** by default (a
  persistent-engine `karac_jit_runner --repl-mode` subprocess), with cross-cell symbol
  amortization and value-snapshot persistent-`let` for primitives / String / Vec / Map / Set.

## Environment variables

- `KARAC_AUTO_PAR` — gate auto-parallelization ([[design-concurrency-and-providers]]).
- `KARAC_PAR_WORKERS` — override the auto-par worker-pool count (round 3).
- `KARAC_PAR_MAX_FORK_DEPTH` — cap recursive-reduction fork depth (round 8, default 1).
- `KARAC_RUN_JIT=0` — force `karac run` onto the tree-walk interpreter (round 9; `--interp`).
- `KARAC_FORCE_TARGET_ARCH` — force a target arch for the `#[repr(C)]` ABI signature-match
  tests (round 9; how the arm64/Windows ABIs are CI-verified without the hardware).
- `KARAC_ERROR_TRACE_FORMAT=json|jsonl|text` — error-trace printer format.
- `KARAC_HTTP_BLOCK_IN_PLACE` — HTTP layer perf A/B probe ([[examples-and-benchmarks]]).

Related: [[compiler-pipeline]], [[stdlib-and-traits]], [[package-management]].
