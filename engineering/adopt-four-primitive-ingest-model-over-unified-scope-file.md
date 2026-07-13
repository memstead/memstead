---
type: decision
created_date: 2026-07-13T16:43:02Z
last_modified: 2026-07-13T16:43:02Z
status: superseded
decided_on: 2026-06-17
deciders: dasboe
scope: subsystem
tags: plugin, ingest, pipeline, four-primitive, architecture
---

# Adopt four-primitive ingest model over unified scope file

## Decision
We split the single ingest `scope` file into a four-primitive model — [[plugin--medium-four-primitive-territory]] (passive territory), [[plugin--facet-four-primitive-engagement]] (an engagement perspective on one medium), [[plugin--projection-four-primitive-obligation]] (maps source facets and reference mems to one destination mem), and the [[plugin--ingest-config-wiring-object]] (wires a projection to a `mode` and `trigger`). Each primitive is a separately-authored JSON file at a fixed `.memstead/{mediums,facets,projections,ingests}/` path, and the whole set is pinned by the [[plugin--memstead-plugin-v0-file-format]] contract. A facet references exactly one medium; a projection composes many facets.

## Context
The retired `scope` file carried two unrelated concerns in one shape: *what body of information is in scope* (a typed pointer) and *how to read or write it* (verbs, tools, discipline). When more than one perspective needed to engage the same source — several facets over one code tree, or a read-side and a write-side view of the same mem — the single-file shape forced the pointer to be duplicated per perspective, and left no natural home for the per-source change-detection strategy. The 2026-06 pipeline refactor separated these concerns so the ingest loop could compose perspectives without copying territory.

## Consequences
- Multiple facets engage one medium from different perspectives without duplicating the medium's `pointer`; the pointer is authored once on the territory.
- The `change_detection` strategy attaches to the medium (territory) where it belongs, so the ingest loop's changed-slice detection keys off the body of information rather than off a perspective.
- A projection can compose facets across mems and pull in read-only `reference_mems`, enabling cross-mem edges without granting the source write access.
- Cost: four files and two levels of indirection replace one file — authoring a simple single-source ingest now touches a medium, a facet, a projection, and an ingest config rather than one `scope`.
- The `scopes_dir`/`projections_dir`/`ingests_dir` pointer keys were removed from `.memstead.toml`; the four primitives now live at fixed paths, so the loader no longer resolves directory-pointer keys.

## Relationships
- **REFERENCES**: [[plugin:medium-four-primitive-territory]]
- **REFERENCES**: [[plugin:facet-four-primitive-engagement]]
- **REFERENCES**: [[plugin:projection-four-primitive-obligation]]
- **REFERENCES**: [[plugin:ingest-config-wiring-object]]
- **REFERENCES**: [[plugin:memstead-plugin-v0-file-format]]

## Options

- Split into the four-primitive medium/facet/projection/ingest model — chosen; separates passive territory from active engagement so perspectives compose without duplicating the pointer.
- Keep the unified `scope` file — rejected; conflating territory and engagement forced pointer duplication whenever two perspectives shared a source and left change-detection with no natural home.
- A two-file territory/engagement split without the projection and ingest layers — rejected; without a projection primitive there is no place to compose several facets into one destination-mem obligation or to attach read-only reference mems, and without an ingest primitive there is no place to declare run `mode` and `trigger` independently of what gets built.

## Notes

**Superseded in part (E2 projection promotion, 2026-07).** The `projection` + `ingest` split this decision introduced was collapsed into a single versioned *binding* file (one record per source-to-mem obligation, carrying the former projection declaration plus an `operations { build, sync, verify }` block); the flat `.memstead/ingests/` directory was retired and the `refinement` build mode was deleted. The **medium + facet** territory/engagement split this decision established survives unchanged and remains live. The superseding decision is the engine-side “collapse the pipeline into one versioned binding per source-to-mem obligation.”
