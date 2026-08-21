---
title: "Binding format"
---

A **binding** (stored as a `projection` file at `.memstead/projections/<mem>/<name>.json`) is one versioned record per source→mem obligation: the declaration plus an `operations { build, sync, verify }` block. It collapses the retired projection + ingest pair into a single file.

> This page is generated from the v1 binding JSON Schema and the engine's medium-capability matrix. Do not edit it by hand — regenerate with `cargo run -p xtask -- generate-docs`.

## Fields

| Field | Type | Required | Allowed values | Description |
| --- | --- | --- | --- | --- |
| `version` | integer | yes | `2` | Binding format version. v2 bindings carry the integer 2; a file without it (or with a prior version) is refused by the loader, which names `memstead projection migrate`. |
| `intent` | string | no | — | What the binding is trying to accomplish — prose for the agent. Optional. |
| `reference_mems` | array | no | — | Read-only reference mems (by name) that supply cross-mem context. |
| `destination_mem` | string | yes | — | The mem this binding writes into. |
| `deny_paths` | array | no | — | Paths excluded from the binding's scope (workspace-relative globs). Strategy-invariant — moved up from the per-ingest record. A glob deny list is legal only over a path-shaped medium namespace (codebase / filesystem / git); the engine refuses it at binding-validation time over a graph or web medium. |
| `coverage_semantics` | string | no | `exhaustive`, `curated` | Whether the binding claims to cover everything in its declared scope (`exhaustive`) or a deliberately partial slice (`curated`). Optional: absent means NOT STATED — the effective value is resolved per medium (all sources on enumerable media → exhaustive; at least one non-enumerable source, e.g. web → curated). A declared `exhaustive` over a non-enumerable source is refused at binding validation with `curated` as the remedy. |
| `rules` | object | no | — | Free-form binding rules (e.g. a one-shot lens `routing` string). Opaque to the engine — consumed only by the one-shot brief renderer. |
| `prune` | object | no | — | Prune policy — additive, optional. Absent means prune is disabled (no deletion proposals). Present means prune produces deletion proposals in the sync brief under the requested guarantee. Prune has no independent schedule: it rides the sync brief, so it carries no trigger/batch_size. |
| `operations` | object | yes | — | The operations this binding declares. Every operation is optional; an absent mutating operation (build/sync) refuses at run time with a remedy, an absent verify means engine defaults. |
| `sources` | array | no | — | The inline sources the binding consumes, in declaration order. Each `name` is unique within the record. |

## Operations

Each operation under `operations` is optional. An absent **build** or **sync** makes that *mutating* operation refuse at run time with a `projection enable <op>` remedy; an absent **verify** means engine defaults (verify never refuses on an absent operation block). Verify mutates no entity, but it is not a pure read: a completed run records its findings store, backfills observed hashes onto hash-less anchors, and records a `#verified` baseline.

### `build`

The build operation — the only operation carrying a mode. Grows new coverage (`discovery`) or runs a single bounded pass (`one-shot`). trigger / batch_size / post_actions are scheduling attributes.

| Field | Type | Required | Allowed values | Description |
| --- | --- | --- | --- | --- |
| `batch_size` | integer | yes | ≥ 1 | How many artifacts a single run processes. |
| `mode` | string | yes | `discovery`, `one-shot` | `discovery` builds out new coverage; `one-shot` runs a single bounded pass. The retired `refinement` value is not accepted. |
| `post_actions` | object | no | — | Free-form post-run actions (e.g. a one-shot `archive_source` flag). Opaque to the engine — consumed only by the one-shot brief renderer. |
| `trigger` | string | yes | `loop`, `manual`, `on-event` | What sets an operation running. `loop` runs continuously under `/loop`; `manual` runs only when invoked; `on-event` is reserved for event-driven hooks. |

### `sync`

The sync operation — the sole maintenance writer. Carries no mode. An absent sync block makes that mutating operation refuse at run time.

| Field | Type | Required | Allowed values | Description |
| --- | --- | --- | --- | --- |
| `batch_size` | integer | yes | ≥ 1 | How many artifacts a single run processes. |
| `trigger` | string | yes | `loop`, `manual`, `on-event` | What sets an operation running. `loop` runs continuously under `/loop`; `manual` runs only when invoked; `on-event` is reserved for event-driven hooks. |

### `verify`

The verify operation — measurement. Mutates no entity, but records findings, backfills observed anchor hashes, and writes a `#verified` baseline. Carries no mode. adjudication_cap and full_resync_every are additive tier-3 scheduling knobs that default to the engine's dogfood-tuned values when absent.

| Field | Type | Required | Allowed values | Description |
| --- | --- | --- | --- | --- |
| `adjudication_cap` | integer | no | ≥ 0 | The maximum number of hash-drift adjudications a single verify run asserts before queueing the remainder as backlog. 0 disables the cap. |
| `batch_size` | integer | yes | ≥ 1 | How many artifacts a single run processes. |
| `full_resync_every` | integer | no | ≥ 0 | Every N verify runs, a full-enumeration coverage sweep runs (for enumerable mediums). 0 disables scheduled full walks. |
| `trigger` | string | yes | `loop`, `manual`, `on-event` | What sets an operation running. `loop` runs continuously under `/loop`; `manual` runs only when invoked; `on-event` is reserved for event-driven hooks. |

## Per-medium capability matrix

Which fields and operations a binding may legally declare depends on the **medium** its source facets resolve to. The engine derives this from the capability matrix below and refuses an illegal combination at **binding-validation** time. A scope pattern its medium cannot express is additionally refused when the binding is resolved for a run, because validation reaches only the paths that EDIT a record — a hand-authored or pre-vocabulary binding would otherwise run over a scope that selects nothing.

| Medium | Enumerable | Change signal | Base retrievable | Anchor namespace | Glob `deny_paths` | Prune guarantee |
| --- | --- | --- | --- | --- | --- | --- |
| `codebase` | yes | yes | yes | `path` | yes | `never-clobber` |
| `filesystem` | yes | yes | yes | `path` | yes | `never-clobber` |
| `git` | yes | yes | yes | `path+commit` | yes | `never-clobber` |
| `graph` | yes | yes | yes | `entity` | no | `never-clobber` |
| `web` | no | no | no | `url` | no | `conflict-flag` |

- **Glob `deny_paths`** are legal only over a path-shaped namespace — declaring them over a medium whose **Glob `deny_paths`** column is *no* is refused at binding validation.
- The **Prune guarantee** column is the strongest guarantee the medium can *support*: `never-clobber` (full three-way merge) only where a base version is retrievable, otherwise `conflict-flag`. Requesting a stronger guarantee than the medium supports is refused at binding validation.
- **`graph` scope is an entity selector, not a path glob.** A graph source selects entities, so its `scope` patterns are `*` (the whole mem), `type:<entity_type>`, or `id:<glob>` over the full `mem--slug` id. A path-shaped pattern is refused — at binding validation and again when the binding is resolved for a run — rather than accepted and ignored — scope that selects nothing while looking like selection is the failure this rule prevents. The columns describe the medium's declared capability, not the current state of every operation over it.
