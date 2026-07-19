---
type: actor
created_date: 2026-07-15T07:25:07Z
last_modified: 2026-07-15T07:34:32Z
kind: team
active: true
handle: kara-maintainers
tags: ownership
---

# Kara Maintainers

## Role
The team that designs and builds the Kāra language and the `karac` compiler — owner of the language design, the compiler pipeline, the runtime, and the standard library.

## Relationships
- **OWNS**: [[kara-compiler]]
- **OWNS**: [[redesign-to-a-rust-inspired-systems-language]]

## Responsibilities

- Language design (design.md, syntax.md) and the phased roadmap.
- The `karac` compiler: lexer, parser, resolver, typechecker, effect checker, ownership checker, interpreter, and LLVM codegen.
- The `karac_runtime` library and the baked standard library.
- Benchmarks and performance investigations (Parallax, hash-quality, HTTP layer).

## Contact

GitHub: github.com/gowthamswe/kara-rust

## Notes

Modeled as a single team actor. Individual commit authorship (e.g. the `gowthamswe` GitHub handle) is provenance, not a separate actor entity.
