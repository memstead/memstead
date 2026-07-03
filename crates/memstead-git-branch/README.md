# memstead-git-branch

Mem-repo engine backend for
[Memstead](https://github.com/memstead/memstead) — the schema-agnostic
graph engine that gives AI agents a durable, typed memory stored as plain
markdown in git.

This crate implements the git-backed storage backend: each mem lives as
its own root in a multi-root `mem-repo` git repository, mutations are
applied as git tree edits (no working-tree writes for mem content), and
every commit carries provenance. It supplies the full multi-mem surface —
history, diffing, optimistic locking via content hashes, packaging
(`.mem` export/import), and the tantivy-backed search index.

This is the backend the shipped `memstead` and `memstead-mcp` binaries
compile in by default (their `mem-repo` feature).

## Use

Most users want the [`memstead-cli`](../memstead-cli/)
binary or the [`memstead-mcp`](../memstead-mcp/)
server rather than this library. Depend on `memstead-git-branch` to embed
the git-backed engine in your own process.

## License

MIT OR Apache-2.0, at your option.
