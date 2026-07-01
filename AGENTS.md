# Memstead

- Memstead is a schema-agnostic graph engine — each mem keeps a typed model of a chosen subject
- The schema decides the modal flavour — knowledge, plans, inquiry, specs, or any mix
- Markdown + git as foundation — readable by both humans and LLMs, diffable, no vendor lock-in
- MCP as the AI agent access layer

For project purpose and design rationale, read [VISION.md](VISION.md). For precise term definitions (mem, schema, workspace, mount, storage backend, …), read [GLOSSARY.md](GLOSSARY.md). To build and test, read [docs/build.md](docs/build.md).

## Structure

The engine workspace is root-hoisted — `Cargo.toml`, `crates/`, and `xtask/` live at the repo root.

| Path | What it is |
|------|------------|
| `crates/` | Rust engine crates — schema, base, engine, git-branch backend, MCP server, CLI, UniFFI + WASM bindings |
| `xtask/` | Build tooling — regenerates the deterministic API reference from live source |
| `plugins/claude-code/` | Claude Code plugin — skills and hooks; self-contained |
| `docs/` | Diátaxis documentation |
| `docs-site/` | Astro/Starlight site that publishes the auto-generated API reference |
| `examples/` | Worked examples |

The engine surfaces the graph to LLM agents through MCP (`memstead-mcp`) and to humans through a CLI (`memstead-cli`); in-process embedders consume it via UniFFI bindings (`memstead-swift`).

## Decision process

1. Read the relevant code or entity before proposing changes
2. If the request is unclear or could mean two things — ask, don't guess
3. If there's a non-obvious trade-off — state it, get confirmation
4. Challenge assumptions even when the user sounds certain. If you think a decision is wrong, say so clearly — don't just state the trade-off and defer
5. If the user insists after hearing your objection — execute what they asked

## Project context

Pre-release. Breaking changes are fine — prioritize getting the design right over backwards compatibility.

When working on a high-priority task, bundle low-effort items into the same session — don't defer cheap wins just because they're lower priority.

## Autonomous decisions

**Fix silently**: broken wiki-links (via MCP), typos in code you're already editing, missing imports in code you just wrote.

**Ask first**: restructuring the graph, changes to workspace config, adding or modifying schemas (built-in or user-defined), adding dependencies.

**Never (explain why first)**: skipping downstream tests after schema changes, weakening a guard hook or validation to make something "work."

## Design principle

The engine, graph, and MCP interface are built for LLM agents as the primary consumer. Data formats, tool interfaces, naming, structure — evaluate them from the agent's perspective first. Human-facing layers (some projections, documentation) exist too — design those for humans separately.

Concretely: commit messages are context for LLMs reconstructing history; MCP tool names, parameter shapes, and error envelopes minimize agent round-trips. Community summaries exist so agents can navigate the graph without reading every entity. Projections that target humans (exports, documentation) optimize for human readability.

## MCP tool policy

`memstead-mcp`'s tool count stays small — Anthropic's published threshold names degradation past 30-50 tools, and agents already stack many built-ins. Before adding a new tool, first try extending an existing tool's parameters.

Two anti-patterns to avoid when consolidating:

- **Action-discriminators** — `foo(action: create|update|delete, ...)` where required parameters vary per action value. Models struggle to pick the right params; each action is its own tool waiting to be extracted.
- **Response-shape polymorphism** — return type depends on which optional params are set. Callers can't decode without branching on request shape. Use additive optional fields on a stable response shape instead.

### `MEM_RELOADED` drift warning

A tool response carrying `MEM_RELOADED` means a sibling engine instance committed to the same mem-repo since this engine last looked. The response already carries fresh content — the engine auto-reloaded — but cached `expected_hash` values are stale (a follow-up `memstead_update` will trip `HASH_MISMATCH`). Re-derive any conclusions that depended on the affected mem before continuing.

## Code

- Solve the problem at hand, nothing more — no speculative features, no premature abstractions (YAGNI)
- Keep it simple — if a solution feels complex, find a simpler one. Three similar lines are better than a clever abstraction
- Don't repeat knowledge — if the same logic lives in two places, it will drift (DRY)
- Design for change — depend on abstractions, keep modules small, avoid tight coupling
- Think ahead — don't build what's not needed yet, but never write code that blocks what's obviously coming
- Leave code better than you found it — but only touch what's related to your task

## Rules

- English only — code, commits, issues
- Schema changes in `crates/memstead-schema/` affect downstream crates — run the full workspace test surface before considering them done
- Workspace tests use `cargo nextest run --workspace --features mem-repo` (and `cargo nextest run --workspace --no-default-features` for basis). `cargo test` works but is ~15× slower on warm runs; the canonical invocations live in `run-tests.sh`, the nextest profile in `.config/nextest.toml`, the macOS first-run gotcha in `docs/macos-dev-setup.md`. Use `cargo nextest run` for verifications and ad-hoc test runs alike
- The engine has two flavours: **basis** (default features, folder backend only, no `gix`) and **pro** (`--features mem-repo`, adds the git-branch backend). Local dev needs pro; helper scripts wire the flag. CI runs both
- Never create a git branch unless the user explicitly asked for one in advance. Work and commit on the current branch
- **The engine owns mem-repo state.** External consumers (plugin code, scripts, in-process embedders — anything outside the engine crates) MUST NOT mutate mem-repo directly: no `git` commands against the mem-repo, no raw `.md` file writes, no `mem-repo/.git/` introspection. Direct mutations skip schema validation, write rules, link-graph integrity, search-index updates, optimistic locking, and commit provenance — the graph corrupts silently. Mutations route through the engine: MCP (`memstead-mcp`) is the documented agent contract; UniFFI is for in-process embedders; CLI is for human and script use. Reads may use any surface — read paths don't violate engine invariants
- Every operation reachable through the engine SHOULD be reachable via both UniFFI and CLI. Asymmetry requires explicit justification — typically a composition-layer-specific operation
- Breaking changes to MCP tool parameter shapes propagate in the same session through `plugins/` and `memstead-cli`
