---
name: tidy
description: >
  Graph hygiene for your mems — assesses structure (orphans, stubs, thin or missing links,
  stale entries), proposes concrete improvements, and applies only what you approve. Works
  entirely through the graph: it never reads or writes your sources.
allowed-tools: mcp__memstead__*
---

# Memstead — Tidy

Keep a mem's graph in good shape: find its structural weaknesses, propose concrete fixes, and apply only the ones you approve. Tidy works entirely through the graph — it inspects and mutates entities through the Memstead tools and never touches whatever a mem was built from.

The shape is always the same: **assess → propose → apply**. The assess and propose phases are read-only; nothing changes until you say so.

## Phase 1 — Assess

Call `memstead_health` with `include: ["orphans", "stubs", "missing_fields", "stale", "most_connected"]`. The response carries the counts and the per-issue lists directly — read them, do not re-derive a tally of your own.

If the graph holds no real entities (`total_nodes` is 0), report "nothing to tidy yet" and stop.

Group what health returns into the hygiene findings:

| Health field | What it flags |
|--------------|---------------|
| `orphans` | Entities with no relationships — stranded from the rest of the graph |
| `stubs` | Wiki-link targets that resolve to nothing — a link pointing at an entity that was never written |
| `missing_fields` | Entities missing a field their type requires to be healthy |
| `stale` | Entities untouched past their type's staleness threshold |

Then call `memstead_overview` and read the community structure: a cluster whose members read as an unrelated grab-bag signals a too-broad grouping or missing links — record it as a finding too.

## Phase 2 — Propose

Turn the findings into a short, concrete plan. For each one, name the specific entities and the exact change you would make:

- **Stub** → create the missing entity, or, if the link is simply wrong, repoint or drop it.
- **Orphan** → link it to the entities it belongs with — or, if it is genuinely irrelevant, propose removing it.
- **Missing field** → fill the required field from what the entity already states.
- **Stale** → flag for review; propose an actual update only when the newer truth is already available in the graph.
- **Thin or incoherent cluster** → propose the relationships that would tie it together, or the split that would separate two concerns.

Present the plan and ask which items to apply. Never apply a speculative or judgement-dependent change without approval.

## Phase 3 — Apply

Apply only the approved items, each through the Memstead tools:

- Create a missing entity with `memstead_create`.
- Add or remove a relationship with `memstead_relate`.
- Fill a field or rewrite a section with `memstead_update`.
- Remove a genuinely dead entity with `memstead_delete`.

When you are done, re-run `memstead_health` and report what changed and what remains open.

## Rules

- **Graph only** — tidy never reads or writes whatever a mem was built from. It has no read, search, or shell access by design; everything happens through the Memstead tools.
- **Propose before you apply** — assess and propose are read-only. Nothing is mutated until the user approves it.
- **Respect read-only mems** — never attempt to change entities in a read-only mem; surface their findings as advisory only.
- **One mem at a time** — when several mems are writable, tidy the one the user names rather than fanning out across all of them unasked.
