---
type: spec
created_date: 2026-07-15T19:06:23Z
last_modified: 2026-07-15T19:06:23Z
level: M1
stability: experimental
tags: security, secret, constant-time, stdlib, phase-8
---

# Secret Handling and Constant-Time Comparison

## Identity
Kāra's `Secret[T]` wrapper type and the `std.secret` module: a sensitive-value container whose contents are redacted in derived Debug/Display, compared in constant time via `ct_eq`, read only through an explicit `expose()` borrow, and structurally forbidden from leaking through blocklisted trait impls.

## Purpose
To give sensitive data (tokens, passwords, keys) a type-level discipline that prevents accidental logging, timing-side-channel comparison, and implicit conversions — catching leaks at compile time rather than in production.

## Relationships
- **PART_OF**: [[standard-library]]
- **USES**: [[compiler-builtin-baking]]
- **REFERENCES**: [[standard-library]]

## Realization

- runtime/stdlib/secret.kara (std.secret scaffolding; #[compiler_builtin] enabled in gated stdlib modules)
- Interpreter: src/interpreter/method_call_secret.rs
- Codegen + typecheck: ct_eq, expose, Debug/Display redaction, blocklisted-trait rejection
- Book: docs/book/src/ch19-secrets.md

## Specifies

- `Secret[T]` wraps a value; derived Debug/Display render it redacted (never the underlying content).
- `Secret[String].ct_eq` — a constant-time equality via a `ConstantTimeEq` capability (no early-exit timing leak).
- `expose()` — the only read path, an explicit borrow of the inner value on both backends.
- Blocklisted trait impls on `Secret[T]` are rejected with `E_SECRET_TRAIT_FORBIDDEN` (blocks implicit conversions / leaking traits).

## Constraints

- The inner value is reachable only through `expose()`; there is no implicit deref or Display of the secret.
- Equality on secrets must go through the constant-time `ct_eq`.

## Rationale

The security floor of the [[standard-library]]; built on the `#[compiler_builtin]` baking substrate (enabled for gated stdlib modules). Documented as The Kāra Book chapter 19 (Handling Secrets).
