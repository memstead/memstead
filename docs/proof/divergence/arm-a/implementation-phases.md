---
type: implementation-plan
title: Implementation phases and dependencies
updated_round: 10
---

# Implementation phases and dependencies

The roadmap (`docs/roadmap.md`) defines the implementation plan. The implementation
checklist (`docs/implementation_checklist/`) now tracks phases **1–12** — round 5 added
**Phase 12 (self-hosting)**. Each phase builds on the prior one — the pipeline order in
[[compiler-pipeline]] mirrors the dependency chain.

**Round-10 headlines:** **[[self-hosting|Phase 12 (self-hosting)]] advanced hugely** — the
front-end port now spans lexer → parser (all three oracles green) → a new **Resolver** → a new
**TypeChecker** (~21 slices) → and the **codegen backend BEGUN** (an LLVM-IR emitter Slice 1,
after a backend-feasibility spike returned **GO**); **[[stdlib-and-traits|Phase 8 stdlib floor]]**
saw a large expansion (String/char/integer/float method families, f-string format specifiers,
iterator terminals, `ref_eq`, `SortedSet`/`SortedMap` codegen, `Map`/`Set.try_insert` codegen,
`f16`/`bf16` types, `std.mem take`); **[[gpu-compute|Phase 10 GPU / Track C]]** COMPLETE — the
GPU-LBM cluster (full LBM substep on Metal) + GPU-SLIP cluster with a device/pipeline cache
(**1.9×**); **[[numerical-stdlib-and-tensors|Phase 11 numerical longtail]]** added `std.embeddings`,
`std.simd.math`, `f16`/`bf16`, and Tensor `iter_axis`; and **three new surfaces** — a security
surface (`Secret[T]` / `std.secret` / Book ch19, see [[design-secrets]]), an embedded/MMIO surface
(`volatile_read`/`volatile_write` + a `Hardware` effect, `VolatileCell`, `critical_section`,
`fence`, `Atomic` memory ordering, see [[embedded-mmio]]), and a **language server** `kara-lsp`
(6 slices, roadmap Track 3, see [[lsp]]).

**Round-9 headlines:** **[[cli|`karac run` flipped to JIT-default]]** (LLJIT productionization
Slice 6c — the interpreter becomes an `--interp` fallback), which **resequenced the roadmap**
(LLJIT-productionization inserted *before* Phase 12) and drove a large run-vs-build correctness
sweep; **[[gpu-compute|GPU LBM kernels on Metal]]** (struct-SoA `gpu.dispatch`, multi-field SoA
groups, `#[gpu]` control flow); the **[[design-unsafe-ffi-and-pointers|additive-interop /
producer-mode]]** surface (a `[lib]` manifest, raw-pointer methods, auto-boxing, `#[repr(C)]`
enum crossing, and a per-target **`#[repr(C)]` struct-by-value ABI** that a 5-target CI matrix
gates); **[[design-concurrency-and-providers|A2b-2 auto-parallel network I/O]]** with
parameterized-resource partition keys; **[[stdlib-and-traits|`std.mem`/`std.cmp`]]**, `Display`
for `Option`/`Result`, `SortedSet`/`SortedMap` codegen, and **protobuf under codegen**; and the
**[[self-hosting|self-host parser]]** un-paused (the expression oracle green, the item/type
oracle residual `B-2026-07-10-4` open). **[[package-management|Dependency pinning]]** (lockfile
pin-over-catalog, target-deps, workspace discovery).

**Round-8 headlines:** the **[[codegen|heap-env closure epic]]** (`B-2026-06-22-2`, round 7's
headline open bug) **closed** — an RC heap env for escaping capturing closures across every
place; **[[gpu-compute|GPU codegen]]** advanced from spike to a working **slice-0** (a `wgpu`
compute spine **proven on Metal**, WGSL codegen, end-to-end `gpu.dispatch`); **[[columnar-data|
DataFrame + `Stats.*`]]** joined `Column[T]` (Phase 11); a **[[stdlib-and-traits|Reduce /
ElementwiseMap / ElementwiseOrd]]** trait surface + a broad **[[stdlib-and-traits|trait-system]]**
build-out (default methods, primitive impls, generic-bound dispatch, derived-`Ord` ordering);
a whole **[[package-management|dependency-fetching]]** subsystem (git deps, a `registry-proxy`
crate + protocol, **PubGrub** version solving, yank awareness); iterator-adaptor **`.collect()`**
lowering + a rolling-DP **length-pin BCE**; and the **[[self-hosting|self-host parser]]**
reaching attributes + doc comments. A large **run-vs-build divergence** sweep dominated the
[[bug-tracker|ledger]].

**Round-7 headlines:** a new **[[gpu-compute|GPU compute-shaders]]** front end (Phase 10 —
explicit `#[gpu]` + the `GpuSafe` trait + a call-graph/effect gate; WGSL codegen was still a
spike), a nullable **[[columnar-data|`Column[T]`]]** Arrow type with SQL 3VL (Phase 11),
**first-class function values** completed and a large **[[codegen|heap-env closure epic]]**
(the round's headline open bug, `B-2026-06-22-2`), a broad **[[protobuf]]** field-type
build-out (nested/repeated/enum/map/float/oneof/sint-fixed), new interpreter-slice stdlib
primitives (`OnceLock`/`OnceCell`, `Arena`, `Symbol`/`Interner`), and the
**[[self-hosting|self-host parser]]** reaching trait/impl + generics.

**Round-6 headlines:** the **[[windows-and-cross-platform|M3 cross-platform-parity gate
flipped to DONE]]** (native Windows IOCP), a whole **[[metaprogramming|compile-time
metaprogramming (comptime)]]** layer landed and carried the new **[[protobuf]]** stdlib,
**[[per-layout-monomorphization|SoA layouts now cross function boundaries]]**, the
**[[self-hosting|self-host parser port]]** advanced through item grammar, and the
**[[bug-tracker|bug ledger]]** was stood up as a machine-countable JSONL.

**v1 sequencing (round-5 roadmap resequence):** `8 → 9 → 10 → 12 → 11`. **[[self-hosting|
Self-hosting]] (Phase 12) is now the v1 pivot**, prioritized *ahead* of the Phase 11 stdlib
longtail (see [[history-reversals-and-deprecations]]). **Round 9 inserted an
[[codegen|LLJIT-productionization]] track *before* Phase 12** (owner-confirmed, `44169c73`):
self-hosting was briefly paused to flip `karac run` to the JIT, then un-paused once the JIT gate
cleared.

| Phase | Name | Depends on | Status at round 5 |
|------|------|-----------|-------------------|
| 1 | Lexer | — | Built; round 3 added `b'A'` byte-char literals |
| 2 | Parser + AST | 1 | Built (recursive descent + Pratt; spans on every node); round 5 added `Dim`/`Shape` params, `...S` variadic shapes, shape literals, `#[target]` |
| 3 | Effect checker / semantic analysis | 2 | Built; round 5 added the **`#[target]` gate** (`E0411`) and **resource-receiver contradiction** (`E0412`) |
| 4 | Interpreter | 3 | Built; contract/refinement interpreter enforcement (r4); round 5 added Tensor / SIMD / BufReader / BufWriter / Mutex interpreter MVPs |
| 5 | Diagnostics | 3, 4 | Round 3 landed the **[[package-management|package manager]]**; still the diagnostics home |
| 6 | Runtime | 4 | Network I/O stack + scheduler (r3), coroutine async (r4); round 5 **sharded the event-loop reactor** (Stage B2/B3), fixed a **Nagle p50** regression, added a **futex `Mutex`** and **AOT channel** lowering |
| 7 | Codegen (LLVM) | 3–6 | **Active heavy work** — round 5: **[[rc-elision|RC elision]]**, **[[simd|SIMD]]**, **[[numerical-stdlib-and-tensors|Tensor]]** lowering, **[[wasm-targets|WASM]]** codegen, string-literal `match` switch-tree, monotone-variable BCE, owned-temp tracking, oversized-enum-payload boxing |
| 8 | Stdlib floor | 4, 7 | **Active** — round 5: **[[fallible-allocation|fallible allocation]]**, BufReader/BufWriter, `Pool.with_health_check`, process `Stdio.Piped`, `try_*` companions; "Phase 8.5 v1 ship readiness" |
| 9 | Verification | 7, 8 | Round-4 surface (**[[design-contracts-and-verification|contracts, refinement, distinct types]]**) intact; round 5 added a **book-snippet test harness** (code blocks as CI-gated test cases) |
| 10 | Targets | 7 | Round-5 **[[wasm-targets|WASM]]** work; round 6 flipped the **[[windows-and-cross-platform|M3 cross-platform-parity gate to DONE]]** (native **Windows IOCP** + AOT build) and added **[[wasm-targets|`std.web.events` / `std.web.time`]]** browser host producers |
| 11 | Stdlib longtail | 8 | Home of the **[[numerical-stdlib-and-tensors|numerical stdlib (Tensor)]]**; round 6 added **[[protobuf]]** (built on **[[metaprogramming|comptime]]**), **`SortedMap[K,V]`**, and a wide **[[stdlib-and-traits|String / integer / char method]]** completeness sweep |
| 12 | **Self-hosting** | 1–8 | **The v1 pivot** — round 5 shipped the **[[self-hosting|Kāra-in-Kāra lexer]]**; round 6 modularized it and ported the **parser** (slices 1 → 3c: expressions, control flow, types, patterns/`match`, item grammar); drives most codegen fixes |

The CHANGELOG's "Next Steps" (written at the root commit) named Phase 3 semantic analysis
as the next step; the project has since advanced well past it into Phases 5–12.

## Round-7 checklist deltas

Round 7's churn spans **Phase 7 codegen**, **Phase 10 targets**, and **Phase 11 stdlib**:

- **Phase 10 — [[gpu-compute|GPU compute shaders]]** (`docs/implementation_checklist/
  phase-10-targets.md`, +38): the GPU gate was **split into tracked slices** with the routing
  decision resolved to **explicit `#[gpu]`** (Option B). New compiler files:
  `src/typechecker/gpu_safe.rs` (+847), `src/typechecker/gpu_call_graph.rs` (+550),
  `src/effectchecker/gpu_effect_gate.rs` (+230), and the `docs/spikes/gpu-wgsl-slice0.md`
  codegen sketch.
- **Phase 7 — first-class fn values + heap-env closures** — a large `src/codegen/closures.rs`
  (+2114), `call_dispatch.rs` (+509), and typechecker/lowering side-tables. See [[codegen]],
  [[bug-tracker]].
- **Phase 11 — [[columnar-data|`Column[T]`]]** — `runtime/stdlib/column.kara` (+147),
  `src/codegen/column.rs` (+2079), `src/interpreter/method_call_column.rs` (+263).
- **Phase 8/11 stdlib primitives** (interpreter slices) — `runtime/stdlib/once.kara` (+151),
  `arena.kara` (+116), `interner.kara` (+92), each with an interpreter `method_call_*.rs`.
- **[[protobuf]]** — `runtime/stdlib/protobuf.kara` (+1169) field-type build-out;
  `tests/protobuf_derive.rs` (+1100), `tests/protobuf_proto.rs` (+442).
- **[[self-hosting|Phase 12]]** — parser slices 3c-iv/3c-v (trait/impl + generics).
- **CI** — a `--features llvm` **codegen-backend clippy** surface gate (`e5efc4dd`,
  `.github/workflows/ci.yml` +50), and a new `bench/wasm_size` track (see
  [[examples-and-benchmarks]]).

## Round-10 checklist deltas

Round 10's churn is dominated by the **Phase 12 self-host front-end** completing its middle-end
and starting its back-end, a broad **Phase 8 stdlib-floor** expansion, the **Phase 10 GPU** close-out,
and three brand-new surfaces:

- **Phase 12 — [[self-hosting]]** — a new **`selfhost/src/resolver.kara`** (name resolution) and
  **`selfhost/src/typechecker.kara`** (~21 slices), each with parser→resolver→typecheck oracles
  green, then a **codegen backend BEGUN**: `selfhost/src/codegen_llvm.kara` (LLVM-IR emitter Slice
  1) after a backend-feasibility spike (`docs/spikes/selfhost-backend-feasibility.md`) returned GO.
- **Phase 8 — [[stdlib-and-traits|stdlib floor]] expansion** — String/char/integer/float method
  families, **f-string format specifiers**, iterator terminals, `ref_eq`, `SortedSet`/`SortedMap`
  codegen, `Map`/`Set.try_insert` codegen, `f16`/`bf16` primitive types, and `std.mem take`.
- **Phase 10 — [[gpu-compute|GPU / Track C]] COMPLETE** — the GPU-LBM cluster (full LBM substep on
  Metal) and the GPU-SLIP cluster with a **device/pipeline cache** (1.9×).
- **Phase 11 — [[numerical-stdlib-and-tensors]]** — `std.embeddings`, `std.simd.math`, `f16`/`bf16`,
  and Tensor `iter_axis`.
- **New surface — security ([[design-secrets]])** — `Secret[T]` / `std.secret` + Book `ch19-secrets.md`.
- **New surface — embedded/MMIO ([[embedded-mmio]])** — `volatile_read`/`volatile_write`, a
  `Hardware` effect, `VolatileCell`, `critical_section`, `fence`, and `Atomic` memory ordering.
- **New surface — [[lsp|language server]]** — `kara-lsp` (6 slices, roadmap Track 3).

## Round-9 checklist deltas

Round 9's churn spans a new **LLJIT-productionization** track, **Phase 10 targets** (GPU + repr(C)
ABI + producer-mode interop), **Phase 6 concurrency** (A2b-2), and **Phase 8/12** tails:

- **LLJIT productionization (inserted before Phase 12)** — Slices 1/3/4/5/6a/6b/6c flipping
  `karac run` to the JIT (`src/codegen/lljit.rs`, `src/bin/karac_jit_runner.rs`,
  `src/repl/jit_runner_client.rs`), a **codegen-e2e-via-LLJIT (run==build parity)** CI leg, and
  the run-vs-build blast-radius fixes. See [[codegen]], [[cli]].
- **Phase 10 — [[gpu-compute|GPU LBM codegen]]** — `src/gpu_wgsl.rs` (+543) and
  `runtime/src/gpu.rs` (+424) for struct-SoA / multi-field-group / control-flow WGSL emission
  (GPU-LBM-1..4, CG-4, GPU-GATE-1).
- **Phase 10 — [[design-unsafe-ffi-and-pointers|additive-interop / producer mode]]** — a big
  `src/cheader.rs` (+1414, C-header emission), `src/cli.rs` (+1106) / `src/manifest.rs` (+186)
  for the `[lib]` table + producer artifacts, `src/codegen/types_lowering.rs` (+427) for the
  per-target `#[repr(C)]` struct ABI, `tests/abi_repr_c_struct.rs` (+513), and a **5-target CI
  matrix** (`.github/workflows/ci.yml` +328).
- **Phase 6 — [[design-concurrency-and-providers|A2b-2 network fan-out]] + parameterized
  resources** — `src/concurrency.rs` (+682), the flagship-demo benchmark regression gate.
- **Phase 8/11 stdlib** — `std.mem`/`std.cmp`, `SortedSet`/`SortedMap` + `Map`/`Set.try_insert`
  codegen (`src/codegen/maps.rs` +396, `runtime/src/map.rs` +259), `Display` for `Option`/
  `Result` (`src/codegen/synth_display.rs` +463), C strings (`runtime/stdlib/nul_error.kara`),
  and **protobuf under codegen** (`src/comptime.rs`, `runtime/stdlib/protobuf.kara`).
- **Phase 5 — dependency pinning** — `src/lockfile.rs` (+78), `src/pubgrub_solve.rs` (+179),
  `src/dep_graph.rs` (+331) for pin-over-catalog / target-deps / workspace discovery.
- **Ownership/drop mechanization** — `src/ownership_oracle.rs` (+1334), `src/drop_differential.rs`
  (+212), `src/bin/drop_fuzz.rs` (+1766) — a drop-soundness fuzzer + executable ownership/drop
  judgment whose oracle↔codegen differential reached **100%**.
- **Mend loop** — `examples/mend/harness/mend_batch.py` / `mend_score.py`, `TASK_FORMAT.md`,
  and a task+oracle corpus ("develop Kāra through the loop"). See [[examples-and-benchmarks]].
- **Book** — a new **"Kāra as a Library"** interop chapter (`ch18-interop.md`).

## Round-8 checklist deltas

Round 8's churn spans **Phase 7 codegen**, **Phase 11 stdlib longtail**, **Phase 10 targets**,
and the **Phase 5 [[package-management|package manager]]**:

- **Phase 11 — [[columnar-data|DataFrame + `Stats.*`]]** — `src/codegen/dataframe.rs` (+1402),
  `src/interpreter/method_call_dataframe.rs`, `src/codegen/stats.rs` (+759),
  `runtime/stdlib/dataframe.kara` / `stats.kara`, and a large `src/codegen/column.rs` /
  `tensor.rs` churn for the [[stdlib-and-traits|Reduce trait]] methods.
- **Phase 11/7 — the shared reduce kernel** — `src/reduce_kernel.rs` (+480),
  `src/codegen/kernel.rs` (+1836), `src/codegen/reduce.rs`; new surface traits
  `runtime/stdlib/reduce.kara` / `elementwise_map.kara` / `elementwise_ord.kara`.
- **Phase 7 — heap-env closures (epic closed) + iterator `.collect()` + BCE** —
  `src/codegen/closures.rs`, the new `src/codegen/bce_length_pin.rs` (+2041, rolling-DP
  length pins), `src/codegen/consume_class.rs` (+524, use-site consumption classifier), and a
  huge `tests/codegen.rs` / `tests/memory_sanitizer.rs` growth.
- **Phase 5 — dependency fetching** — `src/git_fetch.rs` (+470), `src/registry_proxy.rs`,
  `src/registry_extract.rs` (+487), `src/pubgrub_solve.rs` (+464), a new **`registry-proxy/`
  crate** (lib +556 / main +201), `docs/registry-proxy-protocol.md` (+188), and
  `tests/git_fetch_e2e.rs` / `registry_fetch_e2e.rs` / `registry_proxy_wire.rs` /
  `resolver.rs`. New CLI: `karac resolve`.
- **Phase 10 — [[gpu-compute|GPU codegen slice-0]]** — `runtime/src/gpu.rs` (+244),
  `src/gpu_wgsl.rs` (+433), `docs/spikes/gpu-wgsl-slice0.md` — the `wgpu` spine proven on Metal.
- **Phase 12 — [[self-hosting]]** — parser slice 3d-i (attributes + doc comments),
  `selfhost/src/ast_render.kara` (+119).
- **Spikes run + resolved** — `docs/spikes/reduce-elementwise-trait-unification.md` (+856),
  `shallow-depth-parallel-reduction.md` (+213), `caller-retains-param-model.md` (+166),
  `codegen-autovectorization.md` (ruled out), `overflow-check-elision.md` (not worth
  building), `collection-capacity-presizing.md` (scoped). See [[deferred-work]].
- **Book** — new chapter `ch09b-strings-and-bytes.md` + a sweep rewriting ch02/05/09/10/11/
  14/15/16 to the real syntax (CI-gated snippets).

## Round-6 checklist deltas

Round 6's churn is spread across new compiler subsystems rather than one phase:

- **New compiler files** — `src/comptime.rs` (+1258, the **[[metaprogramming|comptime]]**
  core), `src/effect_graph.rs` (+421, the whole-program effect/concurrency graph),
  `src/presize.rs` (+682, loop-bound collection pre-sizing → auto `with_capacity`),
  `src/codegen/borrow_elision.rs` (+597, read-only `v[i]` clone-elision),
  `src/layout_queries.rs` (+454, the **[[per-layout-monomorphization|SoA layout-choice]]**
  query), `src/codegen/synth.rs` (+494) and `src/codegen/shadow.rs` (+203), plus
  `src/rc_fallback_queries.rs`, `src/specialization_queries.rs`, and
  `src/fork_threshold_queries.rs` (the new **[[design-ai-first-compiler|P1.x agent
  queries]]**). Reflection lives in `src/interpreter/reflection.rs` + `comptime_builtins.rs`.
- **New stdlib** — `runtime/stdlib/protobuf.kara` (+623, **[[protobuf]]**),
  `sorted_map.kara`, `web_events.kara` (+605), `web_time.kara`, `exitcode.kara`,
  `io_error.kara`, `var_error.kara`.
- **Runtime** — a large `runtime/src/event_loop.rs` (+2116) and `scheduler.rs` (+411)
  churn for **[[windows-and-cross-platform|Windows IOCP]]** + the detached-eager-reap
  spawn-leak fix; `runtime/src/clone.rs` gained the Unicode String-transform helpers.
- **Phase 12 self-hosting** (+156) — the parser port (`selfhost/src/parser.kara` +2408,
  `ast.kara`, `ast_render.kara`, `token.kara`, `span.kara`).
- **Test growth** — the machine-countable [[bug-tracker|bug ledger]]
  (`docs/bug-ledger.jsonl`), and huge `tests/codegen.rs` / `tests/memory_sanitizer.rs`
  growth (the LeakSanitizer gate); new `tests/comptime*.rs`, `tests/protobuf*.rs`,
  `tests/selfhost_parser*.rs`, `tests/layout_queries.rs`, `tests/module_graph.rs`,
  `tests/relay_bench.rs`.

## Round-5 checklist deltas

The largest round-5 churn is again in **Phase 7 codegen** (~217 checklist lines) and its
sibling surfaces: **[[rc-elision|RC elision]]** (`src/ownership/elision.rs`, +3521),
**[[numerical-stdlib-and-tensors|Tensor]]** codegen (`src/codegen/tensor.rs`, +3360),
**[[simd|SIMD]]** (`src/simd_report.rs`, +853), and **[[wasm-targets|WASM]]**
(`src/wasm_glue.rs` +2019, `src/wasm_exports.rs` +683, `src/wit.rs` +574,
`src/componentize.rs` +340, `src/codegen/cabi.rs` +523, `src/target.rs` +518). **Phase 10
targets** (+210) and **Phase 12 self-hosting** (+208, new) are the roadmap's new front.
**Phase 8** added **[[fallible-allocation|fallible allocation]]** (`src/fallible_alloc.rs`,
`src/typechecker/alloc_rejection.rs` +414). New runtime files: `runtime/src/mutex.rs`,
`runtime/src/channel.rs`, `runtime/src/bounded_channel.rs`, `runtime/src/seq_scheduler.rs`,
`runtime/src/wasm_threads_scheduler.rs`, `runtime/src/wasm_alloc.rs`, `runtime/src/fatal.rs`,
`runtime/src/clone.rs`. A **memory-sanitizer CI job** (ASAN + Linux LeakSanitizer, Tier 2)
became the leak gate — see [[bug-tracker]].

## Round-4 checklist deltas

The largest round-4 churn is again in **Phase 7 codegen** (~222 checklist lines: the
**[[codegen|JIT path]]**, the **[[design-runtime-phases|LLVM-coroutine transform]]** (A2),
contract/refinement/distinct-type lowering, DWARF debug-info) and **Phase 6 runtime**
(~206 lines: WebSocket/TLS handshake hardening, HTTP/2, scheduler dispatch, backpressure).
**Phase 8 stdlib-floor** (~176 lines: HTTP client + HTTP/2 + client-TLS + tracing +
backpressure + `#[unstable]`). **Phase 9 verification** (~109 lines) is now a real feature
surface — see [[design-contracts-and-verification]]. New crate files:
`src/codegen/coro.rs`, `src/codegen/lljit.rs`, `src/codegen/contracts.rs`,
`src/codegen/refinement.rs`, `src/codegen/debug_info.rs`, `src/codegen/test_assert.rs`,
`src/test_jit_dispatch.rs`, `src/test_main_synth.rs`, `src/bin/karac_jit_runner.rs`,
`runtime/src/scheduler.rs`, `runtime/src/emutls.rs`, `runtime/src/tracing.rs`, and the
`bounded_channel.kara` / `semaphore.kara` / `rate_limiter.kara` / `process.kara` /
`mutex.kara` stdlib files.

## Round-3 checklist deltas

The largest round-3 churn was in **Phase 7 codegen** (~303 lines: user-`Drop`,
`defer`/`errdefer`, `Atomic[T]`, `E_CONCURRENT_*_STRUCT`), **Phase 6 runtime** (~150 lines:
the [[networking|TCP/TLS/WS/File stack]], scheduler, structured concurrency), and **Phase 5
diagnostics** (~128 lines, dominated by the new **[[package-management|package manager]]**).
**Phase 8 stdlib-floor** (~45 lines) shipped the `File` handle and completed the
module-level `let` (mod-let) P0. **Phase 4 interpreter** (~22 lines) added the `test { }`
block. New crate files: `runtime/scheduler.rs`, `runtime/file.rs`, `runtime/tls.rs`.

## Notable Phase-4 (interpreter) items

- **item 117** — runtime effect tracking: **skipped** (see [[deferred-work]]).
- **item 119** — weak-reference runtime behavior: implemented.
- **item 129** — `dbg()` task-id tagging + structured output: implemented.
- **item 131** — type-inference: check-mode pushdown, fresh-metavar instantiation,
  bidirectional subsumption with function-type variance.
- NLL drop placement + a **unified drop+defer cleanup stack** with shared-struct interior
  mutability.

## Phase 8 "Themes" and slices

Phase 8 stdlib work is organized into **Themes 1–7** and lettered/numbered **Slices**
(e.g. Slice A/B/C/D/E/F, Slice CP/DP/PB/LB). Theme 6 (providers, closed), Theme 1
(`Slice[T]` borrow-tracking parity), Theme 4 (args-aware impl-table key shape). See
[[stdlib-and-traits]] and [[codegen]] for specifics.

## Cross-phase workflow

Work is tracked in per-phase checklist files, `docs/deferred.md` (deferrals), and
gitignored WIP lists (`wip-list1.md` / `wip-list2.md`) plus a gitignored `bugs` area.
Brainstorm decisions graduate from `brainstorming/archive/vNN.md` files — see
[[history-reversals-and-deprecations]].
