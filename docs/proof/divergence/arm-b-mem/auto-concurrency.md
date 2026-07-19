---
type: spec
created_date: 2026-07-15T07:25:59Z
last_modified: 2026-07-15T10:54:49Z
level: M1
stability: evolving
tags: concurrency, effects, auto-par
---

# Auto-Concurrency

## Identity
Kāra's concurrency model: parallelism is derived by the compiler from effect analysis and lowered onto a runtime worker pool, with no async/await, no futures, and no function coloring.

## Purpose
To give programmers concurrency for free — straight-line code that the compiler parallelizes wherever effects prove independence — while keeping every function composable.

## Relationships
- **REFERENCES**: [[effect-system]]
- **REFERENCES**: [[auto-concurrency-instead-of-async-await]]
- **REFERENCES**: [[auto-parallelization]]
- **DEPENDS_ON**: [[effect-system]]
- **MOTIVATED_BY**: [[auto-concurrency-instead-of-async-await]]
- **DEFINES**: [[auto-parallelization]]
- **REFERENCES**: [[network-runtime-and-cooperative-scheduling]]
- **REFERENCES**: [[standard-library]]

## Realization

- src/concurrency.rs (ConcurrencyAnalysis), src/concurrency_report.rs
- Codegen: src/codegen/par_blocks.rs, src/codegen/reduce.rs (par reductions), auto-par lowering threaded through Codegen state
- Runtime worker pool: karac_par_run (runtime/src/lib.rs)
- Ownership support: src/ownership/par_helpers.rs, closure_escape.rs; Rc→Arc promotion
- CLI: `karac --concurrency-report`

- Structured concurrency: runtime/src/scheduler.rs, runtime/stdlib/task_group.kara, src/codegen/task_group.rs; tests/spawn_e2e.rs
- Cross-task-safe: src/cross_task_safe.rs, src/typechecker/cross_task_check.rs; tests/cross_task_safe.rs

## Specifies

- Explicit concurrency surface: `par` blocks, Channel/Sender/Receiver, `Sender.send(closure)`.
- Implicit [[auto-parallelization]]: independent effect regions run concurrently, gated by `KARAC_AUTO_PAR`.
- A captured-mutation safety net and move-aware container ops preserve correctness under auto-par.
- Atomic RC (inc/dec) for bindings shared across par threads; provider stack inherited into par blocks.

- Structured concurrency: `spawn()` (slot typed as `OnceFn() -> T`), `TaskGroup` + `TaskHandle[T].join()`, TaskGroup wait-for-children on drop — tasks run on the async task scheduler of [[network-runtime-and-cooperative-scheduling]].
- Cross-task-safe boundary check on `spawn` / `TaskGroup.spawn` (cross-task-safe set + transitive walker) with a `ScopeLocal` marker trait rejecting scope-escaping captures; atomic rc_inc for non-trivial shared captures.
- `Atomic[T]` surface: `Atomic[T].new/load/store` (incl. general expression position) for int types, `fetch_add`/`fetch_sub` for lock-free counters, `Atomic[bool]` via i8 slot-widening; `karac migrate --atomic` rewrites shared→par<Type> for cross-task atomic consumers (L215).

- Shared / `par` structs: `par struct` / `par enum` declarations lower to always-Arc types shareable across tasks (definition-site typechecker guarantee, always-Arc codegen, atomic method dispatch on field receivers, user `impl Drop` for aliased holders, shared-struct constructor invariants). Initially rejected with a not-supported error, then shipped as Slices A–D.
- Backpressure & synchronization primitives: `Semaphore` (new/acquire/release), a token-bucket `RateLimiter` (per-key try_acquire), `BoundedChannel[T]` (capacity-bounded send/recv), and `Mutex` — defined in the [[standard-library]].


- Heterogeneous gather: `collect_all` (fixed-arity tuple gather with auto-thunking) and `collect_all_vec` (front-end + interpreter, then parallel-gather lowering).
- Auto-par path A: independent blocking calls auto-parallelize (A1); async `sleep_ms` lowers to a park-on-timer over an event-loop timer wheel (A2a), and auto-par overlaps independent `sleep_ms` timer waits (A2b).
- `Atomic[T]` completed to the full RMW set: `compare_exchange` (CAS) -> `Result[T,T]`, `swap`, and `fetch_and`/`or`/`xor` alongside `fetch_add`/`fetch_sub`.
- `Mutex`: a blocking futex lock (spinlock → 3-state futex), `lock` on a place expression / `par`-struct Mutex field, lock through a `ref`/`mut ref` Mutex parameter, and release-on-all-paths (early exits from a lock body legal). Interpreter models `Mutex[T]`/`Atomic[T]` as a shared `Arc<Mutex<Value>>` so par branches don't race.

## Constraints

- Two regions may run in parallel only if their effects are provably disjoint.
- Values crossing a par-region boundary are promoted Rc→Arc for thread-safe sharing.

## Rationale

Reuses the [[effect-system]] rather than adding an async runtime. Rationale and rejected alternatives in [[auto-concurrency-instead-of-async-await]].
