---
type: principle
created_date: 2026-07-13T16:43:05Z
last_modified: 2026-07-13T16:44:04Z
authority: established
universality: domain-wide
tags: plugin, hooks, security, trust
---

# Plugin hooks never execute workspace-controlled code

## Statement
A plugin hook MUST NOT load, import, or execute code named by a workspace-controlled path, nor spawn a workspace `.mcp.json` command the user has not approved. Anything a hook resolves to an executable — a module path, a config-named schema string, an engine binary — must come from a channel the user has vetted, never from data a cloned repo controls.

## Scope
Every deterministic `.mjs` hook wired in `hooks.json` — the PreToolUse guards, the PostToolUse realization check, the UserPromptSubmit context injector, and the Stop-time engine-spawning hooks (auto-commit, mem-drift). Hooks fire autonomously on the first prompt, edit, or turn-end in ANY repo the user opens, before they have vetted its contents.

## Relationships
- **GOVERNS**: [[plugin:plugin-hook-system]]
- **GOVERNS**: [[plugin:hook-mcp-client]]
- **GOVERNS**: [[plugin:realization-check-hook]]

## Justification

A hook is code Claude Code runs on the user's behalf on lifecycle events, with no per-event confirmation. If the executable a hook resolves is named by workspace-controlled data (a `.memstead/config.json` `schema` string, a `.mcp.json` `command`), merely opening a hostile cloned repo hands it code execution on the very first prompt or edit — before the user has read a line. The plugin ships to arbitrary machines and arbitrary repos; the threat is the untrusted-clone, not the maintainer's own workspace.

## Exceptions

The engine binary itself, once its `.mcp.json` entry has cleared the MCP-trust gate, is a trusted executable — the plugin is designed to spawn it. The invariant governs UNVETTED workspace data resolving to code, not the approved engine surface.

## Consequences

Two concrete surfaces were closed under this invariant. The realization-check hook stopped `import()`-ing the workspace `.memstead/config.json` `schema` string (an arbitrary-module-load surface). The engine-spawning hooks gate on Claude Code's own MCP-trust signal before launching the `.mcp.json` `memstead` command. The cost is inertness where trust is absent: a drift-pattern channel must be engine-sourced before the realization scan can run, and a workspace the user has not approved for MCP gets silent no-op auto-commit/drift hooks.
