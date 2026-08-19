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
- Schema pin:  `default@1.3.0`
- Seed entity: `my-graph--welcome-to-memstead` (remove any time: `memstead delete my-graph--welcome-to-memstead`)
- Claude Code: wrote `.mcp.json` (server `memstead`)

Next: Restart Claude Code so the `memstead` MCP server registers — then try: memstead overview
```

Prefer the strict, script-safe variant with no side effects beyond `.memstead/`? That's `memstead init --name my-graph --schema default@1.3.0` — also the path on the v0.1.0 release binaries, which predate `quickstart`.

### …or start from the repository you already have

An empty directory is one starting point; the other is the project you already work in. `--repo` points quickstart at an existing repository:

```bash
cd my-existing-repo
memstead quickstart --repo .
```

The repository becomes the workspace root — `.memstead/` and the agent wiring land where an agent working in the repo will find them — and the mem takes a folder of its own inside it. That folder is the whole graph: your repository's own `.md` files are *not* adopted as entities, and because a mount's storage location is excluded from every binding's input set unconditionally, the mem's own entities never come back round as source artifacts either. (This is why the mem gets a folder of its own whenever the workspace lands inside the repository: the exclusion is skipped for a mem that *is* the workspace root, since excluding it there would empty every denominator.)

The extra artifact is a **source binding**: the standing "this repository belongs in that mem" obligation, scaffolded with the same defaults `memstead projection init` writes. The receipt adds a brief that states what you actually have:

```text
- Binding:     `my-app/my-app` over `.` (record: `.memstead/projections/my-app/my-app.json`)

## What this mem holds

- Now: one seed entity (`my-app--welcome-to-memstead`). Nothing else — scaffolding a
  binding reads no source file and creates no entity from one.
- Not yet: anything from `/home/you/my-existing-repo`. Its code, docs and history are
  the binding's subject, not its content.
- Growth: the ingest loop against binding `my-app/my-app` — one batch at a time, each
  entity written through the same validated path as the seed. Start with:
  `memstead projection brief my-app/my-app`
```

Quickstart itself ingests nothing: it is a scaffold, not a batch job. What fills the mem is the ingest loop — see [Bind a source and grow the mem](#5-bind-a-source-and-grow-the-mem) below.

Pass a target path as well (`memstead quickstart ./graph --repo ./my-existing-repo`) to keep the workspace *outside* the repository instead. That shape is fully supported; the receipt names its one cost, which is the same one the next paragraph describes.

**Choosing where to root the workspace.** If you plan to bind source repositories into the graph later (`memstead projection init`), pick the workspace root with them in mind: a source *inside* the workspace root gets clean relative artifact ids; a source *outside* it is fully supported — enumeration, change detection, and anchor resolution all work — but its artifact ids render as `../…` chains, and the workspace-to-source relative layout must stay fixed. To model several sibling repositories, root the workspace at their **common parent directory** (e.g. `~/projects/graph/` next to `~/projects/app/` and `~/projects/lib/` works, but `~/projects/` containing all three is cleaner). Inside a git repository, `mem-repo init` prints this same hint and adds `mem-repo/` to the repo's `.gitignore` — `.memstead/` itself is intentionally trackable.

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

## 5. Bind a source and grow the mem

Typing entities by hand is one way to fill a mem. The other is a **binding**: a standing "this source belongs in that mem" obligation the engine tracks, measures, and renders work instructions from. `memstead quickstart --repo .` scaffolds one over the repository you started in; `memstead projection init` creates one for any other source:

```bash
memstead projection init --mem my-graph --source ../some-repo --medium-type codebase
```

Creating a binding reads nothing — it records the obligation. What fills the mem is the **ingest loop**: an agent session that asks the engine what to work on, works one batch, and stops.

```bash
memstead projection brief my-graph/some-repo    # the batch instruction an agent executes
memstead projection verify my-graph/some-repo   # coverage, drift, freshness
```

The brief is written for the agent, not for you: it names the source slice, the destination mem, and the anchoring rules that let `verify` measure the result. Hand it to an agent session and repeat until `verify` reports the coverage you want. The Claude Code plugin's ingest skill runs exactly this loop for you.

Entities created this way go through the same validated write path as the ones you typed — a binding changes who does the writing, not what the engine accepts.

## 6. Connect your AI agent

`quickstart` already wrote the MCP config for the agent targets you selected — for Claude Code that's a project `.mcp.json` pointing at `memstead-mcp`. Restart the agent inside the workspace and it's connected: the same graph is now readable and writable through the `memstead_*` MCP tools, with the same schema validation on every write.

Ask your agent to call `memstead_overview` — that's the agent's cold-start entry point, returning the schema catalogue, mem inventory, and community clusters. From there, [Agent recipes](../../guides/agent-recipes/) shows the worked tool-call sequences (orientation, search → read, create with recovery) with real request and response payloads.

## Where next

- **Model your own domain** — [Author a schema](../../guides/author-a-schema/) scaffolds a custom schema and pins a mem to it.
- **Share your graph** — [Publish a mem](../../guides/publish-a-mem/) walks the registry flow, dry-run first.
- **Drive it from an agent** — [Agent recipes](../../guides/agent-recipes/), then the full [MCP tools reference](../../reference/mcp/).
- **Look something up** — the [CLI reference](../../reference/cli/cli/) covers every subcommand; the [Glossary](../../glossary/) defines every term.
