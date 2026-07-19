---
type: spec
created_date: 2026-07-15T08:41:13Z
last_modified: 2026-07-15T17:32:38Z
level: M1
stability: experimental
tags: ai-first, query, introspection, tooling
---

# Compiler Query API

## Identity
Kāra's programmatic compiler-introspection surface: a family of `karac query`/catalog commands that answer structured questions about a program — its public-API catalog, attribute inventory, call-graph reach, monomorphization set, and codegen hints — as machine-readable output.

## Purpose
To give AI agents and tooling a queryable compiler rather than a grep-the-source workflow — the concrete realization of the AI-first 'compiler query channel'.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **REFERENCES**: [[ai-first-compiler-interface]]

## Realization

- src/queries.rs, src/catalog.rs (public-API surface index, JSONL), src/call_graph.rs (affected-by reach), src/monomorphization.rs (karac query monomorphization), src/codegen_queries.rs (P1.3 inlining + branch-hint analyzer)
- src/query_attributes.rs, src/def_path.rs (DefPath foundation + queries field); tests/codegen_queries.rs, tests/query_attributes.rs, tests/monomorphization.rs


- src/rc_fallback_queries.rs (P1.1), src/specialization_queries.rs (P1.2), src/layout_queries.rs (P1.5), src/fork_threshold_queries.rs (P1.6), src/effect_graph.rs (whole-program effect/concurrency graph)

## Specifies

- `karac catalog`: public-API surface index emitted as JSONL.
- `karac query affected-by`: call-graph reach query (what a change transitively affects).
- `karac query monomorphization`: the monomorphized-instance set for the program.
- `karac query attributes [--tool PREFIX]`: attribute inventory, filterable by tool namespace.
- Codegen-queries analyzer (P1.3): inlining and branch-hint queries over emitted code.
- DefPath foundation with a queries field threaded through the pipeline (Phase 8 P0).


- New query catalogue entries: P1.1 RC-fallback query (where an RC fallback is taken at a use site), P1.2 generic-specialization query (on monomorphization fan-out), P1.5 layout-choice query (struct-of-arrays opportunities), P1.6 auto-concurrency fork-threshold query.
- `karac query effects` / `karac query concurrency` over a whole-program effect/concurrency graph — self-locating spans + a structured exclusion-reason on `query concurrency` (+ reorderable-advisory). Dogfooded by the Cartographer live WASM studio (compiler-in-browser effect graph). A generic-receiver key-join bug that silently dropped `impl[T] Box[T]` methods from the graph was fixed (B-2026-06-14-3).

## Constraints

- Output is stable, machine-parseable (JSONL for the catalog); the query surface is an AI-first contract, not human prose.

## Rationale

Realizes the query-channel clause of [[ai-first-compiler-interface]]. Built on DefPath identity and the call graph.
