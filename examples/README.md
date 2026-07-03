# Examples

Worked Memstead **schemas** — copy-paste-ready models you can point a mem at,
and read as illustrations of how a schema shapes a graph. A schema decides a
mem's whole modal flavour (knowledge / planning / inquiry / spec / hybrid);
these three show that range.

Each schema lives under `schemas/<name>/` with its own `README.md`, a
`schema.yaml`, and a `types/` directory declaring the entity types and their
relationships.

## The schemas

| Schema | What it models | Good illustration of |
|---|---|---|
| [`agent-program`](schemas/agent-program/) | Executable agent programs — a graph an LLM agent reads as instructions and runs autonomously, with declared tools and constraints (types: `program`, `node`, `tool`, `constraint`). | A single-mem, execution-flavoured schema. |
| [`reimpl-source`](schemas/reimpl-source/) | A read-mostly extraction of a legacy system — `evidence` (grounded observations) and `capabilities` (behavioural units supported by evidence). | The "source of truth" half of a paired, cross-mem modelling pattern. |
| [`reimpl-target`](schemas/reimpl-target/) | The design/build surface for a new implementation — target-specs that link back to the legacy capabilities they reimplement, plus per-target `divergence`. | Cross-mem links: one `reimpl-source` mem can feed many `reimpl-target` mems. |

`reimpl-source` and `reimpl-target` are a **pair** — read them together to see a
two-mem model where one mem references entities in another.

## Using one

A schema must be **installed into the workspace before a mem can use it** —
example schemas aren't built into the engine (`memstead mem init` refuses an
uninstalled pin with `SCHEMA_NOT_FOUND`; `memstead init` accepts it but warns
loudly that nothing boots until the package is installed). In a fresh folder,
bootstrap with a built-in pin, install the example package, then move the pin:

```bash
mkdir my-mem && cd my-mem
memstead init --name my-mem --schema default@1.0.0
memstead schema install <path-to-this-repo>/examples/schemas/agent-program
memstead mem set-schema my-mem agent-program@0.1.0
```

`memstead type` now lists the schema's types under
`**Schema:** agent-program@0.1.0` — you're on the example schema.

In a multi-mem (mem-repo) workspace, install first and the pin resolves at
create time — no re-pin step:

```bash
memstead schema install <path-to-this-repo>/examples/schemas/agent-program
memstead mem init my-mem --schema agent-program@0.1.0 --operator-mode
```

(`--operator-mode` bypasses the workspace's mem-creation allowlist — the
right posture when you, the operator, are scaffolding the workspace; a
fresh workspace has no allowlist rules yet, so `mem init` refuses without
it. Verify with `memstead mem list`.)

Each schema's own `README.md` explains its types, relationships, and the
workflow it is built for. For the vocabulary these examples use (mem, schema,
type, relationship), see the repo [GLOSSARY](../GLOSSARY.md).
