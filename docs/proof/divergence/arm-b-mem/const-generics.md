---
type: spec
created_date: 2026-07-15T07:29:50Z
last_modified: 2026-07-15T07:33:53Z
level: M0
stability: evolving
tags: types, const-generics, generics
---

# Const Generics

## Identity
Kāra's const-generics feature: types and functions parameterized by constant values (notably array sizes), with an inference solver, where-clause discharge, and body lowering in codegen.

## Purpose
To let array sizes and other constants be generic parameters, enabling fixed-size array abstractions and `IntSize` variants without runtime cost.

## Relationships
- **REFERENCES**: [[type-checker]]
- **REFERENCES**: [[llvm-codegen-backend]]
- **DEPENDS_ON**: [[type-checker]]

## Realization

- Typechecker: Type::Array.size refactor, const inference solver, check_unsolved_const_param, where-clause discharge (src/typechecker/const_eval.rs)
- Parser/AST + codegen body lowering; IntSize::I128 support
- Shipped across slices 1–4 (1b, 1c, 2, 2b, 3a–3d, 4)

## Specifies

- Const parameters on types/functions; array size as a const generic (Type::Array.size).
- Inference solver with unsolved-const-param checking; where-clause discharge for const params.
- IntSize::I128 alongside the const-generics work; body lowering in codegen (Slice 4).

## Constraints

- Unsolved const parameters are diagnosed rather than silently defaulted.

## Rationale

A committed language feature built incrementally; interacts with [[type-checker]] method resolution and [[llvm-codegen-backend]] monomorphization.
