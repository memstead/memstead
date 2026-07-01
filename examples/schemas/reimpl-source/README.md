# `reimpl-source@0.1.0` — Legacy extraction schema

Paired with [`reimpl-target@0.1.0`](../../reimpl-target/schema/). This schema lives
in the **Legacy mem** — the read-mostly extraction of what the old
system actually is. It captures two things and nothing else: **evidence**
(grounded observations about the old system) and **capabilities**
(behavioral units supported by evidence).

One Legacy mem can feed **multiple** Target mems. Each Target carries
its own divergences; the Legacy stays technology- and target-neutral.

## Purpose

The Legacy mem is the single source of truth about *what the old system
actually does* — extracted from code, tests, data, interviews, and
observation. It is not a wish list and not a design. Every capability
must be backed by evidence; unsourced claims have no place here.

After the reimplementation phase ends, the Legacy mem is sealed and
kept as a historical record.

## Types

| Type | Purpose |
|---|---|
| `evidence` | A single sourced observation — a code snippet, test case, data sample, interview quote, or log pattern. The atomic unit of belief about the old system. |
| `capability` | A behavioral unit the system exhibits — inputs, outputs, invariants, edge cases. Every capability is supported by one or more evidence entities. |

## How to use

```
memstead schema install examples/schemas/reimpl-source
memstead mem init <legacy-mem> --schema reimpl-source@0.1.0
```

In the Legacy mem's `.memstead/config.json`:

```json
{
  "name": "<project>-legacy",
  "schema": "reimpl-source@0.1.0"
}
```

## Relationships

Inherits the default schema's 37 edges plus one source-specific addition:

| Edge | From → To | Purpose |
|---|---|---|
| `SUPPORTS` | evidence → capability | Evidence backs a capability claim. Capabilities without SUPPORTS edges are unsupported belief and should be flagged. |

Cross-mem inbound edges from the Target mem (`REALIZES_CAPABILITY`,
`DIVERGES_FROM`) are declared in the target schema. The Legacy mem
neither produces nor restricts them.

## Evolving the schema

Legacy-extraction practice is new territory. Expect `0.1.0` to evolve
based on the first real reimplementation project. Ship new versions
alongside old ones rather than editing in place.
