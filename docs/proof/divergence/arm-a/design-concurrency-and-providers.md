---
type: design-decision
title: Auto-concurrency, par-blocks, and providers
updated_round: 10
---

# Auto-concurrency, par-blocks, and providers

Kāra derives parallelism from [[design-effect-system|effect analysis]]. There is **no
async/await and no colored functions** — concurrency is a property the compiler infers,
not a type coloring the programmer threads through their code.

## Auto-parallelization

- A **`ConcurrencyAnalysis`** pass identifies regions that can run in parallel; it is
  threaded into codegen state and used to **auto-parallelize non-`par` regions**.
- Gated at runtime by the **`KARAC_AUTO_PAR`** environment variable.
- A **captured-mutation safety net** and pinned auto-par receiver writes / move-aware
  container ops guard correctness. Several auto-par codegen bugs were fixed for
  `Slice`-param, `Vec`-binding, and return-value shapes (see [[bug-tracker]]).
- The CLI exposes a human-readable **`--concurrency-report`** (see [[cli]]).

## Auto-par reductions (round 2, extended round 3)

Beyond parallelizing independent regions, the concurrency pass now recognizes
**reductions** (fold/reduce shapes) as parallelizable and lowers them to a
**`karac_par_reduce`** primitive (fan-out + serial-combine), gated by a **cost model** and
generalized to an **(op, type)** matrix. Benchmarks: **9.87× kata-7 vs Rust** for the
while-shape lowering (4.1× for the earlier narrow-shape v1). Reduction workers **pool-share**
with `karac_par_run`. See [[codegen]].

Round-3 extensions to reduction recognition (todo-api kata driven):

- **Min/Max** reductions (combined slice).
- **Conditional acc-update** — `if cond { acc = acc + delta }` and a **two-arm conditional**
  acc-update are recognized as reductions.
- **Collect-style reductions** — a **`ReductionOp::Collect`** recognizing an `acc.push(...)`
  shape, **gated on the `#[par_unordered]`** attribute (Phase 1–3; see [[attributes]]).
- Cost gates tightened: a **per-statement cost gate** for `karac_par_run`
  (`find_parallel_groups`), an **inlining-aware** gate for known free-fn callees, a
  **memory-bound rejection gate**, and a **dynamic cost gate** for `karac_par_reduce`.

### Shallow-depth parallel reduction (round 8)

A new approach for reductions whose **per-iteration delta recurses** (e.g. a backtracking
N-Queens counter), landed via `9ac3f30d` (`docs/spikes/shallow-depth-parallel-reduction.md`):
a runtime **fork-depth cap** (**`KARAC_PAR_MAX_FORK_DEPTH`**, default 1) so only the
**outermost** recursion level fans out and deeper levels run inline — safe (bounded nesting,
no stack blowup) and useful (~9.5× on N-Queens). Env reads on the hot path are OnceLock-cached.

This **replaced an earlier conservative compile-time decline**: `B-2026-07-03-14` first made
the auto-par reduction recognizer **decline a directly-self-recursive reduction body** (it
caused runaway task nesting → SIGBUS); the shallow-depth spike then removed that decline and
re-enabled the reductions behind the fork-depth cap. See [[bug-tracker]].

## Auto-parallel I/O (new in round 5)

The design doc now distinguishes **two auto-concurrency delivery mechanisms** (Feature 5):
the CPU-bound statement-group auto-par above, and a new **I/O-overlap** path that overlaps
independent blocking / timer operations:

- **Path A1** — **independent blocking calls auto-parallelize** (e.g. two independent
  network reads issued back-to-back run concurrently).
- **Path A2a — async `sleep_ms`** — an **event-loop timer wheel** (A2a-1) + a **C ABI for the
  async-sleep timer** (A2a-2.1) lets `sleep_ms` **park on a timer** rather than block a
  worker (A2a-2.2).
- **Path A2b** — independent `sleep_ms` timer waits **overlap** (A2b-1); a K-sweep bench
  (`bench/auto_par_io/`) verifies the overlap **scales past the pool size**.
- **Dependency-analysis fixes** — the auto-par dep analysis now tracks **`self`
  reads/writes** (`#8`, caught by the differential lexer oracle), records **mut-ref call
  args as writes**, recurses the capture set into `unsafe`/`try`/`par`/`lock`, and handles
  **`for _` wildcard** reductions like `for i`/`while`. A **par slot-ownership transfer**
  UAF (a branch freeing the value it published) was fixed. See [[bug-tracker]].

### A2b-2 — auto-parallel network I/O (new in round 9)

The I/O-overlap path was extended from timer/blocking overlap to **network calls** (`A2b-2`).
Independent network operations now auto-parallelize, gated by a **finer Network resource
granularity** so that two calls to *different* endpoints are not treated as a conflict:

- **Arg-safe network fan-out** (`35916c35`) — independent, argument-disjoint network calls
  auto-parallelize; extended to **borrow-param** calls with variable args (`875298b8`), and the
  **ephemeral-network conflict was relaxed** so a plain `http_get` fan-out parallelizes
  (`ac1cb469`, Phase 1).
- **Phase 2** — fan out **associated (receiver-less) network openers** (`39e849d1`, Slice 1)
  and **method network calls on distinct receivers** (`b9f3e0dc`, Slice 2). A spike first
  confirmed **codegen is not the blocker** for method-call admission (`891d14fc`, `0c9c3425`).
- **Parameterized-resource partition keys** (Slice 3) — a `Network[addr]` resource is
  partitioned by its **key**, so `Network["a"]` and `Network["b"]` do not conflict. Landed for
  **literal keys** (`abd0365c`, Slice 3a) and **method-call keys** (`822e4f4d`), grounded by a
  finer-Network-granularity design spike (`ea92bb1a`, `b58f599a`).
- **A2b-2 COMPLETE (round 10).** Round 9 began the network fan-out; round 10 finished it —
  associated (receiver-less) openers (Slice 1), method calls on distinct receivers (Slice 2), and
  the parameterized-resource partition keys (Slice 3: literal-key + method arm) are all in, gated
  by the finer `Network[addr]` partition keys. The A2b I/O-overlap path is done.

## Round-6 concurrency correctness

- **Missed write-dependency race fixed** (`B-2026-06-20-16`) — the auto-par dependency
  analysis had no arm for a **write through a deref/method-chain target**, so
  `*m.entry(k).or_insert(0) += 1` recorded **no write** to `m` and got co-grouped with a later
  `m.keys()` read — a read-after-write **race under the default `karac build`** (silent wrong
  answer, A/B mismatch vs `karac run`). The write-target resolver now recurses `Unary{Deref}`
  and `MethodCall` targets to the underlying container.
- **Exclusive-borrow rule enforced at call sites** (`B-2026-06-17-6`) — `f(mut v, mut v)` and
  `f(mut v, v)` used to compile (and diverge between interp and codegen); the ownership checker
  now rejects a second active borrow of an exclusively-borrowed binding. This is the
  precondition that makes **`mut ref` `noalias` sound** (`B-2026-06-17-5`).
- **Spawn capture ownership** — a heap value captured **read-only by multiple sibling
  `tg.spawn` tasks** (the canonical parallel-stencil fan-out) is now classified **borrow, not
  move**, so the parent frees it exactly once after the join barrier (`B-2026-06-19-11`),
  completing the `B-2026-06-18-8` / `B-2026-06-19-2` loop-spawn ownership family.
- **Detached-eager-reap spawn leak** — the canonical `loop { tg.spawn(|| handle(conn)) }`
  accept-loop leaked ~100 B/connection; fixed with a detached-gated register-time reap. See
  [[windows-and-cross-platform]], [[bug-tracker]].
- **Auto-par self-conflict lifting** — `allocates + allocates` (A3a) and `panics + panics`
  (A3b) are no longer treated as a self-conflict that blocks parallelization; **auto-par
  ordered output** parallelizes logging-bearing work while preserving output order (after
  earlier learning to **never parallelize console-output statements**, `B-2026-06-13-18`).
- **`query concurrency` sharpened** — self-locating spans + a **structured exclusion-reason**
  and a **reorderable-advisory** on the report. See [[cli]], [[design-ai-first-compiler]].

## Round-8 concurrency correctness

- **TaskGroup / spawn interpreter parity** (`B-2026-06-30-8`, fixed `8f2c8d16`) —
  `TaskGroup.new()`, `tg.spawn(closure)`, `handle.join()`, `tg.cancel()`, and free
  `spawn(closure)` compiled and ran under `karac build` but had **no interpreter evaluation
  rule**, so `karac run` diverged (internal error / "variable 'spawn' not found"). The
  interpreter now runs them **eagerly/sequentially** (join-at-spawn model, matching the
  `ScopeLocal` rules) with new **`Value::TaskGroup` / `Value::TaskHandle`** variants — a
  run/build parity fix. The tree-walk host cannot cross program borrows into a `'static`
  thread, so genuinely-parallel deferred execution stays out of scope; only lexical `par {}`
  gets real threads.
- **`par {}` sibling-branch read diagnostic** (`B-2026-07-02-5`, fixed `fccdd4ab`) — reading a
  sibling `par` branch's binding used to **panic the interpreter** ("variable not found") /
  give an ungraceful codegen error; the resolver now resolves each top-level `par` statement as
  a **concurrent branch in its own scope** and emits a tailored `error[resolve]` on both
  surfaces. The block's **tail expression is the join point**. Probing this surfaced the
  separate par-join codegen gap `B-2026-07-02-31` (fixed — explicit-par join lost branch
  bindings for a non-bare-call RHS).
- **Par-join slot ownership** (`B-2026-07-03-32`, fixed `52a454c3`) — a `Column`/`DataFrame`/
  `Tensor` produced in one auto-par branch and read after the join was **early-freed at branch
  end** → dangling read / SIGBUS under the default build (correct under `karac run` and
  `KARAC_AUTO_PAR=0`). Fixed by adding **`FreeColumn`/`FreeDataFrame`/`FreeTensor` to the
  slot-ownership transfer** (publish-and-forget), matching the existing Map/File/enum/struct
  handle transfers.
- **Auto-par internals** — the **O(n²) dense conflict matrix was replaced with a sparse
  inverted-index conflict graph** (`0093269e`). And `B-2026-07-02-8` (fixed `d5e1165d`):
  `sort`/`sort_by`/`reverse`/`pop`/`remove` were **invisible to the auto-par write-dependency
  gate** (missing effect seeds) → silent data corruption when co-grouped with a later read;
  now seeded as **receiver-mutating**. See [[bug-tracker]].

## Round-9 concurrency correctness

- **`par {}` block bindings escape to the enclosing scope** (`B-2026-07-11-3`, fixed
  `ed07aed`) — a `par { let a = fa(); let b = fb(); }` block's top-level `let`s are now usable
  **after** the block (the join barrier hoists each branch result into the surrounding scope),
  not only in a tail expression. Fixed across resolver / typechecker / interpreter / codegen,
  mirroring how the auto-parallelizer already treats its grouped `let`s as ordinary
  enclosing-scope locals. The sibling-branch read isolation (`B-2026-07-02-5`) is preserved.
- **`spawn(|| work())` thunk inference** (`B-2026-07-11-4`, fixed `8be6c95`) — an
  un-annotated closure-literal thunk into `spawn` now **infers `T` from the closure's return**
  (`spawn[T](f: OnceFn() -> T) -> TaskHandle[T]`); previously it failed type inference. Pass 2
  of the generic-call inference unifies the closure's inferred type back into the slot.
- **Auto-par must serialize sequential `mut ref self` calls** — the self-hosted parser's
  control-flow expressions SEGV'd because `parse_if`'s three `mut ref self` cursor-advancing
  calls were **falsely parallelized** (they share `self.pos`, a strict ordering dependency).
  The write-dependency analysis had no arm for a `mut ref self` method receiver nor for
  inner-writes of a `let` RHS, so no write on `self` was recorded. Now serialized — the
  root cause of `B-2026-07-09-12` (see [[self-hosting]], [[bug-tracker]]).

## Round-10 concurrency correctness

(The `par {}`-binding escape `B-2026-07-11-3`, the `spawn(|| …)` thunk inference
`B-2026-07-11-4`, and the `parse_if` `mut ref self` serialization root-cause of
`B-2026-07-09-12` are recorded under **Round-9** above; round 10 extended that work.)

- **A2b-2 network auto-parallelization COMPLETE** — round 9's parameterized-resource fan-out
  finished with Slice 3: **partition keys on method calls** (a literal-key slice + a method arm),
  so a `Network[addr]` is not a `Network[other]` and independent openers / distinct-receiver
  method calls / variable-arg calls auto-parallelize with the finer gate.
- **Auto-par `mut ref self` write-scan BROADENED** (`B-2026-07-12-5`, `0ec5b8d`) — the same
  false-race family as the round-9 `parse_if` fix, but the analyzer still lost a `mut ref self`
  mutation when the call sat **inside an f-string interpolation or a match / if / while scrutinee**
  (a silent wrong answer under JIT/native, correct under `--interp`). The dependency scan now
  records a **receiver WRITE for a mutating call in every expression position** — not only at
  statement top level. A **`KARAC_NO_AUTOPAR`** escape hatch was added (build every loop
  sequentially). See [[bug-tracker]].
- **Heap-capture escaping closures Slice 2** — escaping closures that capture `String`/`Vec`
  (the heap-env closure work continues); a **stored mut-ref closure** now gets a by-reference env
  capture for the non-escaping case (`B-2026-07-11-23`, see [[codegen]]).

## Channels — AOT lowering + blocking recv (new in round 5)

The prelude channels graduated from interpreter-only to **AOT codegen**:

- **`Channel[T]` AOT lowering** — `send` / `recv` / `try_recv` / `clone`
  (`runtime/src/channel.rs`, `src/codegen/channel.rs`).
- **`BoundedChannel[T]` AOT lowering** — `new` / `send` / `recv` + `Drop`
  (`runtime/src/bounded_channel.rs`, `src/codegen/bounded_channel.rs`).
- **Blocking channel `recv`** — **park-on-empty** + close + cross-task drop.
- **Channel combinators** are tracked and **`select` was promoted to P1**.

### `collect_all` / `collect_all_vec` gathers

- **`collect_all`** — a **heterogeneous fixed-arity tuple** gather (run N thunks, collect a
  tuple of results), with **auto-thunking** so call-args become thunks automatically.
- **`collect_all_vec`** — a homogeneous parallel gather returning a `Vec`, front-end +
  interpreter (slice 1a) then **parallel gather lowering** (slice 1b).

## `Mutex` — spinlock → futex (new in round 5)

`Mutex[T]` was built up over slices and **evolved from a spinlock to a blocking 3-state
futex** (`runtime/src/mutex.rs`): `Mutex.new` + `lock` blocks (slice 1, standalone
spinlock), **`lock` on a place expression** (a `par`-struct `Mutex` field, slice 2), **`lock`
through a `ref`/`mut ref` Mutex parameter**, **release-on-all-paths** (early exits from a
lock body are legal), and finally the **futex** lock. In the interpreter, `Mutex[T]` /
`Atomic[T]` are a **shared `Arc<Mutex<Value>>`** so `par` branches hold a real lock and
don't race. See [[history-reversals-and-deprecations]], [[design-ownership]].

## Structured concurrency — `spawn` / `TaskGroup` / `TaskHandle` (new in round 3)

A structured-concurrency surface landed alongside the [[design-runtime-phases|event-loop]]
scheduler (Phase 6 line 218):

- **Type declarations** — `spawn`, **`TaskGroup`**, and **`TaskHandle[T]`** (slice 1,
  `runtime/stdlib/task_group.kara`).
- **`ScopeLocal` marker trait + escape rejection** (slice 2) — a value that must not escape
  its task scope; the checker rejects escapes.
- **Lowering** — `spawn()` / `TaskGroup.spawn()` / `TaskHandle.join()` codegen (slice 4,
  `src/codegen/task_group.rs`); the **`spawn` slot widened to `OnceFn() -> T`** (slice 8);
  **stdlib struct-by-value param LLVM ABI** (slice 9).
- **`TaskGroup` wait-for-children on drop** (slice 5) — a group joins its children at scope
  exit. A **`spawn()` + `TaskGroup` smoke** E2E pins it (slice 7, `tests/spawn_e2e.rs`).
- Backed by a **`spawn()` task scheduler module** (slice 3, `runtime/src/scheduler.rs`); the
  scheduler dispatcher is confirmed to drive concurrent parked tasks (async-scheduler slice
  1). See [[design-runtime-phases]].

## Cross-task safety (round 3, Phase 6 line 170; extended round 4)

A **cross-task-safe** analysis governs what may cross a task boundary:

- A **cross-task-safe set + transitive walker** (`src/cross_task_safe.rs`, slices 1+2).
- A **boundary check on `spawn` / `TaskGroup.spawn`** (`typechecker/cross_task_check.rs`,
  slice 3a) — captured bindings must be cross-task-safe.
- **Round 4** extended the boundary check to **`Channel.send`** and the **`with_provider`**
  boundary (slice 3c) and to the **`par`-block** boundary (slice 3b), with a
  **`E_NOT_CROSS_TASK` three-line diagnostic shape** (slice 4). Cross-task-safe slices 5/6
  (verification) closed and the par concurrency model was reconciled.
- Non-trivial captures get an **atomic `rc_inc`** for shared bindings in par-codegen (L227).
- **Round 5** closed the **cross-task-safe boundary slice 6**, completing the **P0
  spawn/TaskGroup** surface, and landed a **nested cancel cascade + completion-wins**
  verification and a **defer-on-cancel** coroutine slice. See [[design-runtime-phases]].

## `par struct` / `par enum` (new in round 4)

**`par struct`** and **`par enum`** are shareable-across-tasks aggregate types (the
concurrent counterpart of a plain struct/enum). They first shipped as a **not-supported
parser error** (a minimum fix so the keyword parses cleanly), then were implemented as
slices A–D:

- **Definition-site guarantee** (Slice A, typechecker) — a `par struct` must satisfy the
  cross-task-safe rules at its definition.
- **Concurrency integration** (Slice B) and **always-`Arc` codegen** (Slice C) — a `par`
  aggregate is always reference-counted with an atomic (`Arc`) count.
- Formatter + catalog polish (Slice D, §9476).

Atomic method dispatch works on a **shared/`par` struct field** receiver (see
[[design-ownership]], [[codegen]]).

## Backpressure primitives (new in round 4)

A set of stdlib concurrency primitives for flow control:

- **`Semaphore`** — `new` / `acquire` / `release` (`runtime/stdlib/semaphore.kara`).
- **`RateLimiter`** — a **per-key token-bucket** `try_acquire` (`rate_limiter.kara`).
- **`BoundedChannel[T]`** — a **capacity-bounded** `send`/`recv` channel providing
  backpressure, distinct from the unbounded prelude `Channel` (`bounded_channel.kara`).

See [[stdlib-and-traits]].

## Explicit parallelism

- **`par`-blocks** lower to a runtime spawn; codegen returns auto-par values via a
  parent-allocated slot struct.
- **Par cancellation** (new in round 2): a `par` region returning `Result` uses a
  **Result-slot ABI where an `Err` triggers cancellation** of sibling branches (Phase 7
  line 67). `spawn(closure)` establishes a par-region ownership boundary (line 63).
- The runtime uses a **long-lived worker pool** for `karac_par_run` (replacing an
  earlier thread-per-call fan-out — see [[history-reversals-and-deprecations]] and the
  Parallax perf work in [[examples-and-benchmarks]]); the pool worker count can be overridden
  via **`KARAC_PAR_WORKERS`** (round 3).
- **Channels**: `Channel` / `Sender` / `Receiver` are prelude types; `Sender.send(closure)`
  respects par-region boundaries.

## Network event loop (v1.1)

The [[design-runtime-phases|v1.1 network event loop]] is a separate concurrency substrate
from `par`: an epoll/kqueue/IOCP poller (via `mio`) plus a **state-machine transform** that
turns network-boundary functions into cooperatively-scheduled, resumable tasks — Kāra's
answer to async I/O **without** async/await or colored functions. See
[[design-runtime-phases]].

## Providers (`with_provider[R]`)

**Providers** supply a resource `R` to a scope via `with_provider[R]`, dispatched through
a **provider stack** (a runtime provider-stack ABI and provider vtable emission).

- `R.method` dispatches via the provider stack; **par-blocks inherit the provider stack**;
  nested `with_provider` resolves **innermost-wins**.
- Standard I/O (`Stdout` / `Stderr` / `Stdin`) is routed through providers, enabling
  interception (e.g. `with_provider[Stdin]`).
- A **provider escape check** prevents a provided resource from escaping its scope.

Related: [[design-ownership]], [[design-effect-system]], [[codegen]].
