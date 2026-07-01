---
type: concept
maturity: established
abstraction_level: abstract
tags: memstead, core
---

# Memstead

## Definition

Memstead is a schema-agnostic graph engine: each [[mem]] keeps a typed, interconnected set of markdown entities modelling one chosen subject, readable and writable by both humans and LLM agents.

## Explanation

A [[mem]] pins one [[schema]], and the schema decides the mem's [[modal-flavour]] — knowledge, plans, inquiry, specs, or any mix. Content is plain markdown over git, so the [[graph]] stays diffable, human-readable, and free of vendor lock-in. Agents reach the graph through the [[mcp-layer]]; humans reach it through a CLI and a native app. Every mutation is typed and passes through the engine, which validates it against the schema and records append-only provenance.

## Significance

Memstead's premise is that a knowledge base built for LLM agents as the primary reader should be typed, navigable, and diffable: the [[schema]] makes the model typed, [[wikilink]] relationships make it navigable, and markdown-over-git makes it diffable.
