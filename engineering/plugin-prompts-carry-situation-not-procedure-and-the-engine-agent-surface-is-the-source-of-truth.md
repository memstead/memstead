---
type: principle
created_date: 2026-07-13T16:43:06Z
last_modified: 2026-08-30T00:33:19Z
authority: established
universality: domain-wide
tags: plugin, skills, ingest, agent-first, single-source-of-truth
---

# Plugin prompts carry situation not procedure and the engine agent surface is the source of truth

## Statement
A plugin skill prompt MUST assemble only per-run *situation* — the mandate, the sources, the destination, and the paired-mem facts — and delegate *procedure, mechanics, and authoring guidance* to the engine's agent-facing surface: the MCP tool descriptions and the pinned schema's writing-guidance, which are the single source of truth. A skill prompt MUST NOT restate tool mechanics or schema write-rules that those surfaces already carry.

## Scope
Applies to every prompt-assembling skill in the plugin's skills layer — the prompt text a skill emits to the model or to a forked agent. It constrains the plugin layer that consumes the engine's agent surface, not the engine's own tool/schema authoring.

## Relationships
- **REFERENCES**: [[plugin:writing-guidance-resolver]]
- **REFERENCES**: [[plugin:ingest-situation-brief-assembler]]
- **GOVERNS**: [[plugin:ingest-situation-brief-assembler]]
- **GOVERNS**: [[plugin:writing-guidance-resolver]]

## Justification

The engine, its MCP tools, and its schemas are built for LLM agents as the primary consumer, so their descriptions already are the canonical procedure. A prompt that repeats them is a second source of truth that drifts silently the moment a tool contract or a schema write-rule changes.

**Rechecked 2026-08-30.** The two plugin-side realizations this section used to cite, [[plugin--writing-guidance-resolver]] and [[plugin--ingest-situation-brief-assembler]], are no longer plugin code: the whole assembly moved into the engine. The `/ingest` skill's prompt is now one line that shells `scripts/inject.mjs`, a router with no selection, backoff or brief-assembly logic of its own; the engine renders the situation brief behind `memstead projection brief`, and a skill that wants a type's prose calls `memstead_schema` with `verbosity: "full"` rather than carrying a resolver. The principle is satisfied more strongly than when it was written, since the single source of truth is now also the single assembler, but the two entities named above describe a plugin layer that has been dissolved into the engine.

## Exceptions

- **Corrected 2026-08-30: this exception has expired.** It carved out a frozen `plugin--old-ingest-skill` fallback that hard-coded per-medium procedure in prompt templates, retained as a revert path. No such skill ships: the plugin's skills are `ingest`, `interview`, `learn`, `setup`, `sync` and `tidy`, and none of them carries a procedure template. Every prompt-assembling surface in the plugin is now under the rule with no carve-out.

## Consequences

- Prompt correctness tracks the MCP tool and schema surfaces automatically as they evolve — a renamed tool parameter or a changed write-rule needs no parallel prompt edit.
- Ingest quality now depends on those surfaces being complete and accurate: a gap in a tool description or a schema's writing-guidance surfaces as a weaker brief rather than a confident-but-wrong procedure.
- New skills are authored as situation assemblers over the tool surface, not as procedure documents — the design cost of a skill drops to "what context does this run need" from "how does the agent operate every tool".
