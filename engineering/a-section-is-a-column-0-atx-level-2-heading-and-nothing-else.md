---
type: principle
created_date: 2026-08-20T10:15:02Z
last_modified: 2026-08-20T10:15:10Z
authority: established
universality: domain-wide
tags: parser, sections, commonmark, entity-format
---

# A section is a column-0 ATX level-2 heading and nothing else

## Statement
A section is opened by an ATX heading of exactly level 2 — two `#`, one space, at least one further character — starting at column 0. Nothing else opens one. Setext headings create sections nowhere. Indented ATX headings create sections nowhere. Content inside any CommonMark code block never creates a section, never registers a heading, never becomes the entity's title, and never yields a wiki-link.

## Scope
Every content reader in the engine: section splitting, title extraction, heading spans, wiki-link extraction and rewriting, the strict validator, and the write-time section guard. The contract is normative for entity authors and is stated in `GLOSSARY.md`.

## Relationships
- **REFERENCES**: [[one-commonmark-parser-is-the-referee-for-every-content-reader]]
- **CONSTRAINS**: [[one-commonmark-parser-is-the-referee-for-every-content-reader]]

## Justification

The contract is deliberately narrower than CommonMark's heading grammar, and the asymmetry is the point. Widening it — giving setext or indented ATX section semantics — would change how an unknown number of existing entities parse for no observed benefit; the recorded damage was entirely on the code-block side, where content a renderer shows as code was being read as structure. So the *code* boundary moved to the parser ([[engineering--one-commonmark-parser-is-the-referee-for-every-content-reader]]) while the *heading* boundary stayed exactly where it was. Narrow what code hides; do not widen what counts as a heading.

## Exceptions

None. Deeper headings (`### ` and below) are ordinary content within a section — indexed for search-time heading paths, never a section boundary.

## Consequences

A reader may scan for a column-0 `## ` on the masked body and slice the original at the offsets it finds; both masks preserve byte offsets and line counts so that stays sound. A write whose stored (trimmed) section body would expose a column-0 `## ` or `# ` is refused at ingress rather than forking the entity on its next read — the guard is applied to the content as the reparse will see it, not as it was provided.
