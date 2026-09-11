# Memstead — the open engine

- Memstead is a schema-agnostic graph engine — each mem keeps a typed model of a chosen subject
- The schema decides the modal flavour — knowledge, plans, inquiry, specs, or any mix
- Markdown + git as foundation — readable by both humans and LLMs, diffable, no vendor lock-in
- MCP as the AI agent access layer

This repository is the open-source engine: the Rust crates, the `memstead-mcp` server, the `memstead` CLI, the Claude Code plugin, and the docs. Why it exists: [VISION.md](VISION.md). Terms: [GLOSSARY.md](GLOSSARY.md). Build & test: [docs/build.md](docs/build.md) — the engine suite is `./run-tests.sh` (the engine and plugin legs). **External contributors start at [CONTRIBUTING.md](CONTRIBUTING.md).**

**Is this by design?** [`engineering/`](engineering/README.md) is a live Memstead mem holding this project's standing decisions and principles. Consult it before opening an issue or proposing a change: its README says how to mount it as a mem, and the entities are plain Markdown you can read straight from the checkout.

This file states only what binds every agent working in this repo.

## The binding rule — the engine owns mem-repo state

Nothing outside the engine crates — plugin code, scripts, in-process embedders, you at a shell — may mutate a mem-repo directly: no `git` against the mem-repo, no raw `.md` entity writes, no `mem-repo/.git/` introspection. Direct mutation skips schema validation, write rules, link-graph integrity, search-index updates, optimistic locking, and commit provenance — the graph corrupts silently. All mutations route through one engine surface: **MCP** (`memstead-mcp`, the agent contract) or the **CLI** (`memstead`). Reads may use any surface.

## Conventions

- **Pre-1.0.** Breaking changes are fine — getting the design right beats backwards compatibility.
- **One build:** `cargo build` produces the multi-mem, git-backed engine, and the same binaries serve folder-only workspaces; no crate declares a feature that changes what ships. A schema change in `crates/memstead-schema/` requires the full workspace surface before it's done.
- **MCP tool policy:** keep the tool count small — extend an existing tool's parameters before adding one; no action-discriminators, no response-shape polymorphism.
- **Work and commit on `main`;** never create a branch unless explicitly asked. English only — code, commits, issues.
- **One unit noun: mem** — an entity is never called a mem; a mem is not one "memory"/fact.
- **The docs reference is a build output, never committed:** `docs-site/src/content/docs/reference/` is rendered by `cargo run -p xtask -- generate-docs` at every docs-site build (the `prebuild` script) from the engine sources of the checkout, and git ignores it. Change a help string or a tool description and the next build renders it; there is nothing to regenerate and commit, and no drift check to satisfy.
