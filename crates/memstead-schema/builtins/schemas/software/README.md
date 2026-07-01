# `software@0.1.0` — knowledge-graph schema for software projects

A copy-paste-ready memstead schema covering nine artifact types against a
four-pillar framing of a software-project graph:

| Pillar | Types |
|---|---|
| **WHAT** exists | `spec`, `contract` |
| **WHY** it is so | `decision`, `principle`, `requirement` |
| **WHO** is accountable | `actor` |
| **WHAT** has broken | `incident` |
| Support | `concept`, `memo` |

The graph's value comes from type diversity. A graph dominated by any
one type — especially `spec` — has lost the structure the schema offers.
See [`mem-template.json`](./mem-template.json) in this package for the
per-mem scaffolding — the instance write-guidance keys to fill.

This schema ships **built into the engine** — every install resolves
`software@0.1.0` with no copy step. Pin it directly when you create a
code mem (below); fork it only when you want to customize the
vocabulary.

## How to use

1. Create the code mem pinned to the built-in schema, filling the
   instance write-guidance keys the package's `mem-template.json`
   lists (here, `stack`):

   ```
   memstead mem init <your-mem> --schema software@0.1.0 \
       --write-guidance '{"stack": "<one paragraph naming the runtime stack, datastores, deployment target>"}'
   ```

   The agent will refuse to invent stack facts if `stack` is left as a
   placeholder at run time. To customize the vocabulary itself, fork the
   schema into local storage first with `memstead schema install software`.

2. Create entities via MCP:

   ```
   memstead_create mem=<your-mem> entity_type=decision title="…" sections={…}
   ```

## Types

### WHAT — current-state surfaces

| Type | Purpose | Key test |
|---|---|---|
| `spec` | Internal component, module, or subsystem. The catch-all — never the default pick. | Rule out contract, decision, requirement, principle, concept first. |
| `contract` | External wire-level surface (HTTP, gRPC, CLI, library API, file format). | Is there a concrete request/response shape? |

### WHY — reasoning and commitments

| Type | Purpose | Key test |
|---|---|---|
| `decision` | One design choice with rejected alternatives and durable consequences. Lifecycle: proposed → accepted → superseded / retired. | Can you name what was rejected and why? |
| `principle` | Project-wide rule that holds across many entities. | Would a violation be visible in multiple places? |
| `requirement` | Normative MUST / SHOULD / MAY tied to one capability, testable, from a written source. | Could a test verify compliance? |

### WHO — accountability

| Type | Purpose |
|---|---|
| `actor` | Team, person, or service account. Subject of every OWNS and MAINTAINS edge. |

### WHAT BROKE — operations

| Type | Purpose |
|---|---|
| `incident` | Production failure with timeline and post-mortem. Links VIOLATES to broken commitments. |

### Support

| Type | Purpose |
|---|---|
| `concept` | Precise definition of a term used across 2+ entities. |
| `memo` | Lightweight reasoning without rejected alternatives. Migrates to `decision` if alternatives accumulate. |

## Relationship vocabulary

Strict mode. The 37 edges of the built-in `default` schema plus five
software-specific additions:

| Edge | From → To | Purpose |
|---|---|---|
| `REALIZES` | spec / contract → requirement | Concrete artifact fulfils an abstract requirement. |
| `OWNS` | actor → any | Actor has final authority over the target. |
| `MAINTAINS` | actor → any | Actor maintains the target without sole ownership. |
| `VIOLATES` | incident → requirement / spec / contract | Incident broke the target commitment. |
| `DEPRECATES` | contract / spec → contract / spec | Source supersedes target at a protocol level; consumers should migrate. |

Common default-schema edges in this schema: `PART_OF`, `DEPENDS_ON`,
`IMPLEMENTS`, `USES`, `MOTIVATED_BY`, `SUPERSEDES`, `DERIVED_FROM`,
`GOVERNS`, `CONSTRAINS`, `CAUSED`, `GENERALIZES`, `SPECIALIZES`,
`CONTRASTS_WITH`.

## Evolving the schema

Bump `version` in `schema.yaml` on any shape change. Mems pin exact
versions (`software@0.1.0`), so an existing mem keeps working against
the old version until its config is updated. Ship new versions
alongside old ones rather than editing in place.

## Reference

- Per-mem scaffolding (instance write guidance): [`mem-template.json`](./mem-template.json)
- Authoring guide: [dev/authoring-schemas.md](../../../dev/authoring-schemas.md)
- Built-in schema this one draws from: [engine/crates/memstead-schema/builtins/schemas/default/](../../../engine/crates/memstead-schema/builtins/schemas/default/)
- Fully commented example of a minimal schema: [engine/crates/memstead-schema/examples/minimal/](../../../engine/crates/memstead-schema/examples/minimal/)
