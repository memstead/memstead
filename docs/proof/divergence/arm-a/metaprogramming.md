---
type: architecture
title: Compile-time metaprogramming (comptime, reflection, derive)
updated_round: 9
---

# Compile-time metaprogramming — comptime, reflection, derive

Round 6 introduces a **new compiler layer**: a compile-time metaprogramming subsystem (**comptime**) that lets Kāra code run during compilation to fold constants, inspect types, and synthesize new code. It is `karac`'s general mechanism for **derive** and the foundation the new [[protobuf]] stdlib builds on. Core lives in `src/comptime.rs` (+1258), with runtime support in `src/interpreter/comptime_builtins.rs` (+143) and `src/interpreter/reflection.rs` (+142). Tests: `tests/comptime.rs`, `tests/comptime_ast.rs`, `tests/comptime_derive.rs`, `tests/comptime_reflection.rs`.

## The three comptime forms

The front-end was taught to **recognize the three comptime forms** (`1706fcdc`) — the surface syntax for expressing compile-time constructs. This parser/front-end recognition is the entry point: everything downstream operates on code the front-end has already tagged as comptime.

## Substrate slices

The subsystem was built as a sequence of stacked **substrate** slices, each enabling the next:

- **Substrate 1 — compile-time evaluator + constant folding** (`8382a311`). The evaluation core: comptime expressions are evaluated during compilation and their results folded into constants. This is the engine the higher slices run on.
- **Substrate 2 — `Type` as a first-class value + reflection API** (`81323a96`). `Type` becomes a **first-class comptime value**, and a **reflection API** (`src/interpreter/reflection.rs`) lets comptime code inspect types — fields, variants, and structure — programmatically. Sits close to the generics/type surface; see [[design-generics-and-impl-trait]].
- **Substrate 3 — AST builder + emission** (`5c966b0e`). Comptime code can **build AST nodes and emit them** — a quasi-quoting / code-emission capability. Reflection (read) plus emission (write) together give comptime code the ability to generate code from a type's shape.
- **Substrate 4 — derive desugaring** (`0f427a70`). `#[derive(...)]` is **desugared through the comptime substrate** rather than hand-coded in Rust. Derives become ordinary comptime programs: reflect over the target type, build the impl AST, emit it. This unifies derive lowering with the rest of comptime rather than special-casing each derive in the compiler.

## Derive landings

Built directly on substrate 4:

- **`#[derive(Default)]`** synthesizes a `default()` (`905c6880`).
- **`#[default]`** enum-variant marker selects which variant `#[derive(Default)]` uses (`18de9672`).

Both go through the comptime path — reflect, build, emit — rather than bespoke Rust codegen. See [[attributes]] for the derive/attribute surface and [[codegen]] for how emitted impls lower.

## Foundation for protobuf

This comptime/derive substrate is the **foundation for the new [[protobuf]] stdlib**. Protobuf's `#[derive(Message)]` is implemented as **comptime codegen** over this substrate: it reflects over the annotated type and emits the encode/decode impls at compile time. Without the reflection + AST-emission substrate, protobuf's derive could not exist — the dependency is direct. **Round 7** leaned on this hard: the full proto3 field-type matrix — including **`sint`/`fixed`/`sfixed` wire-encoding selection and per-field number overrides** — is driven by **field-attribute reflection** over the comptime surface. See [[protobuf]].

## Round 9 — `#[derive]` now works under codegen (3-layer fix)

Before round 9, the comptime derive-fold ran in a lowering pass **after** typecheck + operator-lowering, so **derive-generated method bodies were never typechecked or lowered** and codegen's span-keyed side-tables were empty for their locals. A `#[derive(...)]` therefore worked under `karac run` (interpreter dynamic dispatch) but **failed `karac build`** — a run-vs-build gap that the [[cli|JIT-default `run` flip]] made a *default*-path failure. `B-2026-07-08-15` closed it in three landable layers:

- **Layer 3 — skip comptime fn bodies in codegen** (`cb3734ad`) — a user `comptime fn` (whose body does reflection, `T.name()` / `ast.item(..)`) must never reach codegen; both codegen function passes now skip it. This was the first observable end-to-end `#[derive]`-under-codegen case.
- **Layer 1 — typecheck derive-generated bodies + span reanchor** (`960e41f3`) — when a program has derives, the comptime fold runs **first**, then the spliced program is **re-resolved + re-typechecked + operator-lowered**. Each `ast.item(..)` fragment claims a unique high span window (relative offsets preserved) so two generated locals don't collide on one span key (a silent miscompile before).
- **Layer 2 — compile `std.protobuf` bodies under codegen** (`fc6f2308`) — the pure-Kāra protobuf encoder namespace is compiled through codegen (gated on a `#[derive(Message)]` being present, since its bodies form call cycles the dead-code prune can't collect), so a derived `encode`/`decode` calls the real emitted symbols.

The result: a `#[derive(Message)]` (and custom derives with core-ops / standard-collection bodies) now round-trips under interp == JIT == AOT. See [[protobuf]], [[bug-tracker]].

Related: [[protobuf]], [[attributes]], [[design-generics-and-impl-trait]], [[codegen]], [[stdlib-and-traits]], [[compiler-pipeline]], [[bug-tracker]], [[index]]
