---
type: principle
created_date: 2026-07-13T16:43:04Z
last_modified: 2026-07-13T16:44:03Z
authority: established
universality: domain-wide
tags: invariant, mcp, tools, agent-ergonomics, engine
---

# MCP tool surface stays small

## Statement
`memstead-mcp`'s tool count stays small. Before adding a new MCP tool, first try extending an existing tool's parameters. New capability is additive on a stable tool, not a new tool, whenever the existing tool can carry it.

## Scope
Governs the MCP tool surface of `memstead-mcp` (both build configs). Applies to every change that would add or reshape an agent-facing tool.

## Relationships
- **GOVERNS**: [[engine:memstead-mcp-crate]]
- **GOVERNS**: [[engine:mcp-tool-surface]]

## Justification

Anthropic's published threshold names agent degradation past 30-50 tools, and Claude Code already stacks many built-ins, so the Memstead budget is tight. The engine, graph, and MCP interface are built for LLM agents as the primary consumer — tool names, parameter shapes, and error envelopes are evaluated from the agent's perspective first, to minimize round-trips.

## Exceptions

Two consolidation anti-patterns are themselves forbidden, so 'extend an existing tool' is not unconditional: **action-discriminators** (`foo(action: create|update|delete)` where required params vary per action value — each action is a tool waiting to be extracted) and **response-shape polymorphism** (return type depends on which optional params are set — callers cannot decode without branching on request shape). Use additive optional fields on a stable response shape instead.

## Consequences

Parameter-shape changes to MCP tools propagate in the same session through the plugin, the macOS app, `memstead-cli`, and spec entities — a breaking tool change is a cross-repo change, which is itself a reason to prefer additive extension.
