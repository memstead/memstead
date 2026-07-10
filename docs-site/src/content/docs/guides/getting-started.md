---
title: Getting started
description: "From nothing to a typed, MCP-connected graph in a few minutes: install, memstead quickstart, first entities, agent connect."
sidebar:
  order: 1
---

Memstead gives AI agents a typed, validated model of a project. Knowledge lives as plain markdown in a **mem** — a typed graph of interconnected entities, validated on every write against a **schema** you control. This tutorial takes you from nothing to a working, agent-connected graph.

Terms like *mem*, *schema*, *workspace*, and *entity* have precise meanings — the [Glossary](../../glossary/) is the normative reference; this page uses its vocabulary.

## 1. Install the binaries

The install script fetches the latest [release](https://github.com/memstead/memstead/releases) binaries — `memstead` (the CLI) and `memstead-mcp` (the MCP server agents connect to):

```bash
curl -sSf https://memstead.io/install.sh | sh
```

Or via Homebrew (macOS / Linux):

```bash
brew install memstead/memstead/memstead-cli memstead/memstead/memstead-mcp
```

Or build from source. You need the [Rust toolchain](https://rustup.rs) — `rustc --version` should print a version. Then, from a clone of the repository:

```bash
git clone https://github.com/memstead/memstead
cd memstead
./build-engine.sh
```

The source build installs both binaries to `~/.cargo/bin`. Whichever path you took, check the install:

```bash
memstead --version
```

## 2. Bootstrap a workspace

In a fresh directory, one command does the whole cold start:

```bash
mkdir my-graph && cd my-graph
memstead quickstart
```

`quickstart` creates the workspace, registers a mem named after the directory, pins it to the built-in `default` schema, seeds one entity so the graph isn't empty, and writes the MCP wiring for the agent(s) you pick (Claude Code, Codex, Cursor, Gemini CLI). On a terminal it asks which agents to configure; pass `--agent claude-code` (repeatable) to skip the prompt. The output names every artifact it created:

```text
# Quickstart complete — mem `my-graph`

- Workspace:   `/home/you/my-graph`
- Schema pin:  `default@1.0.0`
- Seed entity: `my-graph--welcome-to-memstead` (remove any time: `memstead delete my-graph--welcome-to-memstead`)
- Claude Code: wrote `.mcp.json` (server `memstead`)

Next: Restart Claude Code so the `memstead` MCP server registers — then try: memstead overview
```

Prefer the strict, script-safe variant with no side effects beyond `.memstead/`? That's `memstead init --name my-graph --schema default@1.0.0` — also the path on the v0.1.0 release binaries, which predate `quickstart`.

## 3. Create your first entities

The `default` schema ships ten general-purpose types (`concept`, `assertion`, `memo`, `spec`, `inquiry`, …) — run `memstead type` to list them. Each type declares which sections an entity must carry; the engine refuses writes that don't conform. Create a `concept` (it requires a definition and an explanation):

```bash
memstead create --type concept \
  --title "Idempotency" \
  --section definition="An operation is idempotent when applying it twice has the same effect as applying it once." \
  --section explanation="It matters for retries — a client can safely resend a request without double-applying it."
```

```text
# Created `my-graph--idempotency`

- Title: Idempotency
- Mem: my-graph
- File: idempotency.md
- Hash: `f668d8042f4499ee`
```

Entities link into a graph: a `[[wiki-link]]` in a section body becomes a typed `REFERENCES` edge automatically.

```bash
memstead create --type concept \
  --title "Retry" \
  --section definition="Re-sending a request after a failure in the hope it succeeds the second time." \
  --section explanation="Safe only when the retried operation is idempotent — see [[my-graph--idempotency]]."
```

Inspect the edge the wiki-link produced:

```bash
memstead relations my-graph--retry
```

```text
# Relations — my-graph--retry

## Outgoing
- **REFERENCES** → [[my-graph--idempotency]]

## Incoming
_none_
```

## 4. Find it back

```bash
memstead status             # node / edge counts, type distribution, projection state
memstead search idempotency # ranked full-text search
memstead entity my-graph--idempotency  # read one entity as markdown
```

`search` returns scored hits with matched-term snippets; `entity` prints the full markdown, including the `_hash` token that mutation commands use for optimistic locking.

Everything you just created is plain markdown on disk — open `idempotency.md` in the workspace and you'll see exactly what the engine sees. Human-readable, diffable, no database.

## 5. Connect your AI agent

`quickstart` already wrote the MCP config for the agent targets you selected — for Claude Code that's a project `.mcp.json` pointing at `memstead-mcp`. Restart the agent inside the workspace and it's connected: the same graph is now readable and writable through the `memstead_*` MCP tools, with the same schema validation on every write.

Ask your agent to call `memstead_overview` — that's the agent's cold-start entry point, returning the schema catalogue, mem inventory, and community clusters. From there, [Agent recipes](../../guides/agent-recipes/) shows the worked tool-call sequences (orientation, search → read, create with recovery) with real request and response payloads.

## Where next

- **Model your own domain** — [Author a schema](../../guides/author-a-schema/) scaffolds a custom schema and pins a mem to it.
- **Share your graph** — [Publish a mem](../../guides/publish-a-mem/) walks the registry flow, dry-run first.
- **Drive it from an agent** — [Agent recipes](../../guides/agent-recipes/), then the full [MCP tools reference](../../reference/mcp/).
- **Look something up** — the [CLI reference](../../reference/cli/cli/) covers every subcommand; the [Glossary](../../glossary/) defines every term.
