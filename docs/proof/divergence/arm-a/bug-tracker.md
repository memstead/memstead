---
type: bug-tracker
title: Bug tracker
updated_round: 10
---

# Bug tracker

## The canonical ledger — now populated (round 6)

The project's canonical bug tracker is **`docs/bug-ledger.jsonl`** — one JSON object per
line, with fields:
`id`, `date`, `source`, `surface` (affected compiler surface: `interp`, `codegen`,
`typecheck`, `parser`, `runtime`, `ownership`, `resolver`, `autopar`, `effect`, …),
`class`, `severity`, `status`
(`open` / `fixed` / `partial` / `not-reproduced` / …), `fix` (fixing commit SHA),
`title`, `tracker`.

**Round 6 stood the ledger up for real.** The rounds-1–5 note that the ledger was empty is
now **superseded**: round 6 landed **`docs/bug-ledger.jsonl` with ~152 entries** plus a
**human-readable `docs/bug-ledger.md`** view (generated from the JSONL, +243 lines) —
`d675004f` **retired the old `bugs.md`** in favor of the single JSONL tracker with a
generated readable view (see [[history-reversals-and-deprecations]]). This is a
**machine-countable** ledger: the old scheme of numbered "bug #N" / dated `B-2026-…-N` tags
living **only in commit messages** was replaced by structured records an agent (or a
bug-count curve) can query.

- **Ledger tooling** — `scripts/bug-lint.sh` (+89, validates the JSONL) and
  `scripts/bug-curve.py` (+231, plots a **bug-discovery/-fix curve**, `docs/bug-curve.svg`);
  a `scripts/oracle-sync-guard.sh` guards the [[self-hosting|self-host oracle]] provenance.
  `bug-curve.py` was made UTF-8/LF-safe so `--inject` works on Windows (`73d3dfce`).
- **Back-dated history.** Entries carry their original discovery date (`2026-05-20` through
  `2026-06-20`), so the JSONL back-fills the whole `B-2026-…` history the earlier rounds
  recorded only in commits — plus every round-6 fix.
- **Sources are tracked** — each entry's `source` names where it surfaced: `kata:NN`
  (exercise katas), `selfhost:lexer` / `selfhost:parser`, `dogfood:relay` / `:fathom` /
  `:slipstream` / `:weave` / `:cartographer`, `kata-gap-audit:<family>` (the cross-kata
  canonical-idiom audits), `lsan-gate:*` (the Linux LeakSanitizer gate), and `internal`.

**Open defects are now recorded.** Unlike rounds 1–5 (fixes only), the ledger carries real
`status: open` records. Round 6's headline open entry (**`B-2026-06-20-1`**, bare `fn` as a
first-class `Fn(...)` value) was fixed in round 7; **round 7's headline open entry — the
[[#Notable fixes recorded in the ledger (round 7)|heap-env closure epic]] `B-2026-06-22-2` —
is now `fixed` (round 8, `be2ef68e`, the whole ~30-slice epic closed)**, and the round-7
`partial` **`B-2026-06-19-13`** (`char.to_digit`) is now fully `fixed` (`4e4b57de`, codegen
landed). Most of the ~250 entries are `fixed`; `B-2026-06-20-17` remains `not-reproduced`
(a value-side `Map.remove` SEGV closed with churn-stress guards).

**All five of round 8's open entries are now closed** (round 9): the interpreter `u64` model
landed (`B-2026-07-04-8` → `fixed`, `45eb926` — a span-threaded unsigned-64 model), the
for-loop-element-move double-free family closed (`B-2026-07-04-17` → `fixed`, `278e1a91`;
plus the enum/Vec siblings `B-2026-07-05-2` / `B-2026-07-07-1`), the whole **iterator-adaptor
`.collect()` surface** was **fully closed** (`B-2026-07-04-2` → `fixed`, `9230632`), and
`B-2026-07-04-15` turned out to be a **misdiagnosis** (a legit `T: Ord` bound rejection of
`f64`, `c495dda3`) and `B-2026-07-05-1` was a real close (`f1a5d49`).

**Every one of round 9's open entries except the carried perf-gap is now closed** (round 10).
The round's headline open bug, **`B-2026-07-10-4`** (the self-hosted **item/type parser**
crashing at runtime, heap corruption), is **fixed** (`1b5f543`) — a copy-depth<drop-depth gap
in the caller-retains defensive copy of a by-value `Vec[struct]` arg carrying an
`Option[String]`/`Option[shared]` field, closed with a refcount-correct `Option[heap]`-value
deep-clone (`emit_option_value_clone_fn` + `te_owns_option_heap_payload`). **All three
self-host parser oracles (`selfhost_parser`, `selfhost_parser_items`,
`selfhost_parser_types`) are now un-`#[ignore]`'d and GREEN** — the full differential diff vs
the Rust seed parser passes, closing a multi-session drop/ownership investigation (the
clone-on-extract / entry-copy rc-inc family `a49a29f`, `55670cc`, `1b5f543`). The rest also
closed: `B-2026-07-10-7` (SoA collection-literal field-read segfault → `e86942e`, the SoA
let-init now scatters an AoS collection-literal RHS into SoA), `B-2026-07-10-6`
(`karac run`/JIT `gpu.dispatch` → `05d72ed`, a `#[gpu]`-kernel program now routes to the
tree-walk interpreter under `karac run`), `B-2026-07-11-1` (**a colliding id** — a turbofish
raw-pointer binding losing `T`, `8c4a32f`, and `Vector[T,N]` not classified `Copy`,
`4346b48b`; both fixed), `B-2026-07-11-9` (`687df6c`, an rc-fallback false-positive on a
loop-accumulator moved out via a terminal early `return` — the original filing's "no
`Iterator.collect()`" claim was author error, `collect()` already existed; the
`String.chars()` random-access/length half became `B-2026-07-11-13`, fixed),
`B-2026-07-11-10` (an empty `Vec.new()` pushed into a `Vec[Vec[i64]]` now infers the inner
element type via deep-resolve after unify), and `B-2026-07-08-7` (resolved as a macOS-only
allocator artifact — no allocator surgery warranted).

**The current open set (end of round 10) is eight entries — seven newly-standing plus the
carried `B-2026-07-10-5` — with exactly ONE `high`:**

| id | surface | class | severity | one-liner |
|----|---------|-------|----------|-----------|
| `B-2026-07-14-15` | codegen | double-free | **high** | `let r = m.get(k).unwrap()` on a `Map[K, V]` with a NON-shared heap value (`Vec`/`String`) double-frees under JIT/native (`free(): double free` at scope exit) — the `.unwrap()` binding is registered as an owned Vec/String with a scope-exit drop but its `{ptr,len,cap}` shallow-aliases the map bucket's value, so the map's value-drop and the binding both free it; interpreter correct. The non-shared sibling of the (fixed) shared-value over-retain `B-2026-07-14-3`; flagged to coordinate with the `.get().unwrap()` heap-value area (`B-2026-07-14-11`) |
| `B-2026-07-13-5` | codegen/typecheck | feature-gap | partial | three composable Tensor limitations block idiomatic generic-dim numerical `.kara`: (A) a reduction on a chained/non-identifier receiver (`a.zip_with(b,f).sum()`) fails codegen; (B) a generic shape param `D` is unusable in a function-BODY type annotation; (C) a `ref Tensor` arg to a tensor method — **gap C, the hard blocker, is FIXED**, unblocking the 1-D `std.embeddings` core; A and B remain open (ergonomic, clean workarounds) |
| `B-2026-07-14-11` | codegen | feature-gap | low | `Vec[Vec[T]].get(i).unwrap()` loses the inner `Vec[T]` type; a later `.len()`/index fails LOUD ("no handler for method 'len'"); interpreter correct. Workaround: the index form `g[i]` |
| `B-2026-07-14-6` | stdlib | feature-gap | low | several standard `Option`/`Result` combinators are UNIMPLEMENTED end-to-end (`map_err`, `map_or`, `take`, `err`, `ok`(Result), `flatten`, `get_or_insert`, `and_then`(Result), `or`/`or_else`) — now cleanly REJECTED at compile time (was a silent typecheck-poison before `B-2026-07-14-5`) |
| `B-2026-07-14-8` | codegen | feature-gap | low | proper codegen lowering for the iterator for-loop adaptor family (enumerate single-var, zip, skip/take/chain, step_by-on-iterator, flat_map, chunks, windows, cycle, scan, peekable, inspect) — currently they LOUD-BAIL (`B-2026-07-14-7`); the interpreter handles them |
| `B-2026-07-14-10` | codegen/interp | feature-gap | low | `for x in xs.iter_mut()` (mutable iteration) is unimplemented end-to-end; both backends now fail LOUD (`B-2026-07-14-9`); workaround is the index loop |
| `B-2026-07-14-14` | typecheck | ergonomics | low | a slice-pattern binding types as `ref T` and does not coerce to `T` in an arm tail (`[x] => x` errors, though `[a,b] => a+b` auto-derefs); AND `[] + [head, ..]` is reported non-exhaustive though total |
| `B-2026-07-10-5` | codegen | perf-gap | low | (carried) a diffuse instruction-density / scheduling gap (kata #76 two-pointer); the two-pointer-BCE fix direction stays REVERSED as unsound; independently re-confirmed no sound lever on x86 (kāra is actually 2–10% AHEAD of equal-safety rustc on x86 density; the residual is aarch64 scheduling) |

(`B-2026-07-14-15` is the sole `high`, `B-2026-07-13-5` is `partial`, the rest `low`. For the
record: `B-2026-07-11-8` and `B-2026-07-11-36` are `invalid`/reverted misdiagnoses, and
`B-2026-07-11-24` was `partial` with its residual layers closed by the fixed
`B-2026-07-11-29`.)

## Notable fixes recorded in commits (round 1)

- **Codegen — chained field access** returning `0` at depth ≥ 2. (fixed)
- **Codegen — `var_type_names` struct-identity collision** on UFCS calls. (fixed)
- **Codegen — three `Vec[T]` auto-par bugs** blocking `Slice`-param and `Vec`-binding
  shapes; plus three earlier `Vec[T]` auto-par codegen bugs. (fixed)
- **Codegen — match-arm bindings** now reconstitute struct payloads from an i64 word.
  (fixed)
- **Codegen — three real bugs** surfaced by hardening the codegen test harness
  (fail-loud on parse/compile errors). (fixed)
- **Codegen — `bfs_sieve` leak**: closed by per-arm match cleanup + early-return cleanup;
  a residual leak was attributed to two further gaps and tracked before closure. (fixed)
- **Exhaustiveness — Maranget O(N²)** performance bug. (fixed)
- **Test infra — Windows debug stack ceiling**: run interpreter / fib e2e on an 8 MB fat
  stack thread. (fixed)
- **Test infra — env-var races** in `http_server` and `ACTIVE_FRAMES` tests: serialize on
  a shared mutex. (fixed)
- **Codegen — `af97e03e` rc-inc nested bare-shared field UAF**: this fix is noted in the
  source-repo record as being **reverted later** (in a subsequent slice, commit
  `948d5527`) — **still not in the round-2 window** either.

## Notable fixes recorded in commits (round 2)

RC / drop correctness dominates this round's fixes:

- **RC on shared struct/enum in Maps/Sets** — `rc_inc` on move-out of shared-struct/enum
  locals (**bug #7**), `rc_dec` of shared struct/enum **keys** and **values** on `Map`/`Set`
  drop, walking shared-K/V halves on a struct-field `Map` drop, and decrementing a displaced
  shared value on a discarded `Map.insert` overwrite. (fixed)
- **Codegen — bug #8 call-chain field access** extended to `MethodCall` callees, branch-tail
  RHS fresh-ref detection, and skipping receive-side `rc_inc` on `Call`/`MethodCall` RHS.
  (fixed)
- **Codegen — f-string self-assign + tail-return double-free**. (fixed)
- **Codegen — `char` rendering**: render `char` as a glyph in `println` / f-strings and fix
  a zero `CharLit`. (fixed)
- **Codegen — per-iteration leaks**: closed a per-iter `Vec`/`String` leak on the auto-par
  branch + slot paths; added per-iteration cleanup + null-guarded `RcDec` for body-local
  lets; suppressed RC dec for par-branch return-slot sources. (fixed)
- **Codegen — `Vec.extend_from_slice`**: reject src/dst overlap on grow; per-element clone
  for RC-bearing `T`. (fixed)
- **Runtime (event loop) — drain a pending waker event** at background-thread shutdown.
  (fixed)
- **Concurrency — skip par dispatch** when N−1 statements are constant-init lets (avoid
  pointless fan-out). (fixed)
- **Test infra** — gate POSIX-only process-spawn tests on `unix`; make the clean-global path
  assertion platform-native.

## Notable fixes recorded in commits (round 3)

- **Intermittent suite hang** — root-caused to **user-vs-seeded-enum disambiguation** not
  covering `Json` + `TcpError` (fix `bdbaadd`); the concurrent-`Command::output()`
  investigation closed on it. A **per-spawn hang watchdog** was added to the codegen e2e
  `Command::output()` harness (`62af025`), plus three P0/P1 hang-guardrail tracker entries.
- **Match — byte-literal + range patterns matching unconditionally** (fixed).
- **Codegen — `vec[i]` to a `ref` param**: borrow the `Vec` element in place instead of
  copying (fixed).
- **Codegen — ref-param slot**: dereference a ref-param slot before a field-receiver method
  GEP (fixed); materialize an rvalue at a `ref T` call-arg position and drop-register ref-arg
  rvalue temps with `Vec`/`String` layout.
- **Codegen — f-string interpolation**: sign-/zero-extend narrow ints (fixed).
- **Typechecker** — add missing arms the codegen already handled: `Vec.from_slice`,
  `Vec.filled`, `push_str`; push expected element type into `Vec.filled` / `Entry.or_insert`
  fill args; propagate expected type through `Vec.with_capacity` / `VecDeque.with_capacity`.
- **Test infra** — serialize env-var tests in `build_cache` on a module-static mutex (flake
  fix); **kata-8 `atoi` end-to-end ASAN regression guard** (`tests/memory_sanitizer.rs`).

## Notable fixes recorded in commits (round 4)

- **Bug C — network-call-in-helper miscompile** (the biggest): the round-2/3 hand-written
  **state-machine body-splitter** miscompiled a network call inside a helper function. It was
  confirmed demo-affecting and broader than control flow, IR-localized, and drove the
  **architectural fork to LLVM coroutines** (the A2 track). See [[design-runtime-phases]],
  [[history-reversals-and-deprecations]].
- **Coroutine heap overflows** — module data layout was **pinned so `coro.size` matches the
  AOT frame**, and **fixed-size `Array[N]` coro-frame slots** are sized inline; a WS-over-TLS
  **resume race** (a redundant accept park) was removed.
- **A "bug N of N" codegen series** — suppress enum drop after match destructure (1), suppress
  source `Vec` cleanup on tuple construction (2), array-binding `Vec`-cleanup guard +
  force-link expansion (3), per-field hash+eq for non-shared user-struct map keys (4);
  **JIT-only** (5) `__emutls_get_address` for JIT `thread_local`, (6) JIT-published
  `SPAWN_SITES` addresses rather than bin stand-ins.
- **Double-frees / drop correctness** — zero-init the f-string accumulator at entry;
  move-aware `Drop` for HTTP `Response`/`RequestBuilder` + struct let-move; synthesized `Drop`
  for `HttpError` frees its message; free an abandoned chained-`RequestBuilder` temporary;
  suppress `FreeMapHandle` for a tail-returned `Map`; `ret void` / `ret i32 0` terminator
  fixes.
- **Refcount** — coherent shared-struct RC for let-copy / `Option[shared]` capture+return;
  retain-before-release on an `Option[shared T]` field store; per-branch refcount compensation
  for mixed `Option[shared]` tails; shared-list cursor + branch-return + field-arg refcount.
- **Runtime — macOS WS-upgrade bug**: an accepted socket stayed non-blocking after a
  non-blocking listener; forced blocking after accept. Also parallelized the WS-over-TLS
  handshake off the accept thread and gated `event_loop` `register_fd` wake on the background
  poller.
- **`heterogeneous-type binop`** now returns a structured error instead of panicking.
- **REPL/JIT** — preserve per-line newlines in JIT stdout capture (this was two of the
  JIT-default-flip blockers, a real newline bug); dedup impl-method bodies across cells
  (duplicate-symbol); drop the runner on a cross-cell `let` shadow.

## Notable fixes recorded in commits (round 5)

Round 5's fixes cluster around **codegen ownership / drop correctness** — mostly surfaced by
the [[self-hosting|self-hosting port]] and the [[examples-and-benchmarks|density
benchmarks]]. All are recorded as fixed.

- **Self-hosting blocker chain (`#1`–`#20`)** — the lexer port drove a long series of
  codegen fixes: **#1** a struct field of a user enum collapsing to i64; **#2** mut-ref self
  method mutations not persisting (interp CICO write-back); **#3** an f-string as a
  match-arm value compiling to empty; **#5** a String/Vec method on a `self.field` receiver;
  **#6** skip a stdlib module whose type name the user redefines; **#7** coerce if-branch int
  widths before phi; **#8** auto-par dep analysis not tracking `self` reads/writes (**caught
  by the differential lexer oracle**); **#9** let-bound enum heap-payload drop on move-out;
  **#10** `char.try_from`; **#11** integer parse typing; **#12** tail-less diverging block
  typed `Never` not `Unit`; **#14** callee-own by-value aggregate double-free; **#15/#17/#18/
  #19** enum-field / nested-enum-leaf / aggregate move-suppression + drop; **#20** inline
  call/method-result String arg-temp frees.
- **The `B-2026-06-1x` codegen-drop clusters** — dated bug families, e.g. **`B-2026-06-12-6`**
  (Vec[struct/enum] element drop, match-bound Map/Set frees, boxed-Option inner struct,
  explicit-return field-alias double-inc — clusters 2/4/5 + gap 2), **`B-2026-06-11-*`**
  (block-expression tail heap value, raw-pointer deref `*p`, chained tuple index, by-value
  aggregate heap-field drops, block-arg temp leak, lean-fatal-paths ~250 KB std-IO anchor),
  **`B-2026-06-10-*`** (Vec[(.., String)] tuple-heap clone-UAF + scope-drop leak,
  borrow-returning-call double-frees, `Map.clear()` key/value buffer leak, inline-heap
  `Option[T]` payload drop), and **`B-2026-06-07-*` / `B-2026-06-09-*`** (borrow-return
  tiers, f-string interpolation span rebasing, Map/Set move-suppression at enum ctors).
- **wasm alloc-wrapper** — corrected the `size_t` width + a `malloc` extern clash
  (`B-2026-06-12-1`, `B-2026-06-11-9`).
- **`process.exit` / narrow-int arithmetic traps** — AOT integer faults (overflow / div-zero
  / MIN-div) reached interpreter parity; sub-64-bit widths coerced at ABI boundaries.
- **Concurrency correctness** — **par slot-ownership transfer UAF** (a branch freed the
  Map/enum/struct/Drop value it published); **`llvm.coro.save` before I/O-park publish**
  (cross-thread frame UAF); **SIGPIPE masked in reactor init** (silent server death under
  reconnect storm); **poll-fallback wakeups** that Stage B2 dropped; interp `Atomic[T]` /
  `Mutex[T]` made a **shared `Arc<Mutex<Value>>`** so `par` branches don't race.
- **Windows build reds** (a run of cfg-gating fixes) — unix-gate ws-helper tests, run the
  CLI on a **16 MB fat-stack thread** (the 1 MB main stack overflowed), normalize path
  separators in the lockfile hasher, drive-prefix in `affected-by` targets.

## Notable fixes recorded in the ledger (round 6)

Round 6's ~152 ledger entries cluster into a few families, most surfaced by the
[[self-hosting|self-host parser port]], the **kata-gap audits**, and the browser/proxy
[[examples-and-benchmarks|dogfoods]]. Highlights:

- **Memory-ownership class (Map/Set keys + values).** A long, systematic sweep closed a
  map/set key/value ownership class the earlier map fixes only started: no-adopt incoming
  key frees (`B-2026-06-20-9`), present-key `Map.remove`/`Set.remove` freeing the **stored**
  key/value via a new runtime **drop-flag ABI** (`B-2026-06-20-10`, archive rebuild),
  `Set` incoming-element no-adopt frees (`B-2026-06-20-12`), and `Map.entry().or_insert()`
  **write-through** via a `MapSlotRef` place-ref + `Map.get_or` (`B-2026-06-20-8`). Most were
  **Linux-LSan-only** — macOS ASAN misses still-reachable leaks, so the **Linux LeakSanitizer
  gate** (`scripts/lsan-local.sh`, a colima harness) is the authoritative leak gate.
- **Self-host shared-enum drop/leak cluster.** The parser's `shared enum` AST model drove a
  run of drop-walker fixes: shared-enum String/Vec payload move-out double-free + boxed-struct
  payload rc-drop + method-arg materialize (`B-2026-06-20-14`), the render-leak cluster, and a
  cross-function basic-block reference regression (`B-2026-06-14-34`).
- **Moved-heap-into-spawn family.** A heap value moved/borrowed into a `spawn`/`tg.spawn`
  closure being freed by the wrong owner — double-free in a loop (`B-2026-06-18-8`), borrowed
  capture leak (`B-2026-06-19-2`), shared read-only capture across sibling tasks
  (`B-2026-06-19-11`), and a moved channel-end closing before its send (`B-2026-06-17-9`). Same
  family as the [[windows-and-cross-platform|Windows spawn-leak reap]] work.
- **kata-gap audits → stdlib completeness.** Cross-kata canonical-idiom audits filed and fixed
  whole method families: String `trim`/`replace`/`to_lowercase`/`to_uppercase`
  (`B-2026-06-20-2`), `Vec/Slice.binary_search` codegen (`B-2026-06-20-3`), `SortedMap[K,V]`
  (`B-2026-06-20-5`), checked/saturating/overflowing arithmetic + `pow` + bit intrinsics
  (`B-2026-06-19-10`/`-12`), `char.to_digit` (`-13`), `char.is_uppercase/lowercase` (`-18-4`),
  `s.char_at`/`char_count` (`-18-3`), `s.chars().collect()` (`-18-1`), and `Set[Vec[T]]` /
  `Map[Vec[T],_]` **content dedup** (`B-2026-06-20-15`). See [[stdlib-and-traits]].
- **SoA cross-function** — the entire [[per-layout-monomorphization|per-layout-mono]] story is
  ledger entry `B-2026-06-19-14` (a very large multi-slice record), plus field-level SoA
  index-store (`B-2026-06-20-7`) and SoA heap-field elements (`B-2026-06-20-18`).
- **Auto-par correctness** — a **missed write-dependency race**: a map-mutating loop was
  co-grouped with a later read of the same map because the dependency analysis had no arm for a
  write through a deref/method-chain target (`*m.entry(k).or_insert(0) += 1`), racing
  `m.keys()` — a silent wrong-answer under the default `karac build` (`B-2026-06-20-16`). See
  [[design-concurrency-and-providers]].
- **Codegen quality/correctness** — a String `==`/`!=` **memcmp overread** clamped to
  `min(len)` (`B-2026-06-20-4`), sub-word `Vec`/slice element stores narrowed to element width
  (a silent heap overflow, `B-2026-06-19-5`), shared-struct structural `==` field-walk
  (`B-2026-06-19-9`), a `Vec.filled` 2D-DP-table shallow-copy miscompile (`B-2026-06-19-8`),
  and read-only `let r = v[i]` **borrow-elision** to halve enumerate-then-scan allocation
  (`B-2026-06-19-6`). See [[codegen]].
- **Many original diagnoses corrected.** A recurring ledger discipline this round: entries
  record the **disproven hypothesis** alongside the real root cause (e.g. `B-2026-06-20-4` was
  first mis-attributed to a double-free but was a memcmp overread; `B-2026-06-18-8` was
  mis-read as an "ASAN-invisible race" that turned out to be miscounted concurrent `println`).
  See [[history-reversals-and-deprecations]].

## Notable fixes recorded in the ledger (round 7)

Round 7's ledger churn is dominated by **first-class function values** and the **heap-env
closure epic** — a multi-week saga that turned a silent miscompile into a large,
incrementally-landed codegen feature. See [[codegen]].

### First-class fn values — B-2026-06-20-1 closed, then a four-entry chain

- **`B-2026-06-20-1` — bare `fn` as an `Fn(...)` value** (the round-6 open entry) is
  **fixed** (`79f1de14`). `FnType` now lowers to the **`{fn_ptr, env_ptr}` closure fat
  pointer** (was an 8-byte `i64` fall-through); a bare fn name at an `Fn`-typed arg site is
  **reified into `{trampoline, null env}`** via a memoized env-ignoring forwarder
  `__karac_fnval_<name>`, so a plain free fn conforms to the env-first closure ABI.
- **`B-2026-06-21-1` — `let`-bound fn value** (`fcc2c925`): `let f = doubler; apply(f, x)`
  and a direct `f(x)` through the local. The direct-call case had *silently returned 0*
  before (unregistered name → unknown-callee stub) — a wrong-output miscompile.
- **`B-2026-06-21-2` — return / struct-field / `Vec[Fn]` positions** (`98731b72`): lowering
  the bare fn name to the fat pointer at the single `compile_expr` Identifier source fixes
  every value position at once; the interpreter already handled fn values (verified, not
  fixed).
- **`B-2026-06-21-3` — un-annotated extraction from a field / element** (`7010bc86`): the
  architecturally-general fix — the typechecker maps `Type::Function` to a `FnType` TypeExpr,
  a lowering side-table (`fn_value_typed_exprs`) carries it, and codegen recovers the
  signature there. **First-class named-fn values are now complete across arg / let / return /
  struct-field / Vec positions, annotated or inferred, in both `karac build` and `karac run`.**

### The heap-env closure epic — `B-2026-06-22-2` (was open/high in round 7; **fixed round 8**)

A capturing closure that **outlives its defining frame** (returned, or stored in an escaping
struct/Vec/global) read its captures from **freed stack memory** — a silent wrong-output
miscompile (`fn make(k){ |x| x+k }; f(5)` printed garbage). Root cause: closure envs were
stack allocas in the outer frame, with **no heap env, no escape analysis, no env drop**. The
interpreter is unaffected. This one entry accreted ~15 slices across the round:

- **Guard slices** (`ab6ff88b`, `c006ccda`, `3829b1de`, `f6a6db65`, `da046ae8`) — a
  one-sided **`reject_escaping_capturing_closure`** pass turns the silent miscompile into an
  honest `error[E_ESCAPING_CLOSURE_NOT_YET]` for tail returns, explicit returns, aggregate
  literals, local-aggregate-then-return, field projection (`return h.f`), and the
  comprehensive **store-escape class** (push/insert/index-store/field-store). Sound
  under-approximations throughout (never rejects a program that compiles+runs today).
- **Heap-env feature slices** (`88bfa7de` and on) — a function returning a capturing closure
  gets a **reference-counted heap env** (`{i64 refcount, env}`), freed via
  `CleanupAction::FreeClosureEnv`; an **exhaustive no-wildcard misuse guard** rejects every
  not-yet-supported use. Then, slice by slice, the rejected shapes became *supported*:
  **copy** (`let g = f`, inc-on-copy, `ca644379`), **return-again** (move-out, `5141672e`),
  **store-in-struct** / binding-source store (`e556fd01`, `e516bad5`), **aggregate escape**
  (return a closure-owning struct, `39bfd196`), **tuple / array / Vec stores** (`7888a761`,
  `f7b9851c`, `c11f3492` — the first *dynamic-count* env drop loop), and **container escape**
  (return a closure-owning tuple/array/Vec, `40e682eb`, `e6856fba`). An **auto-par bail-out**
  keeps a heap-env closure out of a par-group return-struct join.
- **Round-7 residual (owner copy / by-value arg-pass / field & element reassignment) — all
  closed in round 8.** At end of round 7 these shapes stayed rejected (owner **copy**
  `let s = a` / `let w = v`, by-value owner **arg-pass**, and **reassignment** `g = f` /
  `r.f = g` / `v[i] = make(j)`). Round 8's remaining slices supported every one — owner copy
  (struct/tuple/array) + Vec owner move, by-value arg-pass borrow, and all three reassignment
  forms (binding `30986b39`, struct field `a51a09c0`, Vec element `be2ef68e`) — closing the
  epic (`status → fixed`). See [[#Notable fixes recorded in the ledger (round 8)|the round-8 section]].

### Other round-7 ledger entries

- **`B-2026-06-22-4` — `(h.f)(arg)` returns 0** (`fixed`, `4feed3b1`): calling a closure
  stored in a **struct field** silently returned 0 under `karac build` (correct under
  `karac run`) — `compile_closure_call` only dispatched a **named-identifier** callee, so a
  `FieldAccess` callee fell to a const-0 stub. No existing test *ran* a struct-field closure
  call, so the gap was invisible. Fix generalizes closure-call lowering to any expression
  producing a closure value. A distinct dispatch gap, independent of `B-2026-06-22-2`'s
  escape hole.
- **`B-2026-06-22-1` — `Map.new()`/`SortedMap.new()` K/V inference** (`fixed`, `387c9346`):
  an un-annotated `let m = Map.new(); m.insert("a", 1)` failed `karac check` because the
  insert/get arms only ran `check_assignable` and never **unified** the argument type into
  the slot typevar (unlike `Vec.new()+push`). A `check_map_slot_arg` helper now pins K/V.
  Pure inference gap; surfaced building protobuf `map<K,V>` support. Note: this is *distinct*
  from admitting `Map.new()`/`Set.new()` as **module-binding const-init** (`e9bc1a6d` /
  `d30e076e`), which is a separate round-7 feature (see [[stdlib-and-traits]]).
- **`B-2026-06-22-3` — non-reproducible wasm component bytes** (`fixed`): `--bindings
  component` (the `wasm_wasi` default) produced a **different SHA256 per build** — the core
  module was linked under a **process-unique `karac_<pid>_<stem>.core.wasm`** scratch name
  that `wasm-ld` baked into the `name` section. Not HashMap order (the first hypothesis).
  Fix links under a **source-derived basename inside a pid-unique directory**, so parallel
  builds stay collision-free on disk while the component is byte-identical run to run.
  Surfaced by the new **[[examples-and-benchmarks|`bench/wasm_size`]]** size receipt. See
  [[wasm-targets]].

## Notable fixes recorded in the ledger (round 8)

Round 8 added **~120 new ledger entries** (`B-2026-06-29-*` through `B-2026-07-05-1`) and
closed the two round-7 open/partial headliners. The churn clusters around **run/build
divergences** (the interpreter and LLVM backend disagreeing), the **heap-env closure epic**,
the new **[[columnar-data|DataFrame / Stats]]** and **[[stdlib-and-traits|Reduce trait]]**
surfaces, and a broad **[[stdlib-and-traits|trait-system]]** build-out.

- **Heap-env closure epic CLOSED — `B-2026-06-22-2` → `fixed`** (`be2ef68e`). The round-7
  `high` open bug (an escaping capturing closure reading a freed stack env). Every heap-env
  closure place is now supported — return, store (struct/tuple/array/`Vec[Fn]`), copy/move,
  aggregate + container escape, by-value arg-pass borrow, and reassignment — via an
  RC heap env box and an exhaustive over-rejecting misuse guard. See [[codegen]].
- **A large run-vs-build divergence sweep.** Many entries are a surface where `karac run`
  and `karac build` disagreed — often a *silent* miscompile under build. Examples:
  `B-2026-06-30-1` (`char` across a call-return boundary printed as its codepoint under
  build), `B-2026-07-02-6` (narrow-element collection **literals** packed at i64 width at
  every sink — a big silent-wrong-answer class, `1078e747`), `B-2026-06-30-7`
  (`for s in self.field.iter()` silently iterated 0 times), `B-2026-07-03-2/-3/-16`
  (`-> Self` returns + `make().field` chained access read 0), and a run/build **unification**
  family — `mut ref` scalar auto-deref (`B-2026-06-30-9/-10`), atomic-op explicit-ordering
  (`B-2026-06-30-5`), int×float arithmetic now rejected on all three surfaces
  (`B-2026-07-04-11`, `444e6cb0`), and float+int-literal promotion in the interpreter
  (`B-2026-07-04-12`).
- **Trait-system correctness** — trait default methods now inherited (`B-2026-07-03-8`),
  generic-trait defaults (`-10`), user impls on primitive scalars (`-5`), generic-bound
  dispatch + monomorphization (`-11`/`-15`/`-23`), operators on operator-trait-bounded type
  params (`-18`), and derived-`Ord` struct/enum ordering (`-7`) with the sibling interpreter
  data-loss + declaration-order fixes (`-6`/`-12`) and the bare-`Ordering`-variant match
  miscompile (`-14`). See [[stdlib-and-traits]].
- **Ownership-gate false-positives.** Standing up an **E2E ownership gate** on the codegen
  harness flushed a batch of `karac check` false-positives that the harness had masked:
  printing consumed its arg (`B-2026-07-02-21`), `for x in v` was read as a move
  (`-22`), comparison operators / field-path uses / a bare fn value consumed the whole
  binding (`-23`/`-25`/`-24`), and a borrowed `mut ref self` field-match consumed `self`
  (`B-2026-07-03-26`, which had reddened the self-host parser oracles). Several
  grandfathered tests were un-grandfathered as the fixes landed.
- **Map/Set + collection ownership tail** — a String-element / heap-element drop-surface
  sweep (owned-temp slices 3d–3v, e.g. `B-2026-07-02-1/-2/-3`), Vec-literal element
  double-frees (`B-2026-07-04-1`), and the `Option[<heap agg>]` struct-field drop class
  (`B-2026-07-03-27/-28/-31`, `B-2026-07-04-7/-9`) — largely the **caller-retains param
  model** being completed, and Linux-LSan-only.
- **wasm playground was 100% broken** — `B-2026-07-02-29/-30` (`fixed` `0624f0fe`): the
  Windows fat-stack fix lifted every interpreter run onto a `spawn_scoped` thread, which
  `wasm32-unknown-unknown` cannot spawn, so *every* playground run trapped; and
  `SystemTime::now()` seeding panicked "time not implemented". Both guarded, plus the rest of
  the sys/unsupported class (sleep, `par{}`→sequential, clock, rate-limiter). See
  [[playground]], [[wasm-targets]].
- **Auto-par correctness** — silent data corruption when `sort`/`pop`/`remove` were invisible
  to the write-dependency gate (`B-2026-07-02-8`), a `Column`/`DataFrame`/`Tensor` early-freed
  across a par slot boundary (`B-2026-07-03-32`), and a recursive-delta reduction that
  SIGBUS'd (`B-2026-07-03-14`, superseded by the shallow-depth fork-depth cap). See
  [[design-concurrency-and-providers]].
- **`karac test` cross-package** — `B-2026-07-01-4/-5` (`fixed` `c6aa55c6`): a test body
  calling an imported name panicked / a companion re-import tripped E0101; both fixed by
  running tests against the merged super-program + deduping exact re-imports. See [[cli]].
- **REPL shadow-replay** — `B-2026-07-02-36/-37` (`fixed`): re-binding a persistent `let`
  deterministically bricked a REPL session; fixed with per-shadow alpha-rename CFG frames +
  a last-binder-span snapshot key.

## Notable fixes recorded in the ledger (round 9)

Round 9 added **~90 new ledger entries** (`B-2026-07-06-*` through `B-2026-07-11-*`, plus the
GPU-LBM `B-2026-07-10-{5,6,7,8}`) and **closed all five round-8 open bugs**. The dominant
theme is the fallout of flipping **[[cli|`karac run` to JIT-default]]** — the interpreter's
leniency had been masking a long tail of codegen-vs-interpreter gaps that the sweep exposed.

- **The JIT-default flip + its blast-radius sweep.** The LLJIT productionization slices
  (6a strip run-leniency, 6b route `run` through LLJIT, **6c flip to JIT-default** `ef7d355d`)
  first surfaced JIT-only defects — a borrow-returning fn writing out of bounds under -O2
  (`B-2026-07-07-4`, `fddfb9af` — a `ref` slot mis-treated as an inline `{ptr,len,cap}`), an
  **ELF `--export-dynamic-symbol` gap** where all `karac_*` runtime symbols lived only in
  `.symtab` so the JIT couldn't resolve them (`B-2026-07-07-5`, `199098e4` — masked on macOS),
  and a REPL cross-type-rebind crash (`B-2026-07-07-6`, `8ab9e794`). Then, routing every `run`
  through codegen exposed ~16 **run-vs-build** gaps: `Option`/`Result` `Display` under codegen
  (`B-2026-07-08-9`), a struct with a `Map` field built in a constructor emitting invalid IR
  (`B-2026-07-08-12`), interpreter `Map`-through-a-struct-field mutation not persisting
  (`B-2026-07-08-14` — the *interpreter* was wrong), a tuple-with-shared-struct-element
  destructured from an `Option` (`B-2026-07-08-16`), `<map>.values().collect()`
  (`B-2026-07-08-17`), and a multi-witness `impl Trait` now rejected on all three surfaces
  (`B-2026-07-08-1`, `def4648`). A JIT-run gpu.dispatch gap (`B-2026-07-10-6`) stays open.
- **`#[repr(C)]` struct-by-value ABI — a silent miscompile CI caught (`B-2026-07-09-2`).** A
  `#[repr(C)]` struct passed by value across the C export boundary was **mislowered on
  AArch64** (a raw LLVM struct-by-value relied on the backend default instead of explicit
  per-target ABI classification), silently returning wrong data — `stats_mean` returned `0.00`
  instead of `7.50` on Apple silicon, `7.50` on x86-64. The new **codegen-e2e-macos (arm64) CI
  leg** flagged it. Fixed in slices: AArch64 AAPCS params (`991d3e2c`) + returns (`fa180294`),
  >16B indirect params (`6a6294fc`) + sret returns (`c3a68206`), x86-64 SysV byval/sret
  (`bc6a78cc`), and the Windows-x64 classifier (`4c90993d`, `B-2026-07-09-8` — its native
  execution CI leg deferred, `llvm-config.exe` missing upstream). A **Linux forced-arch
  signature-match test** (`KARAC_FORCE_TARGET_ARCH`) verifies each target's ABI without the
  hardware. See [[design-unsafe-ffi-and-pointers]].
- **The self-host parser drop/ownership saga.** The [[self-hosting|parser port]] on the
  `shared enum` AST model drove a long multi-session investigation. Cross-module resolution
  reopened and fixed (`B-2026-06-19-3/-4`, shared-enum fn-ret temp arg + struct-variant
  whole-binding leaks). The parser then *compiled but crashed*: `B-2026-07-09-11` (niche
  `Option[shared]` into a conventional field slot, `706a71e4`), `B-2026-07-09-12` (control-flow
  expressions SEGV — root-caused to **auto-par falsely parallelizing sequential `mut ref self`
  calls** that share the cursor, closed via write-dependency serialization), and
  `B-2026-07-10-1` (a let-bound `Vec[shared]` element moved into an enum ctor — the move
  suppressor zeroed only the Vec `cap`, not `len`). The **expression oracle is now green**; the
  **item/type oracle residual `B-2026-07-10-4` stays open** — a parse-side premature-free the
  entry's next-agent handover documents in detail.
- **Iterator `.collect()` surface fully closed (`B-2026-07-04-2` → `fixed`).** Every adaptor
  now lowers under `karac build` at run==build parity, LSan-clean: heap `zip`, `chunks`/
  `windows` (a fresh-temp block-return — the real blocker was the synthetic AST bypassing the
  ownership RC fallback, *not* a move-into-aggregate gap), adaptor-carrying `chain`/`zip` sides,
  non-terminal f-string map, `cycle().take(n)`, and `scan` running-accumulator (`9230632`, and a
  long chain of sub-part commits). Two documented non-implementable edges (bare unbounded
  `cycle`, a `None`-returning `scan` body) loud-fail cleanly, never miscompile.
- **The interpreter `u64` model (`B-2026-07-04-8` → `fixed`, `45eb926`).** The tree-walk host
  now reinterprets the i64 carrier as `u64` at every signedness-sensitive sink (print / compare
  / div-rem-shift / sort) when the static type says so, closing the run-vs-build divergence for
  `u64 ≥ 2⁶³`. Two codegen bugs found while verifying parity were fixed in the same commit
  (operators inside f-string interpolations were never lowered; `Vec[uN].sort()`'s default thunk
  compared signed). See [[columnar-data]].
- **Codegen correctness.** `? ` on a `Result[<concrete enum>, E]` truncated the Ok payload to
  one word (`B-2026-07-11-7`); a **multi-word `?` error type** (a `Result[T, struct{String}]`)
  round-trips (`B-2026-07-09-20`, `83b3b719`); a `ref`-scrutinee enum with a non-i64-word
  payload (String/bool/narrow) miscompiled (`B-2026-07-11-5`); a `Vec[struct]` enum payload lost
  its element type (`B-2026-07-11-6`); a `u8` byte index into a `Vec[T]` emitted an invalid
  ICmp (`B-2026-07-11-2`); nested-struct `Display` renders debug-style on both surfaces
  (`B-2026-07-08-18`); and a `#[derive(Message)]` protobuf now **compiles under codegen** via a
  3-layer fix — skip comptime fn bodies, typecheck derive-generated bodies, compile
  `std.protobuf` bodies (`B-2026-07-08-15`). See [[codegen]], [[protobuf]].
- **Diagnostics made machine-applicable.** A diagnostic-fix-invariant audit
  (`docs/diagnostic-fix-audit.md`) drove the whole resolver `did-you-mean` family to carry a
  `.replacement` `TextEdit` (`B-2026-07-06-3`, `830831f`; the label-rename `B-2026-07-07-3`,
  `911db54`), and `karac fix` now applies the ownership `fix_diff` migration it already computed
  (`B-2026-07-06-4`, `0f21b4b`). See [[design-ai-first-compiler]], [[cli]].
- **Structured-concurrency ergonomics.** `par { }` block top-level `let` bindings now **escape
  to the enclosing scope** (the join-barrier model, `B-2026-07-11-3`, `ed07aed`), and a
  no-annotation `spawn(|| work())` thunk **infers `T`** (`B-2026-07-11-4`, `8be6c95`). See
  [[design-concurrency-and-providers]].
- **Trait dispatch tails.** Blanket `impl Trait for Vec[T]` loop-bodied impls (`B-2026-07-06-5`,
  `6321ee8` — the real blocker was `for x in self` dropping a *borrowed* receiver), and
  bound-generic dispatch over a **user-type** implementor under `karac build` (`B-2026-07-06-2`,
  `4f3e5747` — a `ref C` mono receiver wasn't recording its concrete type name). See
  [[stdlib-and-traits]].

## Notable fixes recorded in the ledger (round 10)

Round 10 added **~90 new ledger entries** (`B-2026-07-11-*` through `B-2026-07-14-*`) and
**closed all of round 9's high-severity open bugs**. The dominant themes are the final closure
of the [[self-hosting|self-host parser]] drop/ownership saga, a broad **shared/RC
drop-completeness sweep**, **generic-monomorph heap-type threading**, and the maturation of
**closures**, **iterator terminals**, and **match/scoping** correctness under codegen.

- **Self-host parser drop/ownership family CLOSED.** The multi-session investigation
  (`B-2026-07-09-12`, `B-2026-07-10-1`, `B-2026-07-10-4`) is resolved. `B-2026-07-09-12`
  (control-flow-expression parser SEGV) was an **auto-parallelization bug** — `parse_if`
  falsely raced three sequential `mut ref self` calls sharing the cursor — fixed in
  `concurrency.rs` (`method_receiver_is_mut_ref` + `Let`-arm inner-write collection);
  `B-2026-07-10-1` (a block statement-expr reading back as a garbage `Error`) was a
  let-bound-struct move-suppression bug (zero the Vec LEN, not just CAP); and
  `B-2026-07-10-4`, the item/type oracle residual, fixed as above (`1b5f543`). All three
  self-host parser oracles are green. See [[self-hosting]], [[codegen]].
- **Shared/RC drop-completeness sweep.** A large family of leaks/double-frees where a
  `shared struct`/`shared enum` with a `Vec[shared]` / `Map[K,shared]` / `Option[shared]`
  field or payload failed to drain its shared elements on drop or on match-move:
  `B-2026-07-11-33`/`-39`, `B-2026-07-12-4`/`-21`/`-23`/`-24`/`-25`/`-29`/`-30`, and
  `B-2026-07-13-10` through `-17`. Includes an **arm64-only** leak (`B-2026-07-12-29`,
  `B-2026-07-14-3`) that the x86 CI structurally could not catch — motivating a new **arm64
  memory-sanitizer CI leg**. See [[codegen]], [[windows-and-cross-platform]].
- **Generic-monomorph heap-type threading.** `B-2026-07-11-25`/`-31`/`-35`,
  `B-2026-07-12-16`/`-27`/`-28`, `B-2026-07-13-2`/`-3`/`-9`, `B-2026-07-14-12`: a generic
  struct/enum/fn instantiated at a HEAP monomorph (String/Vec) lost the concrete
  element/payload type in the monomorph body (double-free, garbage read, or invalid IR). A new
  `type_subst_type_exprs` (element-aware, the twin of the head-only `type_subst_names`) plus
  per-monomorph struct/enum layout recovery were the shared levers. See
  [[per-layout-monomorphization]].
- **Closure maturity.** Mut-ref capture now works end-to-end (`B-2026-07-11-23`, `d123c06` —
  by-reference env capture for a non-escaping stored mutating closure); plus
  closure-returning-closure currying (`B-2026-07-12-12`), closure param inference from body
  (`B-2026-07-12-10`) and call site (`B-2026-07-12-20`), and closure heap return-type
  inference (`B-2026-07-13-20`/`-21`). See [[codegen]].
- **Iterator terminals.** `fold`/`sum`/`reduce`/`for_each`/`any`/`all`/`count` on fused
  chains + materialized `let it = v.iter()` bindings (`B-2026-07-11-17`/`-18`/`-19`); a fold
  with a heap accumulator double-free (`B-2026-07-13-18`); and loud-bail for unlowered
  adaptors (`B-2026-07-14-7`/`-9`). See [[stdlib-and-traits]].
- **Match/scoping correctness — a silent-wrong-answer cluster (all interp-correct).** Match
  arm GUARDS were silently ignored under codegen (`B-2026-07-12-9`, HIGH); a tuple-scrutinee
  match was not discriminated (`B-2026-07-12-13`, HIGH); and a nested-scope variable SHADOWING
  leaked past its scope under codegen (`B-2026-07-13-6`, HIGH — a lexical-scope env
  checkpoint). See [[codegen]].
- **Diagnostics.** Reject unknown methods on `Option`/`Result` (`B-2026-07-14-5`, closing a
  silent `Type::Error` poison hole) and dedup byte-identical nested-enum + ownership-cycle
  errors (`B-2026-07-14-4`). See [[design-ai-first-compiler]].
- **`?`-operator.** Multi-word error types (`B-2026-07-09-20`), `?` on `Result[Option[T]]`
  (`B-2026-07-13-19`), and `?` on a `Result[concrete enum]` wide-payload (`B-2026-07-11-7`).
  See [[codegen]].
- **OnceLock heap-`T`.** The long `B-2026-07-12-2` epic (heap-fitting then wide `T`,
  `get_or_init` aggregate) is fully closed via several sub-fixes. See [[stdlib-and-traits]].
- **Channel.** A `send` of a heap payload double-freed (`B-2026-07-13-16` — `send` is a MOVE);
  an unreceived payload then leaked on channel drop (`B-2026-07-13-17`, elem-drop threaded into
  the runtime channel). See [[design-concurrency-and-providers]].

## Bug-adjacent guardrails added

`must_use` / `missing_must_use`, `missing_track_caller`, `missing_non_exhaustive`,
`unsafe_op_in_unsafe_fn` and `unsafe-extern` `# Safety` doc checks, `logical_lint`,
`ffi_lint`, `diagnostic_attrs_lint`, `raii_check`, the FFI-union `E_UNION_*` family, a **CI
codegen containment guard**, fuzz targets (lexer/parser/pipeline), and
memory-sanitizer/ASAN test suites (`tests/memory_sanitizer.rs`). See [[attributes]] and
[[design-unsafe-ffi-and-pointers]].

## CI test-coverage tiers (new in round 5)

Round 5 stood up a **tiered CI** (`docs/spikes/ci-test-coverage.md`):

- **Tier 1 (landed)** — the `--features llvm` **codegen E2E + self-host oracle** on CI
  (LLVM 18 installed via `apt`, not `install-llvm-action`), plus the wasm clippy + archive
  surface gate.
- **Tier 2 (landed)** — a **memory-sanitizer job** (ASAN + **Linux LeakSanitizer**). The
  **Linux-LSan run is the leak gate** and found an **11-leak** batch on landing.
- **Tier 3 (open)** — remaining coverage tiers tracked.
- **Book-snippet test harness** (Phase 9) — book code blocks are now **CI-gated test
  cases**, so documentation examples cannot rot. See [[design-contracts-and-verification]].
- The **differential lexer oracle** ([[self-hosting]]) is itself a correctness gate — it
  caught auto-par bug **#8**.
- **Round-9 additions** — a **5-target matrix** (Linux x86-64/arm64, macOS x86-64/arm64,
  Windows), whose **macOS-arm64 codegen-e2e + producer-mode leg** caught the silent
  `#[repr(C)]` struct ABI miscompile (`B-2026-07-09-2`); a **codegen-e2e-via-LLJIT (run==build
  parity)** leg that gates the JIT-default flip; a **flagship-demo benchmark regression gate**;
  and an **ownership/drop drop-fuzz oracle** whose `oracle↔codegen` drop differential reached
  **100%**. The Windows *execution* leg and the macOS Intel (`macos-13`) codegen-e2e leg were
  **dropped** (upstream `llvm-config.exe` gap / runner-starvation).

Related: [[deferred-work]], [[codegen]], [[history-reversals-and-deprecations]],
[[attributes]], [[design-unsafe-ffi-and-pointers]].
