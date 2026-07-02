# memstead-schema

Schema types for [Memstead](https://github.com/memstead/memstead) — the
schema-agnostic graph engine that gives AI agents a durable, typed memory
stored as plain markdown in git.

This crate defines what a schema *is*: entity type definitions (sections,
metadata fields, required/optional shape), the controlled relationship
vocabulary, validation rules, and the loading pipeline that resolves a
schema reference (`default@1.0.0`) from workspace-installed files or the
embedded built-ins. Every write the engine accepts is validated against
these types, so a mem never drifts away from its pinned schema.

It is the leaf crate of the Memstead workspace: everything else
(`memstead-base`, `memstead-engine`, `memstead-git-branch`, the CLI and
MCP server) depends on it.

## Use

Most users never depend on this crate directly — install the
[`memstead-cli`](https://crates.io/crates/memstead-cli) binary or the
[`memstead-mcp`](https://crates.io/crates/memstead-mcp) server instead.
Depend on `memstead-schema` when you are building your own engine
integration and need to parse, validate, or author schema files
programmatically.

## License

MIT OR Apache-2.0, at your option.
