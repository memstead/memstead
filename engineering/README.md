# `engineering/` — this project's standing engineering knowledge

A live [memstead](../README.md) mem: the engine's and Claude Code plugin's
**decisions, principles, and memos** — the durable WHY behind the code in
this repository, kept as a typed, queryable graph and dogfooded in the
open. The files here are engine-written markdown entities; the project's
maintainers curate them through manual commits.

- **Schema:** the builtin [`engineering@0.1.0`](crates/memstead-schema/builtins/schemas/engineering/) —
  knowledge-only types; current-state types refuse at write time.
- **Mount it yourself:** from a workspace, add a folder mount pointing at
  this directory (or copy it) — the schema ships with the engine, so a
  bare clone resolves the pin. See the schema package README for the
  one-command mem creation.
- **Read it:** any memstead surface works — `memstead search`,
  `memstead entity <id>`, `memstead overview`, or the MCP server.
- **Edits:** route through the engine (MCP / CLI); the markdown is not
  hand-edited. Cross-mem wiki-links (`[[engine:…]]`, `[[plugin:…]]`)
  anchor into the maintainers' code mems and resolve only in a workspace
  that mounts them — standalone readers can ignore them.
