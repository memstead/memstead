---
type: decision
created_date: 2026-07-13T16:43:03Z
last_modified: 2026-07-18T11:37:52Z
status: superseded
decided_on: 2026-07-10
deciders: memstead-core
scope: subsystem
tags: pipeline, binding, projection, ingest, migrate, status, uniffi, refactor, engine
---

# Collapse the pipeline into one versioned binding per source-to-mem obligation

## Decision
We collapsed the four-primitive pipeline into **one versioned binding file per source→mem obligation**, superseding [[engineering--model-the-pipeline-as-four-primitives-medium-facet-projection-ingest]]. The projection becomes the unit: a single `.memstead/projections/<mem>/<name>.json` at `version: 1` carries declaration (intent, source facets, reference mems, destination mem, deny_paths, coverage_semantics, rules) **plus** an `operations` block (`build`/`sync`/`verify`, each with mode/trigger/batch_size/post_actions). The separate flat `Ingest` record and the `ingests/` store tier die; `IngestMode::Refinement` is deleted from the vocabulary, not migrated.

The redesign also lands: `hash(D)` (SHA-256 over the resolved content-defining declaration — the substrate E3b keys findings on), a per-medium **capability matrix** that refuses unsupported operations at binding-validation time with a one-command remedy, the `memstead projection {brief,init,migrate,advance,enable}` CLI tree (retiring `memstead ingest` and `memstead pipeline`), disposition-gated **`projection advance`** with a durable resumable store, `sync_state` re-keyed to `<binding-id>/<facet>#synced|#verified`, and **`memstead status`** replacing `memstead stats`. The break carries through UniFFI (`projection_brief`, `get_status`, `pipeline_configs_json` drops the `ingests` key, ingest CRUD collapses) to a macOS compile-green floor.

## Context
The four-primitive model ([[engineering--model-the-pipeline-as-four-primitives-medium-facet-projection-ingest]]) solved territory-vs-engagement reuse but left the source→mem obligation spread across two config records (per-mem `Projection` + a flat `Ingest`), two identities, a referential chain, and a flat-vs-per-mem store asymmetry — plus skill-written cursor state and a refinement mode that was a second competing maintenance writer. That split was the disease: operations are attributes of the obligation, not a peer record. A single versioned binding makes the engine the sole owner of every piece of pipeline state (declaration, sync tokens, advance dispositions, selection cache) and gives `hash(D)` and the fidelity engine (E3b) a substrate. Executed as the projection-promotion effort, landed 2026-07-10 in four bisect-green units (v1 store adoption + loader gate + migrations; the advance subsystem; status; the CLI retire + UniFFI/macOS floor).

## Consequences
- One binding file, one identity (`<destination-mem>/<stem>`), no referential chain, no flat/per-mem asymmetry. [[engine--pipeline-config-workspace-store-persistence]] now persists the v1 binding behind a version-gate (version-less files refuse with a typed error naming `memstead projection migrate`); [[engine--pipeline-config-edit-layer-with-referential-integrity]] edits the binding while preserving its operations block.
- `memstead projection migrate` promotes **both** legacy generations (root-folder `scopes/` and the gen-2 four-primitive store) to v1; the boot compatibility shim is dropped (pre-1.0 — one loud migrate, never a silent serve).
- The [[engine--pipeline]] is now the binding, run by an operation. The [[engine--cli-command-surface]] loses `ingest`/`pipeline` and gains the `projection` tree + `status`.
- The macOS app tracks the break at a compile-green floor: its ingest-run surface runs a binding's build operation via `projection_brief`; the operations-block editor UI and a status projections panel are **deferred one release** (operator decision 3), the only part of the break not shipping same-session.
- The binding **format is frozen** at E2's completion — E3a (anchors) and E3b (fidelity/verify) build on it; additive fields later are fine, reinterpretations bump `version`.
- Cost: a one-way migration every pre-v1 workspace must run once, and a UDL break propagated through the macOS app in the same session.

## Relationships
- **REFERENCES**: [[model-the-pipeline-as-four-primitives-medium-facet-projection-ingest]]
- **REFERENCES**: [[engine:pipeline-config-workspace-store-persistence]]
- **REFERENCES**: [[engine:pipeline-config-edit-layer-with-referential-integrity]]
- **REFERENCES**: [[engine:pipeline]]
- **REFERENCES**: [[engine:cli-command-surface]]
- **SUPERSEDES**: [[model-the-pipeline-as-four-primitives-medium-facet-projection-ingest]]

## Options

- **Keep the projection+ingest split with a version field on each** — rejected: the split is the disease (two files, two identities, referential chain); versioning it entrenches the seam instead of removing it.
- **Cut `rules`/`post_actions` as speculative** — rejected: both are consumer-backed by the one-shot lens brief; the consumer-backed rule keeps them.
- **Switch the macOS app to `mem_roster`+`get_health` at the UDL break** — rejected for the floor: the rename-preserving `get_status` is the cheapest correct floor; the data-source rework belongs to the deferred-UI release.
- **Auto-derive `worked` dispositions in `projection advance` now** — deferred to E3a: deriving `worked` needs the artifact↔entity mapping only anchors provide; until then every disposition is explicit.

## Notes

Format and machinery live in `crates/memstead-base/src/binding.rs` (`BindingV1`, `hash_binding`, capability matrix), `binding_migrate.rs` (gen-2→v1), `ingest/advance.rs` (the durable advance store), and the `projection` CLI leaves in `memstead-cli`. **E2 is complete** (all 22 acceptance criteria confirmed by an independent grading gate, 2026-07-10). The v1 binding format has no `Ingest` record and no `refinement` mode; the CLI (`ingest`/`pipeline` retired) and UniFFI surfaces are switched over; and the engine machinery is **deleted** — `Ingest`/`IngestMode`/`Refinement` and the refinement renderers are gone from `pipeline.rs`/`render.rs` (only a migrate-local `LegacyIngest` parse struct survives so `projection migrate` can read and refuse the old shape). The five edge-behaviors the grading gate demanded all landed: absent-operation-block refusal with a `projection enable {build,sync}` remedy (`operations.build` is optional), cursor consumption + `reconcile-cursors.json` deletion + `workspace.toml` proposal in migrate, reload-independence doc/test pins, and `signal: none` for detection-less web sources. The binding **format is frozen** — E3a (anchors) and S1a (first plugin release) build on it. Superseded decision retained at `status: superseded` for the historical record. Minor residuals (non-blocking): the legacy-load CLI token is `PROJECTION_LOAD_FAILED` (the message names `projection migrate` correctly) rather than the originally-specified spelling `PROJECTION_STORE_LEGACY`.
