---
type: decision
created_date: 2026-07-13T16:43:03Z
last_modified: 2026-07-13T16:44:01Z
status: retired
decided_on: 2026-07-06
deciders: dasboe
scope: subsystem
tags: plugin, hooks, security, trust, mcp
---

# Engine-spawning hooks anchor to the Claude Code MCP-trust signal

## Decision
The hooks that spawn the engine binary (auto-commit, mem-drift) gate every spawn on the SAME trust signal Claude Code uses to auto-start a project `.mcp.json` server. `isEngineSpawnTrusted` reads four settings layers — `<root>/.claude/settings.local.json`, `<root>/.claude/settings.json`, `~/.claude/settings.json`, and the `~/.claude.json` `projects[<root>]` entry — and grants a spawn only when `enableAllProjectMcpServers === true` or `enabledMcpjsonServers` lists `"memstead"`, with any layer's `disabledMcpjsonServers` deny winning over every allow. `resolveEngineCommand` returns null (silent no-op) when the gate fails. Fail-closed: unreadable or absent config means untrusted.

## Context
`resolveEngineCommand` previously spawned the `.mcp.json` `mcpServers.memstead.command` unconditionally whenever the file existed. The engine-spawning hooks fire autonomously on the first prompt, edit, or Stop in whatever repo is open. A freshly cloned hostile repo carrying a `.mcp.json` whose `command` runs arbitrary code therefore executed that code on the first turn, before the user had approved anything — the untrusted-clone code-execution path the governing principle forbids.

## Consequences
Hooks now only ever launch a command the user has already approved for MCP, inheriting Claude Code's existing per-project approval surface rather than adding a bespoke one. No new prompt, no plugin-owned allowlist to maintain. The accepted cost: a user who runs the engine over MCP but has not persisted the trust signal (e.g. approved only for the session) gets silent no-op auto-commit and drift hooks — acceptable, because MCP itself would likewise not auto-start under those settings, so the hook's reach never exceeds the user's standing MCP trust.

## Relationships
- **REFERENCES**: [[plugin:hook-mcp-client]]
- **MOTIVATED_BY**: [[plugin-hooks-never-execute-workspace-controlled-code]]

## Options

1. Bespoke plugin allowlist file the user maintains separately — rejected: a second approval surface to teach, drift from the MCP one. 2. Prompt the user on first spawn — rejected: hooks run non-interactively at lifecycle events, no clean prompt channel. 3. Anchor to Claude Code's own MCP-trust settings (chosen) — zero new surface, exactly aligned with when the engine would run over MCP anyway.

## Notes

Realized by `isEngineSpawnTrusted` and `resolveLaunchCommand` in `plugins/claude-code/hooks/mcp-client.mjs` (see [[plugin--hook-mcp-client]]).
**Retired 2026-07-11 (plugin diet):** the decision lost its subject — the auto-commit family went with the outer-repo versioning retirement and the mem-drift pair + [[plugin--hook-mcp-client]] were removed in the diet, so no plugin hook spawns the engine binary anymore (check-realization shells the CLI, gated on the setup-recorded binary version — a different, cache-based mechanism). The trust-gate pattern recorded here remains the reference if an engine-spawning hook ever returns.
