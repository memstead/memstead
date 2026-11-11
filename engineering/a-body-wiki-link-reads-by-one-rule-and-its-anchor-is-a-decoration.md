---
type: decision
created_date: 2026-11-11T11:24:21Z
last_modified: 2026-11-11T11:24:21Z
status: accepted
decided_on: 2026-11-11
deciders: engine-soundness bundle, executing session and its independent graders
scope: subsystem
tags: wiki-link, rename, push
---

# A body wiki-link reads by one rule and its anchor is a decoration

## Decision
We will read a body wiki-link by one rule, the strict decoder's, at create, at the push-time validator and in the rename rewriter. A `#anchor`, a leading `../` and a trailing `.md` decorate a link to the entity: the push gate delegates to the decoder, and a rename swaps only the id inside each link and keeps its decorations.

## Context
The engine read a link by two rules. The push validator refused `#` as reserved deep-link syntax, so an entity the create path had committed could not be pushed; the rename rewriter matched only the bare slug, so a decorated link kept the old id and the next update brought it back as a stub. Both intents dated from the first commit and no public document settled them.

## Consequences
- An anchored link to an entity is legal on every path and survives a rename.
- A later deep-link feature stays compatible: such a link would still name the same entity.
- One validator test that asserted the refusal was replaced by one asserting the create path's acceptance.

## Relationships
- **GOVERNS**: [[engine:entity-id-and-wiki-link-grammar]]
- **GOVERNS**: [[engine:rename-mutation]]

## Options

- Refuse `#` at create as reserved syntax: rejected, because existing tests already treated the anchor as a decoration, Obsidian-style authors write it, and links already stored would become unpushable.
- The decoder's rule everywhere: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 06, round-two findings of the outside audit.
