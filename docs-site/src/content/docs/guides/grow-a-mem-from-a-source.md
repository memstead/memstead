---
title: Grow a mem from a source
description: "From a scaffolded binding to a mem with real entities in it: declare the source, run build batches, measure coverage, and stop and resume without losing your place."
sidebar:
  order: 2
---

A **binding** is a standing obligation: *this source belongs in that mem.* Creating one reads nothing — what fills the mem is the **ingest loop**, an agent session that asks the engine what to work on, works one batch, and stops. This guide runs that loop end to end, from a binding to a mem with entities in it that you can measure.

Every command below was executed against the workspace this guide builds.

## 1. Have a binding

`memstead quickstart --repo .` ([Getting started](../getting-started/#or-start-from-the-repository-you-already-have)) scaffolds one over the repository you ran it in, and its receipt names the binding id — `<mem>/<stem>`. If you already have a workspace, declare one directly:

```bash
memstead projection init --mem my-graph --source ../some-repo --medium-type codebase
```

Either way, confirm what you have:

```bash
memstead projection verify my-graph/some-repo --full
```

On a fresh binding this reports **0% anchored** and says so plainly — that is onboarding, not drift. The number it gives you is the denominator: how many source artifacts are in scope after the binding's deny list.

:::note
Working in Claude Code with the [plugin](../../skills/) installed? `/ingest` is this whole loop in one command — it asks the engine for the next due binding, hands you the brief, and you work it. If nothing is set up yet it asks three plain questions and declares the binding for you. The rest of this guide is what `/ingest` does, spelled out for any agent.
:::

## 2. Render a batch brief

```bash
memstead projection brief my-graph/some-repo
```

The output is not for you — it is the prompt an agent works from. It names the source, the destination mem and its schema, the anchoring rules, and what changed since the last pass. Hand it to an agent session verbatim.

Two things in it matter for the loop to work:

- **The destination.** If the mem does not exist yet, the brief says so and gives you the command that creates it. Create it before working the batch.
- **Anchors.** The brief tells the agent to attach an `anchors` list to every write, naming the source artifact the entity is drawn from and the binding source name it came from. This is what makes the next two steps possible: unanchored writes leave coverage blind.

## 3. Work one batch

The agent reads the source, then creates entities through the normal mutation surface — `memstead_create` over MCP, or the CLI:

```bash
memstead create --type concept \
  --title "Token issuing" \
  --section definition="Auth issues and verifies bearer tokens." \
  --section explanation="Both entry points live in src/auth.rs." \
  --anchor '{"artifact":"src/auth.rs","grain":"file","class":"anchored","source":"some-repo"}'
```

The `source` value is the binding's declared source **name**, which the brief prints. It selects the pointer the artifact path is joined onto, so a wrong name usually refuses `INVALID_ANCHOR` — the path resolves under no candidate join. It is not refused when the path happens to resolve workspace-relative anyway, so write both exactly as the brief lists them.

One batch is one bounded piece of work — depth on a coherent area beats breadth across unrelated ones. Stop when the area is covered.

## 4. Measure, then go again

```bash
memstead projection verify my-graph/some-repo --full
```

Coverage moves as anchors land. On the repository this guide was verified against — three files in scope — the first batch took it from `0/3` to `1/3` and the second to `2/3`, with anchor resolution at 100% throughout: every anchor still pointing at a file that exists.

Then render the next brief and repeat. **The mem is your continuity**: each run is a fresh agent with no memory of the last one, and the graph is what persists between them. Stopping mid-loop loses nothing; the next run picks up against the coverage that is already there.

```bash
memstead projection brief my-graph/some-repo
```

When a binding has nothing due — its source has not moved since the last pass — the rotation says so rather than inventing work.

## What this loop does not do

- **It does not keep the mem current.** Growing a mem and maintaining it as the source changes are separate jobs; the second is `memstead projection brief <binding> --sync`, the sole maintenance writer.
- **It does not run itself.** Memstead ships no scheduler. The loop is an agent session on whatever cadence you choose — a `/loop`, a cron'd run, a CI job.
- **It does not batch by itself.** One invocation is one batch. Repetition is yours to drive.

## Where next

- [Agent recipes](../agent-recipes/) — the MCP tool sequences an agent actually runs, with real payloads.
- [The fidelity contract](../../concepts/fidelity-contract/) — what coverage, drift and freshness mean, and what the engine will and will not claim about them.
