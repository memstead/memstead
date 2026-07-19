---
type: design-decision
title: AI-first compiler interface
updated_round: 9
---

# AI-first compiler interface

Kāra's compiler is designed to be consumed by AI agents as well as humans. This is a
declared design bet from the redesign (CHANGELOG), and it graduated further as the
**v63 "LLM compiler query channel"** brainstorm (see
[[history-reversals-and-deprecations]]).

Elements:

- **Structured JSON diagnostics** — machine-readable compiler errors. The `atexit`
  error-trace printer supports **`KARAC_ERROR_TRACE_FORMAT=json|jsonl|text`**, and the
  `?` operator supports a JSON trace mode.
- **Diagnostic classes** (new in round 2, Phase 5 line 619) — a **`DiagnosticClass` enum**
  (`src/diagnostic_class.rs`) is threaded through errors (e.g. `TypeError`), with
  **`--format=json` + `--class=`** on `karac explain`, **typed `expected` / `got` + class**
  on JSON diagnostics, and a **`fixes[]` array** shape carrying suggested fixes.
- **Stub hints** (new in round 2, line 633) — a **`StubHint`** (gated to test files) infers
  a missing function's signature from its call site (**literal-argument inference**) and
  emits it as a **`hints[].diff` JSON envelope** an agent can apply directly.
- **A compiler query API** — a programmatic channel for tools/agents to query the compiler,
  expanded this round: **`karac query affected-by`** (call-graph reach), **`karac query
  monomorphization`**, **`karac query attributes [--tool PREFIX]`**, a **codegen-queries**
  analyzer (inlining + branch hints), and **`karac catalog`** (a public-API surface index
  as JSONL). Built on a new `def_path` stable-path foundation. See [[cli]].
- **Diagnostic-namespace attributes** (new in round 2) — `#[diagnostic::on_unimplemented]`
  and `#[diagnostic::do_not_recommend]` let library authors shape the errors an agent sees
  at a failed trait bound. See [[attributes]].
- **Canonical formatting** — a formatter (`karac fmt`-style) with canonical output, split
  into per-construct printers (items, types, exprs, stmts, patterns).
- **`std.runtime` introspection APIs** and a **debugger contract** — SpawnSiteId metadata
  tables for `par` sites, parent-frame refs, a `KaracWaitTarget` surface, and structured
  `dbg()` output with task-id tagging. The contract was **extended for parked tasks** in the
  [[design-runtime-phases|event-loop]] work (Phase 6 line 7).
- **`karac explain --concept=<name>`** pages (e.g. `--concept=closures`) for
  human/agent-readable concept documentation (see [[cli]]).
- **Interactive surfaces** (new in round 2) — a **[[jupyter-kernel|Jupyter kernel]]** with a
  rich **`DisplayBundle`** output path, and a **[[playground|browser playground]]** on
  wasm32, both lowering the barrier for agents and humans to run Kāra.

- **"Two Surfaces" book chapter** (new in round 3, `docs/book/src/ch01b-two-surfaces.md`,
  Phase 5 line 801) — a doc chapter framing Kāra's **human surface** and **agent surface**
  as first-class, distinct consumers of the same compiler.
- **Typed contract-fault categories** (new in round 4) — when a
  [[design-contracts-and-verification|contract]] fails, the `test_fail` JSONL carries a
  **typed fault class** (contract-violated vs contract-predicate-panicked vs cross-call
  panic), so an agent can tell *why* a check failed, not just *that* it did.
- **Level 2 crash diagnostics** (new in round 4) — compiled binaries emit **panic location**
  (`file:line:col` in fn) and **DWARF debug-info**, so a crash from generated code is
  traceable. See [[codegen]].
- **Machine-applicable effect rewrites** (new in round 5) — the **`E0412` resource-receiver
  contradiction** ships a **machine-applicable `ref self` rewrite**; the **`karac query
  ownership/effects/concurrency`** surface was fixed to target impl methods. The README's
  **AI-First Compiler Interface** section was expanded, and its lead example replaced with a
  **full agent loop** (E0412). See [[design-effect-system]], [[cli]].

- **Whole-program effect/concurrency graph** (new in round 6) — a whole-program
  effect/concurrency graph (`src/effect_graph.rs`, +421) that an agent can query and that the
  **[[examples-and-benchmarks|Cartographer]]** dogfood renders **live in the browser** as the
  compiler's own effect graph. It surfaced (and fixed) a generic-receiver query key-join bug
  where impl methods on generic types reported empty effects (`B-2026-06-14-3`).
- **New `karac query` analyzers** (round 6) — the agent query surface grew four P1.x
  analyzers: **P1.1 RC-fallback** (where an [[design-ownership|RC fallback]] fires, at the use
  site; `src/rc_fallback_queries.rs`), **P1.2 generic specialization** (monomorphization
  fan-out; `src/specialization_queries.rs`), **P1.5 layout-choice** (struct-of-arrays
  opportunities; `src/layout_queries.rs`, feeds [[per-layout-monomorphization|SoA]]), and
  **P1.6 auto-concurrency fork-threshold** (`src/fork_threshold_queries.rs`). `query
  concurrency` also gained **self-locating spans** + a **structured exclusion-reason** and a
  **reorderable-advisory**. See [[cli]], [[design-concurrency-and-providers]].

**Round 9 — diagnostics made machine-applicable + a scored Mend loop.** A
diagnostic-fix-invariant audit (`docs/diagnostic-fix-audit.md`) found the resolver computed the
exact correct name for its `did-you-mean` suggestions but emitted only human prose; round 9
populated the `.replacement` `TextEdit` across the whole `E01xx` family (`B-2026-07-06-3`,
`B-2026-07-07-3`) and taught **`karac fix`** to apply the ownership `fix_diff` migration it
already computed (`B-2026-07-06-4`) — so an agent's `did-you-mean` / concurrency-migration
suggestions are now auto-applicable, not just readable. The **Mend** demo became a scored,
batch-runnable corpus (`mend_batch.py` / `mend_score.py` + a codified task+oracle format) — the
"develop Kāra through the loop" workflow. See [[bug-tracker]], [[cli]], [[examples-and-benchmarks]].

The project itself is AI-orchestrated (it ships a `CLAUDE.md`), and includes the **Mend**
demo where an AI writes Kāra (see [[examples-and-benchmarks]]).

Related: [[design-effect-system]], [[compiler-pipeline]], [[attributes]],
[[jupyter-kernel]], [[cli]], [[bug-tracker]].
