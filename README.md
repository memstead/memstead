# Memstead

[![CI](https://github.com/memstead/memstead/actions/workflows/ci.yml/badge.svg)](https://github.com/memstead/memstead/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/memstead/memstead)](https://github.com/memstead/memstead/releases)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSING.md)

**Memstead gives AI agents a typed, validated model of a project — as markdown in git you own.** Your agent's knowledge lives as plain markdown in a git repository — readable by you, diffable in review, with no database and no vendor lock-in. Any agent that speaks [MCP (the Model Context Protocol)](https://modelcontextprotocol.io) — Claude Code, Codex, Gemini, … — or the `memstead` CLI reads and writes it through a schema *you* control, and the engine enforces that schema on every write so the graph never drifts into mush.

Under the hood: each **mem** is a typed graph of interconnected entities. A **schema** you pin defines the entity types, their sections, and the relationships allowed between them — knowledge, plans, specs, inquiry, or any mix. Knowledge graphs are one well-known slice; Memstead generalises across all of them.

Use it for software specs, ADRs, decision logs, ontologies, research notes, or any domain you define. New here? [CONCEPTS.md](CONCEPTS.md) is the three-minute tour of the fourteen terms everything else assumes — each links into the normative [glossary](GLOSSARY.md).

Memstead is part of the 2026 agent-memory wave — alongside mem0, Zep/Graphiti, Letta, and basic-memory — but sits at the authored end of it: agents write schema-validated, typed entities into markdown files in a git repository you own, rather than an extraction pipeline distilling conversations into a retrieval store. Where neighbours share the markdown substrate (basic-memory, Letta's MemFS), Memstead adds the layer they leave to convention: writes validated against a pinned schema, a typed relationship vocabulary, and git provenance on every mutation. The honest tool-by-tool comparison is in [PRIOR_ART.md](PRIOR_ART.md#the-2026-agent-memory-category).

> **Status: pre-1.0.** APIs, schemas, file formats, CLI flags, and the wire shape of MCP tools and HTTP endpoints may change without notice. Not yet stable. Back up your data before exercising mutation operations. See [LICENSING.md](LICENSING.md) for per-folder licenses and [SECURITY.md](SECURITY.md) for vulnerability disclosure.

## Quickstart

Get from nothing to your own graph in a few minutes. (The [getting-started guide](docs-site/src/content/docs/guides/getting-started.md) is the full tutorial version of this section.)

**1. Install the binaries.** The install script fetches the latest [release](https://github.com/memstead/memstead/releases) binaries — `memstead` (the CLI) and `memstead-mcp` (the MCP server agents connect to):

```bash
curl -sSf https://memstead.io/install.sh | sh
```

Or via Homebrew (macOS / Linux):

```bash
brew install memstead/memstead/memstead-cli memstead/memstead/memstead-mcp
```

Or build from source: with the [Rust toolchain](https://rustup.rs) installed, run `./build-engine.sh` from a clone of this repo — it compiles the workspace and installs both binaries to `~/.cargo/bin`. Whichever path you took, `memstead --version` should now work.

**2. Bootstrap a workspace.** In a fresh directory:

```bash
mkdir my-graph && cd my-graph
memstead quickstart
```

One run leaves a working graph: a workspace, a mem pinned to the built-in `default` schema, a seed entity, and the MCP wiring for the agent(s) you pick (Claude Code, Codex, Cursor, Gemini CLI — pass `--agent <target>` to skip the prompt). It prints each artifact it created plus the single next action. Prefer the strict, script-safe variant with no side effects beyond `.memstead/`? That's `memstead init --name my-graph --schema default@1.0.0` — also the path on the v0.1.0 release binaries, which predate `quickstart`.

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

- **Claude Code:** install the [plugin](plugins/claude-code/) and run its `/setup` skill — it resolves the binary path, initialises the workspace, writes `.mcp.json`, and tells you to reconnect. This is the paved path:

  ```bash
  claude plugin marketplace add memstead/memstead
  claude plugin install memstead@memstead
  ```

  (or `/plugin marketplace add memstead/memstead` + `/plugin install memstead@memstead` inside a session), then `/setup`.
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

  `memstead-mcp` walks up from its working directory looking for `.memstead/workspace.toml`, so spawn it from anywhere inside (or under) the workspace — no extra arguments needed. Restart the agent so it picks up the new server.

## Share and reuse mems

Publish a mem to the [memstead.io](https://memstead.io) registry, and install someone else's with one command. Domain roles: **memstead.io** hosts the registry and the install script; **memstead.com** hosts the docs and contact addresses (`hello@` / `security@memstead.com`).

```bash
memstead export --format mem -o my.mem
memstead publish my.mem        # GitHub Device Flow on first use

memstead install scope/name    # pull a published mem into your workspace
```

**Trust posture — a non-first-party mem is untrusted input.** A mem installed from the registry or adopted from a foreign folder/clone is a channel for *someone else's* text to enter your agent's reasoning loop. Memstead treats it as untrusted: the engine serves a non-first-party mem's schema as structure only (its `system_context` / `write_rules` prose is withheld, never served as instructions), and tags non-first-party entity content with a machine-readable `origin` on every read surface (`memstead_schema`, `memstead_entity`, `memstead_search`, `memstead_overview`, the registry manifest, the served read tier's discovery manifest). A consuming agent/host should treat third-party content as quoted data, not instructions. The engine guarantees its half — omit foreign instruction-prose, label foreign data — but cannot force the calling host to gate consequential actions on untrusted input; that residual is the host's. See `SECURITY.md`.

## Reference

Auto-generated API reference for every callable surface — MCP tools, CLI, UniFFI engine surface, WASM (browser) surface, Registry HTTP, plus a cross-surface parity matrix and error-code index:

**[memstead.com/dev](https://memstead.com/dev)**

Generated from source on every push; the [parity matrix](https://memstead.com/dev/reference/parity/) shows at a glance which operations exist on which surface.

## How it works

```
Schema (.memstead/schemas/<name>@<version>/ — types, sections, metadata, relationships, write rules)
  ↓
Engine (parse ↔ in-memory store ↔ generate, write-through to markdown in git)
  ↓
MCP server (memstead_* + workspace_* tools over STDIO)  ─┐
                                                     ├─→  AI agent or shell
CLI (`memstead` mirrors nearly every MCP tool; parity matrix has the map)  ─┘
```

The schema drives all engine behaviour — there are no hardcoded field names. Any schema that conforms to the contract plugs in without code changes, and every mutation is validated against it before it touches disk.

## What's in this repository

| Folder | What it is |
|---|---|
| `crates/` | The Rust engine — schema layer, in-memory store, the two storage backends (folder + git-branch), the `memstead` CLI, the `memstead-mcp` server, plus serve/bridge/wasm crates |
| `plugins/claude-code/` | The Claude Code plugin (skills + guard hooks). Self-contained, no npm dependencies |
| [`docs/`](docs/), [`examples/`](examples/) | [Documentation](docs/) (organized by Diátaxis: tutorial / how-to / reference / explanation) and [example schemas](examples/) (`agent-program`, and the paired `reimpl-source`/`reimpl-target`) |

Memstead also has a native macOS app and a hosted registry; those are separate, closed-source products and are not part of this open repository.

## What Memstead does not do (yet)

Stated here so you don't have to discover it:

- **No semantic / embedding search.** `memstead_search` is exact and structural (content match, type/metadata filters) — there is no vector index. Agents navigate by structure: communities, types, relationships.
- **No bulk import.** No command turns an existing folder of notes into a mem in one shot; content enters through schema-validated writes (the Claude Code plugin's ingest flows are agent-driven and write entity by entity).
- **No built-in visualization.** The graph is queryable (status, overview, relations) but ships no renderer; projections and exports are the extension point.
- **Windows is untested.** Developed and CI-tested on macOS and Linux. Release archives include a Windows build, but no Windows CI gate exists yet — expect rough edges, path handling especially.

## Development

Build everything and install the binaries in one step:

```bash
./build-engine.sh
```

Run the test suite (engine in both build flavours + the plugin):

```bash
./run-tests.sh
```

The engine builds in two flavours from one set of crates: the default build is the full multi-mem, git-backed engine; `--no-default-features` is the lean folder-only build (a CI / dependency-hygiene config). For which crate produces which binary, profiles, feature flags, and troubleshooting, see [docs/build.md](docs/build.md).

```bash
# Force-restart the MCP server (kills all instances; your agent auto-restarts it)
pkill -f memstead-mcp
```

## Built in the open, on itself

Memstead is built by one person — Björn Bösenberg, a full-stack developer of ~25 years, independent since 2026 — on a single thesis: *correctness enforced at boundaries replaces trust in the author.* That is why the engine is Rust — the compiler and borrow-checker stand in for the human code review a solo builder gives up — and why every write to a mem is validated at the boundary rather than trusted after the fact. The same thesis, applied to knowledge instead of code, *is* the product. The platform — roughly 138K lines of Rust, none written by hand, about 3,100 agent commits against 4 human ones — was built across roughly 4.5 **calendar** months of **part-time** work as an AI-orchestration project, and it keeps its own project knowledge as live Memstead mems, in the open, gaps included.

## License

Memstead is dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option. The one folder-level exception is `plugins/claude-code/` (MIT only); see [LICENSING.md](LICENSING.md) for the full per-folder map.
