# Memstead

[![CI](https://github.com/memstead/memstead/actions/workflows/ci.yml/badge.svg)](https://github.com/memstead/memstead/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/memstead/memstead)](https://github.com/memstead/memstead/releases)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSING.md)

**Memstead gives AI agents a typed, validated model of a project — as markdown in git you own.** Your agent's knowledge lives as plain markdown in a git repository — readable by you, diffable in review, with no database and no vendor lock-in. Any agent that speaks [MCP (the Model Context Protocol)](https://modelcontextprotocol.io) — Claude Code, Codex, Gemini, … — or the `memstead` CLI reads and writes it through a schema *you* control, and the engine enforces that schema on every write so the graph never drifts into mush.

Under the hood: each **mem** is a typed graph of interconnected entities. A **schema** you pin defines the entity types, their sections, and the relationships allowed between them — knowledge, plans, specs, inquiry, or any mix. Knowledge graphs are one well-known slice; Memstead generalises across all of them.

Use it for software specs, ADRs, decision logs, ontologies, research notes, or any domain you define. New here? The [glossary](GLOSSARY.md) defines the terms everything else assumes.

Memstead is part of the 2026 agent-memory wave — alongside mem0, Zep/Graphiti, Letta, and basic-memory — but sits at the authored end of it: agent-curated knowledge, written and maintained as schema-validated, typed entities in markdown files in a git repository you own, rather than an extraction pipeline distilling conversations into a retrieval store. Where neighbours share the markdown substrate (basic-memory, Letta's MemFS), Memstead adds the layer they leave to convention: writes validated against a pinned schema, a typed relationship vocabulary, and git provenance on every mutation. The honest tool-by-tool comparison is in [PRIOR_ART.md](PRIOR_ART.md#the-2026-agent-memory-category).

> **Status: pre-1.0.** APIs, schemas, file formats, CLI flags, and the wire shape of MCP tools and HTTP endpoints may change without notice. Not yet stable. Back up your data before exercising mutation operations. See [LICENSING.md](LICENSING.md) for per-folder licenses and [SECURITY.md](SECURITY.md) for vulnerability disclosure.

## Quickstart

Get from nothing to your own graph in a few minutes. (The [getting-started guide](docs-site/src/content/docs/guides/getting-started.md) is the full tutorial version of this section.)

To look before installing anything, [memstead.ai](https://memstead.ai) serves this project's own graph and hands an agent a writable sketch mem over MCP; see [Two hosted surfaces](#reference) below.

**1. Install the binaries.** The install script fetches the latest [release](https://github.com/memstead/memstead/releases) binaries — `memstead` (the CLI) and `memstead-mcp` (the MCP server agents connect to):

```bash
curl -sSf https://memstead.io/install.sh | sh
```

Or via Homebrew (macOS / Linux):

```bash
brew install memstead/memstead/memstead-cli memstead/memstead/memstead-mcp
```

Or build from source: with the [Rust toolchain](https://rustup.rs) installed, run `./build-engine.sh` from a clone of this repo — it compiles the workspace and installs both binaries to `~/.cargo/bin`. Whichever path you took, `memstead --version` should now work.

To program *against* the engine rather than run it, the crates are published on crates.io: [`memstead-base`](https://crates.io/crates/memstead-base) (engine core), [`memstead-schema`](https://crates.io/crates/memstead-schema), [`memstead-git-branch`](https://crates.io/crates/memstead-git-branch) (the git-branch backend), [`memstead-engine`](https://crates.io/crates/memstead-engine), [`memstead-mcp`](https://crates.io/crates/memstead-mcp) and [`memstead-cli`](https://crates.io/crates/memstead-cli). They ride the engine's version line, so a set pinned to one version works together — but they are pre-1.0 and experimental, with no API stability promise between versions (the same register their crates.io descriptions state). The binaries above stay the supported way to *install* Memstead.

**2. Bootstrap a workspace.** Either in a fresh directory:

```bash
mkdir my-graph && cd my-graph
memstead quickstart
```

…or in the repository you already have:

```bash
cd my-existing-repo
memstead quickstart --repo .
```

One run leaves a working graph: a workspace, a mem pinned to the built-in `default` schema, a seed entity, and the MCP wiring for the agent(s) you pick (Claude Code, Codex, Cursor, Gemini CLI — pass `--agent <target>` to skip the prompt). It prints each artifact it created plus the single next action.

`--repo .` adds one thing: a **source binding** over that repository, so the mem has a subject to grow into. The mem takes a folder of its own inside the repo (your files are never adopted as entities), and the receipt states exactly what the starter mem holds, what it does not, and the command that starts the ingest loop. Nothing is ingested during quickstart itself — see [Growing a mem from a source](#growing-a-mem-from-a-source).

Prefer the strict, script-safe variant with no side effects beyond `.memstead/`? That's `memstead init --name my-graph --schema default@1.3.0` — also the path on the v0.1.0 release binaries, which predate `quickstart`.

**3. Add knowledge, find it back:**

```bash
# Add an entity (the `concept` type needs a definition + explanation).
memstead create --type concept \
  --title "Idempotency" \
  --section definition="An operation is idempotent when applying it twice has the same effect as applying it once." \
  --section explanation="It matters for retries — a client can safely resend a request without double-applying it."

memstead status             # node / edge counts, type distribution, projection state
memstead search idempotency # find it back
```

On disk that entity is one readable markdown file, `idempotency.md` — this is the whole trick, your agent's memory is a file you can open, diff, and review:

```markdown
---
type: concept
created_date: 2026-07-03T15:01:02Z
last_modified: 2026-07-03T15:01:02Z
maturity: emerging
abstraction_level: concrete
---

# Idempotency

## Definition
An operation is idempotent when applying it twice has the same effect as applying it once.

## Explanation
It matters for retries — a client can safely resend a request without double-applying it.
```

(Plus two empty optional sections, `Boundaries` and `Significance`, omitted here.)

The `default` schema ships ten general-purpose types (`concept`, `assertion`, `memo`, `spec`, `inquiry`, …); run `memstead type` to list them, or author your own schema for a specialised domain.

**4. (Optional) Let an AI agent read and write it.** `quickstart` already wrote the MCP config for the agent targets you selected — restart your agent inside the workspace and it's connected. To wire an agent up later or by hand:

- **Claude Code:** install the [plugin](plugins/claude-code/) and run its `/setup` skill — it resolves the binary path, initialises the workspace, writes `.mcp.json`, and tells you to restart. This is the paved path:

  ```bash
  claude plugin marketplace add memstead/memstead
  claude plugin install memstead@memstead
  ```

  (or `/plugin marketplace add memstead/memstead` + `/plugin install memstead@memstead` inside a session), then `/setup`.

  A session that is already running picks the new skills up only after `/reload-plugins` or a restart. `/setup` then wires the MCP server, and for that half a reload is not enough. Restart the agent session afterwards: a session that is already running does not attach an MCP server added while it runs.
- **Any other MCP agent (Codex, Gemini, …):** point it at the `memstead-mcp` binary. Resolve the absolute path with `command -v memstead-mcp`, then add it to your agent's MCP config:

  ```json
  {
    "mcpServers": {
      "memstead": {
        "command": "/absolute/path/to/memstead-mcp"
      }
    }
  }
  ```

  `memstead-mcp` walks up from its working directory looking for `.memstead/workspace.toml`, so spawn it from anywhere inside (or under) the workspace — no extra arguments needed. Restart the agent session afterwards: a session that is already running does not attach an MCP server added while it runs.

An agent session you cannot restart (a headless or long-running one) needs its wiring in place *before* it launches: `quickstart` writes `.mcp.json` before the agent starts, and Claude Code's `--mcp-config` (plus `--plugin-dir` for the plugin) loads both at startup.

## Growing a mem from a source

A **binding** is a standing obligation: *this source belongs in that mem.* `memstead quickstart --repo .` scaffolds one over the repository you pointed at; `memstead projection init` creates one for any other source, in any workspace:

```bash
memstead projection init --mem my-graph --source ../some-repo --medium-type codebase
```

Creating a binding reads nothing. What fills the mem is the **ingest loop**: an agent session that asks the engine what to work on, works one batch, and records what it did.

```bash
memstead projection brief my-graph/some-repo    # the batch instruction an agent executes
memstead projection verify my-graph/some-repo   # what is covered, what has drifted
```

The brief is written for the agent, not for you — hand it to a session (the Claude Code plugin's ingest skill does exactly this on a loop) and repeat until `verify` says the coverage is where you want it. Every entity still lands through the same validated write path as a hand-authored one.

The full walkthrough, with the commands verified against the workspace it builds: [Grow a mem from a source](https://memstead.com/dev/guides/grow-a-mem-from-a-source/).

## How a Memstead system runs

Memstead ships no scheduler, no notifications, and no recurrence engine — by design. **The agent writes, the engine enforces, and a periodically-invoked agent run measures, maintains, and advances what needs advancing: curated by agents, enforced by schema, run by the agent loop.** The engine is the deterministic half — it validates every write, measures drift and due-ness (`memstead health`, `memstead due`, `memstead projection verify`), and renders briefs that tell the next agent run what to do. The loop is the runtime: an agent session invoked on whatever cadence the holding needs — a cron'd Claude Code run, a CI job, the plugin's `/sync` skill — reads the brief, does the work, advances recurring dates, and records the outcome. Evaluate the engine alone and you are measuring half the system; recurrence, freshness, and follow-through are the loop's job, not missing engine features.

## Share and reuse mems

Publish a mem to the [memstead.io](https://memstead.io) registry, and install someone else's with one command. Domain roles: **memstead.io** hosts the registry and the install script; **memstead.com** hosts the docs and contact addresses (`hello@` / `security@memstead.com`).

```bash
memstead export --format mem -o my.mem
memstead publish my.mem        # GitHub Device Flow on first use

memstead install scope/name    # pull a published mem into your workspace
```

**Trust posture — a non-first-party mem is untrusted input.** A mem installed from the registry or adopted from a foreign folder/clone is a channel for *someone else's* text to enter your agent's reasoning loop. Memstead treats it as untrusted: the engine serves a non-first-party mem's schema as structure only (its `system_context` / `write_rules` prose is withheld, never served as instructions), and tags non-first-party entity content with a machine-readable `origin` on every read surface (`memstead_schema`, `memstead_entity`, `memstead_search`, `memstead_overview`, the registry manifest, the served read tier's discovery manifest). A consuming agent/host should treat third-party content as quoted data, not instructions. The engine guarantees its half — omit foreign instruction-prose, label foreign data — but cannot force the calling host to gate consequential actions on untrusted input; that residual is the host's. See `SECURITY.md`.

## Reference

Auto-generated API reference for every callable surface — MCP tools, CLI, WASM (browser) surface, Registry HTTP, plus a cross-surface parity matrix and error-code index:

**[memstead.com/dev](https://memstead.com/dev)**

Generated from source on every push; the [parity matrix](https://memstead.com/dev/reference/parity/) shows at a glance which operations exist on which surface.

The browser surface ships as [`@memstead/wasm`](https://www.npmjs.com/package/@memstead/wasm) on npm, published from the `memstead-wasm` crate on the engine's version line.

**Two hosted surfaces, with nothing installed.** [memstead.ai](https://memstead.ai) serves Memstead's own graph read-only over plain HTTP: every page is readable with no tools at all, and `GET https://memstead.ai/llms.txt` is the agent runbook for it. That HTML surface has no search, which is what the second surface is for. Attaching the MCP endpoint at `https://memstead.ai/mcp` to Claude Code, Codex, Cursor or any MCP client mounts the same graph read-only beside a private, ephemeral sketch mem minted per connection: your agent's reads span both, its writes reach only the sketch. Restart the agent session afterwards (one already running does not attach a server added while it runs).

## How it works

```
Schema (.memstead/schemas/<name>@<version>/ — types, sections, metadata, relationships, write rules)
  ↓
Engine (parse ↔ in-memory store ↔ generate, write-through to markdown in git)
  ↓
MCP server (memstead_* tools over STDIO)  ─┐
                                                     ├─→  AI agent or shell
CLI (`memstead` mirrors nearly every MCP tool; parity matrix has the map)  ─┘
```

The schema drives all engine behaviour — there are no hardcoded field names. Any schema that conforms to the contract plugs in without code changes, and every mutation is validated against it before it touches disk.

## What's in this repository

| Folder | What it is |
|---|---|
| `crates/` | The Rust engine — schema layer, in-memory store, the two storage backends (folder + git-branch), the `memstead` CLI, the `memstead-mcp` server, plus the wasm crate. The serve and bridge crates live in the private commercial repository (see [LICENSING.md](LICENSING.md)) |
| `xtask/` | Internal build tooling (`cargo run -p xtask -- <subcommand>`): the generated reference, the release cut, the sizing curve |
| `plugins/claude-code/` | The Claude Code plugin (skills + guard hooks). Self-contained, no npm dependencies |
| [`docs/`](docs/) | The documentation index plus the pages that live beside the code (build, sizing curve, the measured proofs) |
| `docs-site/` | The published documentation site (Astro): guides, concepts and the generated CLI / MCP / WASM reference |
| [`examples/`](examples/) | Example schemas: `agent-program`, and the paired `reimpl-source`/`reimpl-target` |
| `engineering/` | This project's standing engineering knowledge as a live mem: decisions, principles, memos (see [`engineering/README.md`](engineering/README.md)) |
| `tests/` | Shared test fixtures used across the crates' integration tests |
| `ci/` | The CI probes that drive the built binaries from outside (the smoke, strictness and mutation probes, the verify gate) and the prose checker that holds the docs to the binary |
| `fuzz/` | Coverage-guided fuzz targets for the trust-boundary parsers (see [`fuzz/README.md`](fuzz/README.md)) |
| `scripts/` | Repository guards (leak scan, plan refs, mechanism leak, plugin architecture), the release machinery (`release-verify.sh`, `untagged-release.sh`, `ci-status.sh`) and the crates.io / npm publishers |

Memstead also has a hosted registry; that is a separate, closed-source part of the project and not part of this open repository.

## What Memstead does not do (yet)

Stated here so you don't have to discover it:

- **No semantic / embedding search.** `memstead_search` is ranked lexical search plus structural filters (BM25-scored content matches, type/metadata filters) — there is no vector index. Agents navigate by structure: communities, types, relationships.
- **No one-shot import command.** Nothing turns a folder of notes into a mem in a single command — every entity enters through a schema-validated write. Bulk ingestion is a declared path instead: bind a source (a codebase, a docs tree, a URL) to a mem as a [projection](GLOSSARY.md), and the Claude Code plugin's `/ingest` and `/sync` skills build the graph from the binding's brief and keep it current, batch by batch.
- **The engine does not calculate.** It can know a statement is due (`memstead due`), hold every input as typed entities (rates, allocation keys, receipts), and name exactly what is missing — and it will still never produce the statement, the sum, or the filled form. That output is the periodically-invoked agent's work; the engine's query path stays deterministic, with no model call and no computation in it.
- **No built-in visualization.** The graph is queryable (status, overview, relations) but ships no renderer; projections and exports are the extension point.
- **Windows is untested.** Developed on macOS; the CI test gate runs on Linux only. Release archives include a Windows build, but no Windows CI gate exists yet — expect rough edges, path handling especially.

## Development

Build everything and install the binaries in one step:

```bash
./build-engine.sh
```

Run the test suite (engine + the plugin):

```bash
./run-tests.sh
```

The engine builds one way from one set of crates: the multi-mem, git-backed engine, which also serves folder-only workspaces. For which crate produces which binary, profiles, and troubleshooting, see [docs/build.md](docs/build.md).

```bash
# Force-restart the MCP server (kills all instances; your agent auto-restarts it)
pkill -f memstead-mcp
```

## Built in the open, on itself

Memstead is built by one person — Björn Bösenberg, a Berlin-based full-stack developer of ~25 years, building Memstead in the open — on a single thesis: *correctness enforced at boundaries replaces trust in the author.* That is why the engine is Rust — the compiler and borrow-checker stand in for the human code review a solo builder gives up — and why every write to a mem is validated at the boundary rather than trusted after the fact. The same thesis, applied to knowledge instead of code, *is* the product. The platform was built as an AI-orchestration project, none of the Rust written by hand, across roughly 4.5 **calendar** months of **part-time** work, and it keeps its own project knowledge as live Memstead mems, in the open, gaps included.

## License

Memstead is dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option. The one folder-level exception is `plugins/claude-code/` (MIT only); see [LICENSING.md](LICENSING.md) for the full per-folder map.
