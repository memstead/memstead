---
type: decision
created_date: 2026-11-22T13:15:47Z
last_modified: 2026-11-22T18:00:39Z
status: accepted
decided_on: 2026-11-21
deciders: operator (shared-engine bundle, go of 2026-11-21), implementing agent
scope: component
tags: schema,cli
---

# The schema read verb finds an installed package where the loader resolves pins

## Decision
We chose to have the schema read verb find a workspace-installed schema package at the place the loader resolves pins from, not through the package's README. A package without a README is summarised from what it declares. The JSON form carries the key `summary` on every read, null where a README is shown.

## Context
At 0.26.1 a schema that was scaffolded, installed and pinned could not be read back: `memstead schema <name>@<version>` refused `SCHEMA_NOT_FOUND` although a mem pinned to it was served. The scaffold writes no README, and the read path found an installed package only through that file.

## Consequences
- The command that reads a built-in schema reads an installed one, whoever wrote the package.
- A reader of the JSON form finds `summary` on every read and must take null for a package that shows its README.
- The summary is derived from declarations and says less than a README an author would write.

## Relationships
- **DERIVED_FROM**: [[resolve-an-archive-backed-mounts-schema-from-its-own-archive-the-folder-tier-stays-the-authoring-tier]]

## Options

- Make the scaffold write a README: rejected, packages installed before or written by hand still lack one and the engine serves them.
- Keep the refusal and reword it: rejected, the schema is installed and a mem is pinned to it.
- Read where the loader resolves: chosen.
