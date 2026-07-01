# Memstead — Claude Code plugin

Work with a Memstead knowledge graph directly from Claude Code. The plugin gives
Claude a set of MCP tools (all prefixed `memstead_`) for reading and mutating the
graph, plus a handful of slash commands that drive the common workflows.

A Memstead vault is a typed graph of interconnected Markdown entities, stored as
Markdown + git. This plugin is how a Claude Code session reads and grows one.

## Start here: `/setup`

Run **`/setup`** once per workspace. It resolves the `memstead-mcp` binary,
prompts for a vault name and schema, runs `memstead init`, writes `.mcp.json`,
and tells you to restart Claude Code so the MCP server registers. After the
restart, the `memstead_*` tools and the slash commands below are available.

> Prerequisite: the `memstead` / `memstead-mcp` binaries must be built and on
> your `PATH` (`./build-engine.sh` from the repo root). See the repo's
> [docs/build.md](../../docs/build.md).

## The slash commands

These are the front-door commands you'll type. (The plugin also ships several
power-user skills that Claude invokes on its own when relevant — you don't need
to call them directly.)

| Command | Use it when… |
|---|---|
| **`/setup`** | First-time setup of a vault in this workspace (see above). |
| **`/graph`** `<task>` | You want to work with the graph — create, query, update, or connect entities. The general-purpose entry point for graph work. |
| **`/interview`** | You want to capture what a domain expert knows — a guided, one-question-at-a-time conversation that turns answers into structured entities. |
| **`/ingest`** | You want to build the graph in bulk from a body of source material — a knowledge-graph builder that runs one pass at a time. |
| **`/reconcile`** | Your code changed and you want the graph to catch up — syncs the graph to code changes (reads the code, writes the graph, commits nothing itself). |

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
  rationale and precise term definitions (vault, schema, workspace, …).
- [examples/](../../examples/) — worked schemas you can learn from.
