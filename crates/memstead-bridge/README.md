# memstead-bridge

Read-only HTTP bridge library for
[Memstead](https://github.com/memstead/memstead) — the schema-agnostic
graph engine that gives AI agents a durable, typed memory stored as plain
markdown in git.

This crate is the server half of the browser thin-client story: wire-format
types, the snapshot envelope builder, a change-event SSE adapter over the
engine's mem-change events, and axum handler helpers. A host embeds it to
serve mem snapshots and live commit streams over plain HTTP; in the
browser, `@memstead/wasm` (built from `crates/memstead-wasm/`) hydrates
the snapshot and `@memstead/client` (`crates/memstead-wasm/client-js/`)
orchestrates the snapshot + SSE + commit-apply lifecycle against this
bridge surface. (Both JS packages are prepared for npm but not yet
published — build them from this repo until the first release lands.)

## Use

Depend on `memstead-bridge` when you run an axum (or tower-compatible)
service and want to expose read-only, live-updating views of your mems to
browser clients.

## License

MIT OR Apache-2.0, at your option.
