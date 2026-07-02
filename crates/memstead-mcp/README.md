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
cargo install memstead-mcp
```

The default build produces the full `memstead-mcp` binary (multi-mem,
git-backed via the `mem-repo` feature). `--no-default-features` builds the
lean folder + archive surface.

## Wire it up

The easiest path is `memstead quickstart` from the
[`memstead-cli`](https://crates.io/crates/memstead-cli) crate — it
bootstraps a workspace and writes the MCP config for the agents you pick.
Manual wiring is one entry in your agent's MCP config pointing at the
`memstead-mcp` binary, run from inside a Memstead workspace.

Full documentation and the generated MCP tool reference live at the
[Memstead repository](https://github.com/memstead/memstead).

## License

MIT OR Apache-2.0, at your option.
