---
type: index
title: Kāra knowledge base — index
updated_round: 10
slice: 39e849d1..349066162
---

# Kāra — knowledge base index

**Kāra** is a Rust-inspired systems programming language and its compiler (`karac`),
implemented in Rust. Source files use the `.kara` extension. Repository: `karalang/kara`.

This knowledge base tracks the project's design decisions, implementation phases,
bug tracker, and how things have changed over time.

## Map

- [[kara-overview]] — what Kāra is and its headline design bets
- **Design decisions**
  - [[design-effect-system]] — six effect verbs, user resources, effect inference
  - [[design-ownership]] — tiered ownership, no lifetimes, RC fallback
  - [[design-concurrency-and-providers]] — auto-concurrency, `par`, providers, channels, reductions
  - [[design-data-layout]] — logical struct vs physical layout, opt-in SoA
  - [[design-adt-and-pattern-matching]] — enums, exhaustive matching, non_exhaustive
  - [[design-contracts-and-verification]] — contracts, refinement types, distinct types (Phase 9, new round 4)
  - [[design-ai-first-compiler]] — JSON diagnostics, diagnostic classes, compiler query API
  - [[design-runtime-phases]] — v1 blocking → v1.1 event loop → v2 hybrid
  - [[design-unsafe-ffi-and-pointers]] — unsafe surface, FFI unions, raw pointers, strict provenance, additive interop / producer mode + `#[repr(C)]` struct ABI (round 9)
  - [[design-generics-and-impl-trait]] — const generics, GATs, `impl Trait`, Dim/Shape kinds, variance
  - [[design-secrets]] — `Secret[T]`, constant-time `ct_eq`, redacted Debug/Display, `E_SECRET_TRAIT_FORBIDDEN`, Book ch19 (new round 10)
  - [[fallible-allocation]] — the `panic_on_alloc_failure` profile + `try_*` companions (new round 5)
  - [[attributes]] — lint levels, `deprecated`, `non_exhaustive`, `must_use`, `#[target]`, `#[inline]`/`#[cold]`, derives
- **Implementation**
  - [[implementation-phases]] — the 10 → 12-phase plan and dependencies
  - [[compiler-pipeline]] — lexer → parser → resolver → checkers → interpreter/codegen
  - [[codegen]] — LLVM backend, monomorphization, drop, RC elision, SIMD/tensor, BCE, auto-par
  - [[metaprogramming]] — comptime evaluator, reflection, AST emission, derive desugaring (new round 6)
  - [[protobuf]] — proto3 wire format + `#[derive(Message)]` on comptime; full field-type matrix (round 7)
  - [[gpu-compute]] — GPU compute: `#[gpu]` + `GpuSafe`; element-wise → LBM kernels on Metal — struct-SoA dispatch, multi-field groups, control flow (round 9)
  - [[columnar-data]] — `Column[T]` Arrow columns + `DataFrame` + `Stats.*` + shared reduce kernel (round 8)
  - [[per-layout-monomorphization]] — SoA `layout` blocks across function boundaries (new round 6)
  - [[windows-and-cross-platform]] — native Windows IOCP, M3 cross-platform-parity DONE (new round 6)
  - [[rc-elision]] — count-free RC chains, the density-benchmark backbone (new round 5)
  - [[simd]] — portable `Vector[T, N]`, masks, gather/scatter, WASM SIMD-128 (new round 5)
  - [[numerical-stdlib-and-tensors]] — shape-typed `Tensor[T, Shape]` (Phase 11, new round 5)
  - [[wasm-targets]] — WASI / browser / component-model / threads, `#[target]`, host fn, `std.web` events (new round 5)
  - [[self-hosting]] — the Kāra-in-Kāra lexer + parser + **resolver + typechecker + codegen-backend** port (Phase 12, the v1 pivot; front end complete round 10)
  - [[embedded-mmio]] — `volatile_read`/`volatile_write` + `Hardware` effect, `VolatileCell[T: Copy]`, `critical_section`, `fence`, `Atomic[T]` memory ordering (new round 10)
  - [[lsp]] — the `kara-lsp` language server: diagnostics, hover, go-to-def, symbols, formatting, references (6 slices, roadmap Track 3, new round 10)
  - [[stdlib-and-traits]] — prelude types, baked traits, `Reduce`/`ElementwiseMap` traits, iterators, module `let`
  - [[networking]] — TCP / TLS / WebSocket / File on the event loop (new round 3)
  - [[package-management]] — resolver, lockfile, git deps, `registry-proxy` crate, PubGrub solving (round 8)
  - [[cli]] — the `karac` command surface; **`karac run` is now JIT-default** (`--interp` escape, round 9), `karac query`/`catalog`/`migrate`, `--target`
  - [[jupyter-kernel]] — the `karac` Jupyter kernel + Python shim, notebook magics
  - [[playground]] — wasm32 target + browser playground
  - [[examples-and-benchmarks]] — Parallax, Mend, ws_idle_holder density benchmark, Tangle, SSR
- **Change tracking**
  - [[bug-tracker]] — bug ledger and notable fixes
  - [[history-reversals-and-deprecations]] — renames, reversals, supersessions, brainstorm graduations
  - [[deferred-work]] — deferred and empirically-falsified approaches

## Current status (end of round 10)

Positioning was **backend-first v1** (v64); round 3 **reframed** the README's language to a
**"Production-Ready skeleton"**, round 4 sharpened the AI positioning and added a **dual
MIT/Apache-2.0 license** (repo `karalang/kara`), and **round 5 rewrote the README around a
benchmark headline** (idle-connection density). The interpreter, parser, type checker,
effect checker, ownership analysis, and LLVM codegen are all substantially built.

**Round 10 carried the self-hosted front end from lexer+parser to a full resolver +
typechecker (+ a begun codegen backend), finished the GPU LBM kernels on Metal, and shipped
a large Phase-8 stdlib expansion plus three new surfaces (secrets, embedded MMIO, an LSP):**

- **[[self-hosting|Self-host front end complete]]** — round 9's headline **open** bug
  (`B-2026-07-10-4`, the item/type parser crash) is **fixed** (`1b5f543`); **all three parser
  oracles are green**. Beyond that, the round ported the **Resolver** (name-resolution core +
  program-level two-pass), the **TypeChecker** (~21 slices, inference through
  exhaustiveness/branch-consistency), and **began the codegen backend** (LLVM-IR emitter
  Slice 1, gated by a backend-feasibility spike that returned **GO**). The self-hosted front
  end now spans lexer → parser → resolver → typechecker → (begun) codegen.
- **[[gpu-compute|GPU LBM kernels complete on Metal]]** — the **GPU-LBM** cluster (helper fns
  as WGSL, stencil/`stream`) and the new **GPU-SLIP** cluster (f32 collide, full stream, the
  double-buffered **substep** — the whole LBM step on the device) are COMPLETE; **GPU-SLIP-4a**
  caches the wgpu device + compiled pipelines for a **1.9x** speedup. `B-2026-07-10-6`/`-7`
  (round 9's open GPU-under-JIT and SoA-literal bugs) are both **fixed**.
- **[[stdlib-and-traits|Phase-8 stdlib expansion]]** — String/char/integer/float method
  families, **f-string format specifiers** (`f"{x:spec}"`), **iterator terminals**
  (`fold`/`sum`/`reduce`/`for_each`/`any`/`all`/`count` on fused chains + materialized iters),
  `ref_eq`, **`SortedSet`/`SortedMap` codegen**, `Map`/`Set.try_insert` codegen, `f16`/`bf16`
  types, `Option`/`Result.map`, and `std.mem take`. Method resolution on `Option`/`Result` was
  **tightened** (an unknown method is now a compile error, not a silent runtime failure).
- **New surfaces** — a **[[design-secrets|secret-handling surface]]** (`Secret[T]`, constant-time
  `ct_eq`, redacted Debug/Display, Book ch19); an **[[embedded-mmio|embedded/MMIO surface]]**
  (`volatile_read`/`volatile_write` + a `Hardware` effect, `VolatileCell[T: Copy]`,
  `critical_section`, `fence`, `Atomic[T]` memory ordering); and a **[[lsp|`kara-lsp` language
  server]]** (6 slices: diagnostics, hover, go-to-def, symbols, formatting, references).
- **[[design-concurrency-and-providers|Concurrency]]** — **A2b-2 network auto-parallelization
  COMPLETE**; **`par {}` block bindings now escape** to the enclosing scope; and an
  **auto-parallelization correctness bug** was root-caused and fixed (the auto-parallelizer had
  raced sequential `mut ref self` calls sharing mutable state — the true cause of the long
  self-host parser SEGV `B-2026-07-09-12`, a documented reversal of the earlier
  "construction/drop memory bug" diagnosis).
- The [[bug-tracker|ledger]] added **~90 entries** and **closed every high-severity round-9
  open bug**. The new **open set is one `high`** (`B-2026-07-14-15`, a `Map[K, heap-value]`
  `.get().unwrap()` double-free) plus a handful of `low` feature-gaps/ergonomics
  (iterator-adaptor & `iter_mut` for-loop lowering, `Vec[Vec].get().unwrap()` type-loss,
  slice-pattern ergonomics, unimplemented `Option`/`Result` combinators) and the carried perf
  gap `B-2026-07-10-5`. Round 10 also surfaced **shared/RC drop-completeness** and
  **generic-monomorph heap-type-threading** as two large fix families, and drove an **arm64
  memory-sanitizer CI leg** (an arm64-only RC leak x86 CI structurally couldn't catch).

### Round 9 (prior) — JIT-default, GPU-LBM on Metal, additive interop, self-hosting un-paused

**Round 9 flipped `karac run` to JIT-default, turned the GPU spine into LBM kernels on Metal,
built the additive-interop / Kāra-as-a-library surface, and un-paused self-hosting:**

- **[[cli|`karac run` is now JIT-default]] (LLJIT productionization, Slice 6c)** — `karac run`
  compiles and **JIT-executes** by default, with an **`--interp`** escape to the tree-walk
  interpreter. `karac test` / `karac repl` were already JIT-default (round 4); round 9 finished
  the surface. This is the deliberate close of the **run-vs-build divergence** — the interpreter
  becomes a dev/debug backend, codegen becomes the oracle. It **resequenced the roadmap**
  (LLJIT-productionization inserted *before* Phase 12) and its **blast-radius sweep** surfaced
  ~16 codegen gaps (`B-2026-07-08-*`) that a lenient interpreter had hidden. See
  [[codegen]], [[history-reversals-and-deprecations]].
- **[[gpu-compute|GPU: element-wise → LBM kernels on Metal]]** — CG-4 lowered a **struct-SoA
  `gpu.dispatch`** (multi-buffer, `karac_runtime_gpu_map_multi`) and **GPU-GATE-1 runs on
  Metal**; **GPU-LBM-3** put **multi-field SoA groups** on the device (per-field interleave)
  and **GPU-LBM-4** added **value control flow** in `#[gpu]` kernels (`if`→WGSL `select`).
- **[[design-unsafe-ffi-and-pointers|Additive interop / Kāra as a library]]** — **producer-mode
  library artifacts** (a `[lib]` manifest table, static + dynamic, Windows too), **raw-pointer
  instance methods** (`.offset`/`.read`/`.write`), **auto-boxing + auto-destructors** at the C
  boundary, `#[repr(C)]` **enum** crossing, owning **`CString`** + zero-copy **`CStr`**, and the
  `forget[T]` handoff primitive. Backed by a full **`#[repr(C)]` struct-by-value ABI**
  (`B-2026-07-09-2`) — AArch64 AAPCS, x86-64 SysV, Windows x64, incl. >16B indirect/sret — that
  a **5-target CI matrix** now gates (it caught a silent arm64 miscompile).
- **[[design-concurrency-and-providers|A2b-2 auto-parallel network I/O]]** — independent network
  calls (associated openers, method calls on distinct receivers, variable-arg) auto-parallelize,
  gated by finer **parameterized-resource partition keys** (a `Network[addr]` is not a
  `Network[other]`).
- **[[self-hosting|Self-host parser un-paused]]** (the LLJIT gate cleared) — cross-module enum
  variants / prelude shadowing / qualified-struct construction fixed; a large multi-session
  parser drop/ownership investigation closed the control-flow-expression crash (an auto-par
  serialization bug) — the **expression oracle is green**; the **item/type oracle residual**
  (`B-2026-07-10-4`) is the round's headline **open** bug.
- **[[stdlib-and-traits|Stdlib / traits]]** — `std.mem` (`swap`/`replace`/`take`), `std.cmp`
  (`min`/`max`/`clamp`), `.cmp() -> Ordering` for derived-`Ord`, `Display` for `Option`/`Result`,
  `SortedSet`/`SortedMap` **codegen**, `Map`/`Set.try_insert`, `unwrap_err`/`expect_err`,
  `#[track_caller]` emitters, and **`#[derive(Message)]` protobuf now compiles under codegen**.
- **[[package-management|Dependency pinning]]** — lockfile-pin-over-catalog with a
  `W_DEPENDENCY_YANKED` warning, `[target.<triple>.dependencies]`, upward workspace-root
  discovery, and dep-resolution diagnostics surfaced in `run`/`check`.
- The [[bug-tracker|ledger]] added **~90 entries** and **closed all five of round 8's open
  bugs**. The new **open set is eight entries — two `high`** (`B-2026-07-10-4` self-host
  item-parser crash; `B-2026-07-10-7` SoA-literal field-read segfault), the rest `low`
  (perf / ergonomics / a GPU-under-JIT gap). Two diagnoses were **reversed** as unsound (the
  two-pointer BCE `B-2026-07-10-5`; a `Vec.filled` fill peephole).

### Round 8 (prior) — heap-env closure epic, GPU slice-0, columnar data + traits

- **[[codegen|Heap-env closure epic]] CLOSED** — `B-2026-06-22-2`, round 7's headline
  **open `high`** bug, is **fixed** (`be2ef68e`) after ~30 slices: escaping capturing closures
  now get a **reference-counted heap env**, RC-accounted across return / store / copy / move /
  escape / arg-pass / reassignment. **No `high`-severity bug is open at end of round 8.**
- **[[gpu-compute|GPU codegen]]** advanced from spike to a working **slice-0** — a `wgpu`
  compute spine **proven on Metal**, WGSL codegen, and an end-to-end `gpu.dispatch` (Phase 10).
- **[[columnar-data|`DataFrame` + `Stats.*`]]** joined `Column[T]` (Phase 11), all riding a new
  **shared reduce kernel** (`src/reduce_kernel.rs`) also used by `Column`/`Tensor` reductions.
- **[[stdlib-and-traits|Trait system]]** matured broadly — `Reduce`/`ElementwiseMap`/
  `ElementwiseOrd` surface traits, inherited **default methods**, user impls on **primitive
  scalars** and over **builtin containers**, **generic-bound dispatch** + monomorphization, and
  derived-`Ord` struct/enum **ordering** (`<`/`<=`/`>`/`>=`, `.sort()`).
- **[[package-management|Dependency fetching]]** — **git deps** (commit-pinned in `kara.lock`),
  a new **`registry-proxy` crate** + wire protocol (caching / retrying / auth), **PubGrub**
  version solving, yank awareness, and a new **`karac resolve`** command.
- **[[codegen|Iterator `.collect()`]]** lowering (map/filter/enumerate/passthrough adaptors) and
  a rolling-DP **length-pin bounds-check elision** (kata #62 → C parity).
- The [[bug-tracker|ledger]] added **~120 entries**, dominated by a **run-vs-build divergence**
  sweep (silent miscompiles where `karac run` and `karac build` disagreed). At end of round 8
  the **open set was five entries — none `high`** (two `med`: no-`u64`-interpreter-model
  `B-2026-07-04-8`, for-loop-element-move double-free `B-2026-07-04-17`) — **all five closed in
  round 9**.

### Round 7 (prior) — GPU compute, columnar data, first-class function values

- **[[gpu-compute|GPU compute shaders]]** (Phase 10) — the routing decision resolved to
  **explicit `#[gpu]`** (Option B), and a full **front-end validation layer** landed: the
  **`GpuSafe`** structural marker trait, a call-graph gate (no recursion / no generic
  non-`#[gpu]` callees / no host-capturing closures), and a **GPU effect gate** (host effects
  and explicit panics rejected). WGSL codegen is still a spike.
- **[[codegen|First-class function values]]** completed across arg / let / return /
  struct-field / `Vec[Fn]` positions (closing round 6's open `B-2026-06-20-1`), and a large
  **heap-env closure epic** (`B-2026-06-22-2`, ~15 slices this round) began a reference-counted
  heap env for escaping capturing closures — round 7's headline open bug, **closed in round 8**.
- **[[columnar-data|`Column[T]`]]** (Phase 11) — a **nullable Arrow-buffer column** with
  **SQL three-valued-logic** arithmetic/comparison, wired through typecheck + interpreter +
  codegen.
- **[[protobuf|Protobuf]]** grew its **full proto3 field-type matrix** (nested / repeated /
  enum / map / float-double / oneof / sint-fixed via field-attribute reflection).
- **[[stdlib-and-traits|New stdlib primitives]]** (interpreter slices) — `OnceLock`/`OnceCell`
  write-once cells, `Arena`/`ArenaRef` bulk allocation, `Symbol`/`Interner` string interning.
- **[[self-hosting|Self-host parser]]** reached **trait/impl items + generic type params**.

### Round 6 (prior) — cross-platform maturity, metaprogramming, self-hosted parsing

- **[[windows-and-cross-platform|Native Windows IOCP]]** (event loop + TLS + AOT build)
  flipped the **M3 cross-platform-parity gate to DONE** — Linux + macOS + Windows. A
  1M-connection soak root-caused a platform-agnostic spawn-leak (detached-eager-reap fix).
- **[[metaprogramming|Compile-time metaprogramming (comptime)]]** — a new compiler layer:
  a compile-time evaluator, `Type` as a first-class value + reflection, AST emission, and
  **derive desugaring** — which carries the new **[[protobuf|protobuf (proto3)]]** stdlib.
- **[[per-layout-monomorphization|SoA across function boundaries]]** — per-layout
  monomorphization on a LayoutId axis, proven by the Slipstream LBM dogfood.
- **[[self-hosting|Self-host parser port]]** — the front end was modularized and the
  **parser** ported (slices 1 → 3c: expressions, control flow, types, patterns/`match`, item
  grammar), on the direct `shared enum` AST model.
- **[[stdlib-and-traits|Stdlib completeness]]** — cross-kata audits filled String / char /
  integer method families, `SortedMap[K,V]`, `binary_search`, and `Set[Vec]` content dedup.
- **[[examples-and-benchmarks|Browser + systems dogfoods]] shipped** — Fathom, Plume, Iris,
  Cartographer (on `std.web` event/timer producers), Relay (reverse proxy + wrk benchmark),
  Slipstream, Weave — surfacing most of the round's codegen/wasm/concurrency fixes.
- **[[bug-tracker|Machine-countable bug ledger]]** — `bugs.md` was retired for a
  ~152-entry `bug-ledger.jsonl` + generated readable view, with lint/curve tooling.

### Round 5 (prior) — performance, portability, and self-hosting

- **[[self-hosting|Self-hosting]]** (new **Phase 12**): a **Kāra-in-Kāra lexer** with a
  differential oracle, plus an LLVM-C FFI proof. The roadmap **resequenced self-hosting as
  the v1 pivot** (`8 → 9 → 10 → 12 → 11`). This drove most of the round's codegen bug fixes.
- **[[rc-elision|RC elision]]** (phases A → D): count-free reference-counted chains — the
  backbone of round 5's **idle-connection-density benchmark leadership**.
- **[[simd|Portable SIMD]]** — `Vector[T, N]` with masks, gather/scatter, and WASM SIMD-128.
- **[[numerical-stdlib-and-tensors|Numerical stdlib]]** (Phase 11): shape-typed
  **`Tensor[T, Shape]`** with `Dim`/`Shape` generic kinds.
- **[[wasm-targets|WASM targets]]** (Phase 10): WASI, browser, the **Component Model**,
  opt-in **wasm-threads**, a **`#[target]`** effect gate, and **host fn** — enabling **SSR**.
- **[[design-concurrency-and-providers|Concurrency]]**: **auto-parallel I/O** (independent
  blocking calls / `sleep_ms` overlap), **AOT Channel/BoundedChannel** lowering, a
  **spinlock → futex `Mutex`**, `collect_all` gathers, and `Atomic` `swap`/`CAS`/RMW.
- **[[fallible-allocation|Fallible allocation]]** (Phase 8): a `panic_on_alloc_failure`
  profile with `try_*` companions and three rejection errors.
- **[[examples-and-benchmarks|Benchmarks]]**: a full comparator cohort for `ws_idle_holder`
  (Java/Netty, Go, Node, Phoenix, .NET) and a Nagle p50 fix (45 → 1.63 ms at 250K).

Active heavy work remains in [[codegen]] (Phase 7) and the [[stdlib-and-traits|stdlib
floor]] (Phase 8). Round 4's verification surface ([[design-contracts-and-verification|
contracts / refinement / distinct types]], Phase 9), the [[codegen|JIT path]], and the
[[design-runtime-phases|LLVM-coroutine async transform]] remain in place; round 5 added a
**book-snippet test harness** to Phase 9 but did not otherwise touch them.
