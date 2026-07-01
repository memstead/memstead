---
type: concept
maturity: established
abstraction_level: abstract
tags: memstead, core
---

# Vault

## Definition

A vault is a named, schema-pinned markdown entity graph about exactly one chosen subject — a typed model of that subject.

## Explanation

A vault pins one [[schema]], which fixes the entity types and relationship vocabulary its [[entity]] documents must conform to; its [[modal-flavour]] follows from that schema. A vault's bytes live in a [[storage-backend]] — a folder, a git branch, or a sealed `.mem` archive — and it is made available to a running engine by a [[mount]]. Mutations pass exclusively through the engine, leaving append-only provenance, which is what distinguishes a vault from a raw folder of markdown.

## Boundaries

- A vault is not a [[workspace]]: a workspace mounts a set of vaults; a vault is a single subject corpus.
- A vault is not a [[schema]]: the schema is the type vocabulary, the vault is the content that conforms to it.

## Significance

The vault is the atomic unit for [[mount]], schema-pin, cross-vault permissions, and distribution. Sizing one vault to one coherent subject keeps its [[graph]] navigable for agents.
