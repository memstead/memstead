# memstead-base

Install expertise into your agent. `memstead-base` holds the engine
internals of [Memstead](https://github.com/memstead/memstead): the store,
the parser, the validators, the mem lifecycle and the workspace policy.

`memstead-base` is a dependency of the two Memstead products, the
[`memstead-cli`](../memstead-cli/) binary and the
[`memstead-mcp`](../memstead-mcp/) server, and not a supported library.
It is on crates.io because cargo publishes a crate only when every path
dependency is on the registry at the same version; nothing else about it
is a product.

> **Stability:** none promised. The Rust API is pre-1.0 and changes
> without deprecation cycles whenever the products need it. For a stable
> contract, consume the binaries or the MCP surface instead.

This crate is the engine core: the entity store and markdown parser, the
schema validators, wiki-link/relationship graph integrity, full-text search
(tantivy, native targets only), the change-event surface, and the
engine that runs folder-backed mems, plus the multi-mem lifecycle
orchestrators and the workspace-policy writer. One layer builds on it:
`memstead-git-branch` adds the git-backed mem-repo backend.

## Features

- `tokio`: opt-in broadcast adapter (`subscribe_mem_changes_broadcast`)
  for axum-style consumers; the core callback API stays runtime-agnostic.

On `wasm32` targets the tantivy-backed search index is compiled out and
`Engine::search` returns a typed `SearchUnavailable` refusal, keeping the
crate portable for browser builds.

## Use

Install the [`memstead-cli`](../memstead-cli/) binary or the
[`memstead-mcp`](../memstead-mcp/) server. A direct dependency on this
crate is unsupported: it may build today and break at the next release.

## License

MIT OR Apache-2.0, at your option.
