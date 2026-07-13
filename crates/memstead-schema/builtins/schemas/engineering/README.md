# `engineering@0.1.0` — standing engineering knowledge

The knowledge-only counterpart of [`software@0.1.0`](../software/):
three types answering one question — WHY is the system the way it is?

| Type | Purpose | Key test |
|---|---|---|
| `decision` | One design choice, with the alternatives it beat | Can you name what was rejected? |
| `principle` | A rule that holds project-wide | Would a violation be visible in many places? |
| `memo` | Reasoning that isn't a decision yet | Might it harden into one later? |

Current-state types (`spec`, `contract`, `actor`, `incident`, …) are
**deliberately absent**: what the system IS belongs in the
`software@0.1.0` mems this knowledge sits beside, and this schema
refuses those types at write time (`UNKNOWN_ENTITY_TYPE`) — the
knowledge/system-model boundary is a gate, not a convention. Knowledge
entities anchor into the system via cross-mem wiki-links.

This schema ships **built into the engine** — every install resolves
`engineering@0.1.0` with no copy step.

## How to use

1. Create the standing-knowledge mem pinned to the built-in schema,
   filling the instance write-guidance key the package's
   [`mem-template.json`](./mem-template.json) lists (here, `subjects`):

   ```
   memstead mem init engineering --schema engineering@0.1.0 \
       --write-guidance '{"subjects": "<one paragraph naming the system(s) this knowledge is about and the sibling mems modelling them>"}'
   ```

2. Create entities via MCP:

   ```
   memstead_create mem=engineering entity_type=decision title="…" sections={…}
   ```

## Relationship vocabulary

Strict mode, knowledge-shaped: structural (`PART_OF`, `REFERENCES` —
alias-emitted from wiki-links), reasoning (`DERIVED_FROM`,
`MOTIVATED_BY`/`MOTIVATES`, `INFORMED_BY`), lifecycle (`SUPERSEDES`,
`ENABLES`, `BLOCKS`), rule (`GOVERNS`, `CONSTRAINS`, `OVERRIDES`),
abstraction (`IMPLEMENTS`, `GENERALIZES`/`SPECIALIZES`), evidence
(`VALIDATES`, `CONTRADICTS`). See [`schema.yaml`](./schema.yaml) for
definitions and weights.

- Built-in schema this one is the knowledge slice of: [software/](../software/)
- Field shapes for `decision` / `principle` / `memo` match `software@0.1.0`,
  so entities migrate between the two schemas with metadata intact.
