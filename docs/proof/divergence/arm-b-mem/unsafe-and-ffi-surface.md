---
type: spec
created_date: 2026-07-15T07:30:02Z
last_modified: 2026-07-15T08:42:27Z
level: M1
stability: evolving
tags: unsafe, ffi, extern, lints
---

# Unsafe and FFI Surface

## Identity
Kāra's unsafe and foreign-function-interface surface: `unsafe` at module scope, `extern { }` blocks with opaque foreign types and a layout-intrinsic family, FFI unions, raw-pointer construction under strict provenance, c-string literals, plus the lints that enforce safety-documentation discipline.

## Purpose
To let Kāra call C and manipulate raw memory where necessary, while confining and documenting the unsafety so the safe surface stays trustworthy.

## Relationships
- **REFERENCES**: [[bounds-check-elision]]
- **REFERENCES**: [[deferred-work-tracker]]
- **REFERENCES**: [[effect-checker]]
- **DEPENDS_ON**: [[parser-and-ast]]
- **DEPENDS_ON**: [[effect-checker]]

## Realization

- Parser/AST: unsafe surface at module scope; ExternItem::OpaqueType (src/parser/items_extern.rs)
- Lints: src/unsafe_lint.rs (unsafe_op_in_unsafe_fn, block-level # Safety doc checks); effectchecker block-level @noblock propagation
- FFI: src/ffi_lint.rs; opaque foreign type use-site precision

- FFI unions: src/parser/items_extern.rs, codegen union lowering; c-string literals in lexer/parser + typechecker
- Strict-provenance ptr APIs: runtime/stdlib/intrinsics.kara, codegen ptr.* lowering

## Specifies

- `unsafe` at module scope (v2 unsafe-track).
- `extern { }` blocks with per-child doc pages; opaque foreign types (ExternItem::OpaqueType) with use-site precision; layout-intrinsic family.
- Lints: unsafe_op_in_unsafe_fn (operation lint, no-#[allow], diag shape), block-level # Safety doc-comment checks, inline block-level prose per child page.
- Extern-fn effect declarations with @noblock propagation (via [[effect-checker]]).

- FFI unions: parse + resolve + decl-time validation, codegen lowering, and a guard family — E_UNION_READ_REQUIRES_UNSAFE, E_UNION_BORROW_REQUIRES_UNSAFE, E_UNION_LITERAL_REQUIRES_ONE_FIELD, E_UNION_DROP_FORBIDDEN, E_UNION_NON_EXHAUSTIVE_FORBIDDEN.
- Raw pointers: `ptr.const` / `ptr.mut` construction + stdlib; `ptr.container_of` / `ptr.container_of_mut`; `offset_of`.
- Strict provenance: ptr↔int cast rejection, the `ptr.*` stdlib API surface, and codegen lowering for it.
- C-string literals: parser + typechecker `ref CStr`.
- E_OPAQUE_TYPE_NO_METHODS on opaque foreign types.

## Constraints

- Unsafe operations inside an unsafe fn must still be marked; every extern block needs a # Safety doc comment.
- Opaque foreign types are only usable where their use-site is precise.

## Rationale

Prerequisite for [[bounds-check-elision]]'s `get_unchecked` and other raw-memory optimizations tracked in [[deferred-work-tracker]].
