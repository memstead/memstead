# memstead-schema

Typed project knowledge for AI agents. `memstead-schema` holds the schema
types of [Memstead](https://github.com/memstead/memstead): entity
definitions, the relationship vocabulary and the validation rules.

`memstead-schema` is a dependency of the two Memstead products, the
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

This crate defines what a schema *is*: entity type definitions (sections,
metadata fields, required/optional shape), the controlled relationship
vocabulary, validation rules, and the loading pipeline that resolves a
schema reference (`default@1.3.0`) from workspace-installed files or the
embedded built-ins. Every write the engine accepts is validated against
these types, so a mem never drifts away from its pinned schema.

It is the leaf crate of the Memstead workspace: everything else
(`memstead-base`, `memstead-git-branch`, the CLI and
MCP server) depends on it.

## Use

Install the [`memstead-cli`](../memstead-cli/) binary or the
[`memstead-mcp`](../memstead-mcp/) server. A direct dependency on this
crate is unsupported: it may build today and break at the next release.
A program that links the engine depends on `memstead-mcp` and builds on
`memstead_mcp::library`.

## License

MIT OR Apache-2.0, at your option.
