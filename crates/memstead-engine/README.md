# memstead-engine

Full engine extension for [Memstead](https://github.com/memstead/memstead)
— the schema-agnostic graph engine that gives AI agents a durable, typed
memory stored as plain markdown in git.

> **Stability:** this crate is an internal library of the Memstead engine,
> consumed by the `memstead` / `memstead-mcp` binaries. Its Rust API is
> pre-1.0 and experimental — it changes without deprecation cycles. For a
> stable contract, consume the binaries or the MCP surface instead.

This crate layers the multi-mem surface on top of
[`memstead-base`](../memstead-base/): mem lifecycle
(create / delete / set-schema / set-version), workspace mutation policy
(create/delete/cross-link grants), and cross-mem link resolution. Backends
plug in underneath — the git-backed mem-repo backend lives in
[`memstead-git-branch`](../memstead-git-branch/).

## Use

Most users want the [`memstead-cli`](../memstead-cli/)
binary or the [`memstead-mcp`](../memstead-mcp/)
server rather than this library. Depend on `memstead-engine` to embed the
full multi-mem engine in your own process.

## License

MIT OR Apache-2.0, at your option.
