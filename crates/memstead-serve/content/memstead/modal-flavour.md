---
type: concept
maturity: stable
abstraction_level: meta
tags: memstead, core
---

# Modal Flavour

## Definition

A modal flavour is the conceptual genre a [[vault]] inhabits — knowledge, planning, inquiry, spec, or hybrid — determined by the [[schema]] the vault pins.

## Explanation

The flavour is a read-back of the schema choice rather than an independent setting: a schema whose types are factual claims and definitions makes a knowledge graph; one whose types are goals, options, and decisions makes a planning graph; one whose types are prescriptions makes a spec graph. The flavours are not hard-coded in the engine — adding a new one means authoring a new [[schema]] with a coherent type vocabulary, with no engine change.

## Boundaries

- A modal flavour is not a separate axis from the [[schema]]: every schema implies a flavour by the types it declares.
- A modal flavour is the user-facing name; the technical vocabulary uses [[vault]] plus its [[schema]].

## Significance

The flavour is the concrete word a person uses for what their [[vault]] is — "a knowledge graph", "a spec graph" — and it follows entirely from one schema choice.
