---
type: decision
created_date: 2026-07-18T11:37:35Z
last_modified: 2026-07-18T11:37:35Z
status: accepted
decided_on: 2026-07-18
deciders: operator
scope: subsystem
---

# Consolidate the pipeline into one record per pipeline with inline sources

## Decision
We folded the standalone medium and facet record kinds into the binding, making the pipeline **one versioned record**: `.memstead/projections/<mem>/<name>.json` at `version: 2`, whose inline `sources[]` each carry the medium half (`type` / `pointer` / `change_detection`) and the facet half (`scope` / `engagement` / `preparation`) under the retired facet's name byte-verbatim — the name keys per-source sync/verify state, so watermarks survive migration. This supersedes [[engineering--collapse-the-pipeline-into-one-versioned-binding-per-source-to-mem-obligation]] as the [[engine--pipeline]] model: the binding remains the unit that decision established (operations block, `hash(D)`, capability matrix), but nothing joinable remains beside it. Medium and facet survive only as the names of a source description's two halves, never as records.

**Not backwards compatible by design (operator directive, 2026-07-18):** the engine reads only v2 — a pre-v2 store (version-less gen-2 projection or v1 three-file binding) refuses at load and at boot with a typed error naming `memstead projection migrate`; no dual-format loader, no silent upgrade-on-read. The migrate command converts every prior on-disk generation in place, removes the emptied `mediums/`/`facets/` trees, refuses on orphan records rather than dropping them, and is idempotent.

## Context
The operator reviewed the three-record model against the live dogfood workspace on 2026-07-18. Every pipeline there was 1:1:1 — one medium, one facet, one binding, even name-equal — and the store's per-destination-mem namespacing (`mediums/<mem>/…`) already made cross-mem reuse impossible, capping the normalization's sharing benefit at intra-mem reuse that never occurred. Meanwhile every consumer and every agent paid for three files: cross-record integrity errors (`PIPELINE_DANGLING_REFERENCE`, `PIPELINE_RECORD_REFERENCED`), create-ordering medium→facet→binding, orphan cleanup on delete, and a twelve-handler edit surface. The model had already been consolidated once in the same direction — the separate ingest run-config folded into the binding — for reasons that applied verbatim again: detached objects drift, half-configured states exist, fidelity accounting wants one object to key on. With no external users, a compatibility window would have doubled the surface under test for nobody.

## Consequences
The cross-record reference error class is gone with the references; in-record source validation (empty/duplicate source names, the capability matrix) replaces it. The edit surface is projection-only on every mutation surface — engine JSON methods, CLI, ui-api routes, UniFFI all dropped their eight medium/facet verbs. Resolution is join-free: the record is the runtime shape. `hash(D)` derives from the record alone, so pre-consolidation verify findings invalidated by construction (accepted: they are re-derivable measurements). Sync watermarks and advance state survived migration unchanged — verified on the dogfood workspace with observable equivalence (same five flow statements, `project/graph`'s 116-item watermark intact). The macOS three-tier pipeline editor could not adapt mechanically and renders an honest unavailable state with a read-only v2 listing until the redesigned editor lands (feature-freeze posture). A future named-shared-source layer can be reintroduced additively if a real multi-facet-over-one-medium case ever materializes; it was rejected now as YAGNI.

## Relationships
- **SUPERSEDES**: [[collapse-the-pipeline-into-one-versioned-binding-per-source-to-mem-obligation]]
- **REFERENCES**: [[collapse-the-pipeline-into-one-versioned-binding-per-source-to-mem-obligation]]
- **REFERENCES**: [[engine:pipeline]]

## Options

Keep the three-record model and hide it in UI only — rejected: every consumer still pays for the split, for a reuse capability structurally capped and empirically unused. Consolidate with a compatibility window (dual-format loader, deprecation cycle) — rejected: no external users to protect, and a dual loader doubles the surface under test against the pre-1.0 posture. Add named shared sources as an optional layer now — rejected as YAGNI, reintroducible additively.

## Notes


