---
type: decision
created_date: 2026-07-13T16:43:04Z
last_modified: 2026-07-13T16:43:04Z
status: superseded
decided_on: 2026-06-08
deciders: memstead-core
scope: subsystem
tags: pipeline, medium, facet, projection, ingest, refactor, engine
---

# Model the pipeline as four primitives medium facet projection ingest

## Decision
We chose to model the [[engine--workspace]]-level [[engine--pipeline]] as four separable primitives, each a first-class engine-side type persisted per-mem under `.memstead/{mediums,facets,projections,ingests}/<mem>/<name>.json`:

- **Medium** — passive *territory*: a named, typed reference to a body of information (codebase, filesystem, another mem's graph, git, web). No selection, no engagement metadata, no preparation.
- **Facet** — *engagement*: how a projection engages a medium — an allow/deny scope, a free-form engagement contract (verbs, tools, discipline), and an optional preparation step. A facet references exactly one medium; a *source* facet reduces (scope + preparation), a *destination* facet disciplines (engagement contract).
- **Projection** — *obligation*: maps source facets (plus optional read-only reference mems) into one destination mem. The single place agent reasoning lives; it carries no scope, preparation, or medium metadata of its own.
- **Ingest** — *schedule*: runs a projection in a mode (discovery/refinement/one-shot), on a trigger (loop/manual/on-event), in batches, with optional per-run deny-path overrides.

Facet is one heterogeneous type with an asymmetric optional-field shape rather than two types for the source and destination sides.

## Context
The earlier pipeline shape conflated three concerns in a single `Scope` record and carried its configuration as engine-unparsed `the legacy workspace config` directory-pointers (`scopes_dir`/`projections_dir`/`ingests_dir`). Two problems forced a redesign: (1) territory (what body of information) and engagement (how a projection reads it) were fused inside `Scope`, so the same body of information could not be reused across engagements without duplicating its selection logic; (2) the dir-pointer config was opaque to the engine — unparsed and unvalidated — which blocked making pipeline configuration first-class and engine-owned. Separating territory, engagement, obligation, and schedule was the precondition for a validated, referentially-safe, engine-persisted pipeline. Precipitated by the 2026-06 pipeline refactor.

## Consequences
- A medium is named once and engaged many ways — facets and projections reference it by name, so territory is reusable rather than re-declared per engagement.
- Projection becomes the single reasoning locus (intent + source facets + destination mem); operational concerns move out to ingest, so one projection can run discovery at one time and refinement at another.
- The four primitives get durable workspace-store persistence and boot hydration ([[engine--pipeline-config-workspace-store-persistence]]) and a referentially-safe edit layer that enforces the Medium <- Facet <- Projection <- Ingest reference model on delete/rename ([[engine--pipeline-config-edit-layer-with-referential-integrity]]).
- The Claude Code plugin reads the same on-disk files to drive ingest runs; mem content stays under [[engine--storage-backend]] control.
- Cost: four config directories and types instead of one flat shape — more surface to persist, validate, and edit.
- Requires a one-way migration from the legacy `scopes/` shape, reachable only as `memstead pipeline migrate` on the [[engine--cli-command-surface]].
- `Facet.preparation` is a reserved-but-unimplemented slot: a facet naming a preparation the engine cannot run is accepted at rest but reported unsupported at run time (no silent skip, no crash) — deferred complexity, not live capability.

## Relationships
- **REFERENCES**: [[engine:workspace]]
- **REFERENCES**: [[engine:pipeline]]
- **REFERENCES**: [[engine:pipeline-config-workspace-store-persistence]]
- **REFERENCES**: [[engine:pipeline-config-edit-layer-with-referential-integrity]]
- **REFERENCES**: [[engine:storage-backend]]
- **REFERENCES**: [[engine:cli-command-surface]]

## Options

- **Keep the conflated Scope/Projection/Ingest shape with `the legacy workspace config` dir-pointers** — rejected: engine-opaque configuration with no validation, and territory fused with engagement so nothing could be reused.
- **A uniform Facet type forcing the source and destination sides into one identical shape** — rejected: it would smuggle the source/destination asymmetry into some other layer; the single-type-with-optional-fields modelling was chosen for machinery simplicity.
- **Separate `SourceFacet` and `DestinationFacet` types** — rejected in favour of one heterogeneous Facet that always references exactly one medium, keeping the loader's structural validation uniform.

## Notes

The primitives live in `crates/memstead-base/src/pipeline.rs` (`Medium` / `Facet` / `Projection` / `Ingest`). The pipeline is per-mem — each mem declares its own mediums, facets, projections, and ingests. This is the mechanism that produces graphs like the engineering mem itself: an ingest running a projection over a codebase medium.
