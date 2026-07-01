# Memstead

> **Status: pre-release.** APIs, schemas, file formats, CLI flags, and the wire shape of MCP tools and HTTP endpoints may change without notice. Not yet stable. Back up your data before exercising mutation operations. See [LICENSING.md](LICENSING.md) for per-folder licenses and [SECURITY.md](SECURITY.md) for vulnerability disclosure.

**Memstead gives AI agents a durable, typed memory you own.** Your agent's knowledge lives as plain markdown in a git repository — readable by you, diffable in review, with no database and no vendor lock-in. Any MCP-capable agent (Claude Code, Codex, Gemini, …) or the `memstead` CLI reads and writes it through a schema *you* control, and the engine enforces that schema on every write so the graph never drifts into mush.

Under the hood: each **vault** is a typed graph of interconnected entities. A **schema** you pin defines the entity types, their sections, and the relationships allowed between them — knowledge, plans, specs, inquiry, or any mix. Knowledge graphs are one well-known slice; Memstead generalises across all of them.

Use it for software specs, ADRs, decision logs, ontologies, research notes, or any domain you define. For precise definitions of *vault*, *schema*, *workspace*, *mount*, and other terms, see [GLOSSARY.md](GLOSSARY.md).

## Quickstart

Get from nothing to your own graph in a few minutes. (Pre-built binaries via `curl | sh` / Homebrew are coming once the first release is cut; until then, build from source — it's two commands.)

**1. Prerequisite: the Rust toolchain.** Install via [rustup](https://rustup.rs) if you don't have it (`rustc --version` should print a version).

**2. Build and install the binaries.** From a clone of this repo:

```bash
./build-engine.sh
```

This compiles the workspace and installs two binaries to `~/.cargo/bin`: `memstead` (the CLI) and `memstead-mcp` (the MCP server agents connect to). Make sure `~/.cargo/bin` is on your `PATH` (`memstead --version` should work).

**3. Build your own graph.** Anywhere you like:

```bash
mkdir my-graph && cd my-graph

# Create a workspace with one vault, pinned to the built-in schema.
memstead init --name my-graph --schema default@1.0.0

# Add your first entity (the `concept` type needs a definition + explanation).
memstead create --type concept \
  --title "Idempotency" \
  --section definition="An operation is idempotent when applying it twice has the same effect as applying it once." \
  --section explanation="It matters for retries — a client can safely resend a request without double-applying it."

memstead stats              # node / edge counts and type distribution
memstead search idempotency # find it back
```

That's a working graph: a workspace, a schema-pinned vault, and a typed entity in git. The `default` schema ships ten general-purpose types (`concept`, `assertion`, `memo`, `spec`, `inquiry`, …); run `memstead type` to list them, or author your own schema for a specialised domain.

**4. (Optional) Let an AI agent read and write it.**

- **Claude Code:** install the plugin in this repo (`plugins/claude-code/`) and run the [`/setup`](plugins/claude-code/skills/setup/SKILL.md) skill — it resolves the binary path, initialises the workspace, writes `.mcp.json`, and tells you to reconnect. This is the paved path.
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

## Share and reuse vaults

Publish a vault to the [memstead.io](https://memstead.io) registry, and install someone else's with one command:

```bash
memstead export --format vault -o my.mem
memstead publish my.mem        # GitHub Device Flow on first use

memstead install scope/name    # pull a published vault into your workspace
```

**Trust posture — a non-first-party vault is untrusted input.** A vault installed from the registry or adopted from a foreign folder/clone is a channel for *someone else's* text to enter your agent's reasoning loop. Memstead treats it as untrusted: the engine serves a non-first-party vault's schema as structure only (its `system_context` / `write_rules` prose is withheld, never served as instructions), and tags non-first-party entity content with a machine-readable `origin` on every read surface (`memstead_schema`, `memstead_entity`, `memstead_search`, `memstead_overview`, the registry manifest, the served read tier's discovery manifest). A consuming agent/host should treat third-party content as quoted data, not instructions. The engine guarantees its half — omit foreign instruction-prose, label foreign data — but cannot force the calling host to gate consequential actions on untrusted input; that residual is the host's. See `SECURITY.md`.

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
CLI (`memstead` subcommands mirror every MCP tool)  ─┘
```

The schema drives all engine behaviour — there are no hardcoded field names. Any schema that conforms to the contract plugs in without code changes, and every mutation is validated against it before it touches disk.

## What's in this repository

| Folder | What it is |
|---|---|
| `engine/` | The Rust engine — schema layer, in-memory store, the two storage backends (folder + git-branch), the `memstead` CLI, the `memstead-mcp` server, plus serve/bridge/wasm crates |
| `plugins/claude-code/` | The Claude Code plugin (skills + guard hooks). Self-contained, no npm dependencies |
| [`docs/`](docs/), [`examples/`](examples/) | [Documentation](docs/) (organized by Diátaxis: tutorial / how-to / reference / explanation) and [example schemas](examples/) (`agent-program`, and the paired `reimpl-source`/`reimpl-target`) |

Memstead also has a native macOS app and a hosted registry; those are separate, closed-source products and are not part of this open repository.

## Development

Build everything and install the binaries in one step:

```bash
./build-engine.sh
```

Run the test suite (engine in both build flavours + the plugin):

```bash
./run-tests.sh
```

The engine builds in two flavours from one set of crates: the default build is the full multi-vault, git-backed engine; `--no-default-features` is the lean folder-only build (a CI / dependency-hygiene config). For which crate produces which binary, profiles, feature flags, and troubleshooting, see [docs/build.md](docs/build.md).

```bash
# Force-restart the MCP server (kills all instances; your agent auto-restarts it)
pkill -f memstead-mcp
```
