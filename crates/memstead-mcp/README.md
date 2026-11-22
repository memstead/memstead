# memstead-mcp

Typed project knowledge for AI agents. `memstead-mcp` is the MCP server of
[Memstead](https://github.com/memstead/memstead), the schema-agnostic
graph engine that keeps typed knowledge graphs as plain markdown in git.

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

## Linking the engine as a library

The crates behind this server are not a general library. The one
exception is the library surface: the module `memstead_mcp::library`
re-exports a closed list of items that a program which links the engine
may build on.

- Booting an engine from a workspace root: `engine_from_workspace_root`.
- Building the server over an engine it owns (`McpServer::new`) and a
  further instance over the same engine for another caller
  (`McpServer::for_caller` with a `CallerKey`, made by `CallerKey::new`).
- Setting a default identity and pinning an identity:
  `McpServer::with_default_identity`, `McpServer::with_pin_identity`.
- Filtering tools (`McpServer::new_with_filter`) and listing them
  (`McpServer::filtered_tool_list`).
- The default token budget: `DEFAULT_TOKEN_BUDGET`.
- Laying a mem-repo: `init_mem_repo`.
- Mounting a mem that exists as a branch (`mount_mem`) and unregistering
  a mem (`delete_mem`).

The engine-crate types those signatures name are re-exported beside
them. Of `McpServer` and `Engine` the promise covers the methods named
here and no others.

The promise: before 1.0 the library surface changes only in a minor
release, with a changelog entry under the heading "Library surface", and
never in a patch release. Everything else in the crates is internal and
may change in any release. An item joins the list only by such a
changelog entry.

The server implements `rmcp`'s `ServerHandler`; a program serves it over
a transport of its own. The engine ships no transport beyond the stdio
binary.

## License

MIT OR Apache-2.0, at your option.
