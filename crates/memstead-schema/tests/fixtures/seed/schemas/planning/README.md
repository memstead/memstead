# `planning` — schema for planning-phase mems

Workspace-level schema for every mem that pins `"schema": "planning"` — typically the short-lived mems under `exec_mems/` that capture one planning phase each. Seven types capture the deliberation between stating a goal and having a plan executed:

| Phase of thought | Type |
|---|---|
| What are we planning | `goal` |
| What alternatives exist | `option` |
| What did we choose and why | `decision` |
| How will we execute | `step` |
| What could go wrong | `risk` |
| What is still unclear | `open_question` |
| Everything else (observation, constraint, session log, idea) | `note` |

The graph grows as planning unfolds — it is written during conversation, not bulk-extracted from sources. After execution completes, a lens projection lifts the durable artifacts (accepted decisions, validated risks, surviving open questions) into each project mem listed in `belongs_to`; the planning mem is archived as historical record.

## Location

This schema lives at the workspace level and is shared by every planning mem that pins it. The workspace's `.memstead/workspace.toml` declares `schemas_dir = "schemas"`, and every `schemas/<name>/` directory is discovered at load time. No per-mem copying is required — a planning mem's `.memstead/config.json` just references the schema by name.

Local usage is **unversioned**: a mem simply writes `"schema": "planning"` and the engine resolves against this directory. The `version:` field in `schema.yaml` is metadata preserved for publish/archive workflows; it is not a pin.

## Lifecycle and mem placement

Per-phase mems live under `exec_mems/<plan-name>/` (controlled by `allowed_create_paths` in `.memstead/workspace.toml`). Each planning mem declares `belongs_to: [<project-mem>, …]` — the project mems it intends to lens into — and carries a lens projection at `projections/<plan-name>/lens.json` that writes into each destination mem.

```
brief / directive                    ┐
existing project-graph state         │ read via cross-mem refs
                                     │
       ↓                             ┘
┌────────────────────────────────────────┐
│ exec_mems/<plan-name>/               │
│   planning mem                       │ ← grows during conversation
│   goal → options → decision            │ ← grows during execution sessions
│          ↓                             │
│          step ┬→ step ┬→ …             │
│          ↓   ↘ ↓     ↘                 │
│        risk    open_question           │
└────────────────────────────────────────┘
                │
                │ lens projection (reductive, multi-destination):
                │   accepted decisions, validated spec updates,
                │   surviving open questions
                ↓
┌────────────────────────────────────────┐
│ project_mems/<each in belongs_to>    │
│   software-schema graphs absorb shares │
└────────────────────────────────────────┘
                │
                │ archival
                ↓
  archive/<plan-name>/        (convention; not wired into the engine today)
```

The planning mem is the **child**; project mems are its **parents**. Cross-mem wiki-links flow from planning → project (child → parents declared in `belongs_to`) but not the reverse.

## Types

### Root — what is being planned

| Type | Purpose |
|---|---|
| `goal` | What the planning phase is set to achieve. Usually 1-3 per mem. Carries `success_criteria` that make 'done' checkable. |

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

Common default-schema edges in use: `PART_OF` (goal hierarchy), `MOTIVATED_BY` (external trigger for a goal or decision), `REQUIRES` (step precedence), `PRODUCES` (step outputs), `BLOCKS` (open_question → decision/step), `CONSTRAINS` (constraint-note → decision/step), `SUPERSEDES` (new decision replaces old).

## Evolving the schema

Because local use is unversioned, shape changes are in-place edits. Active planning mems see changes on the next engine reload — coordinate rollouts carefully.

For publishable or portable schema variants, author a copy under `recipes/planning/schema/` where the `version` field matters.

## Reference

- Paired recipe config: [recipes/planning/config.json](../../../recipes/planning/config.json)
- Companion software schema (lens destination): [../software/](../software/)
- Authoring guide: [dev/authoring-schemas.md](../../../dev/authoring-schemas.md)
