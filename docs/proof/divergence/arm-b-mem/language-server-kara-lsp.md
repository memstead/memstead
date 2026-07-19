---
type: spec
created_date: 2026-07-15T19:06:03Z
last_modified: 2026-07-15T19:06:03Z
level: M1
stability: experimental
tags: lsp, tooling, ide, roadmap-track-3
---

# Language Server kara-lsp

## Identity
Kāra's Language Server Protocol implementation: a standalone `kara-lsp` stdio server (lsp/ crate) that reuses the compiler front-end to serve live diagnostics, hovers, navigation, references, and formatting to editors.

## Purpose
To give Kāra editor integration — inline errors, type/effect hovers, go-to-definition, find-references, and format-on-save — driving the roadmap's IDE-support track (Track 3) off the same analysis the compiler already runs.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **USES**: [[diagnostics-system]]
- **USES**: [[effect-system]]

## Realization

- lsp/Cargo.toml, lsp/src/{lib,main,analysis}.rs, lsp/tests/server.rs
- Reuses the compiler analysis pipeline (resolver / typechecker / effectchecker)
- Tracker: roadmap.md Track 3 (IDE support)

## Specifies

- Slice 1: an stdio LSP server with live diagnostics (publishDiagnostics from the compiler's own diagnostic stream).
- Slice 2: type-of-expression hover.
- Slice 3: go-to-definition + document symbols.
- Slice 4: whole-document formatting (reuses the compiler formatter).
- Slice 5: find-references.
- Slice 6: effect-signature hover — surfaces a function's inferred effect record on hover, exposing the effect system in-editor.

## Constraints

- Analysis is served from the compiler front-end, so LSP results stay consistent with `karac check`.

## Rationale


