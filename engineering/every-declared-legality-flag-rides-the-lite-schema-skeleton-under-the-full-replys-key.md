---
type: decision
created_date: 2026-09-03T12:47:32Z
last_modified: 2026-09-03T12:47:32Z
status: accepted
decided_on: 2026-09-03
deciders: backlog-repairs bundle, C4 executing session
scope: subsystem
tags: schema, mcp, agent-surface
---

# Every declared legality flag rides the lite schema skeleton under the full reply's key

## Decision
We will have the lite schema skeleton carry every declared fact the engine enforces at write time, or that a writer must weigh when choosing a type, under the same key the full reply uses and only where the schema declares it: required flags, enum values, defaults, the value pattern, section-format declarations, required-outgoing blocks, constraints, leaf, last_resort, must_reach and signals. The full reply is a superset of the skeleton; nothing legality-relevant lives in full alone.

## Context
The server instructions promise that the skeleton carries every legality flag needed to author a valid write. A built-in schema was the first to declare a value pattern and a last-resort type; the skeleton dropped the pattern its full sibling already carried and neither level rendered last_resort, so a grader was refused on a shape the skeleton never showed. Sending agents to the full verbosity instead was rejected: the instructions tell them to plan from the skeleton, and a large schema's full reply degrades to that skeleton anyway.

## Consequences
- An agent planning a write from the lite reply sees the pattern a field enforces and which type the schema names as its fallback.
- A schema declaring neither renders byte-identical to before at both levels.
- Adding a new legality declaration to the schema language now carries an obligation: project it into the lite allowlist in the same change.

## Options

- Project every declared legality flag into the skeleton under the full reply's key: CHOSEN.
- Point the instructions at the full verbosity for constraints: rejected; agents are told to plan from the skeleton, and a large schema's full reply degrades to it.
