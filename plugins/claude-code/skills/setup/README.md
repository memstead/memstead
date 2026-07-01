# Setup-memstead — Design Intent

What this skill must achieve. Use this as the reference when tuning SKILL.md.

## Core purpose

First-time onboarding for a filesystem-mem Memstead workspace. The user just installed `memstead` and `memstead-mcp` (basis flavour); they need a one-shot path from "empty folder" to "MCP server registered, ready to use".

## Acceptance shape

The setup flow:

- Running it in an empty directory: prompts for mem name and schema, runs `memstead init`, resolves `memstead-mcp` to its absolute path at setup time (e.g. via `command -v memstead-mcp`) and writes that absolute path into `.mcp.json` so the spawned MCP server does not depend on the parent shell's `PATH`, and tells the user to restart Claude Code.
- After restart, the `memstead` MCP server is registered, the user issues `/start` (or asks the agent freely) and the agent successfully calls `memstead_search`.

## Principles

- **Absolute path, not `PATH`.** `.mcp.json` carries the resolved binary path so the agent-spawned MCP server doesn't need `memstead-mcp` on the parent shell's environment.
- **Strict empty.** Don't silently overwrite existing `.memstead/` or `.mcp.json`. Detect re-init scenarios and ask before touching them.
- **Recommend the default schema.** First-time users don't know the schema landscape. `default@1.0.0` is the always-correct starting point; custom schemas come later via `memstead link`.
- **Don't try to verify the MCP server is reachable.** The new server only spawns on the next Claude Code startup. Telling the user to restart is the correct closing move.

## Out of scope

- Resolving custom schemas via `memstead link` (that's its own command — the user runs it after `setup` if they want a non-default schema).
- Bootstrapping mem-repo workspaces (`memstead mem-repo init` is a separate, pro-only path).
- Verifying the MCP server is wired correctly post-setup (only runs at next session start).
