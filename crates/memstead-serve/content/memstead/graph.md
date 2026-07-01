---
type: concept
maturity: stable
abstraction_level: abstract
tags: memstead, core
---

# Graph

## Definition

A graph is the live, mutable form of a typed model — at its smallest one [[vault]]'s content, at its largest a whole [[workspace]]'s mounted vaults united by their cross-vault edges.

## Explanation

"Graph" is the prose word for the working state, as distinct from the sealed `.mem` distribution form, which is a [[vault]]. A single vault's content is a homogeneous graph — one [[schema]], one subject — built from typed [[entity]] nodes and their [[wikilink]] edges. A workspace's mounted vaults plus the edges between them form a heterogeneous graph spanning multiple schemas and subjects. A union of typed sub-graphs with cross-edges is itself a graph, so the word works at both levels.

## Boundaries

- A graph is the live form; a sealed [[vault]] archive is the frozen distribution form of the same content.
- A graph is the user-facing word; [[vault]] is the technical unit of mount, schema-pin, and distribution.

## Significance

Agents navigate a graph by walking [[entity]] nodes and [[wikilink]] edges and by reading community summaries, so they can reason about structure without reading every entity.
