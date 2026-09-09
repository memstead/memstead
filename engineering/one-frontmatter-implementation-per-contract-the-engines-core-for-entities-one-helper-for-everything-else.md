---
type: principle
created_date: 2026-09-05T16:00:34Z
last_modified: 2026-09-09T00:07:00Z
authority: proposed
universality: contextual
tags: frontmatter, parser, core, scripts
---

# One frontmatter implementation per contract: the engine's core for entities, one helper for everything else

## Statement
The frontmatter delimiter contract has exactly one implementation per document contract. Entity frontmatter (schema-validated, the engine's) is split by memstead-base's core alone: split_frontmatter_core with its Frontmatter verdict, and the borrowed views frontmatter_parts and body_after_frontmatter, exported at the crate root so every Rust reader in either tree calls it, and a workspace script that needs an entity's fields reads the CLI's JSON output rather than a file. Non-entity frontmatter (a skill's SKILL.md, a generated docs page: a YAML block, no schema) has one helper in the public tree's scripts, frontmatter.mjs, with a reader, a writer and a command line for shell callers, and its own tests. Nothing else in either repository tests a document's first line for a fence or searches a document for one.

## Scope
Both repositories: the engine crates, the private serve crate, the public tree's scripts and docs-site build, and the workspace scripts. The eval harness under xtask is outside it until it leaves (corrected 2026-09-08: the filesystem server this sentence also named was deleted on 2026-09-05).

## Relationships
- **GOVERNS**: [[engine:markdown-to-entity-parser]]
- **MOTIVATED_BY**: [[frontmatter-is-not-markdown-trim-it-before-any-markdown-reader]]

## Justification

The 2026-08 consistency sweep merged four copies of the offset arithmetic into one core, and a hand census then found seven more sites, two defective: one read a carriage-return document with an LF-sized offset, one located the fence by searching the document instead of testing its start, so a render without frontmatter would have had content injected at the first fence anywhere in the file. A discovery check walked both trees through many grades to keep a fifth copy from appearing unnoticed. The 2026-09-04 census (posture C) ruled that a class ends as a removed cause first: with one implementation per contract there is no copy left for a discovery check to find, so the check retired with the copies. The two contracts stay separate because their documents differ: an entity's frontmatter is validated against a schema and its body is parsed by the same engine, a skill's is a YAML block a JavaScript tool reads; one helper per contract is one implementation per contract, and a memstead frontmatter verb for scripts was rejected because it would expose a parsing detail as a product surface for two private scripts.

## Consequences

A new reader of entity frontmatter in Rust imports the core; one in a script asks the engine for the entity's fields (memstead entity --json, memstead export --format json). A new reader or writer of a SKILL.md or a generated page imports the helper. The core's tests carry the two census defect shapes (a CRLF document splits with a five-byte opening fence and a body after the closing fence's line break; a fence that is not on the first line is no frontmatter, and neither is an unclosed block), and the helper's tests carry the same shapes, so a regression in either contract is caught where the contract lives.
