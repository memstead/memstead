---
type: spec
created_date: 2026-07-15T07:29:04Z
last_modified: 2026-07-15T08:43:24Z
level: M1
stability: evolving
tags: diagnostics, lints, ai-first, phase-5
---

# Diagnostics System

## Identity
The Kāra diagnostics subsystem (Phase 5): structured, span-anchored error/warning reporting with machine-readable JSON output, typo suggestions, canonical formatting, a lint family, and an `explain` help channel.

## Purpose
To make compiler output a first-class machine surface for AI agents while staying readable to humans — the concrete realization of the AI-first interface.

## Relationships
- **REFERENCES**: [[ai-first-compiler-interface]]
- **PART_OF**: [[kara-compiler]]
- **REFERENCES**: [[attribute-and-lint-level-system]]

## Realization

- Diagnostics + spans across passes; src/span_visitor.rs; snapshot tests under tests/snapshots/
- Lints: src/must_use_lint.rs, src/missing_must_use_lint.rs, src/unsafe_lint.rs, src/logical_lint.rs, src/ffi_lint.rs
- src/cli/explain.rs (`karac explain --concept=...`); src/doc.rs; src/formatter/ (canonical formatting)
- Error-trace printer: KARAC_ERROR_TRACE_FORMAT=json|jsonl|text (runtime)

## Specifies

- Structured JSON diagnostics alongside human text; multi-error reporting with source spans.
- Focused diagnostics with suggestions: E0236 method typos, E_FN_ANONYMOUS_PARAM, E_EMPTY_PREFIX_LITERAL_NEEDS_ANNOTATION, cast-pair rejection, case-class checks.
- Lints: must_use / missing_must_use (stdlib hygiene), unsafe_op_in_unsafe_fn, unsafe-extern # Safety checks, logical lint.
- Canonical (idempotent) formatter; per-module file context for late-phase diagnostics.
- `karac explain --concept=closures`-style concept pages.

- Typed DiagnosticClass on JSON diagnostics and a StubHint diff envelope for missing test-referenced functions.
- Lint-level control, diagnostic-namespace attributes, and declarative attributes (`#[deprecated]`, `#[non_exhaustive]`, `#[track_caller]`, `#[profile]`) live in [[attribute-and-lint-level-system]].

## Constraints

- Diagnostics must be emittable as structured JSON, not only prose.
- The formatter must be idempotent so agent rewrites are stable.

## Rationale

Phase 5. Directly realizes [[ai-first-compiler-interface]].
