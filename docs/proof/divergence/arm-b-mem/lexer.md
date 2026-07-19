---
type: spec
created_date: 2026-07-15T07:27:11Z
last_modified: 2026-07-15T07:33:15Z
level: M0
stability: stable
tags: compiler, lexer, phase-1
---

# Lexer

## Identity
The Kāra tokenizer (Phase 1): converts source text into a token stream covering every keyword, symbol, and literal form, with codepoint-aware error recovery.

## Purpose
To turn raw Kāra source into the tokens the parser consumes, recovering gracefully from malformed bytes so downstream diagnostics stay useful.

## Relationships
- **PART_OF**: [[kara-compiler]]

## Realization

- src/lexer.rs, src/token.rs; tests/lexer.rs; fuzz/fuzz_targets/fuzz_lexer.rs

## Specifies

- All keywords, operators, and literals; `c"..."` C-string literals; `r#NAME` raw identifiers.
- A reserved `expr_<NNNN>` fragment-specifier identifier namespace.
- Codepoint-aware recovery for non-ASCII bytes.

## Constraints

- Must recover from invalid input rather than aborting, so the parser can report multiple errors.

## Rationale

Phase 1 of the roadmap; the foundation the entire pipeline builds on.
