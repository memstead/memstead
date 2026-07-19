---
type: spec
created_date: 2026-07-15T07:30:08Z
last_modified: 2026-07-15T07:33:34Z
level: M0
stability: stable
tags: lints, diagnostics, must-use
---

# Must-Use Lints

## Identity
Kāra's `#[must_use]` lint family: honors `#[must_use]` on functions and types and warns when the stdlib's must-use values (e.g. Peekable[T], PooledConnection[T]) are dropped without use.

## Purpose
To catch a common class of bugs — discarding a value whose whole point is to be consumed — as a compiler warning, part of the AI-first diagnostic surface.

## Relationships
- **REFERENCES**: [[diagnostics-system]]
- **PART_OF**: [[diagnostics-system]]

## Realization

- src/must_use_lint.rs (general #[must_use] honoring), src/missing_must_use_lint.rs (stdlib-hygiene lint)
- stdlib annotations: Peekable[T], PooledConnection[T] marked #[must_use]
- tests/must_use_lint.rs, tests/missing_must_use_lint.rs

## Specifies

- General `#[must_use]` honoring on functions and types (must_use slice 4 closed the mandate).
- `missing_must_use` stdlib-hygiene lint (slice 3): flags stdlib values that should be #[must_use].
- Applied annotations on Peekable[T] and PooledConnection[T].

## Constraints

- A must-use value dropped without being used produces a warning.

## Rationale

Part of the [[diagnostics-system]] lint set; realizes the AI-first goal of surfacing latent bugs as structured diagnostics.
