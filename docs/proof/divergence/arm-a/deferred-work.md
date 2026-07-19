---
type: deferred
title: Deferred and blocked work
updated_round: 10
---

# Deferred and blocked work

Tracked in `docs/deferred.md` (a large deferral ledger) plus commit close-outs. Distinct
from resolved [[bug-tracker|bugs]] and from adopted [[history-reversals-and-deprecations|
design changes]]. A new **`[->]` "explicitly deferred"** tracker marker was introduced this
round to make deferrals visible in the checklists.

## Deferred

### Round 10 deferrals

- **Iterator for-loop adaptor lowering** (`B-2026-07-14-8`, open) — `for x in v.iter().skip(2).take(3)`,
  `enumerate` single-var, `zip`, `chain`, `flat_map`, `chunks`, `windows`, `cycle`, `scan`,
  `peekable`, `inspect` — these now **loud-bail** in codegen (`B-2026-07-14-7`) instead of silently
  skipping the loop; proper lockstep/materialize lowering deferred. The interpreter handles them.
- **`for x in xs.iter_mut()`** (`B-2026-07-14-10`, open) — mutable iteration unimplemented
  end-to-end (needs a mut-ref element bind + deref-store); both backends loud-bail
  (`B-2026-07-14-9`); workaround is the index loop. See [[stdlib-and-traits]].
- **Unimplemented `Option`/`Result` combinators** (`B-2026-07-14-6`, open) — `map_err`, `map_or`,
  `take`, `err`, `ok`(Result), `flatten`, `get_or_insert`, `and_then`(Result), `or`/`or_else` —
  now cleanly compile-time-rejected (after the method-resolution tightening `B-2026-07-14-5`),
  awaiting end-to-end implementation.
- **Generic-dim Tensor ergonomics** (`B-2026-07-13-5` gaps A/B, partial) — a tensor reduction on a
  chained/non-identifier receiver (`a.zip_with(b,f).sum()`) and a generic shape param `D` in a
  function-BODY type annotation stay deferred; gap C (a `ref Tensor` arg) was fixed, shipping the
  1-D `std.embeddings` core. See [[numerical-stdlib-and-tensors]].
- **Reduced-precision float arithmetic/backend** — `f16`/`bf16` now lex as type-name identifiers,
  but their arithmetic/casts/backend are a **Phase-7** follow-up; F16/BF16 wrappers + `Tensor[f16]`
  are scoped follow-ups. See [[history-reversals-and-deprecations]].
- **Two-pointer BCE** (`B-2026-07-10-5`) — stays deferred; independently re-confirmed as having no
  sound lever (`l < n` is a semantic invariant no `assume` can supply). See [[codegen]].

### Earlier deferrals

- **`Vec.new` + push-loop → `Vec.filled` lowering** (Phase 7) — still deferred (re-confirmed
  round 2). (Round 9's `Vec.filled` fill peephole was **reversed as unsound**.)
- **`Vec.sort_by` FFI-boundary comparator inlining** — deferred; "Path A+" was
  empirically falsified (2026-05-14). Shipped path uses an FFI-bridge thunk.
- **Recursive Drop tuple-destructure** — struct-field/Set/Map recursive drop shipped
  (slices α/β/γ), but a tuple-destructure blocker deferred those slices' remainder.
- **`impl Trait` Phase 8 effect-check** (round 2) — the effect-check follow-up on the
  `impl Trait` epic was split out and deferred. See [[design-generics-and-impl-trait]].
- **TAIT** (round 2) — type-alias `impl Trait` shipped only as a **v1 stub**.
- **Windows IOCP + codegen sub-item** (round 2) — a line-17 sub-item for the
  [[design-runtime-phases|event loop]] tracked for later.
- **Package-manager v1.1.x carve-outs** (round 3) — several lines that shipped their v1
  scope (registry proxy 851, build cache 861, update 843) recorded **v1.1.x carve-outs**
  for out-of-scope sub-features. See [[package-management]].
- **Roman-kata codegen follow-ups** (round 3) — the `docs/investigations/roman_kata_codegen.md`
  investigation filed several **deferred follow-ups** surfaced by the kata. See
  [[examples-and-benchmarks]].
- **Small-string optimization (SSO)** (round 5) — scoped as a **corpus-wide allocation
  lever** (`docs/spikes/small-string-optimization.md`) and **deferred** as a campaign.
- **Alias-scope metadata** (round 5) — filed **deferred**: measured **~0 runtime gain, ~132 B
  per kernel size cost only**; plain param `noalias` measured inert (only `mut ref` noalias
  kept). See [[rc-elision]].
- **Non-temporal-store lever** (round 5) — filed as a deferred codegen lever alongside the
  alias-scope benchmark scaffold.
- **`StringSlice` borrowed-view type** (round 5) — was recorded as **v2**; **round 6
  graduated the base type to v1** (`slice`/`find` + source-pinned return). Only
  **split-views stay v2**. See [[history-reversals-and-deprecations]] and Resolved, below.
- **Escaping capturing closures** (round 7 open → **RESOLVED round 8**, `B-2026-06-22-2`) —
  the [[codegen|heap-env closure epic]]. A capturing closure that outlives its frame originally
  read freed stack memory (silent miscompile). **No longer deferred:** ~30 slices closed the
  whole epic (`be2ef68e`) — the reference-counted heap env now covers *every* place, including
  the previously-deferred owner **copy** / by-value owner **arg-pass** / field & element
  **reassignment**. See [[bug-tracker]], [[codegen]], and Resolved, below.
- **GPU WGSL codegen** — the [[gpu-compute|`#[gpu]` front end]] landed round 7; round 8 landed
  device-backend slice-0 (element-wise-map on Metal); **round 9 advanced to LBM kernels** —
  struct-SoA `gpu.dispatch` (GPU-GATE-1 on Metal), multi-field SoA groups, and value control
  flow in `#[gpu]` kernels (GPU-LBM-1..4). Remaining GPU gaps: a **SoA collection-literal
  read-back** segfaults under codegen (`B-2026-07-10-7`, open high — the kata uses `push`), and
  **`gpu.dispatch` won't run under the JIT-default `karac run`** (`B-2026-07-10-6`, open low —
  the `gpu` feature isn't in the JIT-runner rlib). See [[gpu-compute]], [[bug-tracker]].
- **Windows `#[repr(C)]` execution CI leg** (round 9) — the Windows-x64 struct-ABI **classifier**
  landed and is signature-verified on Linux, but the **native Windows execution** CI leg is
  **deferred**: `llvm-sys 181` needs `llvm-config.exe`, which the upstream LLVM Windows installer
  omits (`B-2026-07-09-8`). Reinstate when upstream ships it. See [[design-unsafe-ffi-and-pointers]].
- **`Vec.new()`+push-fill → `Vec.filled` peephole** — the auto-peephole was **tried and
  reverted** in round 9 (regressed kata #63's BCE); only the safe **`calloc`-backed
  `Vec.filled(n, 0)`** lowering (static all-zero fill, matching `vec![0; n]`) is retained. The
  residual per-call small-buffer allocator gap is a macOS-only characterization (`B-2026-07-08-7`;
  kāra wins on Linux/glibc). See [[history-reversals-and-deprecations]], [[bug-tracker]].
- **`Column[T]` codegen for non-scalar element types** — the round-7
  [[columnar-data|`Column[T]`]] Arrow lowering covers the scalar/3VL surface; broader element
  types are follow-ons.
- **[[per-layout-monomorphization|SoA]] heap-field read-back** (round 6) — reading a
  heap-bearing SoA field back as a method receiver / index base (`grid[i].name.len()`,
  `grid[i].data[k]`) needs the field's address; the per-field read materializes a
  non-addressable value. The next SoA slice. (Push/store/whole-element write + drops landed.)
- **`SortedMap` codegen** (round 6) — `SortedMap[K,V]` is **interpreter-only in v1** (no
  B-tree runtime); a `SortedMap.new()` construction emits an honest interpreter-only error,
  same v1 status as `SortedSet`. See [[stdlib-and-traits]].
- **`char.to_digit` codegen** (round 6, **partial** — `B-2026-06-19-13`) — typecheck +
  interpreter done; codegen emits an honest "works under `karac run`" error (shares the
  Option construction lowering with the float→int follow-on).
- **Sequential WASM `recv` yield** (round 5) — sequential WASM `recv` deferred; the threaded
  case folded into producers. See [[wasm-targets]].
- **Coroutine cancellation trigger** (round 4) — the cooperative-cancellation *mechanism*
  landed (cancel-check shim + destroy-edge slot-signal), but the **trigger half** is tracked
  as remaining. See [[design-runtime-phases]], [[codegen]].
- **HTTP streaming** (round 4) — keep-alive + chunked were verified; a **streaming follow-on**
  (phase-8 line 16) was split out. See [[networking]].
- **Always-JIT follow-ups** (round 4) — migrated out of a WIP doc into tracker entries as the
  [[codegen|JIT path]] flipped to default; the wip doc was deleted.
- **Incremental module-env reuse** (round 4) — tracked as its own Phase-7 follow-on entry
  (test-JIT compile-time optimization).
- **Residual TLS-bulk follow-up** (round 4) — filed alongside the binary-size regression fix.
- **Collection-capacity pre-sizing** (round 8, **partial**) — scoped as a codegen lever
  (`docs/spikes/collection-capacity-presizing.md`, `26b9342f`). The highest-value target,
  `.map().collect()`, did not even compile under codegen, so the correctness fix
  (`B-2026-07-03-25`) had to precede any pre-sizing. Some presize work landed — pre-size counted
  fills over `<=` loops and cumulative reads (`23287e52`), count pre-loop seed pushes into the
  fill reservation (`e9c33a8d`) — but the general `.map().collect()` pre-sizing target remains
  open. See [[codegen]].

## Skipped

- **Phase-4 item 117 — runtime effect tracking** — skipped (closed as skipped).

## Blocked on predecessors

- **`with_provider` GAP-N** — reframed as a codegen-only gap; plan drafted.
- **`impl Option[Ordering]`** — partial-comparison helpers reframed during WIP triage.

## Resolved since round 1 (no longer deferred)

- **`get_unchecked` (source-level)** — was blocked on the unsafe-enforcement predecessor;
  **now shipped** as part of source-level bounds-check elision (`get_unchecked`, for-range,
  `Slice[T]` reads/writes). See [[codegen]].
- **`ptr.container_of`** — was soft-blocked on line 511; **now shipped** (line 509). See
  [[design-unsafe-ffi-and-pointers]].
- **`bfs_sieve` residual leak** — the main cleanup fix plus follow-up gap work closed it.
- **`%rc` notebook magic** (round 2) — was deferred to v1.1.x; **un-deferred and shipped in
  round 3** (2026-05-20). See [[jupyter-kernel]], [[history-reversals-and-deprecations]].
- **`BufReader`** — was a round-3 tracker entry (not yet built); **round 5 shipped a
  `BufReader[R]` interpreter MVP** (`lines()`, `fill_buf`/`consume`) plus a `BufWriter[W]`.
  See [[stdlib-and-traits]].
- **Coroutine cancellation trigger** — the round-4 mechanism landed; **round 5 added a
  `defer-on-cancel` slice** and nested cancel-cascade + completion-wins verification. See
  [[design-concurrency-and-providers]].
- **`StringSlice` (base type)** — round-5 v2 deferral; **round 6 shipped the v1 base**
  (`slice`/`find` + source-pinned return); split-views remain v2. See
  [[history-reversals-and-deprecations]].
- **`mut ref self` interpreter write-back** — a self-host blocker; **round 6 closed it**
  (`3cc77e42`, CICO write-back). See [[self-hosting]].
- **Windows IOCP + codegen** — the round-2 line-17 sub-item; **round 6 shipped native
  Windows IOCP + the AOT build pipeline**, flipping M3 parity to DONE. See
  [[windows-and-cross-platform]].
- **Bare `fn` as an `Fn(...)` value** — round 6's open `B-2026-06-20-1` (workaround: wrap in a
  closure); **round 7 fixed it** (`79f1de14`, `FnType` → closure fat pointer + bare-name
  reify), then completed first-class fn values across arg / let / return / field / Vec. See
  [[bug-tracker]], [[codegen]], [[history-reversals-and-deprecations]].
- **All five round-8 open bugs — closed in round 9.** The interpreter **`u64` model**
  (`B-2026-07-04-8`, `45eb926`), the **for-loop-element-move double-free** class
  (`B-2026-07-04-17`, `278e1a91`, plus enum/Vec siblings `B-2026-07-05-2` / `B-2026-07-07-1`),
  the whole **iterator-adaptor `.collect()` surface** (`B-2026-07-04-2`, `9230632`),
  `B-2026-07-05-1` (heap-enumerate whole-tuple-copy), and `B-2026-07-04-15` (a **misdiagnosis** —
  a legit `T: Ord` bound rejection of `f64`). See [[bug-tracker]].
- **Collection-capacity pre-sizing** — the round-8 blocker (`.map().collect()` didn't compile)
  is gone: round 9 **fully closed the `.collect()` surface** and shipped the safe
  `calloc`-backed `Vec.filled(n, 0)` zeroed-alloc; the general presize peephole stays out (see
  the reverted fill peephole above). See [[codegen]].

## Negative results (archived, do not retry as-is)

- **Bounds-check elision via `llvm.assume` (general form)** — falsified; superseded by
  source-level BCE. Archived as v68. **But round 5 revived a narrow monotone-variable
  form** — do not retry the *general* approach; the induction-variable case is now live. A
  BCE "merge two checks" tier was separately found **unsound**. See [[codegen]],
  [[history-reversals-and-deprecations]].
- **Param `noalias` metadata** — measured inert (see above); only `mut ref` `noalias` kept.
- **Codegen auto-vectorization** (round 8) — a spike (`docs/spikes/codegen-autovectorization.md`,
  `01327bbd`; profiled in `f3feac22`) investigated auto-vectorizing codegen loops, surfaced by
  profiling kata #59. **Ruled out** — resolved as not worth pursuing. See [[codegen]].
- **Overflow-check elision** (round 8) — a spike (`docs/spikes/overflow-check-elision.md`,
  `3c8478b1`) to elide integer overflow checks was run and resolved as **not worth building**.
  See [[codegen]].

Related: [[codegen]], [[bug-tracker]], [[implementation-phases]].
