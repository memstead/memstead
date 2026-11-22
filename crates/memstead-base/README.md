# memstead-base

Typed project knowledge for AI agents. `memstead-base` holds the engine
internals of [Memstead](https://github.com/memstead/memstead): the store,
the parser, the validators, the mem lifecycle and the workspace policy.

`memstead-base` is a dependency of the two Memstead products, the
[`memstead-cli`](../memstead-cli/) binary and the
[`memstead-mcp`](../memstead-mcp/) server, and not a general library.
It is on crates.io because cargo publishes a crate only when every path
dependency is on the registry at the same version; nothing else about it
is a product.

> **Stability:** none promised for this crate's own API. It is pre-1.0
> and changes without deprecation cycles whenever the products need it.
> For a stable contract, consume the binaries or the MCP surface.
>
> **The one exception is the library surface.** A program that links the
> engine as a library builds on the closed list of items the module
> [`memstead_mcp::library`](../memstead-mcp/src/library.rs) re-exports.
> Before 1.0 the library surface changes only in a minor release, with a
> changelog entry under the heading "Library surface", and never in a
> patch release. Everything else in this crate is internal and may change
> in any release.

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
A program that links the engine depends on `memstead-mcp` and builds on
`memstead_mcp::library`.

## License

MIT OR Apache-2.0, at your option.
