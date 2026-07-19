---
type: decision
created_date: 2026-07-15T07:23:02Z
last_modified: 2026-07-15T07:23:02Z
status: accepted
decided_on: 2026-06-15
deciders: kara-maintainers
scope: subsystem
tags: stdlib, prelude, CR-202, refactor
---

# Bake Standard Library in Kara Source

## Decision
We chose to define the readable parts of the standard library as actual `.kara` source files compiled into the binary via `include_str!` and gated with `#[compiler_builtin]`, replacing the previous scheme of registering every stdlib type and trait programmatically in Rust. Dispatch tables stay programmatic; only the declarative surface (type/trait/enum definitions, method signatures) moves to baked source. Tracked as change-request CR-202.

## Context
The stdlib prelude was hand-registered in Rust (`register_stdlib_traits`, `register_builtin_types`), which duplicated every type's shape in Rust code and drifted from how the same types would be written in Kāra. Baking the declarations as real Kāra source under `runtime/stdlib/*.kara` makes the prelude self-documenting, dogfoods the language, and lets one definition serve as the source of truth.

## Consequences
- `runtime/stdlib/` holds ~60 `.kara` files (option, result, vec, map, set, iterator, ordering, the operator traits, http, json, cli, process, pool, tracing, …).
- `#[compiler_builtin]` is gated to stdlib source only (resolver + typechecker skip body checks for these items).
- `register_stdlib_traits` was retired; source-of-truth for Option/Result/Vec swapped to the baked definitions (CR-202 slices 3–4).
- Migration ran as a long slice sequence (5a–6.5); dispatch/codegen tables were deliberately kept programmatic.

## Options

- Keep programmatic Rust registration — rejected: duplicated shapes, drift, not dogfooded.
- Bake entire stdlib including dispatch — rejected: dispatch is performance-critical and stays in Rust.
- Bake declarations, keep dispatch programmatic — chosen (CR-202 slice 6 reframing).

## Notes


