# memstead-cli

Install expertise into your agent. `memstead-cli` is the command-line
interface of [Memstead](https://github.com/memstead/memstead), the
schema-agnostic graph engine that keeps typed knowledge graphs as plain
markdown in git.

The `memstead` binary queries and mutates typed entity graphs from the
shell: bootstrap a workspace (`memstead quickstart`), create and search
entities, manage mems and schemas (`memstead mem <verb>`,
`memstead schema new`), inspect history, and publish/install packaged
mems.

## Install

From the GitHub release (installer script or Homebrew, see the
[repo README](../../README.md#quickstart)), or from crates.io:

```bash
cargo install memstead-cli
```

From a repo checkout, `cargo install --path crates/memstead-cli --locked`
does the same. Each installs the `memstead` binary. There is one build:
the multi-mem, git-backed engine, which also serves folder-only
workspaces.

## Start

In a fresh directory:

```bash
mkdir my-graph && cd my-graph
memstead quickstart
```

…or in the repository you already have:

```bash
cd my-existing-repo
memstead quickstart --repo .
```

`--repo` adds a source binding over that repository, with the mem in a
folder of its own so none of your files are adopted as entities. Nothing
is ingested — the receipt names the command that starts the ingest loop.

One run leaves a working graph: a workspace, a mem pinned to the built-in
`default` schema, a seed entity, and MCP wiring for the agent targets you
pick. Restart the agent session afterwards: a session that is already
running does not attach an MCP server added while it runs. Full
documentation lives at the
[Memstead repository](https://github.com/memstead/memstead).

## License

MIT OR Apache-2.0, at your option.
