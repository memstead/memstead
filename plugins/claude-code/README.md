# Memstead — Claude Code plugin

Work with a Memstead knowledge graph directly from Claude Code. The plugin gives
Claude a set of MCP tools (all prefixed `memstead_`) for reading and mutating the
graph, plus a handful of slash commands that drive the common workflows.

A Memstead mem is a typed graph of interconnected Markdown entities, stored as
Markdown + git. This plugin is how a Claude Code session reads and grows one.

## Install the plugin

The repo doubles as a Claude Code plugin marketplace (marketplace name:
`memstead`). From a terminal:

```bash
claude plugin marketplace add memstead/memstead
claude plugin install memstead@memstead
```

Or from inside a Claude Code session: `/plugin marketplace add
memstead/memstead`, then `/plugin install memstead@memstead`. Restart Claude
Code (or run `/reload-plugins`) and the skills below are available.

## Start here: `/setup`

Run **`/setup`** once per workspace. It resolves the `memstead` and
`memstead-mcp` binaries — installing them for you if they aren't on `PATH`
(release installer, Homebrew, or source build, in that order) — then runs
`memstead quickstart`, which creates the workspace, registers a mem named
after the folder, pins it to the built-in `default@1.0.0` schema, seeds one
entity, and writes `.mcp.json`. Finally it tells you to restart Claude Code
so the MCP server registers. After the restart, the `memstead_*` tools and
the slash commands below are available. (Want a different schema? Pins can
be changed after setup — the skill points the way.)

## The slash commands

These are the front-door commands you'll type. (The plugin also ships several
power-user skills that Claude invokes on its own when relevant — you don't need
to call them directly.)

| Command | Use it when… |
|---|---|
| **`/setup`** | First-time setup of a mem in this workspace (see above). |
| **`/interview`** | You want to capture what a domain expert knows — a guided, one-question-at-a-time conversation that turns answers into structured entities. |
| **`/ingest`** *(early)* | You want to build the graph in bulk from a body of source material — a knowledge-graph builder that runs one pass at a time. Declaring the source it reads is documented in the [ingest-declaration guide](https://memstead.io/guides/declare-an-ingest/). |
| **`/reconcile`** | Your code changed and you want the graph to catch up — syncs the graph to code changes (reads the code, writes the graph, commits nothing itself). |

**Everyday graph work has no command — just talk to Claude.** The `memstead_*`
MCP tools are always live, and Claude reaches for them on its own whenever you
describe what you want. Ask in plain language:

- *"Show me every entity that references the auth module"* (a read).
- *"Add a note that the parser now handles UTF-16, and link it to the parser entity"* (a mutation).

Claude picks `memstead_search` / `memstead_entity` for the first and
`memstead_create` / `memstead_relate` for the second — you never name a tool or
a command.

> These commands are **early and will consolidate ahead of 1.0** — names and
> shapes can still change.

## How mutations work

All graph changes go **through the MCP tools**, never by editing entity Markdown
by hand. The engine owns the graph: routing writes through MCP is what carries
schema validation, relationship/link integrity, and commit provenance. Editing
the `.md` files directly bypasses all of that. If you ask Claude to "add a note"
or "link these two things," it uses `memstead_create` / `memstead_update` /
`memstead_relate` under the hood.

## Learn more

- The repo [README](../../README.md) — what Memstead is and the quickstart.
- [VISION.md](../../VISION.md) and [GLOSSARY.md](../../GLOSSARY.md) — the design
  rationale and precise term definitions (mem, schema, workspace, …).
- [examples/](../../examples/) — worked schemas you can learn from.
