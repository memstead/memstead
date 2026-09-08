---
type: decision
created_date: 2026-09-05T16:51:55Z
last_modified: 2026-09-08T21:09:01Z
status: accepted
decided_on: 2026-09-05
deciders: engine-moves plan (census posture C, 2026-09-04); agent decision under the engine-changes rule
scope: subsystem
tags: anchors, health, rendering, vocabulary
---

# The anchor resolution figure is a type that carries its population, and the anchor-state vocabulary is the enum alone

## Decision
The positive anchor resolution count lives in memstead-base as AnchorResolutionFigure: a type that cannot be constructed, deserialized or therefore printed without the population statement it was computed over (what rows it counted and how much of them the pass could not adjudicate) and its fully_adjudicated verdict. It prints in exactly two ways, both carrying the statement: as one sentence (the count then the population) and as a ratio over a denominator; it serializes as the three fields every surface already emitted at the same level (resolves, population, fully_adjudicated), flattened into the standalone verification, the fidelity report's anchor composition and the health anchors axis, so JSON consumers see the shape they saw before and a renderer of JSON reads the figure back through the type (from_json) rather than lifting the count. No renderer reaches the raw count: the standalone verification and the report carry the figure instead of a count field, the CLI's verify-anchors and health markdown and the MCP text channel print through it, and a bare count is now a construction error, not a lint finding. The anchor-state vocabulary is the engine's enum alone: AnchorState::ALL lists every variant, describe gives each its doc line, vocabulary_help renders both for the CLI help and the reference generated from it, a serde round-trip test pins the wire names, and a new variant fails to compile until it joins the list. Every surface spells a row's state with the enum's wire name, so a gone artifact's row reads orphaned everywhere; the summary count beside it keeps its surface name, unresolvable (the artifact is gone, a measured failure), and the finding class UnresolvableAnchor keeps the name the findings store is keyed by.

## Context
Every W3 finding of the 2026-08 consistency sweep was a resolution figure that meant less than it looked without ever being wrong in a visible way, so sweep 03/05 made the population statement mandatory beside the figure and guarded the rule with two workspace scripts: one walked both trees for every site reading or rendering the count and held it against a declared manifest of nine sites, one compared the enum's variants against a declared list of states. The first went red on a false positive (an unrelated resolved key) the day before the 2026-09-04 census and was declared around. The census (posture C) ruled that a class ends as a removed cause: a type reaches every renderer, present and future, where a list of sites never did. Renderers kept appearing that the list did not know; the type makes them impossible to write bare. The same census found the row state of a gone artifact spelled unresolvable by the CLI while the enum, the fidelity report and the declared state list all said orphaned: two names for one state, the drift the vocabulary check existed to catch and had not.

## Consequences
The two scripts and their manifests retired with their hygiene job. The fidelity report's resolution line leads with the ratio and its population (the bare resolves count that opened it is inside the ratio); the verify-anchors markdown carries the population on the Resolves line instead of a separate Population bullet; the health markdown puts the population in the resolves clause instead of after a dash. JSON shapes are unchanged except for two additive keys on the fidelity report's anchors object (population, fully_adjudicated) and the row state orphaned where unresolvable stood. Rejected: dropping the statement (every W3 finding was a bare figure), and keeping the site list and adding sites as they appear (the list is what kept missing them). Left as a known second name: the summary count key unresolvable and the finding class UnresolvableAnchor for the state the enum calls orphaned; renaming them is a wire change across the health axis, verify-anchors, the open-questions axis and the findings store, a decision of its own.

## Relationships
- **GOVERNS**: [[engine:anchor-population-a-binding-answers-for]]
- **GOVERNS**: [[engine:anchor-primitive]]
