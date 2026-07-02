# memstead-engine

Full engine extension for [Memstead](https://github.com/memstead/memstead)
— the schema-agnostic graph engine that gives AI agents a durable, typed
memory stored as plain markdown in git.

This crate layers the multi-mem surface on top of
[`memstead-base`](https://crates.io/crates/memstead-base): mem lifecycle
(create / delete / set-schema / set-version), workspace mutation policy
(create/delete/cross-link grants), and cross-mem link resolution. Backends
plug in underneath — the git-backed mem-repo backend lives in
[`memstead-git-branch`](https://crates.io/crates/memstead-git-branch).

## Use

Most users want the [`memstead-cli`](https://crates.io/crates/memstead-cli)
binary or the [`memstead-mcp`](https://crates.io/crates/memstead-mcp)
server rather than this library. Depend on `memstead-engine` to embed the
full multi-mem engine in your own process.

## License

MIT OR Apache-2.0, at your option.
