# memstead-mcp

MCP server for [Memstead](https://github.com/memstead/memstead) — the
schema-agnostic graph engine that gives AI agents a durable, typed memory
stored as plain markdown in git.

`memstead-mcp` exposes the typed entity-graph engine to any MCP-capable
agent (Claude Code, Codex, Gemini CLI, …) over JSON-RPC stdio: schema
discovery, search, entity reads, and schema-validated mutations with
typed refusal envelopes designed for agent self-recovery.

## Install

```bash
cargo install --path crates/memstead-mcp
```

(From a repo checkout; the GitHub release also ships `memstead-mcp`
binaries — see the [repo README](../../README.md#quickstart). Once the
crate is published to crates.io, `cargo install memstead-mcp` will work
too.)

The default build produces the full `memstead-mcp` binary (multi-mem,
git-backed via the `mem-repo` feature). `--no-default-features` builds the
lean folder + archive surface.

## Wire it up

The easiest path is `memstead quickstart` from the
[`memstead-cli`](../memstead-cli/) crate — it
bootstraps a workspace and writes the MCP config for the agents you pick
(on the v0.1.0 release binaries, which predate `quickstart`, bootstrap
with `memstead init --name <name> --schema default@1.0.0`).
Manual wiring is one entry in your agent's MCP config pointing at the
`memstead-mcp` binary, run from inside a Memstead workspace.

Full documentation and the generated MCP tool reference live at the
[Memstead repository](https://github.com/memstead/memstead).

## License

MIT OR Apache-2.0, at your option.
