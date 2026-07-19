---
type: architecture
title: Attribute system
updated_round: 6
---

# Attribute system

Round 2 built out a broad **attribute system** — a Rust-style `#[...]` surface with a
registry, placement validation, and behavior in the type/effect/lint passes. The book's
`docs/book/src/appendix-d-attributes.md` is the canonical attribute reference. Attribute
parsing lives in `parser/attributes.rs`; validation in `src/attribute_validator.rs` and
per-feature lint modules.

An enabling change threaded attributes onto more AST nodes this round: **`ConstDecl`,
`TypeAliasDef`, `TraitMethod`, and `Variant`** now carry attributes.

## Lint-level attributes

A **lint registry** plus the four level attributes **`#[allow]` / `#[warn]` / `#[deny]` /
`#[forbid]`** (slices 1–7):

- **Scope cascade** — levels attach broadly (beyond `Function`) and cascade through scopes,
  emitting warnings as they resolve; `lint_name` is carried through into structured
  diagnostics.
- **`#[expect]`** — expectation attribute with **fulfilment tracking** and a circular-guard;
  an unfulfilled expectation and `unknown_lint` are themselves reported. `Expect` suppresses.
- **CLI parity** — `karac` accepts **`-A` / `-W` / `-D` / `-F`** with a cross-module
  fall-through; **forbid-mode rejection** cannot be overridden downstream.
- All four level attrs are **rejected on `unsafe_op_in_unsafe_fn`** (slice 6).

## `#[deprecated]`

Slices 1–4: payload (`note` / `since`) + placement check, **`Deprecation` threaded into
the resolver symbol table**, and **use-site warning emission** in the type checker.

## `#[non_exhaustive]`

Slices 1–7 (see also [[design-adt-and-pattern-matching]]):

- Flag + placement check; the `..` **rest-pattern** enabling change.
- **Cross-package** rules: struct-literal rejection, and an **enum-exhaustiveness wildcard
  rule** requiring a `_` arm when matching a foreign non-exhaustive enum.
- A **`missing_non_exhaustive`** stdlib-hygiene lint and a **machine-applicable fix-it** for
  the pattern + match.

## `#[must_use]` and `#[track_caller]`

- **`#[must_use]`** (slices 1–4) — general honoring of `#[must_use]` on functions and types
  plus a **`missing_must_use`** stdlib-hygiene lint (`src/must_use_lint.rs`,
  `missing_must_use_lint.rs`). `Peekable[T]` and `PooledConnection[T]` are annotated.
- **`#[track_caller]`** — flag + placement (slice 1), trait-method-closure support, and a
  **`missing_track_caller`** lint (`src/missing_track_caller_lint.rs`).

## Diagnostic-namespace attributes

The **`#[diagnostic::...]`** namespace (slices 1–6, `src/diagnostic_attrs_lint.rs`):

- Attribute **path migration** + `is_bare` helper; namespace dispatch and
  **`E_UNKNOWN_ATTRIBUTE`**.
- **`#[diagnostic::on_unimplemented]`** — recognized, with `malformed_diagnostic_attribute`
  checks and **substitution at the failed-bound emit site**, plus a "trait X is implemented
  by …" note.
- **`#[diagnostic::do_not_recommend]`** — recognized, with impl-block-only placement checks.

## Tool-namespaced attributes

Slices 1/3/5: **`karac query attributes [--tool PREFIX]`** surfaces tool-prefixed
attributes; v1-reserved tool name-claims are documented. See [[cli]].

## `#[profile(...)]`

The **profile attribute** (slices 1–3) — parser + AST, resolver validation, and
**effect-checker integration** (a profile gate on effects; see [[design-effect-system]]).

## `#[unstable]` (new in round 4)

**`#[unstable]`** (phase-8 line 49) marks an API as not-yet-stable. The **attribute
machinery** landed first, then a typechecker lint fires **`#[unstable]` / `#[deprecated]`
warnings at method- and assoc-fn call sites** (`0cb76707`). It gates pre-1.0 surface such as
`serve_static` in `std.http` (see [[networking]]).

## Round-5 attributes

- **`#[target(...)]`** — selects the [[wasm-targets|target(s)]] an item is built for; the
  parser validates it and the **target-gate pass** (`E0411`) checks provided-resource sets
  per target. See [[design-effect-system]], [[wasm-targets]].
- **`#[require_simd]`** — a **scalarization guarantee**: asserts a [[simd|SIMD]] op must
  lower to real vector instructions, not scalarize. See [[simd]].
- **`#[link_name]`** — honored on `unsafe extern fn` imports (rename the linked symbol). See
  [[design-unsafe-ffi-and-pointers]].
- **`must_use` displaced-value exception** — container mutators (that return a displaced
  `Option`) are **exempt** from the implicit-`Option` `must_use` lint, so `map.insert(...)`
  needn't consume the displaced value.

## Round-6 attributes

- **`#[inline]` / `#[cold]`** — **codegen hint attributes** that emit the corresponding LLVM
  function attributes (`b1b7b79c`). See [[codegen]].
- **`#[derive(...)]` via [[metaprogramming|comptime]]** — round 6 rebuilt derive as
  **comptime desugaring** (reflect → build-AST → emit) rather than hand-coded Rust. New
  derives on that path: **`#[derive(Default)]`** (synthesizes `default()`) with a
  **`#[default]`** enum-variant marker, and **`#[derive(Message)]`** for [[protobuf]]
  encode/decode. See [[metaprogramming]], [[design-adt-and-pattern-matching]].

## Round-3 attributes and marker traits

- **`#[par_unordered]`** — a loop-expression attribute that opts into **collect-style
  auto-par reductions** (`acc.push()` recognized as a reduction only under it). See
  [[design-concurrency-and-providers]], [[codegen]].
- **`#[cancel_unsafe_until]`** — a flow-sensitive attribute feeding the
  [[design-runtime-phases|RAII-across-yield]] `CancelSafe` check (Phase 6 line 155).
- **`CancelSafe` marker trait** — user-extensible opt-in marking a type as safe to hold
  across a cooperative-cancel/yield point.
- **`ScopeLocal` marker trait** — marks a value that must not escape its task scope
  (structured-concurrency escape rejection). See [[design-concurrency-and-providers]].

Related: [[design-adt-and-pattern-matching]], [[design-ai-first-compiler]],
[[design-effect-system]], [[cli]], [[bug-tracker]], [[design-runtime-phases]],
[[metaprogramming]], [[protobuf]].
