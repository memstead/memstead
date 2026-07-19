---
type: spec
created_date: 2026-07-15T07:26:36Z
last_modified: 2026-07-15T07:26:36Z
level: M1
stability: stable
tags: types, enums, pattern-matching
---

# Algebraic Data Types and Pattern Matching

## Identity
Kāra's Rust-style algebraic data types — enums with payload-bearing variants — and the exhaustive pattern-matching that consumes them, including slice/array patterns, range patterns, and `@` bindings.

## Purpose
To give the language sum types with compile-time exhaustiveness so illegal states are unrepresentable and every case is handled.

## Realization

- Pattern AST: src/ast/patterns.rs; parsing: src/parser/patterns.rs
- Exhaustiveness: src/exhaustive.rs (Maranget algorithm, O(N²) fix), W0237 unreachable-arm warnings
- Match codegen: src/codegen/control_flow_match.rs, control_flow_slice.rs; compound-payload + tuple-payload destructure
- Interpreter: src/interpreter/pattern_match.rs

## Specifies

- Enums with unit, tuple, struct, and compound payloads; `Option`, `Result`, `Ordering`, `Entry` as baked enums.
- Match patterns: wildcards, bindings, literals, struct/tuple destructure, qualified paths, slice/array patterns, bounded range patterns, `@` bindings.
- Exhaustiveness checking via the Maranget algorithm; unreachable arms warned (W0237).
- Refutable patterns rejected in plain `let`; compound-payload enums get drop-path and pattern-bound element-type registration.

## Constraints

- Every match must be exhaustive or the program is rejected.
- Plain `let` accepts only irrefutable patterns.

## Rationale

A committed design pillar; exhaustiveness is what makes enum-based error handling (Result/Option) safe at compile time.
