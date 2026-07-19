---
type: decision
created_date: 2026-07-15T07:22:15Z
last_modified: 2026-07-15T07:22:15Z
status: accepted
decided_on: 2026-05-01
deciders: kara-maintainers
scope: system
tags: language-design, redesign, foundational
---

# Redesign to a Rust-Inspired Systems Language

## Decision
We chose to discard Kāra's original `fn`/`flow`/`record`/`->` pipeline-dataflow design entirely and rebuild the language as a Rust-inspired systems language: static types, algebraic data types with exhaustive matching, an effect system, ownership-based memory management, and an LLVM-compiled native backend alongside a tree-walking interpreter.

## Context
The original design expressed programs as `flow` pipelines over `record`s connected by `->`. That model could not carry the systems-programming ambitions the project settled on (predictable memory, native codegen, zero-cost concurrency). An extended brainstorm series (archived v1 through v67) explored alternatives and converged on a Rust-family core with two distinctive additions — an effect system and effect-driven auto-concurrency — rather than incremental patches to the pipeline model.

## Consequences
- Enables native compilation, exhaustive type/effect/ownership checking, and effect-driven auto-parallelism the pipeline model could not express.
- Costs a full rewrite of lexer, parser, AST, and every downstream pass; the `fn`/`flow`/`record`/`->` surface is fully retired.
- Positions Kāra in the crowded systems-language space, forcing differentiation via the effect system, auto-concurrency, and an AI-first compiler interface rather than novelty of the core.
- The design is committed in design.md; syntax.md is the grammar reference; roadmap.md lays out a phased implementation.

## Options

- Keep and extend the `fn`/`flow`/`record`/`->` pipeline design — rejected: cannot express systems-level memory, native codegen, or the intended concurrency model.
- Rebuild as a Rust-inspired systems language with an effect system and auto-concurrency — chosen.
- Fork an existing systems language — rejected: the project wants its own effect-first, AI-first design point.

## Notes

Root decision of the current codebase; the first CHANGELOG entry records it as a 'Complete language redesign.' The retired pipeline surface has no surviving entity — it exists only as history here.
