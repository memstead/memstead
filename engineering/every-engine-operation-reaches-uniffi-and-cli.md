---
type: principle
created_date: 2026-07-13T16:43:03Z
last_modified: 2026-07-13T16:44:02Z
authority: established
universality: domain-wide
tags: invariant, surface-parity, uniffi, cli, consumers, engine
---

# Every engine operation reaches UniFFI and CLI

## Statement
Every operation reachable through the engine SHOULD be reachable via both UniFFI and CLI. An operation exposed on one surface but not the other requires explicit justification — typically that the operation is composition-layer-specific.

## Scope
Governs the engine's non-MCP programmatic consumer surfaces: the [[engine--cli-command-surface]] (human/script consumers) and the [[engine--memstead-swift-uniffi-foreign-function-contract]] (the in-process macOS embedder). Applies whenever a new operation lands on the engine (`memstead-base` / `memstead-engine`). It does not force MCP exposure — the agent surface is governed separately by [[engineering--mcp-tool-surface-stays-small]]. Two families sit outside the both-surfaces default, pinned that way in `xtask/operations.toml`: `fetch`/`pull`/`push` are deliberately CLI-only (no MCP tool, no UniFFI method) because remote contact is a human/script operation; `branch_reset` reaches CLI *and* UniFFI but is deliberately absent from MCP — history rewrite is offered to the in-process app's guarded-undo affordance yet kept off the agent wire.

## Relationships
- **REFERENCES**: [[engine:cli-command-surface]]
- **REFERENCES**: [[engine:memstead-swift-uniffi-foreign-function-contract]]
- **REFERENCES**: [[mcp-tool-surface-stays-small]]
- **REFERENCES**: [[engine:xtask-crate]]
- **GOVERNS**: [[engine:cli-command-surface]]
- **GOVERNS**: [[engine:memstead-swift-uniffi-foreign-function-contract]]

## Justification

The engine serves three consumer classes — agents over MCP, humans and scripts over the CLI, the macOS app in-process over UniFFI. A capability that lands on only one surface silently forks what the product can do per consumer, and the missing wiring is invisible until a consumer needs it. The [[engine--xtask-crate]]'s Surface Parity Matrix is the witness: a hand-maintained `xtask/operations.toml` registry of logical operations is joined against live extractors for MCP tool names, CLI subcommands, UniFFI `Engine` methods, and WASM entry points, rendered to `parity.md`; surface names the registry does not pin land in a dedicated "unaligned" sub-table instead of silently dropping.

## Exceptions

- The entity-content mutation deferral — the standing justified asymmetry: entity-content CRUD (create / update / relate / delete / batch) is deliberately absent from the [[engine--memstead-swift-uniffi-foreign-function-contract]] while that contract is still evolving; the macOS app's everyday entity mutations route through its spawned chat agent over MCP instead. The rest of the earlier UniFFI mutation gap has closed — mem lifecycle, four-primitive pipeline edits, ingest-config edits, `branch_reset`, `apply_parse_recovery`, and `.mem` export all reach UniFFI now.
- Composition-layer-specific operations: operations that only make sense for one consumer's composition layer (e.g. CLI-only workspace bootstrap ergonomics) justify their asymmetry case by case — but the justification must be stated, not implied.

Not every asymmetry here is justified: the pipeline-edit and ingest-config families reach UniFFI but have no CLI subcommand, an unresolved inverse gap the `xtask` surface-parity audit lists as unaligned rather than a stated exception.

## Consequences

A new engine operation ships with CLI and UniFFI wiring in the same change, or with the asymmetry explicitly justified. Because `xtask/operations.toml` is hand-maintained, landing an operation includes pinning its matrix row; the rendered `parity.md` (regenerated via `cargo run -p xtask`) is the audit surface where unjustified asymmetry becomes visible as a `—` cell or an unaligned entry.
