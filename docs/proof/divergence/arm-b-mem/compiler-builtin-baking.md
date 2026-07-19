---
type: concept
created_date: 2026-07-15T07:25:00Z
last_modified: 2026-07-15T07:34:35Z
maturity: stable
abstraction_level: concrete
tags: stdlib, prelude, CR-202
---

# Compiler-Builtin Baking

## Definition
Compiler-builtin baking is the technique of defining a standard-library type, trait, or enum as real Kāra source compiled into the compiler binary with `include_str!` and annotated `#[compiler_builtin]`, so the declaration is the source of truth while the compiler still supplies the underlying dispatch/codegen.

## Explanation
A baked item lives in `runtime/stdlib/*.kara` and is parsed like user code, but `#[compiler_builtin]` tells the resolver and typechecker to skip body checking — the body is a signature-bearing shell whose implementation is provided programmatically (intrinsics, dispatch tables, runtime helpers). This lets Option, Result, Vec, Map, the operator traits, and the I/O providers be written once in Kāra rather than mirrored in Rust registration code.

## Relationships
- **REFERENCES**: [[bake-standard-library-in-kara-source]]
- **DEFINED_BY**: [[bake-standard-library-in-kara-source]]

## Boundaries

- Not a fully self-hosted stdlib: dispatch and codegen for baked types remain in Rust.
- `#[compiler_builtin]` is gated to stdlib source — user code cannot claim it.
- Contrast with the retired programmatic-registration approach (`register_stdlib_traits`, `register_builtin_types`) it replaced.

## Significance

This is the mechanism behind [[bake-standard-library-in-kara-source]] (CR-202). It is why the prelude reads as Kāra and why swapping the source-of-truth for Option/Result/Vec was possible.
