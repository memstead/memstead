---
type: concept
maturity: established
abstraction_level: abstract
tags: memstead, core
---

# Schema

## Definition

A schema is the type vocabulary that constrains a [[vault]]'s content — what entity types exist, what sections each type has, which are required, what relationship types are allowed, and what metadata fields are valid.

## Explanation

A schema is the contract that makes a vault a *typed* model rather than a raw markdown collection: every [[entity]] must conform to one of the schema's declared types, and the engine validates each mutation against the schema's section, metadata, and relationship rules at the boundary. A vault pins exactly one schema by a `name@version` reference. Because the schema fixes the type vocabulary, it also determines the vault's [[modal-flavour]].

## Boundaries

- A schema is not a [[vault]]: many vaults can pin the same schema while modelling different subjects.
- A schema is not enforced content: it declares what is *valid*, while the [[entity]] documents are what actually exists.

## Significance

Swapping the schema is how one engine serves a knowledge, planning, inquiry, or spec [[modal-flavour]] without any code change.
