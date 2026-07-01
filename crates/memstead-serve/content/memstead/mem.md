---
type: concept
maturity: established
abstraction_level: abstract
tags: memstead, core
---

# Mem

## Definition

A mem is a named, schema-pinned markdown entity graph about exactly one chosen subject — a typed model of that subject.

## Explanation

A mem pins one [[schema]], which fixes the entity types and relationship vocabulary its [[entity]] documents must conform to; its [[modal-flavour]] follows from that schema. A mem's bytes live in a [[storage-backend]] — a folder, a git branch, or a sealed `.mem` archive — and it is made available to a running engine by a [[mount]]. Mutations pass exclusively through the engine, leaving append-only provenance, which is what distinguishes a mem from a raw folder of markdown.

## Boundaries

- A mem is not a [[workspace]]: a workspace mounts a set of mems; a mem is a single subject corpus.
- A mem is not a [[schema]]: the schema is the type vocabulary, the mem is the content that conforms to it.

## Significance

The mem is the atomic unit for [[mount]], schema-pin, cross-mem permissions, and distribution. Sizing one mem to one coherent subject keeps its [[graph]] navigable for agents.
