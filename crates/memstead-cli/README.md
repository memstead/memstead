# memstead-cli

Command-line interface for
[Memstead](https://github.com/memstead/memstead) — the schema-agnostic
graph engine that gives AI agents a durable, typed memory stored as plain
markdown in git.

The `memstead` binary queries and mutates typed entity graphs from the
shell: bootstrap a workspace (`memstead quickstart`), create and search
entities, manage mems and schemas (`memstead mem <verb>`,
`memstead schema new`), inspect history, and publish/install packaged
mems.

## Install

From the GitHub release (installer script or Homebrew — see the
[repo README](../../README.md#quickstart)), or from this repo:

```bash
cargo install --path crates/memstead-cli
```

Once the crate is published to crates.io, `cargo install memstead-cli`
will work too. Either way this installs the `memstead` binary. The default build is the full
surface (multi-mem, git-backed via the `mem-repo` feature);
`--no-default-features` builds the lean folder-only surface.

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

(On the v0.1.0 release binaries, which predate `quickstart`, use
`memstead init --name my-graph --schema default@1.3.0` instead.)

One run leaves a working graph: a workspace, a mem pinned to the built-in
`default` schema, a seed entity, and MCP wiring for the agent targets you
pick. Full documentation lives at the
[Memstead repository](https://github.com/memstead/memstead).

## License

MIT OR Apache-2.0, at your option.
