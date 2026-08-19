---
type: principle
created_date: 2026-08-18T19:34:17Z
last_modified: 2026-08-18T19:34:17Z
authority: established
universality: domain-wide
tags: invariant, surface-parity, cli, consumers, engine
---

# Every engine operation reaches the CLI

## Statement
Every operation reachable through the engine SHOULD be reachable via the CLI (`memstead`) in addition to MCP (`memstead-mcp`). An operation exposed on one surface but not the other requires explicit justification — typically that the operation is composition-layer-specific (e.g. CLI-only workspace bootstrap ergonomics, or MCP abstentions recorded in the tool-surface policy).

## Scope
Governs the engine's programmatic consumer surfaces: the [[engine--cli-command-surface]] (human/script consumers) alongside the MCP agent contract. Applies whenever a new operation lands on the engine (`memstead-base` / `memstead-engine`). MCP exposure itself is governed separately by [[engineering--mcp-tool-surface-stays-small]] — this principle does not force an MCP tool; it forces the asymmetry to be stated. `fetch`/`pull`/`push` stay deliberately CLI-only (remote contact is a human/script operation).

## Relationships
- **SUPERSEDES**: [[every-engine-operation-reaches-uniffi-and-cli]]
- **GOVERNS**: [[engine:cli-command-surface]]
- **REFERENCES**: [[engine:xtask-crate]]
- **REFERENCES**: [[engine:cli-command-surface]]
- **REFERENCES**: [[mcp-tool-surface-stays-small]]

## Justification

The engine serves two consumer classes — agents over MCP, humans and scripts over the CLI. A capability that lands on only one surface silently forks what the product can do per consumer, and the missing wiring is invisible until a consumer needs it. The [[engine--xtask-crate]]'s Surface Parity Matrix is the witness: the hand-maintained `xtask/operations.toml` registry of logical operations is joined against live extractors for MCP tool names, CLI subcommands, and WASM entry points, rendered to `parity.md`; surface names the registry does not pin land in a dedicated "unaligned" sub-table instead of silently dropping.

## Exceptions

Composition-layer-specific operations justify their asymmetry case by case — but the justification must be stated, not implied. Deliberate MCP abstentions (batch family, export/publish/install, `status`) are recorded in the MCP tool-surface policy and `tests/tool_surface.rs`.

## Consequences

A new engine operation ships with CLI wiring in the same change, or with the asymmetry explicitly justified. Because `xtask/operations.toml` is hand-maintained, landing an operation includes pinning its matrix row; the rendered `parity.md` is the audit surface where unjustified asymmetry becomes visible as a `—` cell or an unaligned entry.
