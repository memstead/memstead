---
type: decision
created_date: 2026-08-07T09:08:15Z
last_modified: 2026-08-07T09:08:15Z
status: accepted
decided_on: 2026-08-06
deciders: operator (agent-toolbox bundle decision 2), implementing agent
scope: subsystem
tags: schema, constraints, health, write-gates, severity, consistency-layer
---

# Declare unhealthy-to-keep invariants through a five-form constraint vocabulary

## Decision
We chose to give schemas a constraint vocabulary of exactly five forms, so a schema can declare not only what is *legal to write* but what is *unhealthy to keep*: **`requires_when`** (conditional requirement — a field or section required when another metadata field holds a declared value, e.g. `status: checked` requires `checked_by`), **`required_outgoing` severity** (each required-edge block carries a declared severity; the historical warn behaviour stays the default), **`unique`** (a declared tuple of metadata fields unique within the mem), **`enum_from_neighbour`** (a field whose legal values are the entries of a named section on the entity reached via a named edge; a value with no backing entry is a finding), and **`status_propagation`** (a terminal value of a named status field taints entities reaching it via a named rel-type and direction; tainted entities are health findings naming their tainting ancestor). The design posture is uniform: declarations live in the schema package ([[engine--schema-definition-format]]), travel sealed with the mem, and are visible in the `memstead_schema` response at both verbosity levels — no legality condition the schema response omits. The engine enforces them under **one severity model instead of five ad-hoc ones**: every constraint declares `warn` (health finding only) or `block` (write-time refusal plus health finding for pre-existing violations). Uniqueness defaults to block (its whole point is bouncing the duplicate); everything else defaults to warn; propagation is always warn-tier at write time, because a parent falling *after* the child was written cannot retroactively make the child's write illegal. Block-tier refuses on create, update, and relate on every surface — operator mode bypasses allowlists, never validation — alongside the [[engine--runtime-validator]]'s schema conformance; both tiers surface on the [[engine--graph-health-report-surface]] (`constraints` include, `--strict`-participating).

## Context
Two external field projects (anker, plenum) independently built the same substitute — project-local Python check scripts computing standing invariants the schema could not express — without knowing of each other: the strongest single piece of evidence either feedback channel produced. Four confirmed requests mapped to this family (conditional required fields, declarative uniqueness — an orphan process created 37 duplicates a declared key tuple would have bounced, enum-from-neighbour, promotable required-edge severity), plus the propagation gap: `propagating_relationships` was declared by schema authors in the belief it expresses impact while its only functional behaviour is self-edge refusal. Operator decision 2026-08-06 released this member from the 2026-07-18 divergence-eval hold on the strength of that field evidence; the conflict primitive and authority-in-workspace-policy members of the same consistency-layer step stay under the hold. The hold-release is recorded strategically in the project mem's consistency-layer thesis memo (`project--the-consistency-layer-thesis`).

## Consequences
- The proof is deletion: both field projects' standing-invariant check questions are answerable from health output and the write path alone, without a line of project Python — verified by the anker grounding proof and the plenum duplicate/rename proofs.
- `propagating_relationships` is settled honestly: the new propagation declaration carries the real semantics under a self-explanatory name; the old field keeps its actual self-edge-refusal behaviour (regression-pinned) with an honest description and deprecation pointer — no silent repurposing of a sealed vocabulary word.
- Loader honesty: a malformed constraint declaration refuses at schema install/validation with a typed error naming the offender — no declaration can be loaded and silently ignored.
- No new noise: a schema declaring no constraints produces byte-identical health output and write behaviour.
- Cost accepted: constraint evaluation runs on every write and every health sweep; the vocabulary is evidence-bounded and deliberately closed — a sixth form needs a real holding writing a check none of the five express.
- Cross-mem constraints, staleness-against-parent-versions, and resolution conditions are explicitly out of scope of the vocabulary.

## Relationships
- **REFERENCES**: [[engine:schema-definition-format]]
- **REFERENCES**: [[engine:runtime-validator]]
- **REFERENCES**: [[engine:graph-health-report-surface]]
- **REFERENCES**: [[enforce-schema-declared-section-content-formats-with-a-real-commonmark-parser]]

## Options

- **Patch each field request separately** (a `required_when` here, a unique flag there) — rejected: four patches with four severity behaviours is how a vocabulary becomes folklore; one severity model, five forms.
- **A general expression/query language for invariants** — rejected: a second schema language with its own parser, versioning, and injection surface; the five forms cover every invariant the two field projects actually wrote.
- **A new check verb (`memstead check`)** — rejected: health is the one dashboard; a second check surface splits the audience.
- **Hard-blocking propagation at write time** — rejected: the taint arises from the parent's later change; blocking the child's historical write is incoherent. The health finding plus `--strict` exit is the enforcement point.
- **Silently giving `propagating_relationships` the propagation semantics** — rejected: sealed schemas in the wild declare it today with the current semantics; changing meaning under a stable name is the section-heading bug's vocabulary twin.

## Notes

Shipped 2026-08-06 as public-repo commits `e583e90` (forms 1 and 4 as complete verticals) and `cb0d851`/`fc5b303` (forms 2, 3, 5 plus both field proofs); independently graded — all nine acceptance criteria confirmed. Complementary half of the same design move as [[engineering--enforce-schema-declared-section-content-formats-with-a-real-commonmark-parser]]: this vocabulary covers cross-entity and metadata invariants, the section content format covers the markdown shape inside a section; `enum_from_neighbour` reads "the entries of a section" as the item shape of a section declaring `content: list`.
