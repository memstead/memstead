# Plugin format schemas

JSON Schema (draft 2020-12) documents that pin the on-disk shape of every
file the Claude-Code plugin reads from a workspace.

The plugin format is a **versioned interchange contract**. Producers
(humans editing files today; the macOS app's exports per the App-DB-rework
follow-up tomorrow) emit files matching one of these schemas. Consumers
(the plugin) reject anything malformed with errors that name the offending
field. A `format = "memstead-plugin/<version>"` key in `.memstead.toml`
selects which version's schemas apply.

## Layout

```
schemas/
  README.md                              # this file
  memstead-plugin/
    v0/
      memstead-toml.schema.json          # `.memstead.toml` (parsed-as-JSON)
      medium.schema.json                 # `.memstead/mediums/<vault>/<name>.json`
      facet.schema.json                  # `.memstead/facets/<vault>/<name>.json`
      projection.schema.json             # `.memstead/projections/<vault>/<name>.json`
      ingest.schema.json                 # `.memstead/ingests/<name>.json`
      examples/
        memstead-toml.minimal.json
        memstead-toml.full.json
        facet.minimal.json
        facet.full.json
        medium.minimal.json
        projection.four-primitive.json
        ingest.minimal.json
        ingest.full.json
      validator.mjs                      # hand-rolled validator
      validator.test.mjs                 # `node --test` suite
    v1/                                  # (future) — added when v1 lands
  validate-live-workspace.sh             # validates the live workspace's plugin files
```

The `examples/` directory ships two examples per file type — one minimal
(only the required fields) and one full-featured (every documented field
populated). They serve double duty as documentation and as schema-test
fixtures: every example must validate against its schema.

## Versioning

`memstead-plugin/v0` is the initial pinned version. The plugin loader
gates on `format` in `.memstead.toml`:

| `format` value                | Loader behavior                                       |
|-------------------------------|-------------------------------------------------------|
| absent                        | treat as legacy v0 (warn, validate against v0)        |
| `memstead-plugin/v0`         | validate against v0 schemas; load                     |
| any other string              | reject with error listing supported versions          |

A future v1 ships a parallel `memstead-plugin/v1/` directory with its
own schemas and examples; the loader picks the directory whose name
matches the `format` value. v0 and v1 coexist — producers pin one,
consumers support both during a deprecation window. Breaking changes go
in v1; v0 stays frozen.

## Producers and consumers

**Producer** today: humans editing `.memstead.toml`, medium/facet/projection/ingest
files in their workspace.

**Producer** tomorrow: the macOS app's workspace-export pipeline (per the
App-DB-rework follow-up plan). The app's internal model is App-DB; its
export-to-plugin step writes files matching these schemas so a workspace
authored in the app remains readable by the Claude-Code plugin without
custom adapters.

**Consumer**: the Claude-Code plugin (`plugins/claude-code/skills/ingest/scripts/workspace-loader.mjs`
and dependents). The hand-rolled validator at `validator.mjs` is the
runtime gate.

## Engine vs. plugin ownership

`.memstead.toml` carries both engine-owned and plugin-owned keys. The
`memstead-toml.schema.json` document covers the union — `vaults`,
`mutations`, `vault_management`, and `drift` are engine-owned. `schemas_dir`
is retired and ignored by the engine (workspace schemas load from the
fixed `.memstead/schemas/` path); the namespaced `[clients.*]` and `[plugin.*]`
tables are plugin-owned. Engine accepts plugin-owned keys as typed
pass-throughs (so `#[serde(deny_unknown_fields)]` does not reject them)
but does not consume them. The four-primitive configs (medium, facet,
projection, ingest) live at fixed `.memstead/{mediums,facets,projections,ingests}/`
paths — there are no directory-pointer keys — and their schema files are
entirely plugin-owned.

## Validation against the live workspace

```
plugins/claude-code/schemas/validate-live-workspace.sh
```

walks the four-primitive layout under `.memstead/` (`mediums/`,
`facets/`, `projections/`, `ingests/`) and validates every file against
its corresponding schema using a Node script that loads only built-in
modules and the shared `validator.mjs`.
