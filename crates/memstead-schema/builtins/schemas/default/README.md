# `default` schema

`default@1.0.0` is the built-in memstead schema. It ships embedded in the engine binary via `include_dir!` and backs any vault whose `.memstead/config.json` does not override it. Ten entity types spanning spec-authoring and knowledge capture, one shared relationship vocabulary, strict mode.

Use when you want a general-purpose knowledge graph without authoring a custom schema. Author a dedicated schema when the domain vocabulary is specialised enough that generic types would hurt agent judgement — see [../../../examples/minimal/](../../../examples/minimal/) and [dev/authoring-schemas.md](../../../../../dev/authoring-schemas.md) for the authoring flow.

## Vault pinning

```jsonc
// <vault>/.memstead/config.json
{ "schema": "default@1.0.0" }
```

Exact semver only — no ranges, no "latest".

## Types

Every type inherits the engine's four implicit metadata fields — `type`, `created_date`, `last_modified`, `tags` — on top of whatever the YAML declares. See [dev/authoring-schemas.md §"What you don't need to declare"](../../../../../dev/authoring-schemas.md) for the contract.

The schema is split into two loose families. Pick the most specific type — an `assertion` is better than a `memo` for a factual claim.

### Spec-authoring

| Type | Use for |
|---|---|
| [spec](types/spec.yaml) | Current-state documentation — what a thing IS now |
| [memo](types/memo.yaml) | Reasoning artefacts — decisions, observations, lessons, plans |

### Knowledge capture

| Type | Use for |
|---|---|
| [assertion](types/assertion.yaml) | A testable factual claim with evidence |
| [concept](types/concept.yaml) | Precise definition of an abstract idea or term |
| [inquiry](types/inquiry.yaml) | An open question under investigation |
| [model](types/model.yaml) | A recurring pattern observed across instances |
| [narrative](types/narrative.yaml) | A temporal sequence — story, history, post-mortem |
| [perspective](types/perspective.yaml) | A viewpoint or interpretation on a topic |
| [principle](types/principle.yaml) | A rule or guideline that governs other entities |
| [process](types/process.yaml) | A repeatable sequence of steps to achieve an outcome |

## Relationships

Strict mode — 37 declared relationships plus the required `_default` fallback. Full definitions in [schema.yaml](schema.yaml). High-traffic edges:

| Relationship | Meaning |
|---|---|
| `PART_OF` | Hierarchical containment (graph edge only — files stay flat at vault root) |
| `REFERENCES` | Soft reference — auto-emitted from inline wiki-links |
| `DEPENDS_ON` | Logical dependency — source breaks if target removed |
| `IMPLEMENTS` | Concrete implementation of an abstract spec |
| `DERIVED_FROM` / `SUPERSEDES` | Lineage and replacement over time |
| `SUPPORTS` / `CONTRADICTS` | Evidence linking for assertions |
| `GENERALIZES` / `SPECIALIZES` | Concept hierarchies |

Consult [schema.yaml](schema.yaml) `when_to_use` fields before picking an unusual relationship — several neighbours (`CLASSIFIES` vs `GENERALIZES`, `MOTIVATED_BY` vs `DERIVED_FROM`) are easily confused.
