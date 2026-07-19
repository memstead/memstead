---
type: history
title: History — renames, reversals, supersessions
updated_round: 10
---

# History — renames, reversals, and supersessions

Tracks how the project has changed over time: things renamed, reversed, superseded, or
deprecated, and the brainstorm decisions that graduated into the design.

## Whole-language supersession (repo origin)

- **Complete language redesign.** The original **`fn` / `flow` / `record` / `->`
  pipeline** design was **replaced** by the current Rust-inspired systems language. This
  is the headline change in the CHANGELOG and supersedes the entire prior design. See
  [[kara-overview]].

## Renames and splits

- **`Ordering` enum split** — split into a **comparison `Ordering`** (Less/Equal/Greater)
  and a **memory `Ordering`** (`runtime/stdlib/memory_ordering.kara`, atomic memory
  ordering). Filed as a separate CR for disambiguation. See [[stdlib-and-traits]].
- **Hash function swap** — `karac_hash_<T>` changed from **FNV-1a → FxHash** (chosen via
  a `hash_quality` benchmark). See [[codegen]].
- **Stdlib "baking" (CR-202)** — programmatic registration (`register_stdlib_traits`,
  `register_builtin_types`) **superseded** by **baked `.kara` stdlib source** via
  `include_str!` + `#[compiler_builtin]`; readable declarations became the source of
  truth (dispatch tables stay programmatic). `register_stdlib_traits` was retired.
- **`Server.serve` signature** — evolved from `Server.serve(handler)` to
  **`Server.serve(addr, handler)`** (address lifted to the user surface).
- **`char` lowering** — `char` now lowers to **LLVM i32**.
- **Parallax `*_tag String` workaround retired** in favor of the real typed shape.
- **Attribute path migration (round 2)** — the diagnostic-namespace work migrated the
  attribute path representation and added an `is_bare` helper; unknown attributes now
  produce **`E_UNKNOWN_ATTRIBUTE`**. See [[attributes]].
- **Closure-capture mode off the `Moved` state (round 2)** — Phase 7 line 45 migrated how a
  captured binding's mode is tracked away from the `Moved` state. See [[design-ownership]].
- **`#[deprecated]` is now a first-class attribute (round 2)** — with a `note`/`since`
  payload, resolver-symbol-table threading, and use-site warnings, Kāra now has its own
  deprecation mechanism. See [[attributes]].
- **Tracker markers (round 2)** — a new **`[->]` "explicitly deferred"** checklist marker
  was introduced and stale states swept; **`[x]` flips** this round include line 761 (rich
  display), line 747 (`%magic`), line 633 (stub hints), line 619 (diagnostic classes), and
  line 703 (playground).

### Round-3 changes

- **Positioning reframe: "backend-first at v1" → "Production-Ready skeleton"** — the README
  dropped the "backend-first" framing in favor of describing v1 as a **production-ready
  skeleton**. Same codegen-first bet ([[design-runtime-phases]]), reworded. This is why the
  KB now writes "backend-first" in the past tense.
- **`%rc` notebook magic un-deferred — now shipped (2026-05-20)** — round 2 pushed `%rc` to
  v1.1.x; round 3 **reversed that** and shipped `%rc` (surfaces RC-fallback decisions), plus
  a new **`%timeline`** effect-conflict magic. See [[jupyter-kernel]], [[deferred-work]].
- **Test discovery: `fn test_*` naming → `test { }` blocks** — the old convention where any
  `fn test_*` was discovered as a test was **removed**; tests are now first-class
  **`test { }`** blocks (`Item::TestCase`) and fixtures were converted. See
  [[stdlib-and-traits]].
- **Top-level `let` rejected; module-level `let` / `let mut` added** — a bare top-level
  `let` now errors with a did-you-mean-`const` hint, while a proper **module-level `let` /
  `let mut`** binding (const-init, modelled as a synthetic effect resource) was introduced.
  See [[stdlib-and-traits]], [[design-effect-system]].
- **`char` → `u8` relaxation** — a `char`/`u8`-relaxation brainstorming note was retired and
  filed as the phase-1 `b'A'` byte-literal slice. (Round-3 material labelled this "v70";
  round 4's committed archive `v70_arena_handle_v1_surface.md` uses that number for a
  different topic — see the round-4 graduations and the inconsistency note below.)

### Round-4 changes

- **Async I/O: hand-written state-machine transform → LLVM coroutines (A2).** The round-2/3
  network-boundary **state-machine transform** (a body-splitter that emitted a poll function
  with switch-on-tag dispatch) was **superseded** by an **LLVM-coroutine** transform. A bug
  in the body-splitter (bug C — a network call inside a helper miscompiled) forced an
  architectural fork; a spike recommended coroutines; the coroutine path is now **on by
  default** for `karac build`/`run`. Not a reversal of "no async/await" — still an internal
  transform. See [[design-runtime-phases]], [[codegen]], [[bug-tracker]].
- **JIT-default flip — reverted, then re-landed piecewise.** The single commit that flipped
  the whole compiler to the JIT execution path (L577 step (c)) was **reverted** for a third
  blocker (partly a real REPL newline bug). It was then re-landed in two independent pieces:
  **`karac test`** on JIT-default and **`karac repl`** on JIT-default. AOT stays the path for
  shipped binaries. See [[codegen]].
- **MCJIT prototype → orc2/LLJIT.** The initial MCJIT JIT prototype (L558) was recorded as a
  finding and **superseded** by the ORC/LLJIT path (L560), which is what shipped.
- **`par struct` / `par enum`: not-supported error → implemented.** They first shipped as a
  deliberate not-supported **parser error** (a minimum fix), then were implemented as
  always-`Arc` aggregates (Slice A–D). See [[design-concurrency-and-providers]].
- **Network construction: `fd:-1` sentinel → `Result`.** Network construction methods
  (listeners/streams) stopped returning a magic `fd:-1` and now return a `Result` with
  **named error variants** (`AddrInUse` / `ConnectionRefused`). See [[networking]].
- **`karac migrate` Atomic heuristic → default.** Round 3's opt-in `--atomic` heuristic became
  the **default** in round 4; opt out with **`--no-atomic`**. See [[cli]], [[design-ownership]].
- **Contracts / refinement predicates are release-stripped.** A design rule, not a reversal:
  `karac build --release` strips contracts and the `?`-error-return-trace so verification is
  zero-cost in production. See [[design-contracts-and-verification]].
- **Licensing + repo pointers.** The repo added a **dual MIT / Apache-2.0** license
  (`LICENSE-MIT`, `LICENSE-APACHE`), repointed references to **`karalang/kara`**, and the
  README **sharpened its AI positioning**, relocated the GPU note, and trimmed status.

### Round-5 changes

- **Self-hosting resequenced as the v1 pivot (`8 → 9 → 10 → 12 → 11`).** The roadmap was
  reordered so **[[self-hosting|Phase 12 self-hosting]]** comes *before* the Phase 11 stdlib
  longtail — the biggest strategic shift of the round. A self-hosted front end is treated as
  the sharpest exercise of the codegen surface; the [[numerical-stdlib-and-tensors|numerical
  stdlib]] was deprioritized behind it. See [[implementation-phases]].
- **Bounds-check elision via `llvm.assume` — un-falsified in a narrow form.** The v68
  negative result (below) rejected a *general* `llvm.assume` BCE. Round 5 **revived
  `llvm.assume`** for the **monotone-variable / induction-monotonicity** case only: range
  facts fold write-head bounds checks (`bce_monotonic_assume.md`). The tier verdict was
  **opt-validated first** — a "merge two checks" tier was found **unsound**; a KMP-style
  interprocedural tier was scoped — and a compound-index table lookup was recorded as a
  **non-trigger** (kata #28). This is a *reversal of the reversal*, scoped to what the
  optimizer can actually prove. See [[codegen]], [[deferred-work]].
- **`Mutex`: spinlock → 3-state futex.** `Mutex` first shipped as a **standalone spinlock**
  (slice 1), then evolved through place-expression / ref-param locking to a **blocking
  (futex) 3-state lock**. See [[design-concurrency-and-providers]].
- **WASM Component Model: paired form → embedded WIT.** A **paired `.component.wit`
  descriptor** artifact was added, then the **paired component form was removed** — the
  **embedded-WIT component** became the `wasm_wasi` default. See [[wasm-targets]].
- **`demo_ideas.md` → `dogfooding.md`.** The demo-ideas doc was renamed and reframed as the
  **V1 dogfooding roster** (name-keyed entries + a roster table); the **data-engineering
  pipeline demo was demoted to post-launch** (out of the v1 impl checklist). See
  [[examples-and-benchmarks]].
- **`StringSlice` borrowed-view type recorded as v2.** A borrowed `StringSlice` type was
  deferred to **v2** ("graduate on demo/kata demand") rather than shipped in v1.
- **Parallax bench cohort trimmed + flipped Graviton-led.** The Parallax throughput runs
  were re-planned for EC2 (x86 + Graviton + Mac); the cohort was **trimmed to K/R/J/G**
  (Kāra/Rust/Java/Go) and the headline flipped to **Graviton-led**. See
  [[examples-and-benchmarks]].
- **README rewritten around a benchmark headline.** The README was reworked with a
  quick-start + "aha" example, a **benchmark headline** (idle-connection density), a
  why-safe section, and an **origin story** ("How Kāra Was Born"); the stale bench *Status*
  section was retired in favor of a **commercial comparator ladder**.

### Round-6 changes

- **`bugs.md` retired → machine-countable `bug-ledger.jsonl` + generated `bug-ledger.md`.**
  The gitignored/working `bugs.md` area and the commit-message-only `B-2026-…-N` / `#N` tag
  scheme were **superseded** by a structured **`docs/bug-ledger.jsonl`** (~152 entries, one
  JSON record each) plus a generated human-readable view (`d675004f`), with `bug-lint.sh` /
  `bug-curve.py` tooling. The rounds-1–5 "the ledger is empty" statement is now stale. See
  [[bug-tracker]].
- **`StringSlice` v2 → shipped in v1.** Round 5 deferred the borrowed `StringSlice` view type
  to v2; round 6 **graduated it to v1** (`32358cac`) — `slice`/`find` + a source-pinned
  by-value return (`first_word` shape). **Split-views stay v2.** This reverses the round-5
  deferral. See [[deferred-work]].
- **`karac run` type-leniency → hard abort.** `karac run` used to **downgrade hard type
  errors to warnings** and run with a placeholder value (silent wrong output, exit 0);
  round 6 makes it **abort on value-corrupting casts** instead (`b59eb070`,
  `B-2026-06-13-15`). A correctness reversal, not a design one.
- **SoA `layout` model: name-keyed `soa_layouts` → per-binding LayoutId carrier.** The
  original codegen model keyed SoA-ness on the **binding name** matching a layout block — a
  footgun where a base parameter merely sharing a name lowered SoA. Round 6's
  **[[per-layout-monomorphization|per-layout monomorphization]]** made `soa_layouts`
  origin-only and drives SoA off a per-binding **LayoutId** value, retiring the name-match
  ABI (`8ad88b24`). See [[design-data-layout]], [[codegen]].
- **`mut ref self` interpreter blocker closed** (`3cc77e42`) — the interpreter now persists
  `mut ref self` method mutations (CICO write-back), unblocking the self-host port's
  method-chained styles.
- **M3 cross-platform-parity gate → DONE.** Native **[[windows-and-cross-platform|Windows
  IOCP]]** shipped, so the M3 milestone (Linux + macOS + Windows) flipped to done
  (`fabe54e7`) — a status advance, recorded here for the timeline.
- **Many original bug diagnoses corrected.** A recurring round-6 pattern: a ledger entry's
  first hypothesis is disproven by investigation and the record documents both. Examples:
  `B-2026-06-20-4` (mis-attributed to a double-free; was a `==` memcmp overread),
  `B-2026-06-18-8` (mis-read as an "ASAN-invisible race"; was miscounted concurrent
  `println`), `B-2026-06-14-29` (a "bind-without-consume" theory disproven — a duplicate of a
  box-drop-walker gap), and `B-2026-06-17-7` (bounds-check-elision and noalias hypotheses both
  refuted; the real cause was loop unrolling). See [[bug-tracker]].

### Round-7 changes

- **GPU note (README) → a real, tracked feature.** Round 4 only *relocated a GPU note* in the
  README; round 7 turns GPU compute shaders into a built feature. The **routing decision was
  resolved to Option B — explicit `#[gpu]`** (the compiler does not infer GPU-eligibility),
  and the Phase-10 gate was split into tracked FE-* slices. Front-end validation landed
  (`GpuSafe` trait + call-graph + effect gate); WGSL codegen is still a spike. See
  [[gpu-compute]], [[wasm-targets]].
- **`B-2026-06-20-1` (open → fixed).** Round 6's headline **open** ledger entry — passing a
  bare named `fn` as a first-class `Fn(...)` value miscompiles — was **fixed** (`79f1de14`):
  `FnType` now lowers to the closure fat pointer and a bare fn name reifies into a
  `{trampoline, null env}` value. This closed the round-6 [[deferred-work|deferral]] and
  opened a four-entry follow-on chain (`B-2026-06-21-1/-2/-3`) that completed first-class fn
  values across every position. See [[bug-tracker]], [[codegen]].
- **v70 arena/handle brainstorm → shipped.** The **`v70_arena_handle_v1_surface.md`**
  brainstorm (round 4) graduated into the round-7 **`Arena[T]` / `ArenaRef[T]`** bulk-allocation
  primitive (interpreter slice, `aaf143d1`). See [[stdlib-and-traits]], the inconsistency note
  below.
- **WASM component build made byte-reproducible.** `--bindings component` (the `wasm_wasi`
  default) had emitted a **different SHA per build** because the core module was linked under
  a **pid-stamped scratch filename** baked into the wasm `name` section; round 7 links under a
  source-derived basename in a pid-unique directory so the artifact is reproducible
  (`B-2026-06-22-3`). A correctness fix, not a design change. See [[wasm-targets]],
  [[bug-tracker]].

### Round-8 changes

- **`B-2026-06-22-2` heap-env closure epic (open → fixed).** Round 7's headline **open**
  `high` ledger entry — an escaping capturing closure reading a freed stack env — was
  **closed** in round 8 (`be2ef68e`) after ~30 slices: escaping closures now get a
  reference-counted heap env, and every place (return / store / copy / move / escape /
  arg-pass / reassignment) is RC-accounted. This retires the round-7 "STAYS OPEN" residual
  list. See [[codegen]], [[bug-tracker]].
- **`B-2026-06-19-13` `char.to_digit` (partial → fixed).** The round-6/7 `partial` (interp +
  typecheck done, codegen an honest "not yet supported" Err) is now fully **fixed** —
  codegen lowers `char.to_digit(radix)` (`4e4b57de`). See [[bug-tracker]].
- **GPU codegen: spike → landed slice-0.** Round 7's note that *WGSL codegen is still a
  spike* is **superseded** — round 8 landed a working slice-0: a `wgpu` compute spine
  **proven on Metal**, WGSL codegen, and an end-to-end `gpu.dispatch`. See [[gpu-compute]].
- **Recursive-reduction parallelization: compile-time decline → runtime fork-depth cap.**
  `B-2026-07-03-14` first made auto-par **decline** a directly-self-recursive reduction body
  (it caused runaway task nesting → SIGBUS). The **shallow-depth-parallel-reduction** spike
  then **reversed that decline**, re-enabling the reductions behind a runtime fork-depth cap
  (`KARAC_PAR_MAX_FORK_DEPTH`, default 1 — only the outermost level fans out). See
  [[design-concurrency-and-providers]].
- **int × float arithmetic: permissive → hard error on all three surfaces.**
  `B-2026-07-04-11` made a cross-domain `i64 * f64` a hard `TypeMismatch` (was silently
  admitted by `karac check`, a runtime error under `karac run`, and a silent miscompile under
  `karac build`); K̄ara has no implicit int→float promotion. Same-type float arithmetic and
  float+unsuffixed-literal promotion are unchanged. See [[stdlib-and-traits]], [[bug-tracker]].
- **Struct/enum sort ordering: alphabetical → declaration order.** The round-8 interpreter
  ordering fix (`B-2026-07-03-6`) first landed **alphabetical** field/variant ordering (to
  stop the SortedSet/SortedMap data-loss), then `B-2026-07-03-12` corrected it to proper
  **derived-`Ord` declaration order** via a per-thread type-order registry — matching the
  codegen `<`/`sort()` ordering. See [[stdlib-and-traits]].
- **Stats `Slice` params: consume → `ref` borrow.** `B-2026-07-01-10` re-declared the baked
  `stats.kara` params `ref Slice[f64]` so two statistics over one dataset pass `karac check`;
  the root enabler was that the ownership pass never read baked-stdlib declared param modes
  (the whole baked surface was consume-default). See [[columnar-data]].

### Round-9 changes

- **`karac run` → JIT-default (interpreter demoted to `--interp`).** Round 4 flipped
  `karac test` / `karac repl` to the JIT execution path; **round 9 flipped `karac run` too**
  (LLJIT productionization Slice 6c, `ef7d355d`). The tree-walk interpreter is now a **dev/debug
  backend behind `--interp`**, and codegen becomes the default execution oracle — the deliberate
  end of the **run-vs-build divergence** the ledger had been chasing for rounds. AOT still backs
  shipped binaries. See [[codegen]], [[cli]].
- **Roadmap resequenced: LLJIT-productionization inserted before Phase 12.** Self-hosting was
  **paused** for the LLJIT work (`fbbcfc9c`) and the roadmap reordered (`44169c73`,
  owner-confirmed), then Phase 12 was **un-paused** once the JIT gate cleared (`2141da23`). See
  [[self-hosting]], [[implementation-phases]].
- **GPU: element-wise slice-0 → LBM kernels on Metal.** Round 8's slice-0 (element-wise-map)
  is **superseded** — round 9 landed struct-SoA `gpu.dispatch` (GPU-GATE-1 on Metal), multi-field
  SoA groups, and value control flow in `#[gpu]` kernels. Two settling decisions: **GPU-LBM-1 →
  `f32`** and the v1 GPU gate **decoupled from the Slipstream LBM**; and the **"build once in
  Kāra" GPU-LBM deferral was dropped** (owner directive, `1cd66c46`). See [[gpu-compute]].
- **Interpreter `u64` model: none → span-threaded unsigned-64.** `B-2026-07-04-8` (round 8's
  open `med`) closed — the tree-walk host now reinterprets its i64 carrier as `u64` at every
  signedness sink (`45eb926`), ending the `u64 ≥ 2⁶³` run-vs-build divergence. A correctness
  reversal, not a design one. See [[columnar-data]], [[bug-tracker]].
- **Diagnostic-fix invariant: resolver suggestions made machine-applicable.** A
  diagnostic-fix-invariant audit found the resolver's `did-you-mean` family emitted prose only;
  round 9 populated `.replacement` `TextEdit`s across the whole E01xx family and made `karac fix`
  apply the ownership `fix_diff` it already computed (`B-2026-07-06-3/-4`, `B-2026-07-07-3`).
  See [[design-ai-first-compiler]], [[bug-tracker]].

### Round-10 changes

- **`f16` / `bf16`: reserved keyword → type-name identifier (REVERSAL).** Bare `f16` / `bf16`
  were **RESERVED** — a compile error. They now **lex as identifiers / type-names** like `f32` /
  `f64` (`B-2026-07-14-2`, `b517d59b` / `42841327` / `09a2fc88`). `design.md` was internally
  contradictory — its "Reserved float keywords" section said error, but a `Tensor[bf16, …]`
  example used the type — and the conflict was resolved in favor of the Rust seed lexer + the
  type-usage example: the **restriction now lives at the type/binding layer (Phase 7), not the
  lexer**. See [[codegen]], [[numerical-stdlib-and-tensors]].
- **Self-host parser SEGV — root cause REVERSED.** The control-flow-expression parser crash
  (`B-2026-07-09-12`) was long chased as a **construction/drop memory bug** — an inline
  shared-enum packing-offset "3-field trigger", now a documented **RED HERRING**. The real cause
  was an **auto-parallelization** bug: three sequential `mut ref self` cursor-advancing calls in
  `parse_if` were being raced. Fixed in `src/concurrency.rs`. See [[self-hosting]], [[codegen]],
  [[bug-tracker]].
- **Two-pointer BCE stays reversed — independently re-confirmed unsound.** `B-2026-07-10-5`: a
  fresh x86 toolchain re-confirmed there is **no sound lever** — `l < n` is a *semantic*
  invariant no `assume` can supply. Kāra is actually **2–10% ahead** of equal-safety `rustc` on
  x86 loop density; the residual gap is **aarch64 scheduling, not x86 density**. This re-confirms
  the round-9 reversal below. See [[codegen]].
- **`B-2026-07-11-8` (interpreter `?`-error-return-trace on a handled error) REVERTED as a
  misdiagnosis.** The trace is a **deliberate stderr debug diagnostic**, handling-agnostic by
  design; the "fix" was reverted and the entry marked `status: invalid`. See [[bug-tracker]].
- **`B-2026-07-11-36` (a direct `Vec[shared struct]` leak) closed as INVALID.** It was a macOS
  `leaks`-tool artifact — the shape is **clean under Linux LeakSanitizer** (the authoritative
  gate). Lesson recorded: use the **docker Linux-LSan harness** as the leak gate for Vec/shared,
  not macOS `leaks`. See [[bug-tracker]].
- **`Iterator.collect()` was NOT missing.** The `B-2026-07-11-9` filing's claim of a missing
  `.collect()` was **author error** — `collect()` already shipped (typecheck + interp + codegen).
  Only the **loop-early-return rc-fallback** half was real, and that was fixed. See
  [[bug-tracker]], [[codegen]].
- **`std.mem take` un-deferred.** `B-2026-07-08-25` fixed generic `T.default()`
  monomorphization, so **`take[T: Default]` now ships as a real generic source body** (previously
  deferred; `swap` / `replace` were intrinsics). See [[stdlib-and-traits]].

## Reversals / empirically-falsified approaches

- **Bounds-check elision via `llvm.assume` (general form)** — tried, **empirically
  falsified** (v68 negative result, archived). Replaced by **source-level** bounds-check
  elision (`get_unchecked`, for-range, `Slice[T]` reads/writes). **Round 5 revived a narrow
  monotone-variable form** (see the round-5 note above) — the general result stands, the
  induction-variable case does not. See [[codegen]], [[deferred-work]].
- **Param `noalias` metadata** — round 5 emitted `noalias` on `mut ref` params (from the
  exclusive-borrow guarantee), but **plain param `noalias` measured inert** and
  **alias-scope metadata** was deferred (~0 runtime gain, ~132 B/kernel size cost). See
  [[rc-elision]], [[deferred-work]].
- **Codegen auto-vectorization — ruled out (round 8).** A spike (profiled from kata #59)
  investigated auto-vectorizing codegen loops and **resolved to rule it out** (`01327bbd`,
  `docs/spikes/codegen-autovectorization.md`). See [[deferred-work]].
- **Overflow-check elision — ran, not worth building (round 8).** A spike to elide integer
  overflow checks was run and resolved as **not worth building** (`3c8478b1`,
  `docs/spikes/overflow-check-elision.md`). See [[deferred-work]].
- **Two-pointer BCE diagnosis reversed as unsound (round 9).** `B-2026-07-10-5` first proposed
  extending the relational-assume BCE to the sliding-window `s[l]` idiom, but the reversal
  (`8c108547`) established the fix would **inject UB**: unlike the midpoint idiom (locally
  provable from the dominating guard), the two-pointer upper bound `l < n` holds only through
  the algorithm's *semantic* invariant `l <= r`, which no syntactic assume can supply — and
  `rustc` bounds-checks `s[l]` identically. The kata-#76 perf gap stays open as a **diffuse
  instruction-density / scheduling** gap with no single lever (two sub-flavors). See [[codegen]].
- **`Vec.new()`+push-loop → `Vec.filled` fill peephole reverted (round 9).** `B-2026-07-08-7`
  landed a peephole rewriting a counted `push`-fill to `Vec.filled(n, 0)`, then **reverted it**
  (`3a4e6135`) — a bisect showed it **regressed its own target kata #63 by +8.4%** (the fill
  rewrite broke the obstacle-predicate BCE the same kata relied on). The corrected diagnosis:
  kāra's push-loop Vec codegen is already excellent (3.7× faster than rust's push-loop); the
  residual is allocator-bound and **does not reproduce on Linux/glibc** (kāra wins there). The
  safe, retained half is a **`calloc`-backed `Vec.filled(n, 0)` lowering** gated on a static
  all-zero fill (matching `vec![0; n]`), which never rewrites anyone's construction. See
  [[bug-tracker]], [[deferred-work]].
- **`Vec.sort_by` "Path A+"** — empirically falsified (2026-05-14). The FFI-boundary
  comparator-inlining variant was deferred; the shipped path uses an FFI-bridge thunk.
- **`Vec.new` + push-loop → `Vec.filled` lowering** — deferred (Phase 7).
- **Phase-4 item 117 (runtime effect tracking)** — skipped.
- **`get_unchecked` downgrade** — reframed during WIP triage (blocked on the
  unsafe-enforcement predecessor before landing).
- **`%rc` notebook magic deferred to v1.1.x (round 2)** — the `%magic` surface graduated,
  but the `%rc` magic was pushed to v1.1.x. See [[jupyter-kernel]], [[deferred-work]].
- **TAIT shipped as a v1 stub (round 2)** — type-alias `impl Trait` was declared but landed
  as a stub, not a full implementation. See [[design-generics-and-impl-trait]].
- **`ptr.container_of` briefly soft-blocked (round 2)** — `offset_of` shipped first;
  `container_of` was soft-blocked on line 511, then landed (line 509). See
  [[design-unsafe-ffi-and-pointers]].

## Clarifications (not reversals)

- **State-machine transform vs. "no async/await".** Round 2 added a **state-machine
  transform** for network-boundary functions (Phase 6 line 26) to power the
  [[design-runtime-phases|v1.1 event loop]]. This does **not** reverse the "no async/await,
  no colored functions" design bet: it is an internal, compiler-driven transform — the
  programmer still writes straight-line code with no `async` keyword and no function
  coloring. See [[design-concurrency-and-providers]].
- **Const generics complete (round 2)** — the roadmap marked const generics done. See
  [[design-generics-and-impl-trait]].

## Brainstorm graduations (design decisions adopted)

Design decisions graduate from `brainstorming/archive/vNN.md` into the design:

- **v62** — interpreter perf + binary-size decisions.
- **v63** — LLM compiler query channel → [[design-ai-first-compiler]].
- **v64** — **backend-first v1 positioning** → [[design-runtime-phases]].
- **v65** — PGO and online-JIT positioning.
- **v66** — general-purpose foundation + "data quiet bonus"; **`std.cli`** graduated here.
- **v67** — SIMD strategy + Codegen architecture canonical record.
- **v68** — bounds-check elision via `llvm.assume`: **negative result** (archived).
- **v69** — **Go-parity gaps** (`brainstorming/archive/v69_go_parity_gaps.md`) graduated to
  the roadmap with bench scaffolding (round 3). See [[examples-and-benchmarks]].
- **v70** — round 3 referenced a `char` → `u8` relaxation note under this number; the
  archive file committed in round 4 (`v70_arena_handle_v1_surface.md`) uses "v70" for an
  **arena/handle stdlib surface** brainstorm (with follow-on landings). See the inconsistency
  note below.
- **v71** — **accumulating diagnostics** (`v71_accumulating_diagnostics.md`, round 4).

## Round-4 spike (architectural)

- **Network async coroutine transform** (`docs/spikes/network-async-coroutine-transform.md`)
  — after bug C in the state-machine body-splitter, this spike compared the hand-written
  state-machine against an **LLVM-coroutine** transform and **recommended the coroutine
  path**, which was then built (A2 track) and flipped on by default. This is the round's
  central architectural decision. See [[design-runtime-phases]], [[codegen]].

## Inconsistency note — the "v70" brainstorm number

Round-3 KB material labelled the `char`/`u8` relaxation brainstorm **v70**, but the
brainstorm archive file actually committed in round 4 is **`v70_arena_handle_v1_surface.md`**
(an arena/handle stdlib surface). Either the round-3 number was inferred incorrectly, or the
repo reused the number. The `char`/`u8` → `b'A'` byte-literal outcome is real regardless of
its brainstorm number.

Related: [[bug-tracker]], [[deferred-work]], [[codegen]], [[design-runtime-phases]],
[[design-contracts-and-verification]].
