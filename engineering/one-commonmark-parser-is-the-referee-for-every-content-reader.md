---
type: decision
created_date: 2026-08-20T10:14:50Z
last_modified: 2026-08-20T10:15:10Z
status: accepted
decided_on: 2026-08-20
deciders: agent
scope: subsystem
tags: parser, commonmark, code-blocks, wiki-links, sections, migration
---

# One CommonMark parser is the referee for every content reader

## Decision
Every content reader in the engine takes its definition of *code* from one CommonMark parse, not from a line scanner. `memstead_base::markdown` exposes two offset- and line-preserving masks built on `pulldown-cmark` — `mask_code_blocks` (every code block: backtick or tilde fence at any legal indent, inside a list item or blockquote, or an indented code block) and `mask_code_blocks_and_spans` (those plus every inline code span) — and section splitting, title extraction, H3–H6 heading spans, wiki-link extraction, wiki-link rewriting, the strict validator and the write-time section guard all consume them. Boundaries come from the parser; bytes come from the original, so content outside a fixed misparse is preserved byte-for-byte.

Three seams close with it. The strict validator no longer draws its own section boundaries — unknown-section and relationship-syntax checks call the same splitter, and the relationship body is sliced from the *original* rather than reassembled from the masked copy, so the CommonMark content checker is no longer handed a body whose code had already become whitespace. Inline code has one definition on all three link paths, replacing three regex variants that paired backticks naively. And the empty target `[[]]` is visible to every path, routing to the typed refusal the validator already emitted instead of being invisible to the extractor.

Heading recognition is deliberately **not** widened: [[engineering--a-section-is-a-column-0-atx-level-2-heading-and-nothing-else]]. This decision generalizes [[engineering--enforce-schema-declared-section-content-formats-with-a-real-commonmark-parser]] from section-content validation to every reader.

## Context
Plan 08 put section *content* validation on `pulldown-cmark` and left the splitter and wiki-link masking on regex-plus-masking, which recognised exactly one shape of code block — a column-0 backtick fence closed by any backtick-prefixed line. Six divergences from CommonMark were verified, each a distinct misparse class: indented code blocks were not masked at all; a fence indented 1–3 spaces (the normal shape inside a list item) did not open; tilde fences were unhandled; a closing line carrying an info string closed the block early; fences inside blockquotes were not masked; and the opening fence length was stored but never compared on close. The title extractor had no masking at all.

The disagreement sat on the write path: the validator judged content the splitter had already mis-partitioned. Two referees for the same markdown is the shape that cannot converge — a reader that disagrees with the renderer every agent uses sends repair loops that do not terminate.

## Consequences
Migration evidence, gathered before the change shipped: a structural re-parse diff (title, raw headings, section bodies, heading spans, wiki-links — never content hashes, which are parse-insensitive by construction) over 766 documents in 12 corpora, including all nine mems of the dogfood workspace (681 entities across both storage backends), the agentic test workspaces, and the crate fixtures. **No section, heading, title or heading span changed anywhere.** The only differences were 60 wiki-link visibility changes in 8 documents, all one root cause — naive backtick pairing — in both directions: 18 links in four `plugin` entities that were prose all along became visible again after a runaway single-backtick regex had swallowed them, and 42 links in four agentic protocol documents that sit inside real inline code spans stopped being visible. A compose-then-reparse round-trip leg over the same corpora found zero forks.

The write-time guard became exact rather than approximate: it masks and trims before checking, so a `## ` inside a code block is admitted (it splits nothing) while content whose stored, trimmed form would expose a column-0 heading is refused at ingress — the indented-code-block-opens-a-section fork the old guard could not see, because it checked the still-indented provided content.

No parse cache exists to serve a stale pre-migration parse: heading spans and raw section headings are regenerated on every parse and never persisted. Anchors resolve against source artifacts, not entity parse coordinates, and are untouched. The wasm lane stays green with no dependency growth — `pulldown-cmark` was already in that closure.

## Relationships
- **REFERENCES**: [[a-section-is-a-column-0-atx-level-2-heading-and-nothing-else]]
- **REFERENCES**: [[enforce-schema-declared-section-content-formats-with-a-real-commonmark-parser]]
- **GENERALIZES**: [[enforce-schema-declared-section-content-formats-with-a-real-commonmark-parser]]

## Options

**Patch the regex case by case** (add tilde fences, indent tolerance, info-string closers) — rejected: it keeps two referees and chases a list that does not close; classes 5 and 6 are the proof that it does not.

**A full AST entity model** (parse to a tree, regenerate all markdown from it) — rejected: a rewrite of the entity layer with round-trip-fidelity risk for every entity, far beyond the observed damage. The sliced-source model gets the same correctness with byte-for-byte preservation.

**Adopt CommonMark's full heading grammar** (setext sections, indented ATX) — rejected: it would change the parse of an unknown number of existing entities for no observed benefit. All observed damage was on the code-block side.

**Gate the migration on content hashes** — rejected: `content_hash` is computed over raw markdown and is therefore insensitive to parse changes. The gate compares parsed structure per entity.

## Notes

Complete census afterwards: every production `^## ` / `^# ` / `#{3,6}` scan in the tree lives in `memstead-base` and routes through the unified definition. The scans in `memstead-mcp` and `ingest/brief.rs` are `#[cfg(test)]`-only assertions over engine-rendered output, not production readers.

The six misparse classes each carry a unit pin in `crates/memstead-base/src/markdown.rs` and an end-to-end pin on the real entity paths in `entity/parser.rs`, each paired with a complement asserting that prose headings and prose links still behave exactly as before.
