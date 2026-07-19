---
type: design-decision
title: Tiered ownership
updated_round: 8
---

# Tiered ownership

Kāra manages memory with a **tiered ownership** model and, crucially, **no lifetime
annotations** (unlike Rust). The tiers:

- **Parameter-mode inference** — the compiler infers whether a parameter is taken by
  value, by borrow, etc.
- **Owned returns by default** — functions return owned values unless told otherwise.
- **Explicit `ref` for borrows** — `ref` and `mut ref` express borrows; there are also
  pointer types and `weak` references.
- **RC fallback with budget controls** — when ownership cannot be resolved statically,
  the compiler falls back to reference counting, governed by a budget. Round 5 added
  **[[rc-elision|RC elision]]**, which removes the count ops where the fallback's lifetime
  is statically provable — so the fallback becomes far cheaper (down to **headerless 16-byte
  nodes** with no rc word).

## Ownership analysis

The ownership checker (`ownership`) performs:

- **Borrow tracking** and call-site borrow analysis, including a **`Slice[T]` borrow
  tracker** with interpreter-level aliasing (Theme 1).
- **Rc → Arc promotion** and RC enforcement: bindings shared across parallel threads are
  promoted from `Rc` to `Arc`, with **atomic RC inc/dec** emitted in codegen for
  `arc_values`. The type system carries `Type::Shared` / `Type::Rc` / `Type::Arc`
  variants, with method-resolution deref through them.
- **Closure capture analysis** — per-closure/per-capture capture-path enumeration
  (disjoint capture), bare-form per-capture-name inference ("Rule 2½"), and
  **closure-escape ref-capture detection**. `mut ref |...|` closure mutations propagate
  to the outer binding. **Round 7** added the codegen counterpart: a capturing closure that
  *owns* a capture and **escapes** its frame (returned / stored) now gets a **reference-counted
  heap environment** with inc-on-copy and move-out semantics, rather than the unsound stack
  env — the [[codegen|heap-env closure epic]] (`B-2026-06-22-2`), **closed in round 8**
  (`be2ef68e`) after ~30 slices covering every place (return / store / copy / move / escape /
  arg-pass / reassignment). See [[bug-tracker]].
- **Spawn-escape detection** and a **provider escape check** (a binding must not escape
  its provider scope). See [[design-concurrency-and-providers]].

## RC budget and creep monitoring (round 2)

The RC "budget" gained enforcement and monitoring surfaces:

- **`#![rc_budget(max: N)]`** — a module-level attribute that **enforces** a ceiling on
  reference-counting fallbacks in that module (Phase 7 line 43).
- **G12 RC-creep monitoring** — a surface that reports when RC usage grows (line 27).
- **`Option[shared T]`** now works end to end (parameter tracking, refcount-aware
  field-store, chained field access, recursive drop for shared chains) — see
  [[design-generics-and-impl-trait]].
- **Closure-capture mode migrated off the `Moved` state** (Phase 7 line 45) — a cleanup of
  how a captured binding's mode is represented.

## User-defined `Drop` dispatch (new in round 3)

Round 2 synthesized `drop` for built-in shapes; round 3 added **user-impl `Drop`
dispatch** (Phase 7 "user-Drop dispatch" prereqs):

- **Signature validation** — a user `impl Drop` is validated and recorded in a
  **`drop_method_keys` side-table** (Prereq.1).
- **Per-type `karac_drop_<T>` wrapper synthesis** (Prereq.2) and **scope-exit drop-call
  placement** at the NLL endpoint (Prereq.3/4), in both interpreter and codegen.
- **Move-suppression** so a user-`Drop` binding is not double-dropped when it is
  **returned by value** or **let-rebound**.
- User-vs-seeded-enum disambiguation was a recurring hazard here (it caused an intermittent
  suite hang until root-caused; see [[bug-tracker]]).
- **Round 4** extended user `impl Drop` dispatch to **shared structs** (RC, L938) and to
  **aliased holders** of a shared struct (L940), in both codegen and interpreter.

## `defer` / `errdefer` (new in round 3)

Kāra gained **`defer`** and **`errdefer`** cleanup statements (Zig-flavoured), lowered onto
the same unified drop+defer cleanup stack:

- **`defer`** — `UserDefer` + LIFO drain (Phase 7 line 96 slice 1), block-scoped with
  runtime-reachability gating (slice 1.5).
- **`errdefer`** — runs only on error-exit paths (phase-1), plus an **`errdefer(e)` binding
  form** that binds the propagating error, with wider-`E` payload reconstruction at `?`.
- **On the `par`-branch cancel path** — `defer`/`errdefer` also fire when a par branch is
  cancelled (see [[design-concurrency-and-providers]]). A test pins the drop+defer LIFO
  interleave. See [[codegen]].

## `Atomic[T]` (round 3, extended round 4)

**`Atomic[T]`** gained real codegen: `.load` / `.store` / `.new` dispatch for the integer
types (Phase 7 line 229) and **`Atomic[bool]` via i8 slot-widening** (line 231). **Round 4**
added **`fetch_add` / `fetch_sub`** (completing a lock-free counter), **atomic method
dispatch on a shared/`par` struct field receiver**, and **`Atomic.new` in general expression
position**. Atomics are the manual escape hatch that the **`karac migrate`** Atomic
heuristic targets (below — round 4 made that heuristic the default, opt out with
`--no-atomic`).

## `par struct` / `par enum` (new in round 4)

**`par struct`** / **`par enum`** are the shareable-across-tasks aggregate types (always
`Arc` in codegen). They satisfy the cross-task-safe rules at their definition site and are
the aggregate counterpart to a plain struct/enum. See
[[design-concurrency-and-providers]], [[codegen]].

## RC elision + alias metadata (new in round 5)

**[[rc-elision|RC elision]]** (`src/ownership/elision.rs`, +3521) is the round's largest
ownership work: it proves single-owner / append-only / cluster lifetimes and **frees
reference-counted values without count ops**, phased A → D. It is the performance backbone
of the [[examples-and-benchmarks|idle-connection-density benchmarks]]. Adjacent alias-metadata
work: **`noalias` on `mut ref` params** landed (from the exclusive-borrow guarantee), but
**plain param `noalias` measured inert** and **alias-scope metadata was deferred** (~0
runtime gain). See [[rc-elision]], [[deferred-work]].

## Fallible allocation (new in round 5)

Under a **`panic_on_alloc_failure = false`** [[fallible-allocation|profile]], the type
checker rejects RC-fallback allocation
(**`E_RC_FALLBACK_ALLOCATES_UNDER_FALLIBLE_PROFILE`**), allocating `derive(Clone)`
(**`E_DERIVE_CLONE_ALLOCATES`**), and panicking allocation calls
(**`E_PANICKING_ALLOC_REJECTED`**); collections gain `try_*` companions returning
`Result[_, AllocError]`. See [[fallible-allocation]].

## Variance declarations (new in round 5)

Round 5 added **per-stdlib-type variance declarations** — **`+T` (covariant) / `-T`
(contravariant) / `=T` (invariant)** markers with a verifier and a **use-site rule**
(`src/typechecker/variance.rs`, +547). See [[design-generics-and-impl-trait]].

## Read-accessors return `Option[ref T]` (new in round 5)

`Vec`/`Slice` `get` / `first` / `last` now return **`Option[ref T]`** (a borrowed view, not
a copy — `B-2026-06-07-5`), and general read-only methods work on borrow-locals beyond
`len`/`is_empty`. `Slice.get_unchecked(i) -> T` is the by-value unchecked escape hatch. See
[[stdlib-and-traits]].

## Round-6 borrow-checking additions

- **Exclusive-borrow rule enforced at call sites** (`B-2026-06-17-6`) — passing the same
  binding as two `mut ref` args, or as `mut ref` + a shared borrow, used to compile (and
  miscompile: codegen passed each borrow a by-value `{ptr,len,cap}` header, diverging from the
  interpreter). The ownership checker now rejects a second active borrow of an
  exclusively-borrowed binding — the invariant that makes **`mut ref` `noalias` sound**
  (`B-2026-06-17-5`). `mut ref T` is also accepted where `ref T` is expected (implicit
  reborrow, `B-2026-06-17-4`).
- **Borrow-elision for read-only `v[i]`** — a conservative whitelist pre-pass
  (`src/codegen/borrow_elision.rs`) elides the deep-clone on a read-only `let r = v[i]`
  heap-element binding, so `r` aliases the element and the container stays sole owner; any
  uncertainty (mutation, escape, `v.push`, a closure) falls back to the clone
  (`B-2026-06-19-6`). See [[codegen]].
- **Same-scope `let` shadowing allowed** (`49e26f18`) — the resolver permits shadowing a
  binding in the same scope (`design.md § Variables`); codegen handles a **type-changing**
  shadow by purging stale metadata (`src/codegen/shadow.rs`).
- **`for`-loop element binding is a borrow** — `for w in vec` binds `w` to an alias of
  `data[i]` and the source `Vec` retains ownership; a retaining consume site (`m.entry(w)`,
  `v.push(w)`) now takes a defensive copy so the loop element is not double-freed
  (`B-2026-06-20-13`). A for-loop binding is also scoped so it cannot collide with an outer
  same-named `let` (`B-2026-06-14-13`).

## Concurrent-mutation diagnostics + auto-fix (new in round 3)

A `concurrent_shared` detector (`src/ownership/concurrent_shared.rs`, the round's largest
new ownership module) flags a struct mutated across concurrent tasks and **emits a
machine-applicable fix**:

- **`E_CONCURRENT_SHARED_STRUCT`** (Phase 7 line 197) and a **`E_CONCURRENT_PLAIN_STRUCT`**
  sibling, each carrying a **JSON `fix_diff` envelope** (keyword rename + `mut`-strip edits).
- The detector covers **closure-captured** shared/plain bindings.
- **Lock-block auto-insertion** — the fix wraps par-internal write sites in a lock block
  (L201b), including **mutating method-call** receivers (L205) and a generalized receiver
  shape (L207, `par_capture_classify.rs`).
- The **`karac migrate`** CLI applies these rewrites at scale — a shared→par type migration
  with consumer-site rewrites and an `--atomic` heuristic. See [[cli]].

## Cross-task safety and `ScopeLocal`

Bindings crossing a task boundary must be **cross-task-safe**, and **`ScopeLocal`** values
must not escape their task scope — both new in round 3. See
[[design-concurrency-and-providers]].

## Cost visibility

Ownership costs are surfaced to the programmer: there is a **cost summary**
(`cost_summary`) covering provider / RC / shared cases, an `rc_predicate` analysis, and
type-checker **perf notes** (e.g. Tier-2 note for a shared struct with `mut` fields). The
REPL has an opt-in `--auto-clone` mode with a perf-note channel.

Related: [[design-concurrency-and-providers]], [[compiler-pipeline]], [[codegen]],
[[design-generics-and-impl-trait]].
