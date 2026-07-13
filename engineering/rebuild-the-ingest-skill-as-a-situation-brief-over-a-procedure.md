---
type: decision
created_date: 2026-07-13T16:43:06Z
last_modified: 2026-07-13T16:44:05Z
status: accepted
decided_on: 2026-06-17
deciders: dasboe
scope: subsystem
tags: plugin, skills, ingest, architecture
---

# Rebuild the ingest skill as a situation brief over a procedure

## Decision
We chose to rebuild the `/memstead:ingest` skill so its `inject.mjs` assembles a *situation brief* — the run's mandate, its sources, its destination, and its paired-process-mem facts — and delegates the actual read and write *mechanics* to the MCP tool descriptions and the pinned schema's writing-guidance, rather than hard-coding per-medium procedure prose into the prompt. The prior procedure-driven skill is preserved unchanged as the frozen [[plugin--old-ingest-skill]] fallback; the rebuilt [[plugin--ingest-skill]] is the live shape, whose prompt core is the [[plugin--ingest-situation-brief-assembler]].

## Context
The pre-rebuild ingest skill authored the agent's full procedure in prompt prose — per-source read instructions, tool lists, and write mechanics lived in prompt templates (`skills/old-ingest/prompts/*.md`) and a prose-heavy `mediums.json`. That duplicated knowledge the MCP tool descriptions and the schema writing-guidance already carry, so the two surfaces drifted: a change to a tool's contract or a schema's write-rules left the ingest prompt stale. The engine and its MCP surface are built for LLM agents as the primary consumer, so those descriptions are the canonical procedure — a prompt repeating them is a second source of truth that ages badly.

## Consequences
- The situation brief stays correct as the MCP tools and schemas evolve — mechanics are read live from those surfaces, not frozen in prompt prose.
- At S1a the router was rewritten from scratch over the promoted projection/binding surface: the brief is now the engine's own `memstead projection brief` output, so the plugin assembles no brief prose at all — the single-source-of-truth intent is realized more fully than the original rebuild.
- Ingest quality now depends on the engine brief and the schema writing-guidance being complete and the tool descriptions being accurate; a gap surfaces as a weaker brief rather than a wrong-but-confident procedure.

## Relationships
- **REFERENCES**: [[plugin:old-ingest-skill]]
- **REFERENCES**: [[plugin:ingest-skill]]
- **REFERENCES**: [[plugin:ingest-situation-brief-assembler]]
- **MOTIVATED_BY**: [[plugin-prompts-carry-situation-not-procedure-and-the-engine-agent-surface-is-the-source-of-truth]]

## Options

- Keep the procedure-driven prompt assembly (the prior generation): fully author per-medium procedure in the prompt from templates. Rejected — it duplicates the tool/schema surface and drifts from it, and the drift is silent.
- Situation-brief assembly that leans on the MCP tool descriptions and schema writing-guidance for mechanics (chosen): a single source of truth for procedure; the prompt carries only the per-run situation.

## Notes

**Update (D0 + S1a):** the frozen `old-ingest` fallback this decision introduced was **deleted at D0** — there is no longer a parallel procedure-driven skill to maintain, so the standing-duplication cost is gone. At S1a the live `ingest` router was rebuilt from scratch to consume the engine's `projection brief` directly. The decision's core — situation-over-procedure, engine surface as the source of truth — holds and is now realized without a fallback twin.
