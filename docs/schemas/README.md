# Workspace format schemas

JSON Schema (draft 2020-12) documents that pin the on-disk shape of the
workspace config files the engine reads (`.memstead/{mediums,facets,projections}/`).

These are **dev/docs tooling, not a runtime surface**: the authoritative
loader and validator is the engine's Rust side (`memstead-base` — serde
types plus `validate_binding` and the medium-capability matrix). The JSON
schemas earn their keep as:

- the render source for the docs-site **binding format reference**
  (`xtask`'s `write_binding_reference` renders `binding.md` from
  `v1/binding.schema.json`);
- the **round-trip pin**: `memstead projection init` output is pinned
  against `v1/examples/binding.from-init.json` (JS half in
  `validator.test.mjs`, Rust half in memstead-cli's suite);
- worked **examples** (one minimal + one full per file type) that double
  as documentation and schema-test fixtures.

They deliberately live under `docs/`, not inside the Claude Code plugin:
a marketplace install copies the entire plugin directory, and these files
are not needed at plugin runtime. (The former `memstead-plugin/v0` tree —
the retired projection + ingest pair — and the unwired `format`-key
version-negotiation layer were removed 2026-07-11; pre-v1 workspace
migration is handled by the engine's own Rust migrate path.)

## Layout

```
docs/schemas/
  README.md                              # this file
  memstead-plugin/
    v1/
      memstead-toml.schema.json          # `.memstead.toml` (legacy marker file; union of engine/plugin keys)
      medium.schema.json                 # `.memstead/mediums/<mem>/<name>.json`
      facet.schema.json                  # `.memstead/facets/<mem>/<name>.json`
      binding.schema.json                # `.memstead/projections/<mem>/<name>.json`
      examples/
        binding.minimal.json
        binding.full.json
        binding.from-init.json           # the `projection init` golden (round-trip pin)
        medium.minimal.json
        facet.minimal.json / facet.full.json
        memstead-toml.minimal.json / memstead-toml.full.json
      validator.mjs                      # hand-rolled validator (Node built-ins only)
      validator.test.mjs                 # `node --test` suite (examples + round-trip pin + refusals)
```

**v1: the binding replaces the projection + ingest pair.** The retired
four-primitive `projection` + flat-`ingest` split collapses into one versioned
`binding` file at `.memstead/projections/<mem>/<name>.json` — the declaration
(`intent`, `source_facets`, `reference_mems`, `destination_mem`, `deny_paths`,
`coverage_semantics`, `rules`) plus an `operations { build, sync, verify }`
block. There is no `ingests/` directory and no `ingest` schema in v1. The
retired `mode: refinement` value is not accepted. The engine gates on the
`version: 1` integer inside each binding file, not on any schema-layer key.

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
directory-pointer keys.

## Test wiring

The `run-tests.sh` "workspace format schemas" leg runs
`node --test docs/schemas/memstead-plugin/v1/validator.test.mjs` —
metaschema shape, every example validates, and the JS half of the
round-trip pin.
