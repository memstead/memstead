# Examples

Worked Memstead **schemas** — copy-paste-ready models you can point a vault at,
and read as illustrations of how a schema shapes a graph. A schema decides a
vault's whole modal flavour (knowledge / planning / inquiry / spec / hybrid);
these three show that range.

Each schema lives under `schemas/<name>/` with its own `README.md`, a
`schema.yaml`, and a `types/` directory declaring the entity types and their
relationships.

## The schemas

| Schema | What it models | Good illustration of |
|---|---|---|
| [`agent-program`](schemas/agent-program/) | Executable agent programs — a graph an LLM agent reads as instructions and runs autonomously, with declared tools and constraints (types: `program`, `node`, `tool`, `constraint`). | A single-vault, execution-flavoured schema. |
| [`reimpl-source`](schemas/reimpl-source/) | A read-mostly extraction of a legacy system — `evidence` (grounded observations) and `capabilities` (behavioural units supported by evidence). | The "source of truth" half of a paired, cross-vault modelling pattern. |
| [`reimpl-target`](schemas/reimpl-target/) | The design/build surface for a new implementation — target-specs that link back to the legacy capabilities they reimplement, plus per-target `divergence`. | Cross-vault links: one `reimpl-source` vault can feed many `reimpl-target` vaults. |

`reimpl-source` and `reimpl-target` are a **pair** — read them together to see a
two-vault model where one vault references entities in another.

## Using one

A schema is pinned to a vault when the vault is created. With the CLI:

```bash
memstead init --name my-vault --schema agent-program@0.1.0
```

or, for an existing workspace, register it as the vault's schema through the
engine. Each schema's own `README.md` explains its types, relationships, and the
workflow it is built for. For the vocabulary these examples use (vault, schema,
type, relationship), see the repo [GLOSSARY](../GLOSSARY.md).
