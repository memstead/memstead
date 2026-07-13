---
type: principle
created_date: 2026-07-13T16:43:06Z
last_modified: 2026-07-13T16:44:05Z
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
- **REFERENCES**: [[plugin:old-ingest-skill]]
- **GOVERNS**: [[plugin:ingest-situation-brief-assembler]]
- **GOVERNS**: [[plugin:writing-guidance-resolver]]

## Justification

The engine, its MCP tools, and its schemas are built for LLM agents as the primary consumer, so their descriptions already are the canonical procedure. A prompt that repeats them is a second source of truth that drifts silently the moment a tool contract or a schema write-rule changes. [[plugin--writing-guidance-resolver]] pulls each mem's schema writing-guidance live, and [[plugin--ingest-situation-brief-assembler]] composes only the run's situation — both are built to read mechanics from the surface rather than freeze them in prose.

## Exceptions

- The frozen [[plugin--old-ingest-skill]] fallback predates this principle and deliberately hard-codes per-medium procedure in prompt templates. It is retained unchanged as a revert path, not brought into conformance.

## Consequences

- Prompt correctness tracks the MCP tool and schema surfaces automatically as they evolve — a renamed tool parameter or a changed write-rule needs no parallel prompt edit.
- Ingest quality now depends on those surfaces being complete and accurate: a gap in a tool description or a schema's writing-guidance surfaces as a weaker brief rather than a confident-but-wrong procedure.
- New skills are authored as situation assemblers over the tool surface, not as procedure documents — the design cost of a skill drops to "what context does this run need" from "how does the agent operate every tool".
