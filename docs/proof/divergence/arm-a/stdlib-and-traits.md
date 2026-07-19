---
type: architecture
title: Standard library, prelude, and traits
updated_round: 10
---

# Standard library, prelude, and traits

## Prelude types

Collections and core types available without import: `Vec`, `VecDeque`, `Map`, `Set`,
`SortedSet`, `SortedMap` (round 6, B-tree ordered map), `Option`, `Result`, `Entry`
(`Occupied` / `Vacant`), `Channel` / `Sender` /
`Receiver`, `Atomic`, `F32`, `F64`, `Peekable`, `Ordering`, `IoError`, `VarError`, plus
`Regex` / `RegexError` / `Match`, `Stats`, HTTP + encoding (`Base64` / `Hex` / `Url`)
surfaces, and `Pool` / `PooledConnection`. Round 5 added **`AllocError`** (the
[[fallible-allocation|fallible-allocation]] error) and **`Mutex`**; the round-5 numerical
types **`Tensor[T, Shape]`** and **`Vector[T, N]`** live in the
[[numerical-stdlib-and-tensors|numerical stdlib]] / [[simd|SIMD]] surfaces.

Note the **`Ordering` split** into a comparison `Ordering` and a memory `Ordering`
(atomic memory ordering) — see [[history-reversals-and-deprecations]].

## Baked traits (CR-202)

Traits are **"baked"** — defined in real `.kara` stdlib source (under `runtime/stdlib/`)
and pulled in via `include_str!` with a `#[compiler_builtin]` gate, rather than
registered programmatically. This was **CR-202**, a large refactor that made the
readable declarations the source of truth while keeping dispatch tables programmatic. It
**retired `register_stdlib_traits`** and split `register_builtin_types`. Source-of-truth
was swapped for `Option`, `Result`, and `Vec`.

Baked traits: `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash`, `Display`, `Debug`,
`Add` / `Sub` / `Mul` / `Div` / `Rem` / `Neg`, `BitAnd` / `BitOr` / `BitXor`,
`Shl` / `Shr`, `Index` / `IndexMut`, `From` / `Into`, `TryFrom` / `TryInto`,
`Iterator` / `IntoIterator`, `Not`. Derivable traits are documented in the book appendix.

`Clone` for collections shipped as a 9-subtask vertical (typechecker + interpreter +
codegen machinery + runtime helper + ASAN heap-cleanup tests).

## Iterator combinators

The `Iterator` trait surface closed at **14 subtasks**: `map`, `filter`, for-loop
consumption, `collect` / `fold` / `count`, `any` / `all`, `enumerate` / `take` / `skip`,
`chain` / `zip`, `flat_map`, `take_while` / `skip_while`, `step_by` / `cycle`,
`inspect` / `scan`, `peekable` / `peek`, `chunk_by`, `chunks` / `windows`. `Range` /
`RangeInclusive` and `Slice[T]` implement `Iterator` (Phase 8). Sorting: `sort_by` /
`sorted_by` (validates `Fn(T,T) -> Ordering`, honors closure comparator) and
`sort_by_key` / `sorted_by_key`.

## `std` modules (P1 longtail)

- **`std.cli`** — builder-style argument parser with subcommands and auto `--help` /
  `--version`. Graduated from the **v66** brainstorm.
- **`std.process`** — `Command` / `Child` / `ProcessTable`; spawn/wait/try_wait/kill
  intrinsic. **Round 4** added **stdin/stdout/stderr redirection** (`Stdio.Inherit` / `Null`).
- **Backpressure primitives** (round 4) — **`Semaphore`** (`new`/`acquire`/`release`),
  **`RateLimiter`** (per-key token-bucket `try_acquire`), and **`BoundedChannel[T]`**
  (capacity-bounded `send`/`recv`). See [[design-concurrency-and-providers]].
- **`std.tracing`** — `Span` / `LogEvent` + `Exporter` trait. **Round 4** added a
  **compiled path**: full `std.tracing` codegen, a **`StdoutExporter`** emission surface,
  ambient **`Log.*`** emission, and **active-span propagation** (`with_span` + auto-stamp,
  inherited across `par`). See [[codegen]].
- **`std.http`** — HTTP server (`Server.serve(addr, handler)`) with a handler-ABI
  trampoline; round 3 exposed `Request.body()` / `Request.header()`. **Round 4** added the
  full server surface (response headers, `headers()`/`query()`, keep-alive/chunked,
  **HTTP/2**), an **HTTP client** (`Client.get`/`post`, chained `RequestBuilder`,
  `Response.text/bytes/header/headers`), and **client/server TLS**. See [[networking]].
- **`std.json`** — JSON support (Slice F); round 3 added a **compiled-binary path** —
  `Json.parse(s)` and `Json.stringify()` codegen (previously interpreter-only). See
  [[codegen]].
- **`std.runtime`** — introspection APIs (debugger contract).
- **`Pool[T]`** — connection-pool primitive; `#[must_use]` on `PooledConnection[T]`.

## Collection methods (round 2 additions)

- **`Vec.with_capacity(n)`** — empty `Vec` with pre-allocated capacity; the type checker
  infers `Vec[?T]` so untyped `let` works.
- **`Vec.extend_from_slice(other)`** — bulk-append, with per-element clone for RC-bearing
  `T` and a src/dst-overlap rejection on grow.
- `Vec.from_slice` accepts a nested-index source (`Vec.from_slice(rows[r])`).

## Round-3 stdlib additions

- **File I/O and networking** — a `File` handle, non-blocking **TCP** / **rustls TLS** /
  **RFC 6455 WebSocket** surfaces, and `FileSystem.read_to_string` — all built on the event
  loop. Covered in [[networking]].
- **`Atomic[T]`** — real `.load` / `.store` / `.new` for int types plus `Atomic[bool]`; see
  [[design-ownership]].
- **Structured concurrency** — `spawn`, `TaskGroup`, `TaskHandle[T]` prelude/stdlib
  declarations (`runtime/stdlib/task_group.kara`); see [[design-concurrency-and-providers]].
- **String** — `String.from_utf8` (with a `Utf8Error`), `String.substring(start)`,
  `String.starts_with(prefix)`, `String.push(char)` / `push_str`, `String.bytes() ->
  Slice[u8]`.
- **`i64.parse(s: String) -> Option[i64]`** — all four layers plus a runtime extern.
- **`Vec.remove(idx)`**, **`Vec.new()` / `VecDeque.new()`** (module-binding const-init),
  `Vec[a, b, c]` prefix literals, and `Vec[T].pop()` dispatched as an alias for `pop_back`.

## Round-4 stdlib / type-surface additions

- **Verification types** — **contracts** (`requires`/`ensures`/`old()`/invariants),
  **refinement types** (a base narrowed by a predicate), and **distinct types**
  (`distinct type T = B`, with `.raw()`). A whole new correctness surface — see
  [[design-contracts-and-verification]].
- **HTTP client + HTTP/2 + client/server TLS**, **backpressure primitives**, and the
  **`std.tracing` compiled path** (above; [[networking]]).
- **`Vec.sort_by` / `sort_by_key`** matured across many key shapes — inline-closure and
  non-inline callees; integer, float (`f64.total_cmp`), String, integer-tuple, nested-struct
  cascade, and `derive(Ord)` struct keys. The **SoA** surface gained index-read,
  `entities[i].field`, and `pop`/`pop_back`/`pop_front`/`remove` (heap-bearing SoA fields are
  rejected). See [[codegen]].

## Round-5 stdlib additions

- **Buffered I/O** — **`BufReader[R]`** (interpreter MVP, `runtime/stdlib/bufreader.kara`):
  `lines() -> LinesIter`, `fill_buf` / `consume`. **`BufWriter[W]`** (`bufwriter.kara`):
  `write` (with a **cancel-safety** annotation), `write_all` (write-to-completion). These
  were the round-3 [[deferred-work|deferred `BufReader`]] entry, now built.
- **`Pool.with_health_check`** — an opt-in validation hook that **evicts on error**;
  `PooledConnection` **auto-releases on `Drop`**. See [[design-ownership]].
- **`std.process`** — **`Stdio.Piped`** with `Child` stdout/stderr/stdin **capture handles**,
  and a **`blocks` execution verb** on the synchronous `Child.wait()`.
- **[[numerical-stdlib-and-tensors|`Tensor[T, Shape]`]]** — the shape-typed numerical stdlib
  (Phase 11), `runtime/stdlib/tensor.kara`.
- **[[simd|`Vector[T, N]`]]** — portable SIMD, with a first-class **`Numeric` trait**.
- **Numeric conversions** — `wrapping_add/sub/mul` (64-bit), **saturating float→int** +
  int↔float conversion method families, `abs()` / `to_string()` / `clone()` on scalar
  primitives, `char.try_from(n) -> Result[char, i64]`, `f64.parse -> Option[f64]`,
  `i64.from_str_radix(s, radix) -> Option[i64]`, integer `parse`/`from_str_radix` typed as
  `Option[<int>]`, and **`u8` ASCII predicates** (`is_ascii_digit`/`_alphabetic`/
  `_hexdigit`). `src/numeric_conv.rs` (+338).
- **String** — two-arg **`String.substring(start, end)`** (byte-range slice), **`String.repeat(n)`**,
  **`String` slicing `s[a..b]`** (interpreter MVP + codegen), **`String.contains`** /
  **`Vec.contains`**, and borrowed `ref String` args accepted for `push_str`/`contains`/
  `starts_with`.
- **Borrowed read-accessors** — `Vec`/`Slice` **`get`/`first`/`last` return `Option[ref T]`**
  (a view, not a copy), and read-only methods work on borrow-locals. See [[design-ownership]].
- **`try_clone`** — a fallible deep clone for `Vec` / `VecDeque` / `String` / `Tuple`; part
  of the [[fallible-allocation|fallible-allocation]] `try_*` family.
- **Collection `Display`** — collections render in f-strings / `to_string` (unified on
  buffer-append). See [[codegen]].
- **Variance declarations** — **per-stdlib-type `+T` / `-T` / `=T`** markers with a verifier
  (`src/typechecker/variance.rs`). See [[design-generics-and-impl-trait]].
- **`CStr.to_string() -> Result[String, Utf8Error]`** and `CStr.from_ptr` — see
  [[design-unsafe-ffi-and-pointers]].
- **Tracing** — a **configurable ambient `Log` exporter** with a min-level filter
  (interpreter + codegen halves); the **active span survives real suspend/resume**.

## Round-6 stdlib additions

Round 6 was heavy on **stdlib completeness**, most of it surfaced by the **kata-gap audits**
(cross-kata canonical-idiom sweeps) and now wired through **all four backends** (typecheck +
interpreter + codegen + runtime) unless noted. See [[bug-tracker]], [[codegen]].

- **String (String→String, full Unicode)** — **`trim`** / **`replace`** / **`to_lowercase`**
  / **`to_uppercase`** (`B-2026-06-20-2`; Unicode case maps can change byte length, e.g.
  `straße`.to_uppercase() == `STRASSE`), **`ends_with`**, and codegen for **`String.from_utf8`**
  (was interpreter-only). Runtime helpers in `runtime/src/clone.rs` reuse the same Rust
  stdlib calls the interpreter uses, so both backends are bit-identical on Unicode.
- **String char access** — **`s.char_at(i) -> Option[char]`** and **`s.char_count() -> i64`**
  (O(n) Unicode-scalar indexing, `B-2026-06-18-3`), **`s.chars().collect() -> Vec[char]`** and
  a standalone `s.chars()` iterator value (`B-2026-06-18-1/-5`), and `for c in s` now binds
  **`char`** (`B-2026-06-18-2`).
- **`char`** — `char.is_uppercase()`/`is_lowercase()` codegen (`B-2026-06-18-4`) and
  **`char.to_digit(radix) -> Option[u32]`** (interpreter; codegen honest-Err, `B-2026-06-19-13`).
- **Integer methods** — **`{checked,saturating,overflowing}_{add,sub,mul}`** at every width
  (`B-2026-06-19-10`), **`n.pow(k)`** (trapping at the receiver width), and bit intrinsics
  **`count_ones`/`leading_zeros`/`trailing_zeros` -> u32** (`B-2026-06-19-12`).
- **`Vec`/`Slice.binary_search(x) -> Option[i64]`** — codegen for int + String elements
  (`B-2026-06-20-3`); **`Vec[String].sort()`** (was integer-only in codegen, `B-2026-06-20-11`).
- **`SortedMap[K: Ord, V]`** — a **B-tree-backed ordered map** (`b7b8b9ec`,
  `runtime/stdlib/sorted_map.kara`): the full map surface plus ordered queries `min`/`max`,
  `floor`/`ceiling`, and `range(lo, hi)`; ascending-key iteration. Interpreter-complete;
  **codegen is interpreter-only in v1** (a `SortedMap.new()` construction emits an honest
  "interpreter-only, use `Map[K,V]`" error), mirroring `SortedSet`. `B-2026-06-20-5`.
- **`Map` entry / lookup** — **`Map.entry(k).or_insert(d)` write-through** (a `mut ref V`
  place-ref via `MapSlotRef`, so `*m.entry(k).or_insert(0) += 1` lands in the map) and
  **`Map.get_or(k, default)`** (`B-2026-06-20-8`).
- **`Set[Vec[T]]` / `Map[Vec[T], _]`** dedupe by **content** (Vec is Hash+Eq iff its element
  is; `B-2026-06-20-15`).
- **[[protobuf|Protobuf (proto3)]]** — `runtime/stdlib/protobuf.kara`, `#[derive(Message)]`
  over the new [[metaprogramming|comptime substrate]].
- **`std.web.events` / `std.web.time`** — browser host-async producers (click / pointer /
  wheel / keyboard / focus / resize / touch, and a recurring `every` timer), lowered to
  `Channel[T]`. See [[wasm-targets]].
- **`ExitCode`** entry-point return type (`runtime/stdlib/exitcode.kara`) and
  **`#[derive(Default)]`** + the **`#[default]`** variant marker (see [[metaprogramming]],
  [[attributes]]).

## Round-7 stdlib additions

Round 7 added a cluster of **allocation / caching / interning primitives** (each an
**interpreter slice** — codegen is a follow-on) plus a fully-backed **columnar** type and
scalar float math.

- **[[columnar-data|`Column[T]`]]** — a **nullable Arrow-buffer column** with **SQL
  three-valued-logic** arithmetic/comparison (`runtime/stdlib/column.kara`). `fillna` (with a
  **`treat_nan_as_null`** flag), `dropna`, `from_iter_nullable`, `iter` / `iter_valid`. Wired
  through **typecheck + interpreter + codegen** (unlike the interpreter-only primitives
  below). The columnar data-engineering primitive. See [[columnar-data]], [[codegen]].
- **`OnceLock[T]` / `OnceCell[T]`** (`runtime/stdlib/once.kara`, `7b7b49e9`) — **write-once
  lazy-init cells** (interpreter slice). `OnceCell[T]` carries a **single-task structural
  enforcement** (3 rules, typecheck, `46ca93af`) — the non-thread-safe cell may not cross
  tasks. `OnceLock` has an **effect surface** (tests, `a33c0069`; Phase-8 checklist mark).
  See [[design-effect-system]].
- **`Arena[T]` / `ArenaRef[T]`** (`runtime/stdlib/arena.kara`, `aaf143d1`) — a
  **bulk-allocation** primitive (arena + handle), interpreter slice. Graduated from the
  **v70 arena/handle** brainstorm (see [[history-reversals-and-deprecations]]).
- **`Symbol` + `Interner`** (`runtime/stdlib/interner.kara`, `2212f17d`) — a **dedup
  string-handle** primitive (string interning → `Symbol` handles), interpreter slice.
- **`Map.new()` / `Set.new()` as module-binding const-init** (`e9bc1a6d` typecheck,
  `d30e076e` codegen) — admitted as module-binding initialiser special forms, joining the
  round-3 `Vec.new()` / `VecDeque.new()`. (Separate from the `Map.new()` **K/V-inference**
  fix `B-2026-06-22-1`; see [[bug-tracker]].)
- **Scalar transcendental + rounding math on floats** (`1b404d96`, `src/float_math.rs`) —
  across typecheck + interpreter + codegen.
- **Uppercase-receiver field access on value bindings** (`c20a85b2`) — all three backends.

## Round-8: the trait system matures

Round 8's headline is the **trait system substantially maturing** — user trait impls now
work over builtin containers and primitive scalars, trait default methods are inherited,
generic-bound dispatch and monomorphization landed, and a family of surface reduction traits
went in. See [[bug-tracker]], [[codegen]], [[implementation-phases]].

- **Surface reduction traits (S6a → S6c).** New stdlib traits `Reduce`, `ElementwiseMap`,
  `ElementwiseOrd` (`85277bd3`; `runtime/stdlib/reduce.kara` / `elementwise_map.kara` /
  `elementwise_ord.kara`). Both `Column` and `Tensor` `impl Reduce`, enabling bound-generic
  dispatch (`fn f[C: Reduce[T]](c: ref C)`). Grew through S6c: `Reduce.fold` (`5728a7f0`),
  `Reduce.prod` (`43fddcd9`), `ElementwiseMap.map` / `zip_with` (`9d00be44`). Column/Tensor
  gained `.fold[A]` / `.map` / `.zip_with` / `.sorted` / `.argsort` / `.argmin` / `.argmax`
  (see [[columnar-data]], [[numerical-stdlib-and-tensors]]); builtin Column/Tensor inherit
  `Reduce.range` = max−min (`82691ca8`, `14ab65ae`).
- **User trait impls over builtin containers (S6c-12).** Users can now write trait impls
  over `Column[T]` (`f53a8035`, slice 1 — a 3-surface fix) and `Tensor[T, S]` (`ade24684`,
  slice 2), including **generic container impls** (`2611229b` slice 3, `1593a402` slice 4)
  and trait-default-methods over containers. Residuals: **B-2026-07-04-15** stays **OPEN** (a
  generic container impl's trait DEFAULT method fails to resolve on the SECOND element
  monomorphization — false-positive, low, `karac run` correct); **B-2026-07-04-16** was
  **fixed** (`735c5717` — a generic Tensor impl's `T+T` was lowered as integer add for a
  non-i64 element, which could silently miscompile).
- **Trait default methods (were not inherited — now are).** B-2026-07-03-8 (`6d488e58`):
  trait methods with default bodies are inherited/dispatched onto implementors via a
  pre-resolve desugar pass `synthesize_trait_default_methods` (`src/desugar.rs`) that copies
  each non-overridden default into every impl block. **Generic**-trait defaults followed
  (B-2026-07-03-10, `2ddfa564` — substitute the impl's trait-args). Baked-stdlib trait
  defaults are spliced into user impls too (B-2026-07-03-19, `c0a83c33`) — e.g. `Reduce.range`
  inherited by a user `impl Reduce[T]`.
- **User trait impls on PRIMITIVE scalar types.** B-2026-07-03-5 (`ae7c9525`):
  `impl Tr for u8 { fn m(self) -> Self { self + self } }` now works end-to-end (was broken —
  `self` not numeric in the body, method never registered, interpreter width-erased dispatch).
  Follow-on B-2026-07-03-24 (`d15c8372`) fixed generic-BOUND dispatch over primitive impls when
  MULTIPLE widths impl the trait.
- **Generic-bound trait dispatch + monomorphization.** B-2026-07-03-11 (`db020ee6`): codegen
  dispatches a trait method called through a generic type-param bound (`fn use_it[X: Named](x:
  X) { x.tag() }`). B-2026-07-03-15 (`cb6919c3`): monomorphize a generic impl/trait method on a
  concrete receiver. B-2026-07-03-23 (4 layers): a generic struct with an inline type-param
  field (`struct Box[T] { v: T }`) now monomorphizes its layout + methods by element type (was
  defaulting element to i64 and mis-reading non-i64 fields). B-2026-07-03-18 (`5c761a6e`):
  arithmetic operators (`+ - * / %` and unary `-`) admitted on an operator-trait-bounded type
  param (`T: Add`); user operator-trait impls stay **forbidden** (stdlib-only), so this remains
  sound.
- **Derived-`Ord` struct/enum ordering (`<` `<=` `>` `>=`).** B-2026-07-03-7 (`ba67416e` +
  `22705148` + `31280a03`): `#[derive(Ord)]` structs/enums support the comparison operators and
  `Vec[Struct/Enum].sort()`, ordered by **declaration order** (variant discriminant / field
  order), on both run and build, gated by an opt-in predicate applied identically on both
  surfaces. Sibling interpreter fixes: B-2026-07-03-6 (`value_compare` had no Struct/EnumVariant
  arm → SortedSet/SortedMap collapsed distinct keys, silent data loss) and B-2026-07-03-12
  (ordering is declaration-order not alphabetical, via a per-thread type-order registry).
  B-2026-06-30-14 (`1cf09cb6`): a SILENT interpreter miscompile where `match x.cmp(y) { Less =>
  .., ... }` with BARE `Ordering` variants always took the first arm (variants were only
  registered under qualified names).
- **String methods.** `String.sorted()` codegen (B-2026-06-30-12, `a00a2f58`,
  `karac_string_sorted` runtime helper); `String.cmp(other) -> Ordering` typecheck + codegen
  (B-2026-06-30-13, `1cf09cb6`; non-identifier receivers B-2026-07-02-9, `3c8cd55b`).
  `Vec.sort()` gained general element comparators for structs / enums / nested-Vec / floats /
  tuples (B-2026-06-30-15, `463fa826` — a recursive `karac_cmp_<T>` family).

New / grown stdlib source files this round: `runtime/stdlib/reduce.kara`,
`elementwise_map.kara`, `elementwise_ord.kara`, `column.kara`, `dataframe.kara`, `tensor.kara`,
and `stats.kara`.

## Round-9 stdlib / trait additions

Round 9 filled a batch of Phase-8 stdlib floor items and pushed several traits/collections
through **codegen** (many were interpreter-only before), plus the round's dominant theme —
closing **run-vs-build divergences** exposed by the [[cli|JIT-default `run`]] flip.

- **`std.mem`** — **`swap` / `replace`** move-without-drop primitives (`b69f9de8`) and
  **`take`** (`d9b561c2`), the latter riding a new **generic `T.default()` codegen** (a
  `T: Default` bound now discharges for `#[derive(Default)]`/primitive args and monomorphizes to
  the concrete `<S>.default()`).
- **`std.cmp`** — **`min` / `max` / `clamp`** free functions (`a9eee653`, Phase 8), with the
  ownership pass fixed so a generic-trait `ref Self` param is a borrow, not a move
  (`B-2026-07-08-11`).
- **`.cmp() -> Ordering` for derived-`Ord` types** (`2a8f10fe`) across typecheck + interpreter +
  codegen — the method form of `<`/`>`.
- **`Display` for `Option` / `Result`** — built-in `Option`/`Result` values render under
  codegen (`Some(7)` / `None` / `Ok(..)` / `Err(..)`, `677b3fd9`), including **call-results**
  (`ae88b135`), and a **debug-format `Display` for nested struct payloads** (`1d8f3e44`,
  `B-2026-07-08-18` — `Some(P { x: 3, y: 4 })`, matching the interpreter). A **unit-typed
  f-string interpolation is now a type error** (`B-2026-07-08-8`).
- **`Result::unwrap_err` / `expect_err`** (`432f07ae`, `B-2026-07-09-10`).
- **`SortedSet` / `SortedMap` now lower under codegen** (`B-2026-07-09-16/-17`) — sharing the
  `Map`-backed hash storage for order-independent ops and materializing ascending order only at
  iteration + `min`/`max` (integer + String keys), retiring their round-6 interpreter-only
  status. **`Map` / `Set.try_insert`** got fallible-allocation codegen + a fallible runtime
  `karac_map_try_insert` (`B-2026-07-09-15`, closing the last phase-8 fallible-alloc leaf).
- **Associated-type projection in mono signatures** (`56036203`, Phase 8) — a `-> Self::Assoc`
  in a monomorphized signature now resolves.
- **The diverging primitive `panic()`** was wired end-to-end (resolver + interpreter + codegen,
  `f0f74d65`, `B-2026-07-09-9`) — it had been recognized by the typechecker/effectchecker but
  rejected by the resolver as undefined; **`#[track_caller]`** caller-location redirection +
  stdlib panic-emitters landed alongside (phase-5).
- **Blanket / user-type trait dispatch tails** — `impl Trait for Vec[T]` loop-bodied impls now
  work on all three surfaces (`B-2026-07-06-5`, `6321ee8`), and bound-generic dispatch over a
  **user-type** implementor lowers under `karac build` (`B-2026-07-06-2`, `4f3e5747`).
- **C strings** — owning **`CString`** + `String.to_cstring` + `NulError`, and a zero-copy
  **`CStr.to_string_slice`** view — see [[design-unsafe-ffi-and-pointers]].
- **Integer widening coercion tightened** (`B-2026-07-09-7`) — a non-literal integer at a
  binding boundary must **widen**; a narrowing or sign-changing implicit coercion (`let s: u32
  = big_i64`, any signed→unsigned) is now a type error naming the required `as`, consistent
  with the mixed-integer-arithmetic rule. An out-of-range integer *literal* at a narrow context
  was already rejected in round 8.

## Round-10 stdlib / trait additions

Round 10 was a **huge Phase-8 stdlib expansion** — a broad "phase-8 stdlib-floor" sweep
plus a "phase-11 stdlib-longtail" tail — landing most additions across **typecheck +
interpreter + codegen** unless noted. See [[bug-tracker]], [[codegen]].

- **String methods (Phase 8).** **`.strip_prefix`** / **`.strip_suffix`** → `Option[String]`;
  **`.split_whitespace() -> Vec[String]`**; **`.trim_start()`** / **`.trim_end()`**;
  **`.lines() -> Vec[String]`**; **`.chars().count()`** / **`.len()`** ergonomics
  (`B-2026-07-11-13`); and **`From[char] for String`** (`String.from(c)` / `c.into()`).
- **`char` methods (Phase 8)** — **`.to_ascii_uppercase()`** / **`.to_ascii_lowercase()`** /
  **`.is_ascii()`**.
- **Integer methods (Phase 8)** — **`.next_power_of_two()`** / **`.is_power_of_two()`**,
  **`.abs_diff(other)`** (returns the unsigned sibling), **`.rotate_left(n)`** /
  **`.rotate_right(n)`**, **`.count_zeros()`** / **`.reverse_bits()`** / **`.swap_bytes()`**,
  **`.div_euclid()`** / **`.rem_euclid()`** (i64), **`.signum()`** (signed ints & floats), and
  scalar **`.min()`** / **`.max()`** / **`.clamp(lo, hi)`** method forms for numeric types
  (the method twins of the round-9 `std.cmp` free functions).
- **Float methods (Phase 8)** — the full **transcendental family**: inverse trig, hyperbolics,
  `exp2` / `log10` / `trunc`, **`.copysign()`**, **`.fract()`**, **`.hypot()`**, inverse
  hyperbolics, **`.exp_m1()`** / **`.ln_1p()`**, **`.recip()`** / **`.to_degrees()`** /
  **`.to_radians()`**. See [[numerical-stdlib-and-tensors]].
- **f-string format specifiers (Phase 8)** — **`f"{expr:spec}"`** with width, zero-pad, align,
  radix, and precision (new `src/format_spec.rs`, +435). `ParsedInterpolationPart::Expr` gained
  a format-spec field. See [[codegen]].
- **`ref_eq(a, b)`** — reference-identity comparison for shared types (Phase 8).
- **Collections** — **`Vec[T].insert(idx, value)`**, **`Vec[T].clear()`** and **`.extend(other)`**;
  **`Map` / `Set.try_insert` codegen** (was interpreter-only) → `Result[Option[V], AllocError]`
  / `Result[bool, AllocError]`; and **`SortedSet[T]` / `SortedMap[K, V]` codegen** — the
  interpreter-only ordered collections now lower to the `KaracMap` open-addressing storage,
  materializing ascending order only at iteration / min / max via a `karac_map_sorted_keys`
  runtime helper (integer + String keys), continuing the round-9 codegen push.
- **Iterator terminals + adaptors** (the round's big one, across typecheck + interpreter +
  codegen) — **`.sum()`**, **`.reduce()`**, **`.for_each()`**, **`.fold(init, f)`**, **`.any()`**
  / **`.all()`**, and **`.count()`** on fused chains; **`|_|` wildcard closure params** in
  chains; and **materialized iterator bindings** (`let it = v.iter(); it.<terminal>`) in codegen.
  A shared fold/any/all fused-chain codegen engine (`peel_fused_map_filter_chain` +
  `build_fused_chain_body`) backs the terminals and for-loops alike. Unlowered lazy adaptors
  (enumerate single-var, zip, skip/take/chain, step_by-on-iterator, flat_map, chunks, windows,
  cycle, scan, peekable, inspect) now **loud-bail** in codegen instead of silently skipping the
  loop (`B-2026-07-14-7` / `-9`; proper lowering deferred as `B-2026-07-14-8` / `-10`).
  **`for x in xs.iter_mut()`** (mutable iteration) is deferred and loud-bails on both backends.
- **Option / Result combinators.** **`Option/Result.map(f)`** across typecheck + interpreter +
  codegen (`B-2026-07-12-11`). Method resolution on `Option` / `Result` was **tightened** — an
  unknown method is now a clean compile error (`NoMethodFound` with a "did you mean" suggestion)
  instead of a silent `Type::Error` poison that passed typecheck then failed at runtime
  (`B-2026-07-14-5`, `1af84da`). Consequently several combinators are surfaced as **UNIMPLEMENTED**
  (compile-time reject) until built: `map_err`, `map_or`, `take`, `err`, `ok` (Result), `flatten`,
  `get_or_insert`, `and_then` (Result), `or` / `or_else` (`B-2026-07-14-6`, open).
- **`Result::unwrap_err()` / `expect_err()`** across all four layers (`B-2026-07-09-10`,
  matured this round).
- **I/O** — **`fs.read_lines() -> Result[Vec[String], IoError]`** (interpreter-first then
  codegen, `B-2026-07-11-38`); **`stdin.lines()`** streaming line iterator (run + build parity);
  **`BufReader.lines()`**. An adaptor chain over `stdin.lines()` / `br.lines()` is now **loudly
  rejected at typecheck** (`B-2026-07-11-34`) — those are opaque line iterators with no method
  surface. See [[networking]].
- **Type additions** — **`f16`** / **`bf16`** primitive numeric types (`09a2fc88`): bare
  `f16` / `bf16` now lex as identifiers / type-names like `f32` / `f64`, a reversal of the earlier
  "reserved keyword" stance (`B-2026-07-14-2`, see [[history-reversals-and-deprecations]]); the
  reduced-precision arithmetic/backend is a Phase-7 follow-up. Built-in numeric-narrowing
  **`TryFrom`** (`iN.try_from` / `.try_into`), and **`From[T] for Option[T]` / `Result[T, E]`**
  wrapping via `.into()`.
- **Lazy-init** — **`OnceLock` / `OnceCell` `set` / `get` / `get_or_init` codegen** extended from
  scalar to heap-fitting and then **wide `T`** (the `B-2026-07-12-2` epic, fully closed), plus
  **module-global `OnceLock`** via a static-init prologue. See [[design-effect-system]].
- **`std.mem take`** — the generic **`T.default()` under a `T: Default` bound** now monomorphizes
  in codegen (`B-2026-07-08-25`), so `take[T: Default]` ships as a real generic source body
  (finishing the round-9 `std.mem` slice).

## Module-level bindings (`let` / `let mut`) — new in round 3

A **module-level `let` / `let mut`** feature ("mod-let", Phase 8 P0, slices 1–10) allows
top-of-module bindings, distinct from `const`:

- Parser surface (slice 1), resolver registration + naming check (3), a **const-init rule**
  and type inference + **immutability check** (4–5).
- **Synthetic per-binding effect resources** (6) with a **par-block conflict rule** (7) and
  **`pub fn` synthetic-resource rejection** (8) — a module binding is modelled as an effect
  resource so concurrent access is checked. See [[design-effect-system]].
- Codegen lowering + composite-init + cross-fn binding visibility (9–10). See [[codegen]].
- A separate diagnostic **rejects a bare top-level `let`** with a did-you-mean-`const` hint.

## `test { }` blocks — new in round 3 (Phase 4)

Test discovery moved from a `fn test_*` naming convention to a first-class **`test { }`
block** (`Item::TestCase`): a parser/AST surface (slice 1), a discovery walk + name
mangling (2+3), and **modifier attributes** on a test case (4). The old **`fn test_*`
discovery was removed** and fixtures converted (slice 5); `docs/design.md` § Testing was
backfilled. See [[history-reversals-and-deprecations]].

## Low-level / FFI surface (round 2)

A `ptr` stdlib surface (`ptr.const` / `ptr.mut` / `offset_of` / `container_of` /
`container_of_mut`), **strict-provenance** APIs (no `ptr↔int` casts), FFI **union** types,
opaque foreign types, and **`c"..."` / `ref CStr`** c-string literals — all covered in
[[design-unsafe-ffi-and-pointers]].

## `#[must_use]`, attributes, and hygiene lints

General `#[must_use]` honoring plus a `missing_must_use` stdlib-hygiene lint; `Peekable[T]`
and `PooledConnection[T]` are annotated `#[must_use]`. Round 2 also added a
`missing_non_exhaustive` and `missing_track_caller` hygiene lint. The full attribute
surface — lint levels, `deprecated`, `non_exhaustive`, the diagnostic namespace — is in
[[attributes]].

Related: [[design-adt-and-pattern-matching]], [[codegen]], [[cli]],
[[design-unsafe-ffi-and-pointers]], [[design-generics-and-impl-trait]], [[attributes]],
[[protobuf]], [[metaprogramming]], [[wasm-targets]], [[columnar-data]], [[gpu-compute]].
