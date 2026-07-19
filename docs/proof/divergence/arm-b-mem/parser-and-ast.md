---
type: spec
created_date: 2026-07-15T07:27:19Z
last_modified: 2026-07-15T17:33:07Z
level: M1
stability: evolving
tags: compiler, parser, ast, phase-2
---

# Parser and AST

## Identity
The Kāra parser (Phase 2): a recursive-descent parser with Pratt expression parsing that builds a full AST with span tracking on every node, recovering after errors to report many diagnostics per compile.

## Purpose
To produce the typed syntax tree every downstream pass consumes, and to emit precise, multi-error diagnostics with source spans.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[lexer]]

## Realization

- src/parser.rs and src/parser/ (exprs.rs, items.rs, stmts.rs, patterns.rs, types.rs, generics.rs, attributes.rs, items_trait.rs, items_extern.rs, items_effects.rs)
- AST: src/ast.rs and src/ast/ (exprs.rs, items.rs, stmts.rs, patterns.rs, types.rs)
- tests/parser.rs; fuzz/fuzz_targets/fuzz_parser.rs

## Specifies

- All expressions (literals, operators, calls, field/index access, closures, ranges, casts, `?`), statements (`let`/`let mut` with patterns, assignments), and items (functions, structs, enums, traits, impls, effect declarations, layouts, modules, imports, constants, type aliases, extern functions).
- Effects syntax, ownership types (`ref`, `mut ref`, `weak`, pointers), generics with trait bounds, attributes with arguments.
- Grammar extensions: labeled blocks, try blocks (v1 stub), trait aliases (v1 stub), marker-trait syntax, opaque foreign types, unsafe surface at module scope.
- Error recovery: continue after errors, report multiple diagnostics with spans. ~89 parser + ~27 lexer tests at redesign.

- Parallel / destructuring assignment `a, b = b, a`, promoted to a first-class AST node — every RHS is evaluated before any target is written, so it swaps. Also `vec![...]` list-macro literal sugar.

## Constraints

- Every AST node carries a span for diagnostics.
- Parsing continues past errors instead of stopping at the first.

## Rationale

Phase 2 of the roadmap; the grammar of record is docs/syntax.md.
