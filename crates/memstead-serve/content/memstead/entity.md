---
type: concept
maturity: established
abstraction_level: concrete
tags: memstead, core
---

# Entity

## Definition

An entity is an atomic, addressable element in a [[mem]] — a single markdown document conforming to one type from the mem's pinned [[schema]].

## Explanation

An entity is the smallest unit the engine reads, writes, links, or validates. It carries YAML frontmatter (typed metadata fields) and named sections (typed content blocks), and it is referenced by an ID of the form `<mem>--<title-slug>`. An entity may declare outgoing relationships to other entities through [[wikilink]] references in its body. Its identity is content plus ID, not its on-disk encoding — the same entity can be a `.md` file, a git blob, or a zip entry depending on the [[storage-backend]].

## Boundaries

- An entity is not a file: the file is one encoding; the entity is its content plus identity.
- An entity is not raw markdown: it is markdown *constrained by a [[schema]]*, with typed sections, metadata, and relationships.

## Significance

Because entities are typed and addressable, an agent can read one in full, follow its [[wikilink]] edges, and reason about the [[graph]] without parsing free-form prose.
