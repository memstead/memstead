# Ingest — Design Intent

What this skill must achieve. Use this as the reference when tuning `SKILL.md` and `inject.mjs`.

## Core purpose

- pick the next ingest from the workspace, assemble the agent prompt, exit
- the plugin assembles prompts; the agent does the work via Claude Code's tool-use loop
- designed for `/loop 1m /memstead:ingest` — runs sequentially across ingests with backoff

## Workspace file layout

The plugin operates on the **externalised workspace layout**. Plugin-owned files live at the workspace root; per-mem state (schema pin, write guidance, snapshot tokens, mem inventory) is the engine's concern and reaches the plugin only through `memstead workspace dump`:

```
<workspace>/
  .memstead.toml                       # plugin-side config: format + dir names
  scopes/<mem>/<name>.json       # source-side definitions (codebase tree, filesystem, graph)
  projections/<mem>/<name>.json  # source-to-destination wiring + rules (rules.routing for lenses)
  ingests/<name>.json              # operational unit: mode + trigger + batch_size + projection_ref
```

`.memstead.toml` declares `scopes_dir`, `projections_dir`, `ingests_dir` (defaults: `scopes`, `projections`, `ingests`). The `workspace-loader.mjs` reads these and walks the trees, then invokes `memstead workspace dump` to fetch the engine's view of the workspace — which mems exist, each mem's schema pin, write guidance, description, and an opaque snapshot token used for backoff. Every per-mem fact the plugin consumes flows through that JSON document; the plugin does not read `.memstead/config.json`, walk mem `**.md`, or open mem gitdirs.

## Engine binary dependency

The plugin shells out to `memstead workspace dump` once per fire. Discovery order:

1. `MEMSTEAD_BIN` env var (absolute path to a built `memstead` binary)
2. `memstead` on `PATH`

When neither resolves, `inject.mjs` exits with a one-line agent-visible message naming the override mechanism. The plugin does not silently fall back to disk-direct reads — one storage-aware code path lives in the engine, not in the plugin.

## Ingest modes

- **discovery** (default) — minimal context, no scout/writer cycle
- **refinement** — scout reviews source files against destination entities in batches; writer fixes findings on the next fire
- **one-shot** — runs exactly once per trigger; not re-picked on the next round. Used for lenses that lift content across mems

## One-shot lens enrichment

When a one-shot ingest has multiple destinations (a cross-mem lens), the assembled prompt includes four parseable sections:

- **Destination set** — table of mem, schema, purpose per destination
- **Routing rule** — verbatim from `projection.rules.routing`; agent decides per-entity which destinations to target
- **Idempotency** — re-runs use `memstead_update` (or skip-if-exists), never duplicate
- **End-of-run report** — per-destination created/updated/skipped/failed counts

Optional **Archive after run** section appears when the ingest carries `post_actions.archive_source: true`.

## Operational rules

- **Plugin assembles, agent acts.** No MCP-client code path inside `inject.mjs`. The agent owns destination iteration, `memstead_create` / `memstead_update` calls, and end-of-run reporting via Claude Code's tool-use loop.
- **Partial success is the accepted failure mode.** Each destination mem is an independent commit target. No cross-mem rollback exists.
- **Round-robin keyed by ingest filename.** `<workspace>/.memstead.cache/ingest/ingest-cursor.json` tracks the last picked ingest. One-shot ingests are filtered out of the eligible set after their marker lands in `ingest-one-shot-runs.json`.
- **Backoff suppresses idle ingests.** Refinement ingests escape backoff while batches remain in the current rotation.

## Cache layout

```
<workspace>/.memstead.cache/
  .gitignore                                    # contains '*'
  ingest/
    ingest-cursor.json                          # round-robin cursor
    ingest-one-shot-runs.json                   # one-shot completion markers
    ingest-backoff.json                         # per-ingest backoff state + specs snapshot
    refinement/<name>.json                      # batch state (rotation, file_order, cursor)
    refinement/<name>-findings.md               # scout output, consumed by next writer fire
    prompts/                                    # last 10 assembled prompts (debugging)
```

Cache contents never land in git — `.memstead.cache/.gitignore` is dropped on first write.
