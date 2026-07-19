---
type: architecture
title: Kāra Language Server — kara-lsp
updated_round: 10
---

# Kāra Language Server — `kara-lsp`

**New in round 10.** Kāra grew a language server (roadmap **"Track 3"**), delivered as a new
**`lsp/` crate** (`lsp/Cargo.toml`, `lsp/src/{lib,main,analysis}.rs`, `lsp/tests/server.rs`).
It **reuses the compiler front-end** rather than reimplementing analysis — the shared entry
point is `src/analysis.rs` (+496). See [[compiler-pipeline]].

## Capabilities (landed in slices)

- **Slice 1** — stdio server + live **diagnostics**.
- **Slice 2** — **type-of-expression hover**.
- **Slice 3** — **go-to-definition** + **document symbols**.
- **Slice 4** — whole-document **formatting**.
- **Slice 5** — **find-references**.
- **Slice 6** — **effect-signature hover** (surfaces a function's inferred effect row; see
  [[design-effect-system]]).

Because it shares the compiler front-end, hovers and diagnostics reflect the same
type/effect analysis the `karac` toolchain runs. See [[cli]].

Related: [[compiler-pipeline]], [[design-effect-system]], [[cli]].
