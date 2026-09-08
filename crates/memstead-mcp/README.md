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

From a repo checkout, `cargo install --path crates/memstead-mcp --locked`
does the same; the GitHub release also ships `memstead-mcp` binaries (see
the [repo README](../../README.md#quickstart)). There is one build: the
multi-mem, git-backed server, which also serves folder-only workspaces.

## Wire it up

The easiest path is `memstead quickstart` from the
[`memstead-cli`](../memstead-cli/) crate — it
bootstraps a workspace and writes the MCP config for the agents you pick.
Run it in a fresh directory, or as `memstead quickstart --repo .` in a
repository you already have (same artifacts, plus a source binding over
that repository).
Manual wiring is one entry in your agent's MCP config pointing at the
`memstead-mcp` binary, run from inside a Memstead workspace.

Restart the agent session afterwards: a session that is already running
does not attach an MCP server added while it runs. A session you cannot
restart (a headless or long-running one) needs the entry in place before
it launches.

Full documentation and the generated MCP tool reference live at the
[Memstead repository](https://github.com/memstead/memstead).

## License

MIT OR Apache-2.0, at your option.
