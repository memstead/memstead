# memstead-git-branch

Typed project knowledge for AI agents. `memstead-git-branch` is the mem-repo
storage backend of [Memstead](https://github.com/memstead/memstead): the
multi-mem, git-backed engine behind the two products.

`memstead-git-branch` is a dependency of the two Memstead products, the
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
A program that links the engine depends on `memstead-mcp` and builds on
`memstead_mcp::library`.

## License

MIT OR Apache-2.0, at your option.
