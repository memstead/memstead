---
type: decision
created_date: 2026-07-13T16:43:04Z
last_modified: 2026-07-13T16:44:02Z
status: accepted
decided_on: 2026-07-06
deciders: memstead-core
scope: subsystem
tags: mcp, tool-surface, agent-ergonomics, truncation, primary-client, pre-release, engine
---

# Fit MCP tool descriptions to the primary-client truncation ceiling

## Decision
Every MCP tool description is engineered to fit the primary client's tool-description truncation ceiling — Claude Code's 2,048 bytes — rather than written to a generic, client-agnostic budget. Teaching content that does not fit is relocated, not truncated: the shared cross-tool contract (the mutation `note` / `NOTE_MISSING` nudge, `commit_sha` polling via `memstead_changes_since`, schema-conformance recovery-payload shapes, the post-`memstead_relate` `_hash` advance, the health-warning-code glosses) is stated once in the server `instructions` string; per-mem structural detail lives on the [[engine--schema-describe-projection-surface]] lite/full path. A meta-test guard (`descriptions_fit_primary_client_truncation`, ≤2048 bytes measured against the built router output, with an empty over-limit allowlist) locks the ceiling so a later wording edit cannot silently reintroduce a chopped description.

## Context
Claude Code — the primary MCP consumer through the pre-release window — silently truncates any tool description past 2,048 characters, cutting the agent contract mid-sentence at the exact consumer it is written for. Nine tools were over the limit at the time of the change (`memstead_health` worst at 2,619 bytes). A truncated description costs the agent precisely the teaching it most needs (recovery payloads, the mutation contract) with no error signal that anything was lost. Reshaping consumer-visible contract text is a breaking change: cheap to make now, before an announcement pins external expectations, and expensive afterwards. This choice sharpens the project-wide [[engineering--agent-first-surface-design]] stance into a concrete sizing rule on the [[engine--mcp-tool-surface]].

## Consequences
- Descriptions are budgeted: every wording change must keep each tool inside the meta-suite's word band and under the byte ceiling with its backtick refs lintable — enforced by the full build's `tool_surface.rs`.
- Cross-tool teaching centralises in the byte-pinned server `instructions` string, which the engine surfaces to clients once on connect; an instructions edit is a guarded two-site change (`SERVER_INSTRUCTIONS_COPY` restates the literal and a drift test fails on divergence).
- The ceiling is deliberately client-specific — it tracks Claude Code's 2,048-char limit, not a portable standard. A second primary client with a tighter limit would force another pass; that is accepted as a single-primary-consumer, pre-release trade.
- The guard binds the full flavour only: the lean filesystem surface carries its own shorter, unlinted descriptions and ships no `tool_surface.rs` suite, so an agent on the lean build reads contract text the ceiling never checked.

## Relationships
- **REFERENCES**: [[engine:schema-describe-projection-surface]]
- **REFERENCES**: [[agent-first-surface-design]]
- **REFERENCES**: [[engine:mcp-tool-surface]]
- **REFERENCES**: [[mcp-tool-surface-stays-small]]
- **MOTIVATED_BY**: [[agent-first-surface-design]]

## Options

- Client-agnostic descriptions to a conservative generic budget: rejected — it under-uses the budget the actual primary client allows while still risking some future client's limit; during the pre-release window, optimising for the one known primary consumer wins.
- Keep rich descriptions and accept truncation: rejected — the truncation is silent and lands mid-sentence at the primary consumer, the exact failure the agent-first stance exists to prevent.
- Defer the reshaping past launch: rejected — consumer-visible contract text is a breaking surface, free to reshape before the announcement and expensive after.

## Notes

A specialisation of [[engineering--agent-first-surface-design]] applied to description sizing, and a sibling of the count-focused [[engineering--mcp-tool-surface-stays-small]] — together they hold the [[engine--mcp-tool-surface]] within the primary consumer's ergonomic limits (small count, un-truncated text). Guard realized in `crates/memstead-mcp/tests/tool_surface.rs` (`descriptions_fit_primary_client_truncation`); the relocated teaching lives in the server `instructions` literal in `crates/memstead-mcp/src/server.rs`.
