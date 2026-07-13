---
type: decision
created_date: 2026-07-13T16:43:04Z
last_modified: 2026-07-13T16:44:02Z
status: accepted
decided_on: 2026-06-08
deciders: memstead-core
scope: system
tags: edge-model, wikilink, alias, mutation-path, engine
---

# Make body wiki-links the sole authoring channel for edges

## Decision
We chose to make body wiki-links `[[target]]` the single authoring channel for typed edges of a schema's `alias_target_rel_type`, with the rendered `## Relationships` section a derived projection of the body link set rather than an independently-authored source. An agent writes `[[X]]` in prose and the engine synthesises the backing relation; explicit hand-authoring of the pointer rel-type is refused (`RELATION_MANUAL_AUTHORING_FORBIDDEN`), and every body wiki-link must be backed by a relation or the mutation is refused (`WIKILINK_WITHOUT_RELATION` for schemas that opt out of the pointer). The mechanism is realized by [[engine--alias-synthesis-pass]].

## Context
In a markdown-and-git knowledge graph, prose references and the structured relationship list are two representations of the same edges. If both are independently authored, they drift: an agent updates a wiki-link in prose but forgets the relationship entry, or vice versa, and the graph's link integrity silently degrades. The graph is built for LLM agents as the primary consumer, so the authoring ergonomics and the single-source-of-truth guarantee both had to be resolved at the write path, not left to convention.

## Consequences
- The prose body becomes the single source of truth for edges; the `## Relationships` section is regenerated from it on every write, so the two can never diverge.
- Agents author edges inline with no separate relate call for the pointer rel-type — fewer round-trips, which the MCP tool policy explicitly optimizes for.
- Cost: the pointer rel-type can no longer be hand-authored (`RELATION_MANUAL_AUTHORING_FORBIDDEN`), and removing a relation while body links to its target remain is refused unless another relation to that target survives (`RELATION_HAS_BODY_LINKS`) — agents must reason about set-membership semantics rather than treating each edge independently.
- A schema may opt out of the alias model by omitting `alias_target_rel_type`; such schemas refuse unbacked body wiki-links outright rather than auto-backing them.

## Relationships
- **REFERENCES**: [[engine:alias-synthesis-pass]]
- **MOTIVATED_BY**: [[mcp-tool-surface-stays-small]]
- **MOTIVATED_BY**: [[agent-first-surface-design]]

## Options

- **Dual-authored relationship section** (rejected): keep prose wiki-links and the `## Relationships` list as two independently-authored sources. Rejected because they drift — an agent maintaining both as separate sources of truth inevitably desyncs them, and reconciling on read is ambiguous (which side wins?).
- **Relationship section as sole channel, no prose links** (rejected): force all edges through explicit relate calls and forbid prose wiki-links. Rejected because it costs a round-trip per edge and divorces the link from the prose context that motivates it, hurting the agent-authoring ergonomics the project optimizes for.
- **Body wiki-links as sole channel, relationship section derived** (chosen): the prose is the single source; the relationship section is a projection.

## Notes

The chosen option is realized by the synthesis pass and its edge-origin discriminator (`Explicit` | `Hierarchy` | `BodyLink`); see [[engine--alias-synthesis-pass]] for the per-operation mechanics, GC semantics, and refusal codes.
