---
type: spec
created_date: 2026-07-15T07:28:20Z
last_modified: 2026-07-15T18:15:50Z
level: M1
stability: evolving
tags: compiler, codegen, llvm, phase-7
---

# LLVM Codegen Backend

## Identity
The Kāra native backend (Phase 7): lowers the checked AST to LLVM IR, runs the LLVM mid-end optimizer, and links native binaries — including auto-parallel lowering, synthesized clone/drop/display, and monomorphized collections.

## Purpose
To produce optimized native executables that honor Kāra's semantics, giving the language the backend-first performance story its v1 positioning depends on.

## Relationships
- **REFERENCES**: [[tree-walking-interpreter]]
- **REFERENCES**: [[backend-first-v1-positioning]]
- **REFERENCES**: [[monomorphized-collection-codegen]]
- **REFERENCES**: [[bounds-check-elision]]
- **REFERENCES**: [[run-llvm-o2-mid-end-passes-on-emitted-ir]]
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[effect-checker]]
- **DEPENDS_ON**: [[ownership-checker]]
- **DEPENDS_ON**: [[karac-runtime-library]]
- **IMPLEMENTS**: [[auto-concurrency]]
- **IMPLEMENTS**: [[data-layout-separation]]
- **IMPLEMENTS**: [[algebraic-data-types-and-pattern-matching]]
- **USES**: [[provider-system]]
- **MOTIVATED_BY**: [[backend-first-v1-positioning]]
- **REFERENCES**: [[network-runtime-and-cooperative-scheduling]]
- **REFERENCES**: [[jit-execution-path]]
- **REFERENCES**: [[design-by-contract-enforcement]]
- **REFERENCES**: [[refinement-and-distinct-types]]
- **REFERENCES**: [[wasm-target-backend]]
- **REFERENCES**: [[portable-simd]]
- **REFERENCES**: [[tensor-and-numerical-stdlib]]
- **REFERENCES**: [[reference-counting-elision]]
- **REFERENCES**: [[gpu-compute-shaders]]

## Realization

- src/codegen.rs and src/codegen/ (functions.rs, stmts.rs, exprs.rs, calls.rs, method_call.rs, control_flow*.rs, closures.rs, par_blocks.rs, provider.rs, mono.rs, maps.rs, vec_method.rs, synth*.rs, clone_drop.rs, runtime.rs, driver.rs, types_lowering.rs, http.rs)
- runtime/src/lib.rs, runtime/src/map.rs, runtime/src/clone.rs; tests/codegen.rs, tests/par_codegen.rs

- src/codegen/reduce.rs (par reductions); hot-swap indirection + .kara_jit_template; bench/hot_swap_cost/

- Coroutine transform: src/codegen/coro.rs (network async lowering, see [[network-runtime-and-cooperative-scheduling]])
- Crash diagnostics: src/codegen/debug_info.rs (Level 2 DWARF + panic location)
- Verification: src/codegen/contracts.rs, src/codegen/refinement.rs
- JIT install path: src/codegen/lljit.rs (see [[jit-execution-path]])

## Specifies

- IR emission for all expressions/statements/control flow; match lowering (per-arm + early-return cleanup); slice/array pattern lowering.
- Synthesized per-type helpers: hash/eq/drop/display/clone; recursive Vec/Map/Set drop; struct-field drop synthesis.
- Auto-parallel region lowering onto karac_par_run; atomic RC; provider vtable emission; HTTP handler ABI trampoline.
- Generic monomorphization; [[monomorphized-collection-codegen]]; [[bounds-check-elision]]; multi-file concat-super-program project mode.
- Runs LLVM `default<O2>` mid-end passes on emitted IR (see [[run-llvm-o2-mid-end-passes-on-emitted-ir]]).

- Auto-parallel reduction lowering to `karac_par_reduce` behind a cost-model gate (src/codegen/reduce.rs).
- Hot-swap: `--enable-hot-swap` flag + profile gating, codegen call indirection, a `.kara_jit_template` section + version manifest for JIT template replacement (Phase 7 lines 5/14).
- Per-target CPU baseline mirroring rustc defaults; sign/type-aware operator dispatch and widening for narrow/unsigned integer ops.

- `defer` / `errdefer` codegen: UserDefer + unified LIFO drain, block-scope + runtime-reachability gating, `errdefer(e)` binding form (wider-E payload reconstruction at `?`), defer/errdefer on par-branch cancel paths.
- User `Drop` dispatch: user-impl Drop signature validation + per-type `karac_drop_<T>` wrapper synthesis, scope-exit drop-call placement, move-suppression for let-rebind / return-by-value of user-Drop bindings.
- `Atomic[T]` load/store/new (incl. general expression position) + fetch_add/fetch_sub lock-free counters; atomic method dispatch on shared/`par`-struct field receivers (int types; `Atomic[bool]` via i8 slot-widening).
- Module-level `let` / `let mut` binding lowering: composite-init codegen, cross-fn binding visibility, synthetic per-binding effect resources.
- File / TcpStream / TcpListener / TLS / WebSocket codegen + close-on-drop; stdlib struct-by-value param LLVM ABI.

- Network async lowering via LLVM coroutines (coro.rs), flipped on by default for `build`/`run`; shared/`par`-struct always-Arc codegen with atomic RC, user `impl Drop` dispatch, and shared-struct self.field read/write in ref-self methods.
- Compile-time verification codegen: design-by-contract requires/ensures/old()/invariants ([[design-by-contract-enforcement]]) and refinement/distinct-type predicates ([[refinement-and-distinct-types]]) emitted in AOT binaries and stripped by `--release`.
- Level 2 crash diagnostics: panic location (file:line:col in fn) plus DWARF debug-info emission.
- PIC objects so x86_64-Linux binaries link as PIE; module data-layout pinned so a coroutine frame's size matches the AOT frame.
- Emitted code also runs on the in-process JIT path (see [[jit-execution-path]]).


- New codegen-hosted subsystems: the [[wasm-target-backend]] target family, [[portable-simd]] `Vector[T,N]` lowering, [[tensor-and-numerical-stdlib]] core lowering, and [[reference-counting-elision]].
- Oversized enum payloads boxed on the heap (pack/unpack + drop) instead of a silent-truncation miscompile; string-literal `match` compiled as a switch tree, not a memcmp cascade.
- Niche call ABI for `Option[shared T]` (extends to impl methods) with the soundness fixes its convergence tests surfaced.
- AOT integer arithmetic faults (overflow / div-by-zero / MIN-div traps) with interpreter parity; narrow-int arithmetic traps at the declared width; sub-64-bit width coercion at ABI boundaries and struct-field stores; int→int/int→float/float→float/saturating-float→int cast method families.


- First-class function values: a bare `fn` name, a `let`-bound fn value, and fn values in return / struct-field / `Vec[Fn]` positions all lower to the `{fn_ptr, env_ptr}` closure fat pointer (a bare name reifies to a memoized `{trampoline, null-env}` forwarder), annotated or inferred, callable directly or through a `Fn(...)` parameter — closing B-2026-06-20-1 through B-2026-06-21-3. Closure-value calls now dispatch through non-identifier callees (`(h.f)(x)`, `(v[i])(x)`), fixing a silent const-0 miscompile (B-2026-06-22-4).
- Escaping capturing closures (heap-environment epic, B-2026-06-22-2): a captured-local closure that outlives its frame gets a reference-counted **heap** environment (`{refcount, env}`) with move-out on return, inc-on-copy shared ownership, and per-instance drops when stored in a struct / tuple / array / `Vec[Fn]`. The epic is now closed across every heap-env-closure place — return, store, copy/move, escape (be returned) inside those containers, by-value arg-pass to a borrows-only callee, and reassignment of a binding, struct field, or Vec element. An exhaustive escape-analysis misuse guard (`reject_escaping_capturing_closure` / `reject_heap_env_misuse`) turns any not-yet-modeled shape into an honest `E_ESCAPING_CLOSURE_NOT_YET` diagnostic rather than the former dangling-stack-env miscompile; non-capturing and same-frame closures keep the cheap stack env.
- New codegen-hosted subsystems this round: DataFrame lowering (src/codegen/dataframe.rs), the shared reduce-kernel emitters (src/reduce_kernel.rs, src/codegen/kernel.rs, src/codegen/stats.rs) feeding Stats.* / Column / Tensor reductions, and GPU WGSL emission (src/gpu_wgsl.rs, see [[gpu-compute-shaders]]).
- Trait-method codegen: monomorphization of generic impl/trait methods on concrete receivers, dispatch through generic type-param bounds, user trait impls on primitive scalar types, and generic user impls over the builtin `Column` / `Tensor` containers. Auto-par bails any group that constructs or returns a heap-env closure to sequential codegen.
- `Column[T]` (nullable Arrow column) lowering: Arrow-buffer layout, `fillna` / `dropna` / `from_iter_nullable`, `iter` / `iter_valid` Vec-returning transforms, and SQL three-valued-logic (3VL) arithmetic/comparison.
- Scalar transcendental + rounding math on floats (src/float_math.rs); `Map.new()` / `Set.new()` as module-binding initialisers.

## Constraints

- Emitted code must match [[tree-walking-interpreter]] semantics.
- Stripping must preserve runtime intrinsics (SYMBOL_KEEP_LIST).

## Rationale

Phase 7, the largest subsystem. Behind an `llvm` cargo feature. Realizes [[backend-first-v1-positioning]].
