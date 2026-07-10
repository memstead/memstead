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
    v0/                                  # frozen — do not edit
      memstead-toml.schema.json          # `.memstead.toml` (parsed-as-JSON)
      medium.schema.json                 # `.memstead/mediums/<mem>/<name>.json`
      facet.schema.json                  # `.memstead/facets/<mem>/<name>.json`
      projection.schema.json             # `.memstead/projections/<mem>/<name>.json`
      ingest.schema.json                 # `.memstead/ingests/<name>.json`
      examples/ …                        # minimal + full per file type
      validator.mjs                      # hand-rolled validator
      validator.test.mjs                 # `node --test` suite
    v1/
      memstead-toml.schema.json          # `.memstead.toml` — `format = memstead-plugin/v1`
      medium.schema.json                 # `.memstead/mediums/<mem>/<name>.json` (unchanged from v0)
      facet.schema.json                  # `.memstead/facets/<mem>/<name>.json`  (unchanged from v0)
      binding.schema.json                # `.memstead/projections/<mem>/<name>.json` — replaces projection + ingest
      examples/
        binding.minimal.json
        binding.full.json
        binding.from-init.json           # the `projection init` golden (round-trip pin)
        medium.minimal.json
        facet.minimal.json / facet.full.json
        memstead-toml.minimal.json / memstead-toml.full.json
      validator.mjs                      # hand-rolled validator
      validator.test.mjs                 # `node --test` suite (examples + round-trip pin + refusals)
  validate-live-workspace.mjs            # version-generic walker (--schemas-dir picks the version)
  validate-live-workspace.sh             # runs the walker over the live workspace
```

**v1: the binding replaces the projection + ingest pair.** The retired
four-primitive `projection` + flat-`ingest` split collapses into one versioned
`binding` file at `.memstead/projections/<mem>/<name>.json` — the declaration
(`intent`, `source_facets`, `reference_mems`, `destination_mem`, `deny_paths`,
`coverage_semantics`, `rules`) plus an `operations { build, sync, verify }`
block. There is no `ingests/` directory and no `ingest` schema in v1. `medium`
and `facet` are unchanged. The retired `mode: refinement` value is not accepted.

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
| `memstead-plugin/v1`         | validate against v1 schemas; load                     |
| any other string              | reject with error listing supported versions          |

v1 ships as a parallel `memstead-plugin/v1/` directory with its own schemas
and examples; the loader picks the directory whose name matches the `format`
value. v0 and v1 coexist — producers pin one, consumers support both during a
deprecation window. Breaking changes go in v1 (the projection + ingest pair
became the single `binding`); **v0 stays frozen byte-for-byte** — never edit a
file under `v0/`.

## Producers and consumers

**Producer** today: humans editing `.memstead.toml` and medium/facet/binding
files in their workspace, and `memstead projection init` (which scaffolds a v1
binding triple non-interactively — its output is pinned against
`v1/binding.schema.json` by the round-trip test).

**Producer** tomorrow: the macOS app's workspace-export pipeline (per the
App-DB-rework follow-up plan). The app's internal model is App-DB; its
export-to-plugin step writes files matching these schemas so a workspace
authored in the app remains readable by the Claude-Code plugin without
custom adapters.

**Consumer**: the engine's binding loader (`memstead-base`, read version-gated
on `format`), reached from the plugin via the `memstead` CLI / MCP; the
hand-rolled `validator.mjs` + the `validate-live-workspace.mjs` walker are the
authoring-time gate. Both consumers run in the canonical test suite
(`run-tests.sh`, the "plugin format schemas" leg).

## Engine vs. plugin ownership

`.memstead.toml` carries both engine-owned and plugin-owned keys. The
`memstead-toml.schema.json` document covers the union — `mems`,
`mutations`, `mem_management`, and `drift` are engine-owned. `schemas_dir`
is retired and ignored by the engine (workspace schemas load from the
fixed `.memstead/schemas/` path); the namespaced `[clients.*]` and `[plugin.*]`
tables are plugin-owned. Engine accepts plugin-owned keys as typed
pass-throughs (so `#[serde(deny_unknown_fields)]` does not reject them)
but does not consume them. The config files live at fixed
`.memstead/{mediums,facets,projections}/` paths — there are no
directory-pointer keys — and their schema files are entirely plugin-owned. (In
v0 there was also a flat `.memstead/ingests/` directory; v1 folds it into the
binding, so the `ingests/` dir is gone.)

### `drift.realizationPatterns` — deprecated (E3a)

The `drift.realizationPatterns` sub-key (`fileHeader` / `backtickPath`
regexes) is **deprecated and no longer read by any plugin surface.** The
`check-realization` hook used to load these patterns and regex-scan entity
markdown for file references; that scan was both dead (the loader was
hard-nulled after a workspace-controlled-module-load security fix) and the
wrong design. The hook is now anchor-based: it asks the engine which entities
anchored the edited file via `memstead anchors --artifact` and never loads any
schema-derived scan patterns. Provenance from source artifact → entity lives in
engine-owned anchors, not in schema regexes.

The frozen **v0** `memstead-toml.schema.json` keeps its generic
`drift` description ("Engine-owned disk-drift policy") **byte-identical** — v0
stays frozen (see Versioning above), so no realization-patterns removal happens
in v0; the key is simply inert. **v1** never carries a `realizationPatterns`
key.

## Validation against the live workspace

```
plugins/claude-code/schemas/validate-live-workspace.mjs --schemas-dir <version-dir> [--workspace-dir <dir>]
```

is a version-generic walker: it metaschema-shape-checks every `*.schema.json`
in `--schemas-dir`, validates every fixture under that version's `examples/`
against the schema its filename prefix names, and — when `--workspace-dir` is
given — walks a live `.memstead/` layout (`mediums/`, `facets/`, `projections/`,
plus `ingests/` for a v0 workspace) and validates each file. It adapts to the
version by which schemas are present: for v1, `projections/` validates against
`binding.schema.json` and there is no `ingests/` leg. The colocated
`validator.mjs` (Node built-ins only) is the runtime validator. The convenience
wrapper `validate-live-workspace.sh` runs it over the live workspace; the
`run-tests.sh` "plugin format schemas" leg runs it (examples-only) over both
versions.
