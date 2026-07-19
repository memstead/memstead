---
type: architecture
title: Handling secrets — std.secret
updated_round: 10
---

# Handling secrets — `std.secret`

**New in round 10.** Kāra shipped a security surface for sensitive data: a **`Secret[T]`**
wrapper type that keeps a value from leaking through logs, debug output, or timing side
channels. The module scaffolding lives in **`std.secret`** with `#[compiler_builtin]`
enabled in the gated stdlib modules; the runtime side is `runtime/stdlib/secret.kara`
(+75). See [[stdlib-and-traits]], [[attributes]].

## `Secret[T]`

- **`Secret[T]`** — a wrapper that owns a sensitive value and controls how it can be read.
- **`Secret[T].expose()`** — a **read-borrow** to access the wrapped value, available on
  both backends (interp + codegen).
- Derived **`Debug` / `Display` redact** `Secret[T]` fields — the secret is never printed;
  the formatter emits a redaction placeholder instead of the wrapped value.

## Constant-time equality

- **`Secret[String].ct_eq`** — a **constant-time** equality comparison (the
  **`ConstantTimeEq`** capability). It compares without early-out, so an attacker cannot
  recover the secret from how long a comparison takes (resists timing attacks).

## Blocklisted trait impls

- Trait impls on `Secret[T]` that would leak the wrapped value are **rejected** with
  **`E_SECRET_TRAIT_FORBIDDEN`** — you cannot `impl` a blocklisted trait (e.g. one that
  would expose the secret through a general-purpose surface) on `Secret[T]`. See
  [[attributes]].

## Documentation

- Book chapter **ch19 "Handling Secrets"** (`docs/book/src/ch19-secrets.md`, +129) covers
  `Secret[T]`, `ct_eq`, and the security posture. The chapter's secret-rejection fences are
  annotated so the book-snippet test harness passes on the code that is *meant* to fail.

Related: [[stdlib-and-traits]], [[attributes]], [[design-unsafe-ffi-and-pointers]].
