# memstead-base

Engine internals for [Memstead](https://github.com/memstead/memstead) — the
schema-agnostic graph engine that gives AI agents a durable, typed memory
stored as plain markdown in git.

> **Stability:** this crate is an internal library of the Memstead engine,
> consumed by the `memstead` / `memstead-mcp` binaries. Its Rust API is
> pre-1.0 and experimental — it changes without deprecation cycles. For a
> stable contract, consume the binaries or the MCP surface instead.

This crate is the engine core: the entity store and markdown parser, the
schema validators, wiki-link/relationship graph integrity, full-text search
(tantivy, native targets only), the change-event surface, and the
filesystem-mem engine that runs a single folder-backed mem. Higher layers
build on it: `memstead-engine` adds multi-mem lifecycle and policy,
`memstead-git-branch` adds the git-backed mem-repo backend.

## Features

- `tokio` — opt-in broadcast adapter (`subscribe_mem_changes_broadcast`)
  for axum-style consumers; the core callback API stays runtime-agnostic.
- `file-watcher` — cross-process `MemChangedEvent`s via `watch_mem_repo`
  for consumers that don't share the writer's `Engine` instance.

On `wasm32` targets the tantivy-backed search index is compiled out and
`Engine::search` returns a typed `SearchUnavailable` refusal, keeping the
crate portable for browser builds.

## Use

Most users want the [`memstead-cli`](../memstead-cli/)
binary or the [`memstead-mcp`](../memstead-mcp/)
server rather than this library. Depend on `memstead-base` to embed the
engine in your own process.

## License

MIT OR Apache-2.0, at your option.
