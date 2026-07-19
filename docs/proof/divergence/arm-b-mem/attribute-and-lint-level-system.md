---
type: spec
created_date: 2026-07-15T08:41:34Z
last_modified: 2026-07-15T10:23:07Z
level: M1
stability: evolving
tags: attributes, lints, diagnostics, phase-5
---

# Attribute and Lint-Level System

## Identity
Kāra's attribute surface and lint-level control system: the parser/resolver/validator for built-in and tool-namespaced attributes, lint-level attributes (`#[allow]`/`#[warn]`/`#[deny]`/`#[forbid]`/`#[expect]`) with scope cascade, and the diagnostic-namespace attributes that customize error text.

## Purpose
To let source control which diagnostics fire and how, and to carry declarative metadata (deprecation, exhaustiveness, must-use, caller location, profile) that downstream passes enforce — a Rust-style attribute layer for the AI-first diagnostic surface.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **REFERENCES**: [[diagnostics-system]]
- **REFERENCES**: [[must-use-lints]]

## Realization

- src/attribute_validator.rs (E_UNKNOWN_ATTRIBUTE, namespace dispatch, is_bare helper), src/lints.rs (registry + scope cascade)
- src/diagnostic_attrs_lint.rs, src/diagnostic_class.rs; src/missing_track_caller_lint.rs, src/missing_must_use_lint.rs
- Parser/resolver: attributes on ConstDecl, TypeAliasDef, TraitMethod, Variant; src/parser/attributes.rs
- docs/book/src/appendix-d-attributes.md

## Specifies

- Lint-level attributes: `#[allow]`/`#[warn]`/`#[deny]`/`#[forbid]` registry, broad attachment beyond functions, scope cascade + warning-emission integration, `#[expect]` fulfilment tracking + circular guard, `unknown_lint`, forbid-mode rejection, CLI `-A/-W/-D/-F` fall-through; rejected on `unsafe_op_in_unsafe_fn`.
- Diagnostic-namespace attributes: `diagnostic::` path migration, `on_unimplemented` recognition + substitution at the failed-bound emit site, `do_not_recommend`, `malformed_diagnostic_attribute`, 'trait X is implemented by' note.
- Tool-namespaced attributes: `karac query attributes [--tool PREFIX]`, v1-reserved name-claim documentation.
- Declarative attributes with pass enforcement: `#[deprecated]` (payload + placement + resolver symbol-table + use-site warning), `#[non_exhaustive]` (flag + `..` rest-pattern + cross-package struct/enum exhaustiveness + machine-applicable fix-it + `missing_non_exhaustive` lint), `#[track_caller]` (flag + trait-method closure + `missing_track_caller` lint), `#[profile]` (parser/AST + resolver validation + effect-checker integration), `#[must_use]` (see [[must-use-lints]]); `#[unstable]` (attribute machinery plus an `#[unstable]`/`#[deprecated]` lint fired at method / assoc-fn call sites).

## Constraints

- `forbid`-level lints cannot be downgraded by a narrower `#[allow]`.
- Unknown attributes are rejected (E_UNKNOWN_ATTRIBUTE) unless under a reserved tool namespace.

## Rationale

The declarative-attribute and lint-policy layer of the [[diagnostics-system]]; several attributes (`on_unimplemented`, `do_not_recommend`) exist to make failed-bound diagnostics agent-legible.
