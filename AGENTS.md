# Memstead — the open engine

- Memstead is a schema-agnostic graph engine — each mem keeps a typed model of a chosen subject
- The schema decides the modal flavour — knowledge, plans, inquiry, specs, or any mix
- Markdown + git as foundation — readable by both humans and LLMs, diffable, no vendor lock-in
- MCP as the AI agent access layer

This repository is the open-source engine: the Rust crates, the `memstead-mcp` server, the `memstead` CLI, the Claude Code plugin, and the docs. Why it exists: [VISION.md](VISION.md). Terms: [GLOSSARY.md](GLOSSARY.md). Build & test: [docs/build.md](docs/build.md) — the engine suite is `./run-tests.sh` (both flavours + plugin legs). **External contributors start at [CONTRIBUTING.md](CONTRIBUTING.md).**

This file is the public mirror of the private workspace constitution: where they overlap it states only what binds every agent working in *this* repo. It intentionally carries no Decision-process or Autonomous-decisions blocks — those live in the workspace and would only drift here.

## The binding rule — the engine owns mem-repo state

*(Mirrors the workspace constitution; this repo ships publicly, so it must state the rule self-contained.)* Nothing outside the engine crates — plugin code, scripts, in-process embedders, you at a shell — may mutate a mem-repo directly: no `git` against the mem-repo, no raw `.md` entity writes, no `mem-repo/.git/` introspection. Direct mutation skips schema validation, write rules, link-graph integrity, search-index updates, optimistic locking, and commit provenance — the graph corrupts silently. All mutations route through one engine surface: **MCP** (`memstead-mcp`, the agent contract), **UniFFI** (in-process embedders), or the **CLI** (`memstead`). Reads may use any surface.

## Conventions

- **Pre-1.0.** Breaking changes are fine — getting the design right beats backwards compatibility.
- **Two flavours:** full (default build — git-branch backend, `mem-repo` feature on) and lean (`--no-default-features`, folder backend, no `gix`). CI runs both; a schema change in `crates/memstead-schema/` requires the full workspace surface before it's done.
- **MCP tool policy:** keep the tool count small — extend an existing tool's parameters before adding one; no action-discriminators, no response-shape polymorphism.
- **Work and commit on `main`;** never create a branch unless explicitly asked. English only — code, commits, issues.
- **One unit noun: mem** — an entity is never called a mem; a mem is not one "memory"/fact.
- **Versioned git hooks:** wire once per clone with `git config core.hooksPath .githooks`. The `pre-push` hook refuses pushes that change an API surface (`crates/`, `xtask/`, the generated reference) without the regenerated docs reference committed — on drift it regenerates into your working tree; commit the result and push again. `docs-drift.yml` remains the CI backstop; it cannot block a direct push.
