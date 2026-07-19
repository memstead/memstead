---
type: spec
created_date: 2026-07-15T07:27:03Z
last_modified: 2026-07-15T10:56:25Z
level: M2
stability: evolving
tags: compiler, top-level, pipeline
---

# Kara Compiler

## Identity
`karac`, the Kāra compiler: a Rust-implemented pipeline that lexes, parses, resolves, type-checks, effect-checks, and ownership-checks Kāra source, then either interprets it (tree-walking) or compiles it to a native binary via LLVM.

## Purpose
To turn Kāra source into running programs — both a fast interpreter for development and REPL, and an optimized native backend for shipping — while statically enforcing the language's type, effect, and ownership guarantees.

## Relationships
- **REFERENCES**: [[redesign-to-a-rust-inspired-systems-language]]
- **REFERENCES**: [[backend-first-v1-positioning]]
- **REFERENCES**: [[tree-walking-interpreter]]
- **REFERENCES**: [[llvm-codegen-backend]]
- **MOTIVATED_BY**: [[redesign-to-a-rust-inspired-systems-language]]
- **REFERENCES**: [[self-hosting-the-kara-compiler]]
- **REFERENCES**: [[resequence-self-hosting-as-the-v1-pivot]]

## Realization

- Crate root: src/lib.rs, src/main.rs, src/cli.rs
- Pipeline: src/lexer.rs → src/parser/ → src/resolver/ → src/typechecker/ → src/effectchecker/ → src/ownership/ → {src/interpreter/, src/codegen/}
- Runtime crate: runtime/ (karac_runtime + runtime/stdlib/*.kara)
- Roadmap: docs/roadmap.md; per-phase checklists in docs/implementation_checklist/

## Specifies

- Two execution backends over one front end: the [[tree-walking-interpreter]] and the [[llvm-codegen-backend]].
- A phased build plan: 1 lexer, 2 parser+AST, 3 semantic analysis (resolve/type/effect/ownership), 4 interpreter, 5 diagnostics, 6 runtime, 7 codegen, 8 stdlib floor, 9 verification, 10 targets, 11 stdlib long-tail, 12 self-hosting. The v1 tail was resequenced 8 → 9 → 10 → 12 → 11 to pull [[self-hosting-the-kara-compiler]] ahead of the stdlib long-tail (see [[resequence-self-hosting-as-the-v1-pivot]]).
- Multi-file project mode (concat-super-program codegen) plus single-file compilation and a REPL.
- A CLI (`karac`) exposing build/run/clean/install/vendor/explain and reporting flags.

## Constraints

- Every downstream pass depends on the AST the parser produces; semantic analysis must pass before codegen or interpretation.
- Front-end guarantees (types, effects, ownership) hold identically for both backends.

## Rationale

The whole system exists to realize [[redesign-to-a-rust-inspired-systems-language]]. Backend-quality focus follows [[backend-first-v1-positioning]].
