---
type: spec
created_date: 2026-07-15T17:34:08Z
last_modified: 2026-07-15T17:34:08Z
level: M1
stability: experimental
tags: comptime, metaprogramming, derive, reflection, codegen
---

# Comptime Metaprogramming

## Identity
Kāra's compile-time metaprogramming subsystem (comptime): a compile-time evaluator with constant folding, `Type` as a first-class value plus a reflection API, an AST builder + emission API, and derive desugaring — the substrate under `#[derive(...)]` and schema-driven codegen such as protobuf's `#[derive(Message)]`.

## Purpose
To let library and user code run logic at compile time — fold constants, inspect types via reflection, and synthesize code (derives, message serializers) — without a separate macro language, reusing the interpreter as the comptime engine.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **REFERENCES**: [[standard-library]]

## Realization

- src/comptime.rs (~1,260 lines) — the compile-time evaluator
- src/interpreter/comptime_builtins.rs (comptime builtins), src/interpreter/reflection.rs (Type reflection)
- src/desugar.rs (derive desugaring)
- tests/comptime.rs, comptime_ast.rs, comptime_derive.rs, comptime_reflection.rs

## Specifies

- Front-end recognition of the three comptime forms.
- Slice 1: a compile-time evaluator + constant folding (substrate 1).
- Slice 2: compile-time evaluator machinery / substrate.
- Slice 3: `Type` as a first-class value + reflection API (substrate 2).
- Slice 4: AST builder + emission (substrate 3).
- Slice 5: derive desugaring (substrate 4).
- `#[derive(Default)]`: synthesized `default()` + a `#[default]` enum-variant marker.
- `#[derive(Message)]`: protobuf comptime codegen, and a `.proto` schema → message-type generator (see [[standard-library]] protobuf module).

## Constraints

- Comptime evaluation runs on the interpreter, so a comptime form must be interpretable at compile time.

## Rationale

Introduced this round as the substrate under derive-based codegen; the protobuf `.proto` → message-types pipeline is its first non-trivial consumer.
