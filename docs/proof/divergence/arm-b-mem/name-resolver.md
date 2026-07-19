---
type: spec
created_date: 2026-07-15T07:27:31Z
last_modified: 2026-07-15T17:33:09Z
level: M0
stability: stable
tags: compiler, resolver, phase-3
---

# Name Resolver

## Identity
The Kāra name-resolution pass: a two-pass resolver that collects top-level items, then resolves types, patterns, effects, and bounds references throughout blocks and statements.

## Purpose
To bind every identifier to its declaration before type and effect checking, and to gate compiler-only attributes like `#[compiler_builtin]` to stdlib source.

## Relationships
- **REFERENCES**: [[type-checker]]
- **REFERENCES**: [[effect-checker]]
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[parser-and-ast]]

## Realization

- src/resolver.rs and src/resolver/ (collect.rs = Pass 1 top-level collection, resolve_items.rs = Pass 2, resolve_block.rs, resolve_refs.rs)
- tests/resolver.rs

## Specifies

- Pass 1: collect all top-level declarations. Pass 2: resolve item bodies.
- Block/statement/expression resolution; type/pattern/effect/bound reference resolution.
- Gates `#[compiler_builtin]` to stdlib source and to impl blocks/methods.

- Same-scope `let` shadowing is now allowed (design.md § Variables), including type-changing shadows (the codegen metadata-purge handles the slot type change).

## Constraints

- `#[compiler_builtin]` is legal only on stdlib-origin items.

## Rationale

Part of Phase 3 semantic analysis; a prerequisite for the [[type-checker]] and [[effect-checker]].
