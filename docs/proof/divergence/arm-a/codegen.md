---
type: architecture
title: Codegen (Phase 7, LLVM backend)
updated_round: 10
---

# Codegen — Phase 7, LLVM backend

The LLVM code generator is the **active heavy work** and the priority for the
codegen-first v1 (see [[design-runtime-phases]]). Codegen was graduated to a
canonical architecture record as part of the **v67** brainstorm.

Round 5's headline codegen efforts are broad: **[[rc-elision|RC elision]]** (count-free
reference-counted chains), **[[simd|portable SIMD]]** and **[[numerical-stdlib-and-tensors|
Tensor]]** lowering, the **[[wasm-targets|WASM]]** back end (WASI / browser / Component
Model / threads), a **string-literal `match` → switch-tree** dispatch, a revived
**monotone-variable BCE** (`llvm.assume`), **general owned-temp tracking**, and
**oversized-enum-payload boxing** — most of it driven by the [[self-hosting|self-hosting]]
port and the [[examples-and-benchmarks|density benchmarks]]. **Round 6** adds
**[[metaprogramming|comptime/derive]] lowering**, **[[per-layout-monomorphization|SoA across
function boundaries]]**, borrow-elision, collection pre-sizing, and overflow/bit-arithmetic
intrinsics (see [[#Round-6 codegen]]). **Round 7** lands **first-class function values** and
the **heap-env closure epic** (started), the **[[columnar-data|`Column[T]`]] Arrow backend**,
and scalar float math (see [[#Round-7 codegen]]). **Round 8** **closes the heap-env closure
epic** (`B-2026-06-22-2`), lands **iterator-adaptor `.collect()` into `Vec`**, a **rolling-DP
length-pin BCE** pass, and a **shared reduce kernel** (see [[#Round-8 codegen]]). **Round 9**
**flips `karac run` to the LLJIT execution path by default** (the interpreter becomes an
`--interp` fallback), **fully closes the `.collect()` surface**, lands a per-target
**`#[repr(C)]` struct-by-value ABI**, and puts **GPU LBM kernels on Metal** (see
[[#Round-9 codegen]]). **Round 10** adds **compiler-driven inline hints** and **f-string
format specifiers**, lands **iterator-terminal codegen** (`fold`/`sum`/`reduce`/`for_each`/
`any`/`all`/`count` on fused chains), threads **concrete heap types through generic
monomorphs**, fixes **lexical-scope shadowing revert** and **match arm-guard evaluation**, and
retires **TBAA** from the alias-metadata row (see [[#Round-10 codegen]]). Round 4's efforts
remain in place: the **[[#JIT execution path (round
4)|JIT path]]** (LLJIT/orc2), the **[[design-runtime-phases|LLVM-coroutine async transform]]**,
and **[[design-contracts-and-verification|contract / refinement / distinct-type]]** lowering.

## Optimization pipeline

- Emitted LLVM IR is run through **LLVM mid-end optimization passes (`default<O2>`)** —
  this was identified as the bottleneck in the Parallax perf investigation (probe sweep
  ruled out the runtime; IR-opts were the fix). See [[examples-and-benchmarks]].
- **Bounds-check elision**: shipped at the source level for `Vec.get_unchecked`,
  `for i in start..end` ranges, and `Slice[T]` reads/writes. A *general* **`llvm.assume`**
  approach was tried first and **empirically falsified** (v68). **Round 5 revived
  `llvm.assume` in a narrow form**: **monotone-variable / induction-monotonicity** BCE folds
  write-head bounds checks from proven range facts (`bce_monotonic_assume.md`), with a
  diagnostic + assume mechanism. Tiering was **opt-validated** first — a "merge two checks"
  tier was found **unsound**, a KMP interprocedural tier scoped, and a brute-force
  compound-index lookup recorded as a **non-trigger** (kata #28). See
  [[history-reversals-and-deprecations]] and [[deferred-work]].
- **Hashing**: the `karac_hash_<T>` family switched from **FNV-1a to FxHash** (chosen via
  a `hash_quality` bench). Single-char `karac_string_decode_char` made O(1).
- Binary size: strip + `panic=abort` + symbol audit (phase 1), cross-archive LTO + DCE
  (phase 2), fat LTO.
- **Per-target CPU baseline** mirroring rustc defaults (round 2).
- **Type-aware operator dispatch** for unsigned integer ops, plus **sign-aware widening**
  for `println` on narrow integer types (round 2).

## Monomorphization

- Generic **monomorphization** with **trait-bounds verification at the monomorphization
  request** (call-site discharge).
- **Monomorphized collections**: `Map[i64, i64]` (inline probe fast-path for get and
  insert), generalized to `(K, V)` pairs, `Map[char, i64]` (with `char` lowering to LLVM
  **i32**). Mixed bench findings recorded.
- `Const generics` and `IntSize::I128` shipped across many slices.

## Drop / clone synthesis

- Per-type synthesized `hash` / `eq` / `drop` / `display` / `clone` functions.
- **Recursive drop** for `Vec` / `Map` / `Set` keys and struct fields (slices α/β/γ);
  some parts (tuple-destructure drop) deferred. Per-arm match cleanup + early-return
  cleanup closed a `bfs_sieve` leak (see [[bug-tracker]]).
- Move-aware scope-exit cleanup for `Vec` / `String` consumer sites and tail-expression
  `Vec` returns.

## Language lowering highlights

- `char` lowers to LLVM i32; `String.chars()` and `for c in s` UTF-8 decode.
- `VecDeque[T]` lowers to `Vec`'s `{ptr, len, cap}`; `Vec.filled(n, val)`, `Vec.pop`
  returning `Option[T]`.
- Ref-scrutinee `match` write-through via GEP; labeled-block runtime semantics.
- HTTP handler ABI trampoline for `Server.serve(handler)`; free-fn-as-value.
- Multi-file project-mode codegen via a concat-super-program.
- Auto-parallelization codegen — see [[design-concurrency-and-providers]].

## Auto-par reductions (new in round 2)

A new **auto-parallel reduction** path (`src/codegen/reduce.rs`), delivered in slices:

- **Reduction recognition** in the concurrency pass (slice 1) — a fold/reduce shape is
  detected as parallelizable.
- **`karac_par_reduce`** runtime primitive (slice 2) — fan-out + serial-combine, with the
  extern + reduction lookup wired into codegen (slice 3a) and lowering (slice 3b).
- **While-shape reduction lowering** (3b.4/3b.6) benchmarked at **9.87× kata-7 vs Rust**;
  the narrow-shape v1 lowering earlier measured a **4.1× wall-clock speedup**.
- **Cost-model gate** (3b.5) decides when to auto-parallelize a reduction; reductions are
  generalized to an **(op, type) matrix** (3b.1/3b.2).
- `karac_par_reduce` **pool-shares** workers with `karac_par_run` (3b.7). See
  [[design-concurrency-and-providers]].
- **Round-3**: reduction recognition extended to **Min/Max**, **conditional acc-update**,
  and **collect-style** (`ReductionOp::Collect`, `acc.push()`) gated on `#[par_unordered]`;
  cost gates tightened (per-stmt, inlining-aware, memory-bound rejection, dynamic gate).
  Worker-count override via `KARAC_PAR_WORKERS`.

## Par cancellation (new in round 2)

- **Result-slot ABI + Err-triggers-cancel** (Phase 7 line 67 slice 1a) — a `par` region
  returning `Result` cancels sibling work when one branch yields `Err`.
- **`spawn(closure)` par-region boundary** (line 63) — ownership boundary for spawned
  closures; **closure-capture mode migrated off the `Moved` state** (line 45).

## Hot-swap / JIT groundwork (new in round 2)

- **`--enable-hot-swap`** flag with profile gating (line 5 slice 1) and **codegen
  indirection** (slice 2); the indirection cost was **benchmarked**
  (`bench/hot_swap_cost/`, `docs/investigations/hot_swap_indirection_cost.md`).
- A **`.kara_jit_template` section + version manifest** (line 14) — groundwork toward the
  **online-JIT** direction (v65 positioning). See [[design-runtime-phases]].

## RC / codegen queries (new in round 2)

- **`#![rc_budget(max: N)]`** module-level RC-budget enforcement (line 43) and a **G12 RC
  creep monitoring** surface (line 27). See [[design-ownership]].
- **P1.3 codegen-queries analyzer** (line 25) — inlining + branch-hint analysis
  (`src/codegen_queries.rs`), and **`karac query monomorphization`** (line 97). See [[cli]].
- **`Option[shared T]`** lowering end to end (see [[design-generics-and-impl-trait]]).
- A **CI codegen containment guard** (line 3) keeps codegen from leaking across module
  boundaries.

## Round-3 codegen additions

- **User-`Drop` dispatch** — per-type `karac_drop_<T>` wrappers, scope-exit placement, and
  move-suppression on return-by-value / let-rebind. **`defer` / `errdefer`** lower onto the
  unified drop+defer stack. See [[design-ownership]].
- **`Atomic[T]`** — `.load`/`.store`/`.new` dispatch for int types; `Atomic[bool]` via i8
  slot-widening.
- **[[networking|Network / File codegen]]** — `src/codegen/file.rs` (File type lowering,
  method codegen, `KaracIoResult` unpacker, `FreeFileHandle` scope-exit close),
  `src/codegen/tcp.rs` (TCP over `karac_park_on_fd`, close-on-drop), `src/codegen/tls.rs`
  (rustls TLS surface). Structured concurrency lowers via `src/codegen/task_group.rs`.
- **Module-level bindings** — `src/codegen/module_bindings.rs` lowers `let` / `let mut`
  module bindings (const-init + composite-init), with cross-fn binding visibility. See
  [[stdlib-and-traits]].
- **`std.json` compiled path** — `Json.parse(s)` and `Json.stringify()` codegen
  (`src/codegen/json.rs`), so std.json works in a compiled binary, not just the interpreter.
- **Collections / literals** — `Vec[a, b, c]` prefix-literal at expression position,
  `Vec.remove(idx)`, `Vec.new()` / `VecDeque.new()` as module-binding const-init,
  `Vec.len()`/`is_empty()` on non-identifier receivers, and **uppercase-receiver method
  dispatch** (e.g. `Type.method()`) via a typechecker/lowering rewrite; undeclared
  `Type.method()` is now rejected at typecheck and gates the build.
- **Pattern / match** — `match` over a **`String` scrutinee**; byte-literal (`b'A'`) and
  range patterns; narrow enum-payload `bool` bindings to `i1` at reconstruction.
- **String printing** — print `String` via length-bounded `%.*s`; sign-/zero-extend narrow
  ints in f-string interpolation.
- **Binary size** — park the `jit_template` manifest in `__TEXT`, reclaiming ~16 KiB per
  Mach-O binary.

## Round-5 codegen

### RC elision — count-free reference-counted chains

The round's largest ownership/codegen effort: **[[rc-elision|RC elision]]**
(`src/ownership/elision.rs`, +3521) removes reference-count ops where a value's lifetime is
statically provable, phased **A → D** (single-owner frees → append-only chain free-walks →
cluster summaries + caller adoption → program-wide **headerless-T 16-byte nodes**). A
C-phase probe measured **+21% from removing the 8-byte header**. See [[rc-elision]],
[[design-ownership]], [[examples-and-benchmarks]].

### Portable SIMD and Tensor lowering

- **[[simd|Portable SIMD]]** — `Vector[T, N]` lowering to LLVM vector ops, with masks,
  gather/scatter, shuffle, a `Numeric` trait, `--simd-report`, and **WASM `+simd128` /
  `v128`** (`src/simd_report.rs`). See [[simd]].
- **[[numerical-stdlib-and-tensors|Tensor]]** — `Tensor[T, Shape]` core lowering (layout,
  constructors, indexing, drop), shape-transform families, reductions, and shape-generic
  body indexing (`src/codegen/tensor.rs`, +3360). See [[numerical-stdlib-and-tensors]].

### String-literal `match` → switch-tree

`match` on **string literals** now lowers to a **switch tree**, not a `memcmp` cascade —
resolved as the **#1 real-world codegen lever** by the self-hosted-lexer profiling spike.
See [[self-hosting]].

### General owned-temp tracking

A new **owned-temp classification** chokepoint (`materialize_owned_temp` + an
`owned_temp_drops` hint table) frees intermediate owned temporaries that previously leaked:
method-chain receiver temps, `ref_rvalue_arg` paths, discarded block-tail temps, Map/RC/
element discards, and fresh-owned String temps passed to `push_str` / `contains` /
`starts_with` and copy-consuming methods (`docs/spikes/general-owned-temp-tracking.md`).

### Oversized-enum-payload boxing

Enum payloads too wide for the inline representation are now **boxed** — pack/unpack for
`Option`/`Result` wide `T`, drop frees the box at scope exit, box-free for untyped-let boxed
enums and `Result.Err`, and a **fail-loud** guard replaced a **silent-truncation
miscompile** (`docs/spikes/oversized-enum-payload.md`).

### while-let / let-else + pattern-arm drops

`while-let` and `let-else` are lowered (489 slices 1+2), after a **fail-loud** stopgap that
replaced a silent miscompile; pattern-arm unbound heap-field drops for fresh-temp enum
scrutinees closed a spike (`docs/spikes/pattern-arm-unbound-field-drop.md`).

### Integer arithmetic, casts, and narrow ints

- **AOT integer arithmetic faults** — overflow / div-zero / `MIN`-div traps at declared
  width, reaching **interpreter parity**; **real narrow ints** trap at their declared width;
  mixed-width / mixed-signedness arithmetic is **rejected**.
- **`wrapping_add/sub/mul`** on 64-bit ints (an auto-vec unblocker) and **saturating
  float→int** + int↔float conversion method families.
- **Sub-64-bit ABI coercion** — widths coerced at ret / call-arg / binop / struct-field
  boundaries, with signedness printed for method/field reads.

### Ambient-resource + host-fn lowering (WASM)

Codegen lowers the ambient methods (`rand.next_u64`, `env.args`/`env.var`,
`Stdin.read_line`, `FileSystem.write`, `Stdout`/`Stderr` print) and **host fn** to wasm
import entries / `kara_host` imports (server-WASM). See [[wasm-targets]].

### Display and enum equality

- **User-type `Display` under codegen** — structs (with declaration-order parity) and
  all-unit + payload enums; **collection `Display`** in f-string / `to_string` (unified on
  buffer-append); `to_string()` / `clone()` / `abs()` on scalar primitives.
- **Variant-aware enum `==`** for concrete and generic heap-payload enum variants.

## Round-6 codegen

Round 6's codegen work is driven by the [[self-hosting|self-host parser]], the browser/proxy
[[examples-and-benchmarks|dogfoods]], and the **kata-gap audits** (cross-kata canonical-idiom
sweeps). New chokepoints: `src/codegen/borrow_elision.rs`, `src/codegen/shadow.rs`,
`src/codegen/synth.rs`, and the whole-program `src/effect_graph.rs` / `src/presize.rs`.

- **[[metaprogramming|Comptime / derive]] lowering** — `#[derive(...)]` is now desugared
  through the comptime substrate (reflect → build-AST → emit), and the emitted impls lower
  here; `#[derive(Default)]` + the `#[default]` variant marker, and the [[protobuf]]
  `#[derive(Message)]` encode/decode, are the first consumers. See [[attributes]].
- **[[per-layout-monomorphization|SoA across function boundaries]]** — per-layout
  monomorphization on a **LayoutId** axis: SoA `Vec[E]` values cross by-value / by-`ref` /
  return boundaries as distinct monomorphs, with field-level and whole-element SoA
  index-stores and heap-field-bearing SoA elements.
- **Borrow-elision** (`borrow_elision.rs`) — a conservative whitelist pre-pass elides the
  deep-clone on a read-only `let r = v[i]` heap-element binding, halving allocation on
  enumerate-then-scan loops; any uncertainty falls back to the clone (`B-2026-06-19-6`).
- **Collection pre-sizing** (`presize.rs`) — a loop-bound analysis rewrites a
  counted-loop-filled `Vec.new()` into `Vec.with_capacity(n)` (auto pre-size), plus a
  **realloc-based grow path** bringing `Vec`/`String` growth to Rust parity.
- **Overflow / bit arithmetic** — `checked_*` / `saturating_*` / `overflowing_*` lower via
  `llvm.{s,u}{add,sub,mul}.with.overflow.iN` at the declared width (`B-2026-06-19-10`), and
  integer `.pow(k)` + `count_ones`/`leading_zeros`/`trailing_zeros` lower via a counted loop
  + `llvm.ctpop/ctlz/cttz` (`B-2026-06-19-12`).
- **`Vec`/`Slice.binary_search`** lowers to LLVM's branchless `binary_search_by`, bit-for-bit
  matching the interpreter's std call (int + String elements) (`B-2026-06-19-3`).
- **String method lowering** — `trim` / `replace` / `to_lowercase` / `to_uppercase` via new
  full-Unicode runtime helpers (`runtime/src/clone.rs`), `char_at`/`char_count`,
  `chars().collect()` → `Vec[char]`, `String.from_utf8`, `ends_with`, and **ASCII fast-paths**
  for `String.push(char)` and `for c in s.chars()` (skip encode + memcpy). String buffer
  min-cap floored at 8 (matching Rust `RawVec`). See [[stdlib-and-traits]].
- **Shared-struct structural `==`** — an `Arc::ptr_eq` fast-path + a field-by-field walk
  through the RC layout (`B-2026-06-19-9`); comparison ops **auto-deref** reference operands.
- **`Set[Vec[T]]` / `Map[Vec[T],_]` content dedup** — Vec keys hash/compare by **content**
  (length + per-element), not by header pointer identity (`B-2026-06-20-15`).
- **Loop unrolling + BCE** — `llvm.loop.unroll.full` on small constant-trip counted loops
  (`B-2026-06-17-7`), and a **binary-search midpoint** `llvm.assume(mid >= lo && mid < hi)`
  folding the `nums[mid]` bounds check (`B-2026-06-16-1`).
- **Codegen-hint attributes** — `#[inline]` / `#[cold]` emit their LLVM attributes. See
  [[attributes]].
- **Type-changing `let` shadows** — a same-name shadow of a different type purges stale
  metadata (`src/codegen/shadow.rs`), enabling the same-scope `let` shadowing the resolver now
  allows.
- **Sub-word element stores** — a computed scalar pushed into a `Vec[u8]`/`[bool]`/`[u16]`/
  `[u32]` is narrowed to element width before the store, fixing a silent heap overflow
  (`B-2026-06-19-5`).
- **Map/Set key & value ownership** — the round's largest drop/leak family: no-adopt incoming
  key/element frees, present-key `remove` freeing the **stored** key via a runtime drop-flag
  ABI, `entry().or_insert()` write-through, and moved-for-loop-element defensive copies at
  retaining sinks. Mostly caught by the **Linux LSan gate**. See [[bug-tracker]].

## Round-7 codegen

Round 7's codegen work centers on **first-class function values** and the **heap-env closure
epic** — a large `src/codegen/closures.rs` growth (+2114) — plus the **`Column[T]` Arrow
backend** and smaller lowerings. Most of it is tracked in the [[bug-tracker|bug ledger]].

### First-class fn values

`Fn(...)` values now work in every position — argument, `let`, return, struct field,
`Vec[Fn]` element — annotated or inferred, matching the interpreter. The mechanism:
**`FnType` lowers to the `{fn_ptr, env_ptr}` closure fat pointer**, and a bare free-fn name
used as a value is **reified into `{trampoline, null env}`** through a memoized env-ignoring
forwarder `__karac_fnval_<name>`, so a plain free fn conforms to the env-first closure ABI.
Un-annotated extraction (`let g = h.f`) recovers the signature via a
typechecker→lowering→codegen side-table (`fn_value_typed_exprs`). Closed `B-2026-06-20-1`
(round 6's open entry) plus `B-2026-06-21-1/-2/-3`. `(h.f)(arg)` — calling a closure stored
in a struct field — was a **separate dispatch gap** (`B-2026-06-22-4`, silent return-0); now
any expression producing a closure value dispatches indirectly. See [[bug-tracker]].

### Heap-env closures (`B-2026-06-22-2`, epic started)

An **escaping capturing closure** read its captures from freed stack memory — a silent
miscompile. Round 7 built the fix incrementally: a **reference-counted heap env**
(`{i64 refcount, env}`) for a closure that outlives its frame, driven by
`CleanupAction::FreeClosureEnv` at the owner's scope exit, with **inc-on-copy** shared
ownership and **move-out** on return. Container stores (struct/tuple/array/`Vec[Fn]`) and
container escapes land, including a **dynamic `0..len` env-drop loop** for `Vec[Fn]`
teardown. An **exhaustive no-wildcard misuse guard** turns every not-yet-supported shape into
an honest `error[E_ESCAPING_CLOSURE_NOT_YET]` rather than a UAF, and an **auto-par bail-out**
keeps a heap-env closure out of a par-group return-struct join. Owner copy / by-value arg-pass
/ reassignment were still rejected at end of round 7 — **round 8 closes the remaining slices**
(see [[#Round-8 codegen]]). See [[bug-tracker]], [[design-ownership]].

### `Column[T]` Arrow-buffer backend

A large `src/codegen/column.rs` (+2079) lowers the new **[[columnar-data|`Column[T]`]]**
nullable columnar type: Arrow validity-bitmap + data-buffer layout, `fillna` (with the
`treat_nan_as_null` flag) / `dropna` / `from_iter_nullable`, `iter` / `iter_valid` as
Vec-returning forms, and **SQL three-valued-logic** arithmetic and comparison lowering. See
[[columnar-data]].

### Smaller lowerings

- **`Map.new()` / `Set.new()` as module-binding const-init** (`d30e076e` codegen,
  `e9bc1a6d` typecheck) — admitted as module-binding initialiser special forms, joining the
  round-3 `Vec.new()` / `VecDeque.new()`. Distinct from the `Map.new()` **K/V inference** fix
  (`B-2026-06-22-1`). See [[stdlib-and-traits]].
- **Scalar transcendental + rounding math on floats** (`1b404d96`, `src/float_math.rs`) —
  across typecheck + interpreter + codegen.
- **Uppercase-receiver field access on value bindings** (`c20a85b2`) — all three backends.
- **Boxed `Option`/`Result` binding move-into-aggregate UAF** (`5d03e64e`) suppressed.
- **Protobuf `#[derive(Message)]` encode/decode** grew a full proto3 surface (nested,
  repeated, enums, maps, float/double, sint/fixed/sfixed, oneof) via the comptime substrate.
  See [[protobuf]].

## Round-8 codegen

Round 8's codegen work **closes the heap-env closure epic**, adds an
**iterator-adaptor `.collect()`** desugaring, a **rolling-DP length-pin BCE** pass, and new
shared reduce infrastructure, plus a broad sweep of ownership / drop / literal-width fixes.
Most of it is tracked in the [[bug-tracker|bug ledger]].

### Heap-env closure epic CLOSED (`B-2026-06-22-2` → fixed, `be2ef68e`)

The multi-slice reference-counted-heap-env feature for escaping capturing closures is now
**complete** — round 7's headline open bug, closed in round 8. Every heap-env closure place is supported:
**RETURN** (bare tail / explicit / branch-leaf), **STORE** (struct field / tuple / array /
`Vec[Fn]` element — the Vec store `c11f3492` was the first **dynamic-count** env-drop loop),
**COPY/MOVE** (struct/tuple/array owner COPY, Vec owner MOVE, binding copy), **ESCAPE**
(aggregate + container move-out + caller adopt), by-value **ARG-PASS BORROW** (binding +
struct/tuple/array/Vec owner passed to a borrows-only callee), and **REASSIGNMENT** (`g=f` /
`g=make`, struct field `r.f=..`, Vec element `v[i]=..`). The last three reassignment slices:
binding (`30986b39`), struct field (`a51a09c0`), Vec element (`be2ef68e` — closes the epic).
The original soundness hole (an escaping closure reading a **freed stack env**) is resolved:
escaping closures get an RC heap env box (`{i64 refcount, env}`), every use site is
RC-accounted, and the **exhaustive no-wildcard misuse guard over-rejects** (never miscompiles)
any not-yet-modeled shape. An **auto-par bail-out** keeps a heap-env closure out of a par-group
return-struct join. Separately, `B-2026-06-22-4` (`4feed3b1`): calling a closure stored in a
struct field `(h.f)(arg)` silently returned 0 — **generalized closure-call lowering to any
expression producing a closure value**. See [[bug-tracker]], [[design-ownership]].

### Iterator-adaptor `.collect()` into `Vec`

`B-2026-07-03-25` (`009fd479`): `<iter>.map(f).filter(g).collect()` was **rejected under
`karac build` while working under `karac run`** — a run/build divergence. Now **desugared to a
for-loop** that pushes surviving/transformed elements. Extended to stateful passthrough
adaptors `take` / `skip` / `step_by` / `take_while` / `skip_while` / `inspect`
(`B-2026-07-03-29`, `76be2de2`); `enumerate()` terminal + non-terminal-heap
(`B-2026-07-04-4`, `39a1ca46`; `024bfe72`, `210eb93b`); named-fn args `.map(double)`
(`96861edb`); identity `.iter().collect()` (`59e2d892`). **Residual `B-2026-07-04-2` stays
open** (low): multi-source/restructuring adaptors `zip`/`chain`/`flat_map`/`chunks`/`windows`/
`scan`/`cycle`, multi-param/destructuring closures `.map(|(i,x)| ..)`, and non-terminal
f-string map still fall through to a **loud dispatch-fail** (no miscompile). `B-2026-07-05-1`
also open (low): a heap `enumerate` whose tuple is whole-copied downstream still gates to the
loud fail. See [[bug-tracker]].

### Rolling-DP length-pin bounds-check elision

`B-2026-07-04-6` (`b5d18320`): the natural rolling-DP inner scan `dp[c] = dp[c] + dp[c-1]` ran
~3× slower than clang -O3 because of **per-cell bounds checks**. A new pass
(`src/codegen/bce_length_pin.rs`, +2041) proves `bound == v.len()` from a **counted fill
loop** and elides the checks (kata #62 **316ms → 105ms, C parity**). Extended to for-range
fills, seed preludes, arithmetic bounds (`81746fff`), and nested-block fills (`6ca6fe44`), and
is **fail-closed**. A shadow soundness hole — a nested shadow of the pinned Vec or bound var
causing an OOB read or **silent wrong answer** — was found and fixed (`B-2026-07-04-13`,
`6ca6fe44`) with an exhaustive `region_bindings` collector.

### Shared reduce kernel + use-site consumption classifier

New codegen infrastructure: `src/reduce_kernel.rs` / `src/codegen/kernel.rs` — shared
fold / min-max / variance / element-wise-map / argmin-argmax / sort-scratch emitters for
Column / Tensor / Stats (see [[columnar-data]], [[stdlib-and-traits]]) — and a use-site
**consumption classifier** `src/codegen/consume_class.rs` (Phase 0 inert, then wired), which
underpins several ownership fixes (e.g. `B-2026-07-03-31`, borrow-vs-consume of an
Option-aggregate match payload).

### Other round-8 codegen fixes (all fixed)

- **First-class fn value `-> Self` return** (`B-2026-07-03-2`, `f6e35b1c`).
- **Chained call-result struct-field access** `make().field` read 0 (`B-2026-07-03-3/-16`,
  `839beaea`).
- **`f64.to_bits` / `i64.bits_as_f64` bitcasts** (`B-2026-07-03-1`, `0ceda7ab`).
- **Narrow-element collection literals** packed at contextual width (`B-2026-07-02-6`,
  `1078e747`) — a large **silent-wrong-answer class** across all literal sinks.
- **Generic by-value `Slice[T]` param Vec-coercion** (`B-2026-07-03-9`, `dda5d5de`).
- **`char.to_digit` codegen** (`B-2026-06-19-13`, `4e4b57de`, closing the last partial).
- Many **owned-temp / drop-surface slices** (slice 3d–3v) closing String / Vec / Map / Option
  / tuple element drops, and a large **presize / `with_capacity`** pass (`src/presize.rs`).

## Round-9 codegen

Round 9's codegen work is dominated by **making the JIT the default `run` backend** (which
forced a long run-vs-build correctness sweep), **finishing the `.collect()` surface**, a
per-target **`#[repr(C)]` struct-by-value ABI**, and **GPU LBM codegen** (see [[gpu-compute]]).
Most of it is tracked in the [[bug-tracker|ledger]].

### `karac run` flipped to JIT-default (LLJIT productionization)

The [[#JIT execution path (round 4)|JIT path]] — already the default for `karac test` /
`karac repl` since round 4 — now backs **`karac run`** too (**Slice 6c**, `ef7d355d`), with an
**`--interp`** escape to the tree-walk interpreter. The build-up: **Slice 1** folded the JIT
into the `llvm` feature (`99d4a314`); **Slice 3** registered JIT'd DWARF with the GDB JIT
interface (`0139a7d3`); **Slice 6a** stripped `run`-leniency so `run` rejects like `check`/`build`
(`14db4a82`); **Slice 6b** routed `run` through LLJIT opt-in (`19bd79b0`). The move is
deliberate: it collapses the **run-vs-build divergence** — the interpreter becomes a dev/debug
backend and codegen becomes the oracle. The sweep exposed JIT-only defects (`B-2026-07-07-4`
borrow-return -O2 OOB, `B-2026-07-07-5` ELF `--export-dynamic-symbol` so the JIT resolves the
`karac_*` runtime, `B-2026-07-07-6` REPL rebind, `B-2026-07-09-3/-4` REPL cell-output framing +
panic salvage) and ~16 codegen gaps a lenient interpreter had hidden. See [[bug-tracker]],
[[cli]].

### `.collect()` surface fully closed

Round 8 had the map/filter/passthrough/enumerate adaptors; round 9 **closes the residual
`B-2026-07-04-2`** — every iterator-adaptor `.collect()` shape now lowers at run==build parity,
LSan-clean: heap `zip` (via a general heap-index-read-into-owning-sink clone `84e64cf`),
`chunks`/`windows` (a fresh-temp block-return `c8a915d` — the true blocker was the synthetic AST
bypassing the ownership RC fallback), adaptor-carrying `chain`/`zip` sides, non-terminal
f-string map, `cycle().take(n)`, and `scan`. Two edges (bare unbounded `cycle`, a `None`-body
`scan`) loud-fail cleanly. The `B-2026-07-05-1` heap-enumerate whole-tuple-copy residual is also
fixed (`f1a5d49`).

### `#[repr(C)]` struct-by-value ABI (`B-2026-07-09-2`)

A `#[repr(C)]` struct passed by value across the C export boundary was **mislowered on
AArch64** (Apple silicon) — a raw LLVM struct-by-value that relied on the backend default
instead of explicit per-target ABI classification, a **silent miscompile** (returned `0.00`
not `7.50`). A real per-target aggregate classifier now emits: **AArch64 AAPCS** (HFA → v-regs,
non-HFA ≤16B → `[N x i64]`, >16B → indirect ptr / sret `x8`), **x86-64 SysV** (eightbyte
INTEGER/SSE, >16B → `byval`/`sret`), and **Microsoft x64** (1/2/4/8-byte → a register, else
by reference / `sret`). Landed in slices `991d3e2c`/`fa180294`/`6a6294fc`/`c3a68206`/`bc6a78cc`/
`4c90993d`, gated by a Linux forced-arch signature-match test + the 5-target CI matrix. See
[[design-unsafe-ffi-and-pointers]].

### GPU LBM codegen (Metal)

The [[gpu-compute|`gpu.dispatch`]] backend grew from element-wise-map to LBM kernels: a
**struct-SoA `gpu.dispatch`** (multi-buffer, `karac_runtime_gpu_map_multi`, CG-4), **multi-field
SoA groups** on the device (per-field interleave, GPU-LBM-3), and **value control flow** in
`#[gpu]` kernels (`if`→WGSL `select`, GPU-LBM-4 `514ee12c`). The control-flow work co-fixed a
general **float `if`-return phi-width** bug (a float literal branch beside an `f32` sibling fell
to an `i64 0` placeholder → `ret i64 0` against a `float` return, `B-2026-07-10-8`).

### Other round-9 codegen fixes (selected)

- **Interpreter `u64` model** (`B-2026-07-04-8`, `45eb926`) — span-threaded unsigned-64
  reinterpretation at every signedness sink; closes `u64 ≥ 2⁶³` run-vs-build divergence and
  unblocks `Column`/`Tensor[u64]` sort under build (`B-2026-07-07-2`).
- **`?` on `Result[<concrete enum>, E]`** stopped truncating the Ok payload
  (`B-2026-07-11-7`); **multi-word `?` error types** round-trip (`B-2026-07-09-20`).
- **`ref`-scrutinee enum** non-i64-word payloads (String/bool/narrow) — defer to the
  value-source reconstruction path (`B-2026-07-11-5`); a `Vec[struct]` enum payload keeps its
  element type (`B-2026-07-11-6`).
- **`SortedSet`/`SortedMap` now lower** under codegen, sharing the `Map`-backed storage with
  a materialize-at-iteration comparator (`B-2026-07-09-16/-17`); `Map`/`Set.try_insert`
  fallible-alloc codegen (`B-2026-07-09-15`).
- **`Display` for `Option`/`Result`** values / call-results, and **debug-format nested-struct
  `Display`** (`B-2026-07-08-9/-18`); `unwrap_err`/`expect_err` (`B-2026-07-09-10`).
- **`#[derive(Message)]` compiles under codegen** — a 3-layer fix (skip comptime fn bodies,
  typecheck derive-generated bodies, compile `std.protobuf` bodies, `B-2026-07-08-15`). See
  [[protobuf]], [[metaprogramming]].
- **`#[track_caller]`** caller-location redirection + stdlib panic-emitters (phase-5 slices),
  the diverging primitive **`panic()`** wired end-to-end (`B-2026-07-09-9`), **SSO** layout +
  inline-safe String-buffer gates (Slices 1–2, no-op foundation).

## Round-10 codegen

Round 10's codegen work adds two new lowering chokepoints (`src/inline_hints.rs`,
`src/format_spec.rs`), lands **iterator-terminal codegen** over fused map/filter chains,
threads **concrete heap types through generic monomorphs** (closing a large silent
double-free / garbage-read / invalid-IR family), and carries a broad correctness sweep across
lexical scoping, `match`, closures, and shared/RC drop completeness. Most of it is tracked in
the [[bug-tracker|ledger]].

### Compiler-driven inline hints

A new `src/inline_hints.rs` (+468) derives **inline hints from size + call-site heuristics** —
the compiler decides where to attach LLVM inline attributes rather than relying solely on
source `#[inline]` markers.

### f-string format specifiers

A new `src/format_spec.rs` (+435) lowers `f"{expr:spec}"` **format specifiers** — width,
zero-pad, alignment, radix, and precision on interpolated expressions.

### Iterator-terminal codegen

`fold` / `sum` / `reduce` / `for_each` / `any` / `all` / `count` now lower **on fused
map/filter chains** via a shared `peel_fused_map_filter_chain` + `build_fused_chain_body`,
with materialized `let it = v.iter()` bindings and `for x in <map/filter chain>` lowering.
Unlowered lazy adaptors now **LOUD-BAIL** (they were **silent-skip miscompiles**).

### Generic-monomorph heap-type threading

A new element-aware **`type_subst_type_exprs`** map — the twin of the head-only
`type_subst_names` — threads a generic param's concrete `TypeExpr` (e.g. `Vec[i64]`) into the
monomorph body, driving per-monomorph struct/enum layout + drop recovery
(`__karac_drop_struct_S$String`) and generic-enum heap-payload debox at concrete monomorph
width. This closed a family of **silent double-frees / garbage reads / invalid-IR**
(`B-2026-07-11-25/-31/-35`, `B-2026-07-12-16/-27/-28`, `B-2026-07-13-2/-3/-9`,
`B-2026-07-14-12`). See [[bug-tracker]].

### Lexical scoping — shadow revert at scope exit

Nested-scope variable **shadowing** now reverts shadowed bindings at scope exit via a
whole-environment lexical-scope checkpoint (`snapshot_var_env` / `restore_var_env`,
`B-2026-07-13-6`, `07fe865`) — previously a **silent-wrong-answer leak-past-scope**.

### Match — arm guards and tuple scrutinees

- Arm **guards are now evaluated** (were **silently ignored**, `B-2026-07-12-9`).
- **Tuple-scrutinee** `match` is now discriminated **element-wise** (`B-2026-07-12-13`).

### RC-elision guards (gated off)

- A **condition-4 payload-escape guard** for RC-elision (`KARAC_RC_ELIDE_REF_PARAMS`, gated
  off).
- RC-elision for **read-only borrow params** (gated off). See [[rc-elision]].

### Arithmetic / alias metadata

- **`nsw`** on for-range induction variables.
- **`getelementptr inbounds`** on bounds-checked element accesses.
- **Scoped-alias metadata** for slice params.
- **`readonly` / `noalias`** on Freeze `ref T` and owned value-semantics params.
- **TBAA was RETIRED** from the alias-metadata row (unsound for Kāra). See
  [[history-reversals-and-deprecations]].

### `karac run` (JIT) runtime symbol fix

**`karac_realloc_or_panic`** was added to the JIT runner's symbol-preserve list — `karac run`
(JIT) failed on any `Vec`/`String` grow past initial capacity (`B-2026-07-12-22`).

### Closures

- **By-reference env capture** for non-escaping stored mut-ref closures (`B-2026-07-11-23`).
- **Closure-returning-closure currying** (`B-2026-07-12-12`).
- **Heap-capture Slice 2** — escaping closures capturing `String`/`Vec`.

### Shared / RC drop-completeness sweep

A broad drop-completeness sweep landed: `Vec[shared]` / `Map[K,shared]` / `Option[shared]`
fields and payloads are now drained on drop and on match-move. See [[bug-tracker]].

## JIT execution path (round 4)

Round 4 built a **JIT execution path** and flipped it to the default for the test and REPL
surfaces (Phase 7 L558/L560/L577/L581); **round 9 flipped `karac run` too** (see
[[#`karac run` flipped to JIT-default (LLJIT productionization)]]):

- **MCJIT prototype** (L558, slices a.1–a.3) — an initial MCJIT entry point; recorded as a
  **finding** and superseded by the ORC path (see [[history-reversals-and-deprecations]]).
- **orc2 / LLJIT** (L560, W1–W5) — the chosen JIT: a printf round-trip on macOS arm64 (W1),
  a **`ResourceTracker`** + multi-module lifetime story (W2), JIT dispatch + E2E harness
  (W3.1–3.3a), and error-handling / threading edge cases (W5). A **`karac_jit_runner`**
  subprocess (`src/bin/karac_jit_runner.rs`) provides panic + stderr-atexit isolation.
- **`karac test` on JIT** — `test_jit_dispatch.rs` runs each test as a JIT subprocess with a
  per-test watchdog; JIT-published `SPAWN_SITES` addresses, `__emutls_get_address` for JIT
  `thread_local` (`runtime/src/emutls.rs`), and contract-predicate-FFI keep-lists were the
  enabling fixes. Flipped to **JIT-default** (`633bf7e5`, L577 step (c)).
- **`karac repl` on JIT** — a persistent-engine subprocess protocol (`--repl-mode`), cross-cell
  symbol amortization, a shared-module cache, and **value-snapshot persistent-let** ports for
  primitive / String / Vec / Map / Set lets (B.5.x). Flipped to **JIT-default**
  (`e06d877a`).
- **Coroutine passes on the JIT install path** (`456e13db`) — the async-I/O coroutine
  transform works under JIT too.
- The wholesale **"flip JIT to the default execution path"** commit was **reverted** for a
  third blocker, then re-landed piecewise (test half + repl half). See
  [[history-reversals-and-deprecations]].

## LLVM-coroutine async transform (A2, round 4)

The network-boundary async transform was **re-based onto LLVM coroutines** (the A2 track).
A spike (`docs/spikes/network-async-coroutine-transform.md`) recommended the coroutine path
after a bug in the round-2 hand-written **state-machine body-splitter** (a network call in a
helper function miscompiled — "bug C") forced an architectural fork. This **supersedes** the
state-machine transform (see [[design-runtime-phases]], [[history-reversals-and-deprecations]]).

- LLVM **coroutine passes wired into the pipeline** (slice 1), builder + `llvm-sys`
  coro-emission validated (2a), leaf-suspend shape (2b.2a), a **resume-shim drive bridge**
  (2b.1), network-boundary free fn compiled as a **dispatcher-driven** coroutine (2b.2b/2b.3),
  spawn drive for functional + method-handler coroutines (2b.4).
- **Drop-across-suspend correctness** on the coro destroy edge (slice 4) and a
  **cooperative cancellation mechanism** — a shim cancel-check + destroy-edge slot-signal
  (slice 5c). The cancellation mechanism landed; the trigger half is tracked separately.
- **Density-optimal non-blocking coroutine spawn** (slice 5a).
- **Flipped on by default** for `karac build`/`run` (`3eda2b06`), fixing two coro×auto-par
  interactions. A concurrent WS-over-TLS coroutine gate proves the flagship handler executes;
  a resume race (redundant accept park) was fixed (`c4c848bd`).
- Module data-layout was **pinned** so `coro.size` matches the AOT frame (fixed a heap
  overflow); fixed-size `Array[N]` coro-frame slots are sized inline for the same reason.

## Contracts / refinement / distinct-type lowering (Phase 9, round 4)

Codegen for the [[design-contracts-and-verification|Phase 9 verification surface]]:

- **Contracts** (`src/codegen/contracts.rs`) — emit `requires` preconditions, `ensures` +
  `old()` postconditions, and struct/impl invariants at method exits & constructors in AOT
  binaries. `karac build --release` **strips contracts** and the `?`-error-return-trace.
- **Refinements** (`src/codegen/refinement.rs`) — runtime predicate emission, base-layout
  codegen, value-dispatch-as-base; a **compile-time elision pass** removes checks the
  compiler can discharge.
- **Distinct types** — constructor, `.raw()` base extraction, base layout (no implicit deref).
- **Typed contract-fault categories** in the `test_fail` JSONL (contract-violated vs
  contract-predicate-panicked vs cross-call panic).

## Crash diagnostics + DWARF (round 4)

**Level 2 crash diagnostics** (`src/codegen/debug_info.rs`): Part 1 emits **panic location**
(`file:line:col` in fn); Part 2 emits **DWARF debug-info**. Production staticlib is built
without co-emitting the rlib to strip the backtrace symbolizer.

## Other round-4 codegen

- **`Atomic[T]` extensions** — `fetch_add` / `fetch_sub` (lock-free counter complete),
  atomic method dispatch on a **shared/`par` struct field** receiver, and `Atomic.new` in
  general expression position. See [[design-ownership]].
- **`par struct` / `par enum`** — first emitted a not-supported parser error, then
  implemented as always-`Arc` codegen (Slice A–D). See [[design-concurrency-and-providers]].
- **Shared-struct** — `self.field` read/write in ref-self methods; user `impl Drop` dispatch
  for shared structs and aliased holders (RC, L938/L940); constructor invariants.
- **HTTP client codegen** — `Client.get` / `Client.post` end-to-end (`src/codegen/http.rs`),
  chained `RequestBuilder`, `Response.text()`/`.bytes()`/`.header()`/`.headers()`. See
  [[networking]].
- **`std.tracing` codegen** — `StdoutExporter` emission surface, ambient `Log.*` emission,
  and active-span propagation (`with_span` + auto-stamp + `par` inherit). See
  [[stdlib-and-traits]].
- **Match ergonomics** — at-bindings under a `ref` scrutinee via write-through `via_ptr`;
  nested enum-payload bind + discriminate through `Result`/`Option`; `todo()`/`unreachable()`
  lower to a terminator with a live-branch value for diverging arms.
- **PIC / PIE** — emit PIC objects so x86_64-Linux binaries link as PIE.
- **Network construction returns `Result`** — the `fd:-1` sentinel was replaced with a real
  `Result` (see [[history-reversals-and-deprecations]]); named error variants
  (`AddrInUse`/`ConnectionRefused`).

## Intrinsics / trackers landed

`Pool[T]` (acquire/release), `process` (spawn/wait/try_wait/kill; round 4 added
stdin/stdout/stderr redirection via `Stdio.Inherit`/`Null`), `std.cli` subcommands. See
[[stdlib-and-traits]].

Related: [[compiler-pipeline]], [[implementation-phases]], [[deferred-work]],
[[design-runtime-phases]], [[design-contracts-and-verification]], [[rc-elision]], [[simd]],
[[numerical-stdlib-and-tensors]], [[wasm-targets]], [[self-hosting]],
[[fallible-allocation]], [[metaprogramming]], [[per-layout-monomorphization]], [[protobuf]],
[[windows-and-cross-platform]], [[columnar-data]], [[gpu-compute]].
