# memstead-git-branch

`memstead-git-branch` is a dependency of the two Memstead products, the
[`memstead-cli`](../memstead-cli/) binary and the
[`memstead-mcp`](../memstead-mcp/) server, and not a supported library.
It is on crates.io because cargo publishes a crate only when every path
dependency is on the registry at the same version; nothing else about it
is a product.

Mem-repo engine backend for
[Memstead](https://github.com/memstead/memstead) — the schema-agnostic
graph engine that gives AI agents a durable, typed memory stored as plain
markdown in git.

> **Stability:** none promised. The Rust API is pre-1.0 and changes
> without deprecation cycles whenever the products need it. For a stable
> contract, consume the binaries or the MCP surface instead.

This crate implements the git-backed storage backend: each mem lives as
its own root in a multi-root `mem-repo` git repository, mutations are
applied as git tree edits (no working-tree writes for mem content), and
every commit carries provenance. It supplies the full multi-mem surface —
history, diffing, optimistic locking via content hashes, packaging
(`.mem` export/import), and the tantivy-backed search index.

This is the backend the shipped `memstead` and `memstead-mcp` binaries
compile in; there is one build, and it always carries it.

## Use

Install the [`memstead-cli`](../memstead-cli/) binary or the
[`memstead-mcp`](../memstead-mcp/) server. A direct dependency on this
crate is unsupported: it may build today and break at the next release.

## License

MIT OR Apache-2.0, at your option.
