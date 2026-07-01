---
type: concept
maturity: established
abstraction_level: concrete
tags: memstead, relationships
---

# Wikilink

## Definition

A wikilink is a markdown reference from one [[entity]] to another, written as the target's id inside double square brackets — bare for a link within the same [[vault]], or prefixed with a target vault name for one that crosses a vault boundary.

## Explanation

Wikilinks are how the [[graph]]'s edges are authored: a link in an entity's body is a foreign-key reference that the engine resolves and records as a typed relationship. An untyped wikilink defaults to a soft `REFERENCES` edge; the [[schema]]'s relationship vocabulary constrains which other relationship types are valid. A wikilink lives in the source entity's markdown bytes, so it travels with the entity, while resolution and permission checks happen at read or write time.

## Boundaries

- A within-vault wikilink resolves inside the source [[vault]]; a cross-vault wikilink resolves through the [[workspace]] and needs a permission to cross.
- A wikilink is entity content, not [[workspace]] state — the edge is part of the document, not a separate record.

## Significance

Wikilinks are what make a [[vault]] a graph rather than a flat list, letting agents navigate by relationship instead of by full-text search alone.
