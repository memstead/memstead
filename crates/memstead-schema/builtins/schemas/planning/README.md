# `planning@0.1.0` — knowledge-graph schema for planning phases

A copy-paste-ready memstead schema for **one planning phase**. Seven types
capture the deliberation between stating a goal and having a plan
executed:

| Phase of thought | Type |
|---|---|
| What are we planning | `goal` |
| What alternatives exist | `option` |
| What did we choose and why | `decision` |
| How will we execute | `step` |
| What could go wrong | `risk` |
| What is still unclear | `open_question` |
| Everything else (observation, constraint, session log, idea) | `note` |

The graph grows as planning unfolds — it is written during
conversation, not bulk-extracted from sources. After execution
completes, a lens projection lifts the durable artifacts (accepted
decisions, validated risks, surviving open questions) into the main
project graph; the planning vault is archived as historical record.

See [`vault-template.json`](./vault-template.json) in this package for the
per-vault scaffolding — the instance write-guidance keys to fill.

This schema ships **built into the engine** — every install resolves
`planning@0.1.0` with no copy step. Pin it directly when you create a
planning vault (below); fork it only when you want to customize the
vocabulary.

## How to use

### Per planning phase, a new vault

Create a dedicated vault for each planning phase. The convention that
pairs with this schema:

```
<main-vault>/.plans/<plan-name>/           ← active planning vault
<main-vault>/.plans/archive/<plan-name>/   ← archived after lens projection
```

1. Create the planning vault pinned to the built-in schema, filling the
   instance write-guidance keys the package's `vault-template.json`
   lists (here, `phase_context`):

   ```
   memstead vault init <main-vault>/.plans/<plan-name> --schema planning@0.1.0 \
       --write-guidance '{"phase_context": "<one paragraph: what this phase is about>"}'
   ```

   The agent will refuse to plan blind if `phase_context` is left as a
   placeholder at run time.

2. Plan. The vault grows as decisions are made, options are weighed,
   risks are identified, steps are recorded.

To customize the schema vocabulary itself, fork it into the workspace's
local schema storage first (local storage shadows the built-in per the
resolution order), then edit the copy:

```
memstead schema install planning   # → <workspace>/.memstead/schemas/planning@0.1.0/
```

### Lifecycle

```
  brief / directive         ┐
  existing main-graph state │ inputs consulted via cross-vault reference
                            │
          ↓                 ┘
    ┌───────────────────────────────────┐
    │ <main>/.plans/<plan-name>/        │
    │   planning@0.1.0 vault            │ ← grows during planning conversation
    │   goal → options → decision       │ ← grows during execution sessions
    │          ↓                        │
    │          step ┬→ step ┬→ …        │
    │          ↓   ↘ ↓     ↘            │
    │        risk    open_question      │
    └───────────────────────────────────┘
                       │
                       │ lens projection (reduced):
                       │   accepted decisions, validated spec updates,
                       │   surviving open questions
                       ↓
    ┌───────────────────────────────────┐
    │ <main-vault>                      │
    │   software@0.1.0 graph            │ ← absorbs the durable parts
    └───────────────────────────────────┘
                       │
                       │ archival
                       ↓
    <main>/.plans/archive/<plan-name>/
```

The planning vault is the **child**; the main vault is the **parent**.
Cross-vault wiki-links resolve from planning → main (child references
parent) but not the reverse — until the lens projection at the end.

## Types

### Root — what is being planned

| Type | Purpose |
|---|---|
| `goal` | What the planning phase is set to achieve. Usually 1-3 per vault. Carries `success_criteria` that make 'done' checkable. |

### Deliberation — weighing the path

| Type | Purpose | Key test |
|---|---|---|
| `option` | A neutral alternative under consideration. | Does it have at least one ALTERNATIVES_TO sibling? |
| `decision` | A chosen path after weighing options. | Does it CHOSEN exactly one option and REJECTED at least one? |

### Execution — how the plan moves

| Type | Purpose | Key test |
|---|---|---|
| `step` | A concrete action in the execution plan. | Does it EXECUTES a decision and carry a `validation` section? |

### Tracking — what else matters

| Type | Purpose | Key test |
|---|---|---|
| `risk` | Something specific that could go wrong. | Does it THREATENS a concrete target? |
| `open_question` | Blocking unknown with its own lifecycle. | Does it BLOCKS a decision or step, and is the title a question? |
| `note` | Catch-all — observation, constraint, session log, idea, reference. Differentiated by `kind`. | Has it outgrown the catch-all? Migrate when it has. |

## Relationship vocabulary

Strict mode. The 37 default edges plus eight planning-specific:

| Edge | From → To | Purpose |
|---|---|---|
| `ALTERNATIVES_TO` | option ↔ option | Frames a choice. Without it, options are unframed. |
| `CHOSEN` | decision → option | The selected path. Typically exactly one per decision. |
| `REJECTED` | decision → option | Paths explicitly ruled out. Negative knowledge is the highest-value output of planning. |
| `EXECUTES` | step → decision | A step carries out a decision. |
| `MITIGATES` | step → risk | A step addresses a risk. |
| `THREATENS` | risk → goal / decision / step | What the risk endangers. |
| `ANSWERS` | decision / note → open_question | How the question was resolved. |
| `RAISES` | note / option / step → open_question | Where the question arose. |

Common default-schema edges in use: `PART_OF` (goal hierarchy),
`MOTIVATED_BY` (external trigger for a goal or decision), `REQUIRES`
(step precedence), `PRODUCES` (step outputs), `BLOCKS` (open_question →
decision/step), `CONSTRAINS` (constraint-note → decision/step),
`SUPERSEDES` (new decision replaces old).

## Evolving the schema

Bump `version` in `schema.yaml` on any shape change. Planning vaults
pin exact versions (`planning@0.1.0`), so an active planning phase
keeps working against the pinned version until explicitly updated.
Ship new versions alongside old ones rather than editing in place.

## Reference

- Per-vault scaffolding (instance write guidance): [`vault-template.json`](./vault-template.json)
- Main-graph companion schema (lens projection target): [../../software/](../../software/)
- Authoring guide: [dev/authoring-schemas.md](../../../dev/authoring-schemas.md)
- Built-in schema this one draws from: [engine/crates/memstead-schema/builtins/schemas/default/](../../../engine/crates/memstead-schema/builtins/schemas/default/)
- Vault-ideas input this schema implements: [dev/archive/complete/vaults-ideas-input.md](../../../dev/archive/complete/vaults-ideas-input.md)
