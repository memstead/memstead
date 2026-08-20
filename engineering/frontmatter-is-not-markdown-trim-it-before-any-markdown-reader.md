---
type: principle
created_date: 2026-08-20T12:19:10Z
last_modified: 2026-08-20T12:19:16Z
authority: established
universality: domain-wide
tags: parser, frontmatter, commonmark, masking, guards
---

# Frontmatter is not markdown — trim it before any markdown reader

## Statement
A whole entity file is never handed to a markdown reader. Frontmatter is YAML, not markdown, and a CommonMark parser reads block structure into it that is not there — a value line that looks like a fence opener (legal at 1–3 spaces) opens a code block that runs past the `---` terminator to end of file and blanks the entire body. Any caller holding a raw file or a git blob trims the frontmatter first; a caller holding a section body already has one, because section bodies are frontmatter-free by construction.

The trim belongs to the caller that knows what it holds, never to the shared helper. A section body may legitimately open with a `---` thematic break, and a helper that trimmed unconditionally would mistake it for frontmatter and cut real content away.

## Scope
Every function that masks or parses markdown internally in the engine — the two masks of [[engineering--one-commonmark-parser-is-the-referee-for-every-content-reader]] and everything built on them: section splitting, title extraction, heading spans, wiki-link extraction and rewriting, and the merge-conflict guard.

## Relationships
- **REFERENCES**: [[one-commonmark-parser-is-the-referee-for-every-content-reader]]
- **CONSTRAINS**: [[one-commonmark-parser-is-the-referee-for-every-content-reader]]

## Justification

The rule is stated where it is *read* — on the masks themselves — rather than where the remedy lives, because the alternative was tried and failed three times in a single session. Each audit enumerated the callers it went looking for (the link extractors) and missed the ones it did not, and the defect recurred at a new site each round: `parse_markdown`, then the git-branch ripple scanner, then the merge-conflict guard.

The third recurrence is the one that shows the cost. `has_merge_conflict_markers` masks internally and all four of its production callers hand it a whole file; a fence-shaped frontmatter value hid the conflict markers, and a conflicted file loaded as an ordinary entity with **both merge sides fused into one body** — verbatim the outcome the guard's own comment says it exists to prevent. A fourth site, `rewrite_mem_prefix`, failed more quietly still: the scan found no links, the rewrite reported zero changes, and a mem rename left dangling cross-mem references behind, indistinguishable from having nothing to rewrite.

The generalisable lesson is about the census, not the parser. A census must enumerate the property that carries the defect — here *what a function is handed* — not the property that is easy to grep for. The plan's own census counted heading scanners; the recurring class was mask consumers, a different and larger set, and the site that did the real damage appeared in neither list.

## Exceptions

None for markdown readers. A scan whose subject genuinely spans the whole file — merge-conflict detection is the live example, since git writes markers wherever the hunks fall, frontmatter included — does not skip the frontmatter: it scans a rejoined view of raw frontmatter plus masked body, so a marker triple straddling the `---` terminator is still caught. Masking preserves byte length, which is what keeps that view offset-aligned with the original.

## Consequences

A new caller that masks a whole file is a defect, and the doc comment on both masks says so with the three-strikes history attached. Reviewing a change that adds a mask consumer means asking one question: what does this caller hold — a body, or a file?
