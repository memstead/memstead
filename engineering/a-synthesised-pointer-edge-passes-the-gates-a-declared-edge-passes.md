---
type: decision
created_date: 2026-11-11T11:24:21Z
last_modified: 2026-11-11T11:24:21Z
status: accepted
decided_on: 2026-11-11
deciders: engine-soundness bundle, executing session and its independent graders
scope: subsystem
tags: alias-synthesis, relations, validation
---

# A synthesised pointer edge passes the gates a declared edge passes

## Decision
We will run every edge the alias-synthesis pass makes from a body wiki-link through the gates a declared relation passes, the relationship's shape law and its acyclicity law, and refuse the write with the same codes (INVALID_REL_SHAPE, RELATIONSHIP_CYCLE) rather than skip the edge with a warning.

## Context
Synthesis appended the pointer edge without either check. On a schema that constrains its alias relationship, a wiki-link wrote an edge the schema forbids, served until the next load dropped it, or closed a cycle that the next load broke by dropping a different, explicit edge. No built-in schema constrains its pointer, so the defect needed a workspace schema to bite.

## Consequences
- On such a schema a wiki-link can refuse a write; the refusal names the same law a declared edge would.
- The load-path warning PARSED_RELATION_INVALID keeps its documented meaning: never raised by an engine-validated write.
- No new code.

## Relationships
- **GOVERNS**: [[engine:alias-synthesis-pass]]

## Options

- Skip the edge and warn: rejected, because the body link would stand unbacked by a relation and the warning would need a new code or contradict its documentation.
- Refuse through the declared-edge gates: chosen.

## Notes

Decided in the engine-soundness planning bundle, plan 06, round-two findings of the outside audit.
