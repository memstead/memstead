---
type: design-decision
title: Effect system
updated_round: 10
---

# Effect system

Kāra functions declare **effects**. The effect system is the foundation for
[[design-concurrency-and-providers|auto-concurrency]]: because a function's effects are
known, the compiler can decide what may run in parallel without async/await.

## The six built-in effect verbs

1. `reads`
2. `writes`
3. `sends`
4. `receives`
5. `allocates`
6. `panics`

Beyond these verbs, users declare their own **resources** and effect groups.

- **`Hardware` effect (new round 10)** — the memory-mapped-I/O intrinsics
  `volatile_read` / `volatile_write` carry a `Hardware` effect, so bare-metal register
  access is effect-tracked (and thus excluded from auto-parallelization / GPU kernels).
  See [[embedded-mmio]].

## Surface and mechanics

- **Resource declarations**, effect groups, `with` / `with _`, transparent effects, and
  parameterized resources are all in the parser/AST.
- **Effect inference** is a semantic-analysis pass (part of Phase 3, see
  [[implementation-phases]]); the effect checker infers a function's effects, verifies
  declared-vs-inferred effects, applies call-site effect subtyping, and checks
  `with E` unification.
- **Extern / FFI** functions carry effects too: the effect checker handles extern-fn
  effects, an FFI linter, and a profile gate, and propagates block-level `@noblock`
  across `unsafe extern { }` blocks.
- Per-method **allocator effects** are wired for `Map` / `Set`.
- Phase-4 **runtime effect tracking** (interpreter item 117) was **skipped** — see
  [[deferred-work]].
- **`#[profile(...)]`** (new in round 2) — a profile attribute integrated into the effect
  checker as a **profile-compatibility gate** (`effectchecker/profile_compat.rs`): effects
  are checked against the declared profile. Parser + resolver + effect-checker slices. See
  [[attributes]].
- **`E_RAII_ACROSS_YIELD`** (new in round 2) — the [[design-runtime-phases|event-loop]]
  state-machine transform forbids an RAII value from straddling a yield point (Phase 6
  line 31); extended in round 3 with a **`CancelSafe`** opt-in and `#[cancel_unsafe_until]`.
- **Resource-verb inference from `R.method()` dispatch** (round 3) — the effect checker
  infers a resource's verb from a provider-trait method call, so a user resource's effects
  fall out of how its methods are used.
- **Module-binding synthetic effect resources** (round 3) — each module-level `let` / `let
  mut` binding is modelled as a **synthetic effect resource** (`effectchecker/modbind_synth.rs`)
  so concurrent access to it is effect-checked, with a par-block conflict rule and a
  `pub fn` synthetic-resource rejection. See [[stdlib-and-traits]].
- **Contract purity** (round 4) — a contract predicate's inferred **effect set must be ⊆
  `{panics}`**: `requires`/`ensures` predicates and invariants must be pure (they may only
  panic). See [[design-contracts-and-verification]].
- **Network effect propagation** (round 4) — the `sends`/`receives(Network)` effect
  propagates across the **`std.http`** surface and the **TCP/TLS/WebSocket** surface, so
  I/O-bearing code is correctly typed as networked. See [[networking]].
- **Target-gated resources** (round 5) — a **target-gate pass**
  (`effectchecker/target_gate.rs`) enforces that a program's used resources are in the
  **provided-resource set for the selected [[wasm-targets|target]]**, with **`E0411`** when a
  resource is not provided there (e.g. filesystem on a browser target). The **`#[target(...)]`
  attribute** drives per-item target selection. See [[wasm-targets]].
- **Resource-receiver contradiction `E0412`** (round 5) — the effect checker rejects a
  resource-method receiver whose declared verb contradicts the dispatch, and emits a
  **machine-applicable `ref self` rewrite**. A related fix makes dispatch call sites
  **inherit declared-clause effects on *other* resources**.
- **`[profile]` knob substrate** (round 5) — the profile-compatibility gate rides on a
  general **`[profile]`-table knob substrate** (`ProfileConfig`), which also carries
  **`panic_on_alloc_failure`** (see [[fallible-allocation]]).
- **Whole-program effect/concurrency graph** (round 6) — a whole-program graph
  (`src/effect_graph.rs`) joins each function's effects across the call graph; it powers
  `karac query effects`/`concurrency` and the **[[examples-and-benchmarks|Cartographer]]**
  browser studio. A fix made `query effects`/`concurrency` join **impl methods on generic
  receivers** (they previously reported empty effects, `B-2026-06-14-3`). See
  [[design-ai-first-compiler]].
- **`allocates(Heap)` is substrate** (round 6) — `allocates(Heap)` is treated as a substrate
  effect, **not** a must-declare clause on an undeclared `pub fn` (`3a26efe7`,
  `B-2026-06-13-4`), removing a spurious declarability requirement.
- **GPU effect gate** (round 7) — a dedicated **`src/effectchecker/gpu_effect_gate.rs`**
  (+230) enforces effects **transitively from [[gpu-compute|`#[gpu]`]] roots** across the call
  graph (FE-4): a device kernel cannot perform host effects, and an **explicit `panic` is
  rejected anywhere in a `#[gpu]` call graph** (FE-4b). It sits alongside the round-5
  `#[target]` target-gate pass — same explicit-marking, structural-safety approach, reusing
  the effect system rather than a parallel one. See [[gpu-compute]].
- **`OnceLock`/`OnceCell` effect surface** (round 7) — the write-once cells carry an effect
  surface (`OnceLock` tests, `a33c0069`), and `OnceCell[T]` a **single-task structural
  enforcement** (3 typecheck rules) forbidding it from crossing tasks. See
  [[stdlib-and-traits]].

## Implementation

Effect logic lives in the effect checker (`effectchecker`), split into inference,
verification, call-site subtyping, `with E` unification, generics/bounds, `profile_compat`,
and extern/FFI submodules. Related: [[design-concurrency-and-providers]],
[[design-ai-first-compiler]], [[compiler-pipeline]], [[attributes]],
[[design-runtime-phases]], [[gpu-compute]].
