---
type: principle
created_date: 2026-07-13T16:43:05Z
last_modified: 2026-08-30T00:33:19Z
authority: established
universality: domain-wide
tags: plugin, hooks, security, trust
---

# Plugin hooks never execute workspace-controlled code

## Statement
A plugin hook MUST NOT load, import, or execute code named by a workspace-controlled path, nor spawn a workspace `.mcp.json` command the user has not approved. Anything a hook resolves to an executable — a module path, a config-named schema string, an engine binary — must come from a channel the user has vetted, never from data a cloned repo controls.

## Scope
Every deterministic `.mjs` hook wired in `hooks.json`. Rechecked 2026-08-30, that is four hooks: the two PreToolUse guards (`guard-entity-edit.mjs` on Write|Edit, `guard-entity-bash.mjs` on Bash), the UserPromptSubmit context injector (`inject-context.mjs`), and the PostToolUse realization check (`check-realization.mjs`). The Stop-time engine-spawning hooks named in earlier revisions of this scope, auto-commit and mem-drift, were removed in the 2026-07-11 plugin diet and no longer exist; the plugin wires no Stop hook at all. Hooks fire autonomously on the first prompt or edit in ANY repo the user opens, before they have vetted its contents, so the invariant applies undiminished to the four that remain.

## Relationships
- **GOVERNS**: [[plugin:plugin-hook-system]]
- **GOVERNS**: [[plugin:hook-mcp-client]]
- **GOVERNS**: [[plugin:realization-check-hook]]
- **REFERENCES**: [[engine-spawning-hooks-anchor-to-the-claude-code-mcp-trust-signal]]

## Justification

A hook is code Claude Code runs on the user's behalf on lifecycle events, with no per-event confirmation. If the executable a hook resolves is named by workspace-controlled data (a `.memstead/config.json` `schema` string, a `.mcp.json` `command`), merely opening a hostile cloned repo hands it code execution on the very first prompt or edit — before the user has read a line. The plugin ships to arbitrary machines and arbitrary repos; the threat is the untrusted-clone, not the maintainer's own workspace.

## Exceptions

The `memstead` binary itself is a trusted executable that a hook may shell: it is resolved on PATH by bare name and gated on the binary version `/setup` recorded for the workspace, never on a command string a cloned repo supplies. The invariant governs UNVETTED workspace data resolving to code, not the approved engine surface. (Corrected 2026-08-30: this carve-out was previously stated in terms of a `.mcp.json` entry clearing the MCP-trust gate. That gate belonged to the engine-spawning hooks, which the 2026-07-11 plugin diet removed together with the trust-gate code; the setup-recorded binary version is the mechanism in force today.)

## Consequences

The realization-check hook stopped `import()`-ing the workspace `.memstead/config.json` `schema` string (an arbitrary-module-load surface) under this invariant, and that closure stands: a drift-pattern channel must be engine-sourced before the realization scan can run.

**Corrected 2026-08-30.** The second surface this section named, the engine-spawning hooks gating on Claude Code's own MCP-trust signal before launching the `.mcp.json` `memstead` command, no longer exists on either side: the auto-commit and mem-drift hooks and the trust-gate helpers that served them were removed in the 2026-07-11 plugin diet, so no plugin hook spawns a `.mcp.json` command and no workspace gets "silent no-op auto-commit/drift hooks". The trust-anchoring pattern is preserved as the reference for any engine-spawning hook that returns; the retired decision that recorded it is [[engineering--engine-spawning-hooks-anchor-to-the-claude-code-mcp-trust-signal]]. The rule in ## Statement is unchanged and still binds every hook that ships.
