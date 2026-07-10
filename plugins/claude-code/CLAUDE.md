# Memstead — Claude Code plugin

See the root `AGENTS.md` for full project documentation. This plugin provides MCP tools (prefix: `memstead_`) and slash commands for graph interaction.

- Always mutate via MCP tools — never edit entity markdown directly
- There is no command for everyday graph work — just talk to Claude; the `memstead_*` MCP tools are always live and Claude calls them directly
- Plugin code MUST NOT mutate mem-repo state directly — no `git` commands against mem-repo, no raw entity-file writes via `Write`/`Edit`, no `mem-repo/.git/` introspection. All mutations route through Memstead MCP tools, which carry the engine's schema validation, write rules, link-graph integrity, and commit provenance. Reads may use `memstead-cli` (subprocess) or `memstead-mcp` (MCP). The single allowed exception: outer-repo operations on the user's project repo (the cwd containing the workspace, not mem-repo), which the auto-commit hook performs and which are not Memstead-managed git.

## Skill invocation-control frontmatter

Two **inverse** frontmatter keys control how a skill is invoked. They are not redundant, and each skill's assignment is deliberate — do not collapse them into one key (they express different axes: `/`-menu visibility vs. model auto-trigger):

- `user-invocable: false` → **model-only**: hidden from the `/` menu, but the model may auto-invoke it. Used for the internal/power-user skills (audit, maintain, refactor, learn, outer-commit).
- `disable-model-invocation: true` → **human-only**: stays visible in the `/` menu, but the model never auto-triggers it. Used for the front-door skills the human drives (setup, interview).
- **neither key** → **both**: visible in `/` and model-invocable (ingest, reconcile).

The front-door / hidden-rest split of the `/` menu is derivable from `user-invocable` alone. Per-skill state files (e.g. `interview`'s mode flag) live at `<mem-dir>/.memstead/<name>` — the same per-mem location the hooks resolve and read; the writer (SKILL) and reader (hook) must name the same path.

## Subagent safety

Plugin hooks (e.g. the guard that blocks direct entity edits) do NOT fire inside subagents spawned via the Agent tool. When spawning subagents:

- **Research/explore agents**: Use `disallowedTools: Write, Edit` — they only need Read, Grep, Glob, WebSearch, WebFetch
- **Agents that need write access**: Add the guard hook in the subagent frontmatter:
  ```yaml
  hooks:
    PreToolUse:
      - matcher: "Write|Edit"
        hooks:
          - type: command
            command: "node \"${CLAUDE_PLUGIN_ROOT}/hooks/guard-entity-edit.mjs\""
  ```
- **Never** instruct a subagent to edit entity `.md` files — always route mutations through Memstead MCP tools in the main session

## Reacting to mem-drift system reminders

The `mem-drift-notify` hook injects a `<system-reminder>` block at the start of a turn when one or more writable mems advanced under your feet between the previous prompt and this one (parallel terminal, forked subagent, macOS app + chat subprocess). The block names each drifted mem, the short old/new HEAD SHAs, and the entity ids that changed.

Treat it the same way you'd treat a `MEM_RELOADED` warning during a tool call: the engine snapshot is honest at the next read, but anything you currently have *in your conversation context* about those entities is stale. Before answering any question whose answer depends on the prior content of a listed entity, re-read it via `memstead_entity`. Cached `_hash` values for those entities are likely invalid — a follow-up `memstead_update` will trip `HASH_MISMATCH` if so; refetch first. Entities outside the listed set are unaffected.
