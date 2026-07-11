# Memstead — Claude Code plugin

See the root `AGENTS.md` for full project documentation. This plugin provides MCP tools (prefix: `memstead_`) and slash commands for graph interaction.

- Always mutate via MCP tools — never edit entity markdown directly
- There is no command for everyday graph work — just talk to Claude; the `memstead_*` MCP tools are always live and Claude calls them directly
- Plugin code MUST NOT mutate mem-repo state directly — no `git` commands against mem-repo, no raw entity-file writes via `Write`/`Edit`, no `mem-repo/.git/` introspection. All mutations route through Memstead MCP tools, which carry the engine's schema validation, write rules, link-graph integrity, and commit provenance. Reads may use `memstead-cli` (subprocess) or `memstead-mcp` (MCP). Versioning of the user's own project repo is the user's business — the plugin never commits to it (the outer-repo auto-commit concept was retired 2026-07-11).

## Skill invocation-control frontmatter

Two **inverse** frontmatter keys control how a skill is invoked. They are not redundant, and each skill's assignment is deliberate — do not collapse them into one key (they express different axes: `/`-menu visibility vs. model auto-trigger):

- `user-invocable: false` → **model-only**: hidden from the `/` menu, but the model may auto-invoke it. Reserved for internal/power-user skills — none in the current roster.
- `disable-model-invocation: true` → **human-only**: stays visible in the `/` menu, but the model never auto-triggers it. Used for the front-door skills the human drives (setup, interview).
- **neither key** → **both**: visible in `/` and model-invocable (learn, ingest, sync, tidy).

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

## Reacting to concurrent mem changes

The engine self-defends against a sibling instance advancing a mem under your feet (parallel terminal, forked subagent, macOS app + chat subprocess): a `MEM_RELOADED` marker in a tool response means the affected mem was reloaded, and a stale `expected_hash` trips `HASH_MISMATCH`. When either appears, anything you currently have *in your conversation context* about entities of that mem is suspect — re-read via `memstead_entity` before answering questions that depend on prior content, and refetch before a follow-up `memstead_update`. (The former `mem-drift-notify`/`mem-drift-snapshot` hook pair that pre-announced drift per turn was removed in the 2026-07-11 plugin diet — it cost two engine boots per turn to improve only the latency of noticing an event the engine already handles correctly.)
