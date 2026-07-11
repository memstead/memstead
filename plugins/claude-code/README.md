# Memstead — Claude Code plugin

Work with a Memstead knowledge graph directly from Claude Code. The plugin gives
Claude a set of MCP tools (all prefixed `memstead_`) for reading and mutating the
graph, plus a handful of slash commands for the jobs that benefit from a guided
flow.

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
Code (or run `/reload-plugins`) and the commands below are available.

## Start here: `/setup`

Run **`/setup`** once per workspace. It resolves the `memstead` and
`memstead-mcp` binaries — installing them for you if they aren't on `PATH`
(release installer, Homebrew, or source build, in that order) — then runs
`memstead quickstart`, which creates the workspace, registers a mem named
after the folder, pins it to the built-in `default@1.0.0` schema, seeds one
entity, and writes `.mcp.json`. Finally it tells you to restart Claude Code
so the MCP server registers. After the restart, the `memstead_*` tools and
the commands below are available. (Want a different schema? Pins can be
changed after setup — the skill points the way.)

## I want to… → run this

Every command below is one you type. Pick by the job:

| I want to… | Command |
|---|---|
| set Memstead up in this project (once) | **`/setup`** |
| capture what's in an expert's head, one question at a time | **`/interview`** |
| build a mem in bulk from a body of source — code, docs, a git history | **`/ingest`** |
| load what a mem already knows into context before I start working | **`/learn`** |
| tidy a mem's structure — orphans, stubs, thin or missing links | **`/tidy`** |
| bring a mem up to date after its source changed | **`/sync`** |
| measure how faithfully a mem still matches its source | **`/sync --verify`** |

**Everyday graph work has no command — just talk to Claude.** The `memstead_*`
MCP tools are always live, and Claude reaches for them on its own whenever you
describe what you want. Ask in plain language:

- *"Show me every entity that references the auth module"* (a read).
- *"Add a note that the parser now handles UTF-16, and link it to the parser entity"* (a mutation).

Claude picks `memstead_search` / `memstead_entity` for the first and
`memstead_create` / `memstead_relate` for the second — you never name a tool or
a command.

## Keep a mem true

A mem built from a source (a code tree, a docs folder) drifts as the source
changes. Everything in this loop is operable today:

1. **Bind and build — `/ingest`.** Point a mem at a source once, then build it
   one focused batch per run (resumable, and happy on a `/loop`). No binding
   yet? `/ingest` asks three plain questions — what to read, what the mem should
   capture, which mem — and sets one up for you; you never write config.
2. **Check freshness — `memstead status`.** Shows, per mem, what has moved in
   the source since the mem last kept pace — so you know when a mem has fallen
   behind before you rely on it.
3. **Catch up — `/sync`.** The sole maintenance writer: it runs the engine's
   sync brief (what changed since the last sync plus any open findings) and
   updates only the affected entities, conservatively. Reads your source, writes
   your mem — never the reverse, and commits nothing itself.
4. **Measure — `/sync --verify`.** A read-only fidelity report — coverage,
   accuracy, freshness — that leads with a verdict and the top actions, and
   records findings for the next `/sync` run to act on. It changes nothing
   itself.

The engine records every mutation with provenance (and, on the git-backed mem
flavour, commits it to the mem's own history). Your project repo stays yours:
the plugin never commits to it — a mem folder living inside your repo is
versioned like any other files, by you, on your terms.

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
